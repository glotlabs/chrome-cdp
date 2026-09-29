//! Mock files from recorded traffic (plan 11, milestone 11.8): `mock
//! record` (a session's network log) and `mock import` (a HAR file).
//!
//! Both come here as a list of [`Exchange`]s, in the order they were
//! sent. [`record`] keeps those in scope, and writes one `[[mock]]` per
//! distinct request (method, path, query, and a GraphQL operation), with
//! the answers of a request sent more than once as a `sequence`. Bodies go
//! to data files (small JSON and text inline); volatile headers are left
//! out; ids in paths and ids and timestamps in bodies get a comment, so
//! whoever edits the file knows where a template would do.
//!
//! The file is written here, as text, so its layout (inline tables, the
//! comments) stays readable; [`crate::MockSet::parse`] checks it in the
//! tests.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use serde_json::Value;

use crate::glob::PathGlob;
use crate::template::is_template;

/// One request and its answer.
#[derive(Debug, Clone, Default)]
pub struct Exchange {
    pub method: String,
    /// Absolute.
    pub url: String,
    pub request_headers: Vec<(String, String)>,
    pub request_body: Option<String>,
    /// 0 when it got no answer (failed, blocked, still loading).
    pub status: u16,
    pub response_headers: Vec<(String, String)>,
    /// `None` when the body was not recorded.
    pub response_body: Option<Vec<u8>>,
    /// The response's MIME type.
    pub mime: String,
    /// DevTools' resource type (`fetch`, `xhr`, `document`, …), lower case.
    pub resource_type: String,
}

/// What to record.
#[derive(Debug, Clone, Default)]
pub struct RecordOptions {
    /// Path or full-URL globs, as a set's `scope`. Empty: the page's
    /// `fetch` and XHR requests.
    pub scope: Vec<String>,
    /// Where data files go, relative to the mock file's directory:
    /// `data/<name>`.
    pub data_dir: String,
    /// The first comment line of the file.
    pub origin: String,
}

/// A recorded mock set.
#[derive(Debug, Clone, Default)]
pub struct Recorded {
    /// The mock file.
    pub toml: String,
    /// Data files: paths relative to the mock file's directory, and bytes.
    pub files: Vec<(String, Vec<u8>)>,
    /// The requests recorded.
    pub requests: usize,
    /// The `[[mock]]`s written.
    pub mocks: usize,
    /// Requests in scope left out, and why (`OPTIONS /api/x: a CORS
    /// preflight`).
    pub skipped: Vec<String>,
    /// The scope written to `[settings]`.
    pub scope: Vec<String>,
}

/// JSON bodies up to this many characters (compact) are written inline.
const INLINE: usize = 60;

/// Query parameters that only defeat caches: left out of the match.
const CACHE_BUSTERS: &[&str] = &[
    "_",
    "_t",
    "t",
    "ts",
    "timestamp",
    "cachebust",
    "cb",
    "nocache",
];

/// Response headers that change on every answer, or that mocks set
/// themselves: left out.
const VOLATILE: &[&str] = &[
    "accept-ranges",
    "age",
    "alt-svc",
    "cache-control",
    "cf-cache-status",
    "cf-ray",
    "connection",
    "content-encoding",
    "content-length",
    "content-security-policy",
    "content-security-policy-report-only",
    "content-type",
    "cross-origin-embedder-policy",
    "cross-origin-opener-policy",
    "cross-origin-resource-policy",
    "date",
    "etag",
    "expires",
    "keep-alive",
    "last-modified",
    "nel",
    "permissions-policy",
    "pragma",
    "referrer-policy",
    "report-to",
    "request-id",
    "server",
    "server-timing",
    "strict-transport-security",
    "timing-allow-origin",
    "transfer-encoding",
    "vary",
    "via",
    "x-cache",
    "x-cache-hits",
    "x-content-type-options",
    "x-correlation-id",
    "x-frame-options",
    "x-powered-by",
    "x-request-id",
    "x-runtime",
    "x-served-by",
    "x-timer",
    "x-xss-protection",
];

