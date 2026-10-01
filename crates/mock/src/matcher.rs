//! Matching a request against a mock's `match`, with the reason when it
//! does not match (`mock explain`, and plan 11.5's unmatched diagnostics).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use crate::glob::{Glob, PathGlob, decode};
use crate::multipart::{self, FormFile};

/// A request, as mocks see it.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub method: &'a str,
    /// An absolute URL.
    pub url: &'a str,
    pub headers: &'a [(String, String)],
    /// The body, if the request has one.
    pub body: Option<&'a [u8]>,
}

/// A request taken apart once for all mocks.
#[derive(Debug)]
pub(crate) struct Parsed {
    /// Upper case.
    pub(crate) method: String,
    pub(crate) url: String,
    /// The URL without query and fragment.
    pub(crate) base: String,
    pub(crate) path: String,
    /// Decoded.
    pub(crate) query: Vec<(String, String)>,
    /// Lower-case names.
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: String,
    pub(crate) json: Option<Value>,
    /// The fields of a urlencoded or multipart body; a multipart file
    /// field's value is its file name.
    pub(crate) form: Option<Vec<(String, String)>>,
    /// A multipart body's file fields.
    pub(crate) files: Vec<(String, FormFile)>,
    /// Whether the body is multipart (`{{form_files}}` is there).
    pub(crate) multipart: bool,
    /// Why a body that claims to be a form is not one.
    pub(crate) form_error: Option<String>,
}

impl Parsed {
    pub(crate) fn new(req: &Request<'_>) -> Self {
        let (base, path, query) = if let Ok(mut url) = url::Url::parse(req.url) {
            let query = url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            let path = url.path().to_owned();
            url.set_query(None);
            url.set_fragment(None);
            (url.to_string(), path, query)
        } else {
            let base = req.url.split(['?', '#']).next().unwrap_or(req.url);
            (base.to_owned(), base.to_owned(), Vec::new())
        };
        let headers: Vec<(String, String)> = req
            .headers
            .iter()
            .map(|(n, v)| (n.to_ascii_lowercase(), v.clone()))
            .collect();
        let body = req
            .body
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        let json = if body.trim_start().starts_with(['{', '[']) {
            serde_json::from_str(&body).ok()
        } else {
            None
        };
        let raw_type = headers
            .iter()
            .find(|(n, _)| n == "content-type")
            .map(|(_, v)| v.as_str())
            .unwrap_or_default();
        let content_type = raw_type.to_ascii_lowercase();
        let mut files = Vec::new();
        let mut form_error = None;
        let multipart = multipart::boundary(raw_type);
        let form = if let Some(boundary) = &multipart {
            match boundary
                .clone()
                .and_then(|b| multipart::parse(req.body.unwrap_or_default(), &b))
            {
                Ok(parts) => Some(
                    parts
                        .into_iter()
                        .map(|p| {
                            if let Some(file) = p.file {
                                files.push((p.name.clone(), file));
                            }
                            (p.name, p.value)
                        })
                        .collect(),
                ),
                Err(e) => {
                    form_error = Some(e);
                    None
                }
            }
        } else {
            (content_type.starts_with("application/x-www-form-urlencoded")
                || (json.is_none() && !body.is_empty() && content_type.is_empty()))
            .then(|| {
                url::form_urlencoded::parse(body.as_bytes())
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect()
            })
        };
        Self {
            method: req.method.to_ascii_uppercase(),
            url: req.url.to_owned(),
            base,
            path,
            query,
            headers,
            body,
            json,
            form,
            files,
            multipart: multipart.is_some(),
            form_error,
        }
    }

    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Where a mock's URL condition looks.
#[derive(Debug, Clone)]
pub(crate) enum Target {
    Path(PathGlob),
    Url(PathGlob),
    Regex(Regex),
}

/// A condition on the body.
#[derive(Debug, Clone)]
pub(crate) enum BodyMatcher {
    Json {
        value: Value,
        contains: bool,
    },
    Form(Vec<(String, Glob)>),
    Glob(Glob),
    Regex(Regex),
    Graphql {
        operation: Option<String>,
        variables: Option<Value>,
    },
}

/// A mock's `match`, compiled.
#[derive(Debug, Clone, Default)]
pub(crate) struct Matcher {
    /// Upper case; empty for any.
    pub(crate) methods: Vec<String>,
    pub(crate) target: Option<Target>,
    pub(crate) query: Vec<(String, Glob)>,
    /// Lower-case names.
    pub(crate) headers: Vec<(String, Glob)>,
    pub(crate) body: Option<BodyMatcher>,
}

/// How much of a value a reason shows.
const SHOWN: usize = 60;

fn shown(text: &str) -> String {
    let cut: String = text.chars().take(SHOWN).collect();
    if cut.len() < text.len() {
        format!("{cut}…")
    } else {
        cut
    }
}

impl Matcher {
    /// The path parameters (and regex groups) when `req` matches; else the
    /// first condition that fails. `method` stands in for the request's
    /// (a preflight asks about another method); `request_only` skips the
    /// header and body conditions, which a preflight cannot show.
    pub(crate) fn check(
        &self,
        req: &Parsed,
        method: &str,
        request_only: bool,
    ) -> Result<BTreeMap<String, String>, String> {
        if !self.methods.is_empty() && !self.methods.iter().any(|m| m == method) {
            return Err(format!(
                "method differs: expects {}, got {method}",
                self.methods.join(" or ")
            ));
        }
        let params = self.check_target(req)?;
        self.check_query(req)?;
        if request_only {
            return Ok(params);
        }
        self.check_headers(req)?;
        if let Some(body) = &self.body {
            check_body(body, req)?;
        }
        Ok(params)
    }