/// What `--scope` records: requests whose path or URL (without its
/// query) a glob matches, or with no globs the page's `fetch` and XHR
/// requests.
#[derive(Debug, Clone)]
pub struct Scope(Vec<PathGlob>);

impl Scope {
    /// Globs as a set's `scope` takes them: `/api/**` or `https://…`.
    pub fn new(globs: &[String]) -> Result<Self, String> {
        globs
            .iter()
            .map(|s| {
                if s.starts_with('/') {
                    PathGlob::path(s)
                } else {
                    PathGlob::url(s)
                }
                .map_err(|e| format!("--scope {e}"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }

    /// Whether the request to `url` of DevTools resource type `kind`
    /// (`Fetch`, `XHR`, `Document`, …; any case) is in scope.
    pub fn matches(&self, url: &str, kind: &str) -> bool {
        let Ok(mut url) = url::Url::parse(url) else {
            return false;
        };
        if self.0.is_empty() {
            return kind.eq_ignore_ascii_case("fetch") || kind.eq_ignore_ascii_case("xhr");
        }
        let path = url.path().to_owned();
        url.set_query(None);
        url.set_fragment(None);
        self.0.iter().any(|g| {
            if g.is_path() {
                g.matches(&path)
            } else {
                g.matches(url.as_str())
            }
        })
    }
}

/// The exchanges of a HAR file (HAR 1.2, as browsers and `har stop`
/// write it).
pub fn har_exchanges(har: &Value) -> Result<Vec<Exchange>, String> {
    let entries = har
        .pointer("/log/entries")
        .and_then(Value::as_array)
        .ok_or("it has no log.entries; is it a HAR file?")?;
    let text = |v: &Value, key: &str| {
        v.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let headers = |v: Option<&Value>| -> Vec<(String, String)> {
        v.and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|h| Some((text(h, "name"), h.get("value")?.as_str()?.to_owned())))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut out = Vec::with_capacity(entries.len());
    for (i, entry) in entries.iter().enumerate() {
        let request = entry
            .get("request")
            .ok_or_else(|| format!("entry {i} has no request"))?;
        // A method names data files: only an HTTP token may, never
        // `X/../..` from a crafted HAR.
        let method = text(request, "method");
        if method.is_empty() || !method.bytes().all(is_token_char) {
            return Err(format!(
                "entry {i} has the method `{method}`, which is not an HTTP method"
            ));
        }
        let response = entry.get("response");
        let content = response.and_then(|r| r.get("content"));
        let body = content.and_then(|c| {
            let size = c.get("size").and_then(Value::as_i64).unwrap_or(0);
            match c.get("text").and_then(Value::as_str) {
                Some(t) if c.get("encoding").and_then(Value::as_str) == Some("base64") => {
                    decode_base64(t)
                }
                Some(t) => Some(t.as_bytes().to_vec()),
                None if size <= 0 => Some(Vec::new()),
                None => None,
            }
        });
        let mut request_headers = headers(request.get("headers"));
        // Browsers leave `Cookie` out of the headers when they list it
        // in `cookies`.
        if !request_headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("cookie"))
            && let Some(cookies) = request.get("cookies").and_then(Value::as_array)
            && !cookies.is_empty()
        {
            let pairs: Vec<String> = cookies
                .iter()
                .map(|c| format!("{}={}", text(c, "name"), text(c, "value")))
                .collect();
            request_headers.push(("Cookie".into(), pairs.join("; ")));
        }
        out.push(Exchange {
            method: method.to_ascii_uppercase(),
            url: text(request, "url"),
            request_headers,
            request_body: request
                .pointer("/postData/text")
                .and_then(Value::as_str)
                .map(str::to_owned),
            status: response
                .and_then(|r| r.get("status"))
                .and_then(Value::as_u64)
                .and_then(|s| u16::try_from(s).ok())
                .unwrap_or(0),
            response_headers: headers(response.and_then(|r| r.get("headers"))),
            response_body: body,
            mime: content.map(|c| text(c, "mimeType")).unwrap_or_default(),
            resource_type: text(entry, "_resourceType").to_ascii_lowercase(),
        });
    }
    Ok(out)
}

/// Standard base64, with or without padding; `None` if it is not.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0;
    for b in text.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

/// A request taken apart.
struct Req<'a> {
    ex: &'a Exchange,
    path: String,
    /// Without cache busters, sorted.
    query: Vec<(String, String)>,
    /// Cache busters left out.
    busters: Vec<String>,
    graphql: Option<String>,
}

/// What makes requests the same mock.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    graphql: Option<String>,
}

/// Records `exchanges` as a mock set.
pub fn record(exchanges: &[Exchange], options: &RecordOptions) -> Result<Recorded, String> {
    let scope = Scope::new(&options.scope)?;
    let mut out = Recorded::default();
    let mut reqs: Vec<Req<'_>> = Vec::new();
    for ex in exchanges {
        let Ok(url) = url::Url::parse(&ex.url) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https") {
            continue;
        }
        if !scope.matches(url.as_str(), &ex.resource_type) {
            continue;
        }
        let path = url.path().to_owned();
        let shown = format!("{} {}", ex.method, short_url(&url));
        if ex.method == "OPTIONS" {
            out.skipped.push(format!(
                "{shown}: a CORS preflight (cors = \"auto\" answers them)"
            ));
            continue;
        }
        if ex.status == 0 {
            out.skipped.push(format!(
                "{shown}: it got no answer (failed or still loading)"
            ));
            continue;
        }
        let mut query = Vec::new();
        let mut busters = Vec::new();
        for (k, v) in url.query_pairs() {
            if CACHE_BUSTERS.contains(&k.as_ref()) || looks_like_epoch(&v) {
                busters.push(k.into_owned());
            } else {
                query.push((k.into_owned(), v.into_owned()));
            }
        }
        query.sort();
        let graphql = graphql_operation(ex, &url);
        reqs.push(Req {
            ex,
            path,
            query,
            busters,
            graphql,
        });
    }
    out.requests = reqs.len();
    out.scope = if options.scope.is_empty() {
        common_scope(&reqs).into_iter().collect()
    } else {
        options.scope.clone()
    };

    // Groups in the order first seen.
    let mut order: Vec<Key> = Vec::new();
    let mut groups: HashMap<Key, Vec<&Req<'_>>> = HashMap::new();
    for req in &reqs {
        let key = Key {
            method: req.ex.method.clone(),
            path: req.path.clone(),
            query: req.query.clone(),
            graphql: req.graphql.clone(),
        };
        groups
            .entry(key.clone())
            .or_insert_with(|| {
                order.push(key);
                Vec::new()
            })
            .push(req);
    }
    // A mock with fewer query conditions matches the requests of one with
    // more: the more specific goes first.
    let first_seen: HashMap<Key, usize> = order
        .iter()
        .enumerate()
        .map(|(i, k)| (k.clone(), i))
        .collect();
    let mut first_of_path: HashMap<(String, String), usize> = HashMap::new();
    for (i, k) in order.iter().enumerate() {
        first_of_path
            .entry((k.method.clone(), k.path.clone()))
            .or_insert(i);
    }
    order.sort_by_key(|k| {
        (
            first_of_path[&(k.method.clone(), k.path.clone())],
            std::cmp::Reverse(k.query.len() + usize::from(k.graphql.is_some())),
            first_seen[k],
        )
    });

    let needed_cookies = cookies_sent(exchanges);
    let mut files = Files::new(&options.data_dir);
    let mut text = String::new();
    let _ = writeln!(text, "# {}", options.origin);
    text.push_str(
        "# Each [[mock]] is a request as it was sent, and what it got; `chrome-cdp mock check` \
         lints the file.\n",
    );
    text.push_str("\n[settings]\n");
    if out.scope.is_empty() {
        text.push_str(
            "# No scope: the recorded paths share no first segment. Add one, e.g. scope = [\"/api/**\"],\n\
             # so requests no mock answers fail instead of reaching the network.\n",
        );
    } else {
        let _ = writeln!(text, "scope = {}", toml_list(&out.scope));
    }
    for key in &order {
        let group = &groups[key];
        out.mocks += 1;
        text.push('\n');
        let mut comments = Vec::new();
        let ids = path_ids(&key.path);
        if !ids.is_empty() {
            comments.push(format!(
                "ids in the path ({}): path = \"{}\" matches every id, and \
                 {{{{params.id}}}} in json_file picks each one's data file",
                ids.join(", "),
                param_path(&key.path)
            ));
        }
        let busters: Vec<&str> = group
            .iter()
            .flat_map(|r| r.busters.iter().map(String::as_str))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        if !busters.is_empty() {
            comments.push(format!(
                "not matched: the query's cache busters ({})",
                busters.join(", ")
            ));
        }
        // Answers in order; repeats of the last are what repeat-last
        // gives anyway.
        let mut answers: Vec<Answer> = group
            .iter()
            .map(|r| Answer::of(r.ex, &needed_cookies))
            .collect();
        while answers.len() > 1 && answers[answers.len() - 1] == answers[answers.len() - 2] {
            answers.pop();
        }
        let stem = files.stem(key);
        let many = answers.len() > 1;
        let mut written = Vec::new();
        for (i, answer) in answers.iter().enumerate() {
            let name = if many {
                format!("{stem}-{}", i + 1)
            } else {
                stem.clone()
            };
            let (respond, notes) = answer.respond(&name, &mut files, &mut out.files);
            comments.extend(notes);
            written.push(respond);
        }
        comments.dedup();
        for c in &comments {
            let _ = writeln!(text, "# {c}");
        }
        text.push_str("[[mock]]\n");
        let _ = writeln!(text, "name = {}", toml_string(&mock_name(key)));
        let mut matcher = vec![
            format!("method = {}", toml_string(&key.method)),
            format!("path = {}", toml_string(&key.path)),
        ];
        if !key.query.is_empty() {
            let pairs: Vec<String> = key
                .query
                .iter()
                .map(|(k, v)| format!("{} = {}", toml_key(k), toml_string(v)))
                .collect();
            matcher.push(format!("query = {{ {} }}", pairs.join(", ")));
        }
        if let Some(op) = &key.graphql {
            matcher.push(format!("graphql = {{ operation = {} }}", toml_string(op)));
        }
        let _ = writeln!(text, "match = {{ {} }}", matcher.join(", "));
        if many {
            text.push_str("sequence = [\n");
            for r in &written {
                let _ = writeln!(text, "  {r},");
            }
            text.push_str("]\n");
        } else if let Some(r) = written.first() {
            let _ = writeln!(text, "respond = {r}");
        }
    }
    out.toml = text;
    Ok(out)
}

/// `/api/products?page=2` of `url`, without the origin.
fn short_url(url: &url::Url) -> String {
    match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_owned(),
    }
}

/// `GET /api/search?q=a`, `POST /graphql GetCart`.
fn mock_name(key: &Key) -> String {
    let mut name = format!("{} {}", key.method, key.path);
    if !key.query.is_empty() {
        let q: Vec<String> = key.query.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let _ = write!(name, "?{}", q.join("&"));
    }
    if let Some(op) = &key.graphql {
        let _ = write!(name, " {op}");
    }
    name
}

/// A GraphQL request's operation: `operationName`, else the first named
/// operation of the query (in the body, or the GET query string).
fn graphql_operation(ex: &Exchange, url: &url::Url) -> Option<String> {
    let (name, query) = if ex.method == "GET" {
        let get = |key: &str| {
            url.query_pairs()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.into_owned())
        };
        (get("operationName"), get("query")?)
    } else {
        let body: Value = serde_json::from_str(ex.request_body.as_deref()?).ok()?;
        (
            body.get("operationName")
                .and_then(Value::as_str)
                .map(str::to_owned),
            body.get("query")?.as_str()?.to_owned(),
        )
    };
    if let Some(name) = name.filter(|n| !n.is_empty()) {
        return Some(name);
    }
    let mut words = query.split(|c: char| !(c.is_alphanumeric() || c == '_'));
    while let Some(word) = words.next() {
        if matches!(word, "query" | "mutation" | "subscription") {
            return words.find(|w| !w.is_empty()).map(str::to_owned);
        }
    }
    None
}