    /// Whether the match has header or body conditions, the ones a
    /// preflight cannot show.
    pub(crate) fn reads_request(&self) -> bool {
        !self.headers.is_empty() || self.body.is_some()
    }

    /// The path, URL or regex condition: the parameters it captures.
    pub(crate) fn check_target(&self, req: &Parsed) -> Result<BTreeMap<String, String>, String> {
        Ok(match &self.target {
            None => BTreeMap::new(),
            Some(Target::Path(glob)) => glob
                .captures(&req.path)
                .ok_or_else(|| format!("path differs: expects {glob}, got {}", shown(&req.path)))?,
            Some(Target::Url(glob)) => glob
                .captures(&req.base)
                .ok_or_else(|| format!("URL differs: expects {glob}, got {}", shown(&req.base)))?,
            Some(Target::Regex(regex)) => {
                let caps = regex
                    .captures(&req.url)
                    .ok_or_else(|| format!("URL does not match regex {}", shown(regex.as_str())))?;
                regex
                    .capture_names()
                    .flatten()
                    .filter_map(|n| Some((n.to_owned(), decode(caps.name(n)?.as_str()))))
                    .collect()
            }
        })
    }

    pub(crate) fn check_query(&self, req: &Parsed) -> Result<(), String> {
        for (name, glob) in &self.query {
            let values: Vec<&str> = req
                .query
                .iter()
                .filter(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
                .collect();
            if values.is_empty() {
                return Err(format!("query {name}: missing"));
            }
            if !values.iter().any(|v| glob.matches(v)) {
                return Err(format!(
                    "query {name}: expects {}, got {}",
                    quoted(glob.as_str()),
                    quoted(values[0])
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn check_headers(&self, req: &Parsed) -> Result<(), String> {
        for (name, glob) in &self.headers {
            let Some(value) = req.header(name) else {
                return Err(format!("header {name}: missing"));
            };
            if !glob.matches(value) {
                return Err(format!(
                    "header {name}: expects {}, got {}",
                    quoted(glob.as_str()),
                    quoted(value)
                ));
            }
        }
        Ok(())
    }

    /// What it matches, in a few words: `GET /api/products/:id (query q,
    /// json)`.
    pub(crate) fn summary(&self) -> String {
        let mut text = if self.methods.is_empty() {
            String::new()
        } else {
            format!("{} ", self.methods.join(","))
        };
        match &self.target {
            None => text.push_str("any URL"),
            Some(Target::Path(glob) | Target::Url(glob)) => {
                let _ = write!(text, "{glob}");
            }
            Some(Target::Regex(regex)) => {
                let _ = write!(text, "regex {}", regex.as_str());
            }
        }
        let mut more: Vec<String> = Vec::new();
        for (name, _) in &self.query {
            more.push(format!("query {name}"));
        }
        for (name, _) in &self.headers {
            more.push(format!("header {name}"));
        }
        match &self.body {
            None => {}
            Some(BodyMatcher::Json { .. }) => more.push("json".into()),
            Some(BodyMatcher::Form(_)) => more.push("form".into()),
            Some(BodyMatcher::Glob(_) | BodyMatcher::Regex(_)) => more.push("body".into()),
            Some(BodyMatcher::Graphql { operation, .. }) => more.push(match operation {
                Some(op) => format!("graphql {op}"),
                None => "graphql".into(),
            }),
        }
        if !more.is_empty() {
            let _ = write!(text, " ({})", more.join(", "));
        }
        text
    }
}

fn quoted(text: &str) -> String {
    format!("\"{}\"", shown(text))
}

pub(crate) fn check_body(body: &BodyMatcher, req: &Parsed) -> Result<(), String> {
    match body {
        BodyMatcher::Json { value, contains } => {
            let Some(json) = &req.json else {
                return Err(if req.body.is_empty() {
                    "json: the request has no body".into()
                } else {
                    format!("json: the body is not JSON: {}", quoted(&req.body))
                });
            };
            json_subset(value, json, *contains, &mut "json $".to_owned())
        }
        BodyMatcher::Form(fields) => {
            let Some(form) = &req.form else {
                return Err(match &req.form_error {
                    Some(why) => format!("form: the body is not a form ({why})"),
                    None => "form: the body is not a form".into(),
                });
            };
            for (name, glob) in fields {
                let values: Vec<&str> = form
                    .iter()
                    .filter(|(k, _)| k == name)
                    .map(|(_, v)| v.as_str())
                    .collect();
                if values.is_empty() {
                    return Err(format!("form {name}: missing"));
                }
                if !values.iter().any(|v| glob.matches(v)) {
                    return Err(format!(
                        "form {name}: expects {}, got {}",
                        quoted(glob.as_str()),
                        quoted(values[0])
                    ));
                }
            }
            Ok(())
        }
        BodyMatcher::Glob(glob) => {
            if glob.matches(&req.body) {
                Ok(())
            } else {
                Err(format!(
                    "body differs: expects {}, got {}",
                    quoted(glob.as_str()),
                    quoted(&req.body)
                ))
            }
        }
        BodyMatcher::Regex(regex) => {
            if regex.is_match(&req.body) {
                Ok(())
            } else {
                Err(format!(
                    "body does not match regex {}",
                    shown(regex.as_str())
                ))
            }
        }
        BodyMatcher::Graphql {
            operation,
            variables,
        } => {
            let Some(gql) = graphql(req) else {
                return Err("graphql: the request is no GraphQL request".into());
            };
            if let Some(wanted) = operation
                && gql.operation.as_deref() != Some(wanted)
            {
                return Err(format!(
                    "graphql operation: expects {wanted}, got {}",
                    gql.operation.as_deref().unwrap_or("none")
                ));
            }
            if let Some(wanted) = variables {
                let got = gql.variables.unwrap_or(Value::Null);
                json_subset(wanted, &got, false, &mut "graphql variables $".to_owned())?;
            }
            Ok(())
        }
    }
}

/// A GraphQL request's operation name and variables.
#[derive(Debug)]
struct Graphql {
    operation: Option<String>,
    variables: Option<Value>,
}

fn graphql(req: &Parsed) -> Option<Graphql> {
    static NAME: OnceLock<Regex> = OnceLock::new();
    let (query, name, variables) = match &req.json {
        Some(Value::Object(body)) if body.contains_key("query") => (
            body.get("query")?.as_str()?.to_owned(),
            body.get("operationName")
                .and_then(Value::as_str)
                .map(str::to_owned),
            body.get("variables").cloned(),
        ),
        _ => {
            let get = |key: &str| {
                req.query
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.clone())
            };
            (
                get("query")?,
                get("operationName"),
                get("variables").and_then(|v| serde_json::from_str(&v).ok()),
            )
        }
    };
    let named = NAME.get_or_init(|| {
        Regex::new(r"\b(?:query|mutation|subscription)\s+([_A-Za-z][_0-9A-Za-z]*)")
            .unwrap_or_else(|_| unreachable!())
    });
    let operation = name
        .filter(|n| !n.is_empty())
        .or_else(|| named.captures(&query).map(|c| c[1].to_owned()));
    Some(Graphql {
        operation,
        variables,
    })
}

/// Whether `actual` holds `expected`: objects match a subset of keys,
/// `"*"` any value, strings with `*` are globs, arrays match element by
/// element (or, with `contains`, each expected element some element).
/// `path` is where in the body this is, for the reason.
pub(crate) fn json_subset(
    expected: &Value,
    actual: &Value,
    contains: bool,
    path: &mut String,
) -> Result<(), String> {
    let differs = |path: &str| {
        format!(
            "{path}: expects {}, got {}",
            shown(&expected.to_string()),
            shown(&actual.to_string())
        )
    };
    match (expected, actual) {
        (Value::String(s), _) if s == "*" => Ok(()),
        (Value::String(glob), Value::String(value)) if glob.contains('*') => {
            if Glob::new(glob).matches(value) {
                Ok(())
            } else {
                Err(differs(path))
            }
        }
        (Value::Object(want), Value::Object(got)) => {
            for (key, value) in want {
                let len = path.len();
                if is_plain_key(key) {
                    let _ = write!(path, ".{key}");
                } else {
                    let _ = write!(path, "[{}]", Value::String(key.clone()));
                }
                let Some(inner) = got.get(key) else {
                    return Err(format!("{path}: missing"));
                };
                json_subset(value, inner, contains, path)?;
                path.truncate(len);
            }
            Ok(())
        }
        (Value::Array(want), Value::Array(got)) => {
            if contains {
                for (i, item) in want.iter().enumerate() {
                    let found = got
                        .iter()
                        .any(|g| json_subset(item, g, contains, &mut String::new()).is_ok());
                    if !found {
                        return Err(format!(
                            "{path}: no element matches {} (expected element {i})",
                            shown(&item.to_string())
                        ));
                    }
                }
                return Ok(());
            }
            if want.len() != got.len() {
                return Err(format!(
                    "{path}: expects {} elements, got {} (json_mode = \"contains\" matches some)",
                    want.len(),
                    got.len()
                ));
            }
            for (i, (w, g)) in want.iter().zip(got).enumerate() {
                let len = path.len();
                let _ = write!(path, "[{i}]");
                json_subset(w, g, contains, path)?;
                path.truncate(len);
            }
            Ok(())
        }
        (Value::Number(a), Value::Number(b)) if a.as_f64() == b.as_f64() => Ok(()),
        (a, b) if a == b => Ok(()),
        _ => Err(differs(path)),
    }
}

/// Whether `actual` holds `expected` as a mock's `match.json` checks it
/// (objects a subset of keys, `"*"` any value, strings with `*` globs).
pub fn json_contains(expected: &Value, actual: &Value) -> bool {
    json_subset(expected, actual, false, &mut String::new()).is_ok()
}

fn is_plain_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parsed(method: &str, url: &str, headers: &[(&str, &str)], body: &str) -> Parsed {
        let headers: Vec<(String, String)> = headers
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect();
        Parsed::new(&Request {
            method,
            url,
            headers: &headers,
            body: (!body.is_empty()).then_some(body.as_bytes()),
        })
    }

    #[allow(clippy::needless_pass_by_value)]
    fn subset(expected: Value, actual: Value, contains: bool) -> Result<(), String> {
        json_subset(&expected, &actual, contains, &mut "json $".to_owned())
    }

    #[test]
    fn json_subsets() {
        let body = json!({"email": "a@example.com", "qty": 2, "tags": ["x", "y"], "user": {"id": 7, "name": "Ada"}});
        assert!(subset(json!({"email": "*@example.com"}), body.clone(), false).is_ok());
        assert!(subset(json!({"qty": 2.0, "user": {"id": 7}}), body.clone(), false).is_ok());
        assert!(subset(json!({"user": "*"}), body.clone(), false).is_ok());
        assert!(subset(json!({"tags": ["x", "*"]}), body.clone(), false).is_ok());
        assert!(subset(json!({"tags": ["y"]}), body.clone(), true).is_ok());
        assert_eq!(
            subset(json!({"email": "*@corp.com"}), body.clone(), false).unwrap_err(),
            r#"json $.email: expects "*@corp.com", got "a@example.com""#
        );
        assert_eq!(
            subset(json!({"user": {"role": "admin"}}), body.clone(), false).unwrap_err(),
            "json $.user.role: missing"
        );
        assert!(
            subset(json!({"tags": ["y"]}), body.clone(), false)
                .unwrap_err()
                .starts_with("json $.tags: expects 1 elements, got 2")
        );
        assert_eq!(
            subset(json!({"tags": ["x", "z"]}), body.clone(), false).unwrap_err(),
            r#"json $.tags[1]: expects "z", got "y""#
        );
        assert!(subset(json!({"tags": ["z"]}), body, true).is_err());
        assert!(
            subset(json!({"a b": 1}), json!({"a b": 2}), false)
                .unwrap_err()
                .starts_with(r#"json $["a b"]"#)
        );
    }

    #[test]
    fn methods_paths_queries_and_headers() {
        let matcher = Matcher {
            methods: vec!["GET".into()],
            target: Some(Target::Path(PathGlob::path("/api/items/:id").unwrap())),
            query: vec![("q".into(), Glob::new("sh*"))],
            headers: vec![("x-token".into(), Glob::new("abc"))],
            body: None,
        };
        let req = parsed(
            "get",
            "http://x/api/items/7?q=shoes&q=hats",
            &[("X-Token", "abc")],
            "",
        );
        assert_eq!(
            matcher.check(&req, &req.method, false).unwrap(),
            BTreeMap::from([("id".to_owned(), "7".to_owned())])
        );
        let req = parsed("POST", "http://x/api/items/7", &[], "");
        assert_eq!(
            matcher.check(&req, "POST", false).unwrap_err(),
            "method differs: expects GET, got POST"
        );
        let req = parsed("GET", "http://x/api/item/7", &[], "");
        assert_eq!(
            matcher.check(&req, "GET", false).unwrap_err(),
            "path differs: expects /api/items/:id, got /api/item/7"
        );
        let req = parsed("GET", "http://x/api/items/7", &[], "");
        assert_eq!(
            matcher.check(&req, "GET", false).unwrap_err(),
            "query q: missing"
        );
        let req = parsed("GET", "http://x/api/items/7?q=hats", &[], "");
        assert_eq!(
            matcher.check(&req, "GET", false).unwrap_err(),
            r#"query q: expects "sh*", got "hats""#
        );
        let req = parsed("GET", "http://x/api/items/7?q=shoes", &[], "");
        assert_eq!(
            matcher.check(&req, "GET", false).unwrap_err(),
            "header x-token: missing"
        );
        // A preflight cannot show headers or a body.
        assert!(matcher.check(&req, "GET", true).is_ok());
        assert_eq!(
            matcher.summary(),
            "GET /api/items/:id (query q, header x-token)"
        );
    }

    #[test]
    fn regex_groups_are_parameters() {
        let matcher = Matcher {
            target: Some(Target::Regex(
                Regex::new(r"/api/v(?P<version>\d+)/").unwrap(),
            )),
            ..Matcher::default()
        };
        let req = parsed("GET", "https://x/api/v2/items", &[], "");
        assert_eq!(matcher.check(&req, "GET", false).unwrap()["version"], "2");
    }

    #[test]
    fn bodies() {
        let form = Matcher {
            body: Some(BodyMatcher::Form(vec![(
                "email".into(),
                Glob::new("*@x.com"),
            )])),
            ..Matcher::default()
        };
        let req = parsed(
            "POST",
            "http://x/login",
            &[("Content-Type", "application/x-www-form-urlencoded")],
            "email=a%40x.com&password=1",
        );
        assert!(form.check(&req, "POST", false).is_ok());
        let json_req = parsed("POST", "http://x/login", &[], r#"{"email":"a@x.com"}"#);
        assert_eq!(
            form.check(&json_req, "POST", false).unwrap_err(),
            "form: the body is not a form"
        );
        // Repeated fields: any value may match.
        let repeated = parsed(
            "POST",
            "http://x/login",
            &[("Content-Type", "application/x-www-form-urlencoded")],
            "email=a%40y.com&email=b%40x.com",
        );
        assert!(form.check(&repeated, "POST", false).is_ok());
        let multipart = parsed(
            "POST",
            "http://x/login",
            &[("Content-Type", "multipart/form-data; boundary=\"q b\"")],
            "--q b\nContent-Disposition: form-data; name=\"email\"\n\na@x.com\n--q b--\n",
        );
        assert!(form.check(&multipart, "POST", false).is_ok());
        let no_boundary = parsed(
            "POST",
            "http://x/login",
            &[("Content-Type", "multipart/form-data")],
            "email=a%40x.com",
        );
        assert_eq!(
            form.check(&no_boundary, "POST", false).unwrap_err(),
            "form: the body is not a form (multipart body without a boundary)"
        );
        let text = Matcher {
            body: Some(BodyMatcher::Glob(Glob::new("hello *"))),
            ..Matcher::default()
        };
        assert!(
            text.check(
                &parsed("POST", "http://x/", &[], "hello world"),
                "POST",
                false
            )
            .is_ok()
        );
        let regex = Matcher {
            body: Some(BodyMatcher::Regex(Regex::new(r"id=\d+").unwrap())),
            ..Matcher::default()
        };
        assert!(
            regex
                .check(&parsed("POST", "http://x/", &[], "a&id=42"), "POST", false)
                .is_ok()
        );
        let json = Matcher {
            body: Some(BodyMatcher::Json {
                value: json!({"a": 1}),
                contains: false,
            }),
            ..Matcher::default()
        };
        assert_eq!(
            json.check(&parsed("POST", "http://x/", &[], ""), "POST", false)
                .unwrap_err(),
            "json: the request has no body"
        );
    }

    #[test]
    fn graphql_requests() {
        let matcher = Matcher {
            body: Some(BodyMatcher::Graphql {
                operation: Some("GetCart".into()),
                variables: Some(json!({"id": "*"})),
            }),
            ..Matcher::default()
        };
        let named = r#"{"query":"query GetCart($id: ID!) { cart(id: $id) { total } }","variables":{"id":"7"}}"#;
        assert!(
            matcher
                .check(
                    &parsed("POST", "http://x/graphql", &[], named),
                    "POST",
                    false
                )
                .is_ok()
        );
        let by_name =
            r#"{"operationName":"GetCart","query":"{ cart { total } }","variables":{"id":1}}"#;
        assert!(
            matcher
                .check(
                    &parsed("POST", "http://x/graphql", &[], by_name),
                    "POST",
                    false
                )
                .is_ok()
        );
        let other = r#"{"query":"mutation AddItem { add }","variables":{"id":1}}"#;
        assert_eq!(
            matcher
                .check(
                    &parsed("POST", "http://x/graphql", &[], other),
                    "POST",
                    false
                )
                .unwrap_err(),
            "graphql operation: expects GetCart, got AddItem"
        );
        let get =
            "http://x/graphql?query=query%20GetCart%20%7B%20a%20%7D&variables=%7B%22id%22%3A2%7D";
        assert!(
            matcher
                .check(&parsed("GET", get, &[], ""), "GET", false)
                .is_ok()
        );
        assert_eq!(
            matcher
                .check(
                    &parsed("POST", "http://x/graphql", &[], "{}"),
                    "POST",
                    false
                )
                .unwrap_err(),
            "graphql: the request is no GraphQL request"
        );
    }
}