/// `/api/**` when every request's path starts with `/api/`.
fn common_scope(reqs: &[Req<'_>]) -> Option<String> {
    let first = |p: &str| {
        let seg = p.trim_start_matches('/').split('/').next()?.to_owned();
        (!seg.is_empty() && p.trim_start_matches('/').contains('/')).then_some(seg)
    };
    let mut segs = reqs.iter().map(|r| first(&r.path));
    let one = segs.next()??;
    segs.all(|s| s.as_deref() == Some(&one))
        .then(|| format!("/{one}/**"))
}

/// The cookie names requests sent.
fn cookies_sent(exchanges: &[Exchange]) -> HashSet<String> {
    let mut names = HashSet::new();
    for ex in exchanges {
        for (n, v) in &ex.request_headers {
            if n.eq_ignore_ascii_case("cookie") {
                for pair in v.split(';') {
                    if let Some((name, _)) = pair.split_once('=') {
                        names.insert(name.trim().to_owned());
                    }
                }
            }
        }
    }
    names
}

/// A recorded answer.
#[derive(Debug, Clone, PartialEq)]
struct Answer {
    status: u16,
    /// Kept headers, in order.
    headers: Vec<(String, String)>,
    body: Body,
    /// Set-cookie headers left out.
    dropped_cookies: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum Body {
    NotRecorded,
    Empty,
    Json(Value),
    Text { text: String, ext: &'static str },
    Binary { bytes: Vec<u8>, ext: &'static str },
}

impl Answer {
    fn of(ex: &Exchange, needed_cookies: &HashSet<String>) -> Self {
        let mime = ex.mime.to_ascii_lowercase();
        let header_mime = ex
            .response_headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.to_ascii_lowercase())
            .unwrap_or_default();
        let mime = if mime.is_empty() { header_mime } else { mime };
        let body = match &ex.response_body {
            None => Body::NotRecorded,
            Some(b) if b.is_empty() => Body::Empty,
            Some(b) => classify(b, &mime),
        };
        let mut headers = Vec::new();
        let mut dropped_cookies = false;
        for (name, value) in &ex.response_headers {
            let lower = name.to_ascii_lowercase();
            if lower == "set-cookie" {
                // One header may hold several cookies, one per line.
                for cookie in value.lines() {
                    let cname = cookie.split('=').next().unwrap_or_default().trim();
                    if needed_cookies.contains(cname) {
                        headers.push(("set-cookie".to_owned(), cookie.trim().to_owned()));
                    } else {
                        dropped_cookies = true;
                    }
                }
                continue;
            }
            if VOLATILE.contains(&lower.as_str())
                || lower.starts_with("access-control-")
                || lower.starts_with("x-amz")
                || lower.starts_with("cf-")
                || lower.starts_with(':')
            {
                continue;
            }
            headers.push((lower, value.clone()));
        }
        // The content type, when the body's kind would not give it.
        let given = match &body {
            Body::Json(_) => Some("application/json"),
            Body::Text { ext, .. } | Body::Binary { ext, .. } => Some(ext_type(ext)),
            Body::Empty | Body::NotRecorded => None,
        };
        if let Some(given) = given
            && !mime.is_empty()
            && essence(&mime) != essence(given)
        {
            headers.insert(0, ("content-type".to_owned(), mime.clone()));
        }
        Self {
            status: ex.status,
            headers,
            body,
            dropped_cookies,
        }
    }

    /// The `respond` inline table; data files go to `files`.
    fn respond(
        &self,
        name: &str,
        paths: &mut Files,
        files: &mut Vec<(String, Vec<u8>)>,
    ) -> (String, Vec<String>) {
        let mut parts = Vec::new();
        let mut notes = Vec::new();
        if self.status != 200 {
            parts.push(format!("status = {}", self.status));
        }
        if !self.headers.is_empty() {
            let pairs: Vec<String> = self
                .headers
                .iter()
                .map(|(n, v)| format!("{} = {}", toml_key(n), toml_string(v)))
                .collect();
            parts.push(format!("headers = {{ {} }}", pairs.join(", ")));
        }
        match &self.body {
            Body::NotRecorded => {
                notes.push(
                    "the response body was not recorded (Chrome no longer had it); add one"
                        .to_owned(),
                );
            }
            Body::Empty => {}
            Body::Json(value) => {
                let spots = volatile_values(value);
                if !spots.ids.is_empty() {
                    notes.push(format!(
                        "ids in the body ({}): {{{{uuid}}}} or {{{{seq}}}} in a .tmpl.json data \
                         file make new ones",
                        spots.ids.join(", ")
                    ));
                }
                if !spots.times.is_empty() {
                    notes.push(format!(
                        "timestamps in the body ({}): {{{{now}}}} in a .tmpl.json data file \
                         follows the test's frozen time",
                        spots.times.join(", ")
                    ));
                }
                if let Some(inline) = inline_json(value) {
                    parts.push(format!("json = {inline}"));
                } else {
                    let path = paths.take(name, "json");
                    let mut bytes =
                        serde_json::to_vec_pretty(value).unwrap_or_else(|_| b"null".to_vec());
                    bytes.push(b'\n');
                    files.push((path.clone(), bytes));
                    parts.push(format!("json_file = {}", toml_string(&path)));
                }
            }
            Body::Text { text, ext } => {
                if text.chars().count() <= INLINE && !text.contains('\n') && !is_template(text) {
                    parts.push(format!("body = {}", toml_string(text)));
                } else {
                    let path = paths.take(name, ext);
                    files.push((path.clone(), text.as_bytes().to_vec()));
                    parts.push(format!("body_file = {}", toml_string(&path)));
                }
            }
            Body::Binary { bytes, ext } => {
                let path = paths.take(name, ext);
                files.push((path.clone(), bytes.clone()));
                parts.push(format!("file = {}", toml_string(&path)));
            }
        }
        if self.dropped_cookies {
            notes.push(
                "set-cookie left out: no later request sent the cookie; add it to headers if the \
                 app needs it"
                    .to_owned(),
            );
        }
        (format!("{{ {} }}", parts.join(", ")), notes)
    }
}

/// `application/json` of `application/json; charset=utf-8`.
fn essence(mime: &str) -> &str {
    mime.split(';').next().unwrap_or(mime).trim()
}

/// The content type mocks give a data file with extension `ext`.
fn ext_type(ext: &str) -> &'static str {
    crate::respond::content_type(std::path::Path::new(&format!("x.{ext}")))
}

/// What a body is, by its MIME type (else its content).
fn classify(bytes: &[u8], mime: &str) -> Body {
    let mime = essence(mime);
    let text = std::str::from_utf8(bytes).ok();
    if (mime.contains("json") || mime.is_empty())
        && let Some(value) = text.and_then(|t| serde_json::from_str::<Value>(t).ok())
    {
        return Body::Json(value);
    }
    let ext = match mime {
        "text/html" => "html",
        "text/css" => "css",
        "text/csv" => "csv",
        "text/javascript" | "application/javascript" => "js",
        "application/xml" | "text/xml" => "xml",
        "image/svg+xml" => "svg",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
        "font/woff" => "woff",
        "font/woff2" => "woff2",
        "application/pdf" => "pdf",
        "application/wasm" => "wasm",
        m if m.starts_with("text/") => "txt",
        _ if text.is_some() && mime.is_empty() => "txt",
        _ => "bin",
    };
    match text {
        Some(t) if matches!(ext, "html" | "css" | "csv" | "js" | "xml" | "svg" | "txt") => {
            Body::Text {
                text: t.to_owned(),
                ext,
            }
        }
        _ => Body::Binary {
            bytes: bytes.to_vec(),
            ext,
        },
    }
}

/// Data file names: `data/<set>/api/products/1.json`, unique.
struct Files {
    dir: String,
    used: HashSet<String>,
    stems: HashSet<String>,
}

impl Files {
    fn new(dir: &str) -> Self {
        Self {
            dir: dir.trim_end_matches('/').to_owned(),
            used: HashSet::new(),
            stems: HashSet::new(),
        }
    }

    /// A stem no other mock has: the path's segments, with the method
    /// when not GET, the query or operation, and a number when needed.
    fn stem(&mut self, key: &Key) -> String {
        let mut segs: Vec<String> = key
            .path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| clean(&crate::glob::decode(s)))
            .collect();
        if segs.is_empty() {
            segs.push("index".to_owned());
        }
        let mut last = segs.pop().unwrap_or_default();
        if key.method != "GET" {
            let _ = write!(last, ".{}", clean(&key.method.to_ascii_lowercase()));
        }
        if let Some(op) = &key.graphql {
            let _ = write!(last, ".{}", clean(op));
        }
        if !key.query.is_empty() {
            let q: Vec<String> = key
                .query
                .iter()
                .map(|(k, v)| clean(&format!("{k}-{v}")))
                .collect();
            let mut q = q.join("_");
            if q.len() > 40 {
                q.truncate(40);
            }
            let _ = write!(last, ".{q}");
        }
        segs.push(last);
        let base = segs.join("/");
        let mut stem = base.clone();
        let mut n = 2;
        while !self.stems.insert(stem.clone()) {
            stem = format!("{base}-{n}");
            n += 1;
        }
        stem
    }

    /// The data file path for `stem` with extension `ext`.
    fn take(&mut self, stem: &str, ext: &str) -> String {
        // `products.json` stays that, not `products.json.json`.
        let stem = stem.strip_suffix(&format!(".{ext}")).unwrap_or(stem);
        let mut path = format!("{}/{stem}.{ext}", self.dir);
        let mut n = 2;
        while !self.used.insert(path.clone()) {
            path = format!("{}/{stem}-{n}.{ext}", self.dir);
            n += 1;
        }
        path
    }
}

/// Whether `b` may be in an HTTP token (RFC 9110 `tchar`), such as a
/// method.
fn is_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

/// `a-z0-9._-` only.
fn clean(text: &str) -> String {
    let out: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let out = out.trim_matches('.').to_owned();
    if out.is_empty() { "_".to_owned() } else { out }
}

/// Whether a path segment looks like an id: a number, a UUID, a long
/// hex or mixed token.
fn is_id(seg: &str) -> bool {
    if seg.is_empty() {
        return false;
    }
    if seg.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    if is_uuid(seg) {
        return true;
    }
    seg.len() >= 16
        && seg.chars().all(|c| c.is_ascii_hexdigit())
        && seg.chars().any(|c| c.is_ascii_digit())
}

fn is_uuid(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The ids in `path`'s segments.
fn path_ids(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|s| is_id(s))
        .map(str::to_owned)
        .collect()
}

/// `/api/products/:id` for `/api/products/1`.
fn param_path(path: &str) -> String {
    let mut n = 0;
    path.split('/')
        .map(|s| {
            if is_id(s) {
                n += 1;
                if n == 1 {
                    ":id".to_owned()
                } else {
                    format!(":id{n}")
                }
            } else {
                s.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether a query value is a Unix time in ms (a cache buster).
fn looks_like_epoch(v: &str) -> bool {
    v.len() == 13
        && v.chars().all(|c| c.is_ascii_digit())
        && v.parse::<u64>()
            .is_ok_and(|n| (1_000_000_000_000..5_000_000_000_000).contains(&n))
}

/// Where a body has ids and timestamps (a few of each, as JSON pointers
/// `$.items[0].id`).
#[derive(Default)]
struct Spots {
    ids: Vec<String>,
    times: Vec<String>,
}

const SPOTS: usize = 3;

fn volatile_values(value: &Value) -> Spots {
    fn walk(v: &Value, at: &str, key: &str, out: &mut Spots, seen: &mut HashSet<String>) {
        match v {
            Value::Object(map) => {
                for (k, v) in map {
                    let at = if k.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        format!("{at}.{k}")
                    } else {
                        format!("{at}[{}]", serde_json::to_string(k).unwrap_or_default())
                    };
                    walk(v, &at, k, out, seen);
                }
            }
            Value::Array(items) => {
                // The first items are enough to say where.
                for (i, v) in items.iter().take(2).enumerate() {
                    walk(v, &format!("{at}[{i}]"), key, out, seen);
                }
            }
            Value::String(s) => {
                if is_timestamp(s) {
                    push(&mut out.times, at, seen);
                } else if is_uuid(s) {
                    push(&mut out.ids, at, seen);
                }
            }
            Value::Number(n) => {
                let lower = key.to_ascii_lowercase();
                let time_key = lower.ends_with("_at")
                    || (lower.ends_with("at")
                        && key.chars().rev().nth(1).is_some_and(char::is_uppercase))
                    || lower.contains("time")
                    || lower.contains("date");
                if time_key
                    && n.as_u64()
                        .is_some_and(|n| (1_000_000_000..5_000_000_000_000).contains(&n))
                {
                    push(&mut out.times, at, seen);
                }
            }
            _ => {}
        }
    }
    fn push(list: &mut Vec<String>, at: &str, seen: &mut HashSet<String>) {
        // `$.items[0].id` and `$.items[1].id` are one place.
        let general = at
            .split('[')
            .map(|p| p.split_once(']').map_or(p, |(_, rest)| rest))
            .collect::<Vec<_>>()
            .join("[]");
        if list.len() < SPOTS && seen.insert(general) {
            list.push(at.to_owned());
        }
    }
    let mut out = Spots::default();
    let mut seen = HashSet::new();
    walk(value, "$", "", &mut out, &mut seen);
    out
}

/// `2026-01-01T10:00:00Z` and the like.
fn is_timestamp(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 16
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[7] == b'-'
        && matches!(b[10], b'T' | b' ')
        && b[13] == b':'
}

/// A JSON value as an inline TOML value, when it is small and TOML can
/// say it (no `null`) and has no text a template would take.
fn inline_json(value: &Value) -> Option<String> {
    let compact = serde_json::to_string(value).ok()?;
    if compact.len() > INLINE {
        return None;
    }
    toml_value(value)
}

fn toml_value(value: &Value) -> Option<String> {
    Some(match value {
        Value::Null => return None,
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            if n.is_f64() {
                let f = n.as_f64()?;
                if !f.is_finite() {
                    return None;
                }
                let s = f.to_string();
                if s.contains(['.', 'e', 'E']) {
                    s
                } else {
                    format!("{s}.0")
                }
            } else {
                n.to_string()
            }
        }
        Value::String(s) => {
            if is_template(s) || s.contains("{#") {
                return None;
            }
            toml_string(s)
        }
        Value::Array(items) => {
            let items: Option<Vec<String>> = items.iter().map(toml_value).collect();
            format!("[{}]", items?.join(", "))
        }
        Value::Object(map) => {
            if map.is_empty() {
                return Some("{}".to_owned());
            }
            let pairs: Option<Vec<String>> = map
                .iter()
                .map(|(k, v)| Some(format!("{} = {}", toml_key(k), toml_value(v)?)))
                .collect();
            format!("{{ {} }}", pairs?.join(", "))
        }
    })
}

/// A TOML basic string.
pub(crate) fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A TOML key: bare when it can be.
fn toml_key(k: &str) -> String {
    if !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        k.to_owned()
    } else {
        toml_string(k)
    }
}

fn toml_list(items: &[String]) -> String {
    let items: Vec<String> = items.iter().map(|s| toml_string(s)).collect();
    format!("[{}]", items.join(", "))
}
