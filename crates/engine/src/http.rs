//! `http` (plan 11, milestone 11.11): a request sent from a browser
//! context, outside its pages; scenarios use it to arrange data through
//! the API in `@before` and clean up in `@after`.
//!
//! The request goes where the context's pages' requests go: its route
//! rules and mock sets decide first, so in mock mode the mocks answer it
//! (collections too, which is what makes arrange steps portable between
//! backends), and the mock log has it. Otherwise the daemon sends it,
//! with the cookies Chrome would send from the context (`HttpOnly` ones
//! too) and, for a request that may change something, the URL's `Origin`
//! as a same-origin `fetch` would. The cookies an answer sets go back
//! into the context, so `http POST /api/login` logs its pages in. A 401
//! with a challenge is answered with `open --http-credentials`, if set.
//! Redirects are followed (at most 10), each hop routed and its cookies
//! kept. No page sees the request: it is in no tab's network log, so
//! `expect request` does not count it.

use std::fmt::Write as _;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use cdp::domains::network::{self, CookieParam};
use cdp::domains::storage;
use protocol::schema::{HttpData, MockRef};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde_json::Value;

use crate::errors::from_cdp;
use crate::request::{HttpRequest, millis};
use crate::route::{self, Act, Decision, Handler, merge_patch, reason_name, route_name};
use crate::state::parse_same_site;
use crate::{Engine, Running};

/// How many redirects are followed.
const MAX_REDIRECTS: usize = 10;

/// The largest body read.
const MAX_BODY: usize = 50 * 1024 * 1024;

/// One hop's answer.
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    by: By,
}

/// Who answered.
#[derive(Clone)]
enum By {
    Network,
    Mock(Option<MockRef>),
    Route(String),
    Unmatched(String),
}

impl By {
    fn name(&self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Mock(_) => "mock",
            Self::Route(_) => "route",
            Self::Unmatched(_) => "unmatched",
        }
    }

    /// `mock base.toml:14`, `route r2`, `network`.
    fn text(&self) -> String {
        match self {
            Self::Network => "network".into(),
            Self::Mock(Some(mock)) => format!("mock {}", mock.at),
            Self::Mock(None) => "mock".into(),
            Self::Route(rule) => format!("route {rule}"),
            Self::Unmatched(set) => format!("unmatched in mock set {set}"),
        }
    }
}

impl Engine {
    /// `http`: sends the request (see the module documentation).
    pub async fn http(&self, req: HttpRequest) -> Result<Output, ErrorInfo> {
        let timeout = millis(req.timeout_ms);
        let started = Instant::now();
        let url = url::Url::parse(&req.url)
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https"))
            .ok_or_else(|| {
                ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!("`{}` is not an http(s) URL", req.url),
                )
                .hint(Hint::text("give a full URL, or a path with base_url set"))
            })?;
        let exchange = self.exchange(&req, url, timeout);
        let (url, answer, cookies_set) =
            tokio::time::timeout(timeout, exchange)
                .await
                .map_err(|_| {
                    ErrorInfo::new(
                        ErrorKind::Timeout,
                        format!(
                            "{} {} got no answer within {}",
                            req.method,
                            req.url,
                            format::duration(timeout)
                        ),
                    )
                    .hint(Hint::text(
                        "give it longer with --timeout, or check that the server is up",
                    ))
                })??;
        let duration = started.elapsed();
        Ok(output(&req, &url, answer, cookies_set, duration))
    }

    /// Sends the request, following redirects; the last URL, its answer
    /// and the names of the cookies the answers set.
    async fn exchange(
        &self,
        req: &HttpRequest,
        mut url: url::Url,
        timeout: Duration,
    ) -> Result<(String, Answer, Vec<String>), ErrorInfo> {
        let running = self.running()?;
        let scope = self.context_key()?;
        let mut method = req.method.clone();
        let mut body = req.body.clone();
        let mut headers = req.headers.clone();
        let mut cookies_set = Vec::new();
        for hop in 0..=MAX_REDIRECTS {
            let answer = self
                .hop(
                    &running,
                    scope.as_deref(),
                    &method,
                    &url,
                    &headers,
                    body.as_deref(),
                    timeout,
                )
                .await?;
            for name in self
                .keep_cookies(&running, scope.as_deref(), &url, &answer.headers, timeout)
                .await
            {
                if !cookies_set.contains(&name) {
                    cookies_set.push(name);
                }
            }
            let location = matches!(answer.status, 301 | 302 | 303 | 307 | 308)
                .then(|| header(&answer.headers, "location"))
                .flatten()
                .and_then(|l| url.join(l).ok());
            let Some(next) = location else {
                return Ok((url.to_string(), answer, cookies_set));
            };
            if hop == MAX_REDIRECTS {
                break;
            }
            if answer.status == 303
                || (matches!(answer.status, 301 | 302)
                    && !matches!(method.as_str(), "GET" | "HEAD"))
            {
                "GET".clone_into(&mut method);
                body = None;
                headers.retain(|(n, _)| !n.eq_ignore_ascii_case("content-type"));
            }
            url = next;
        }
        Err(ErrorInfo::new(
            ErrorKind::NavigationFailed,
            format!(
                "{} {} redirected more than {MAX_REDIRECTS} times",
                req.method, req.url
            ),
        )
        .hint(Hint::text(
            "check the server's redirects; the last hop was {url}",
        )))
    }

    /// One request, routed like the context's pages' requests.
    #[allow(clippy::too_many_arguments)]
    async fn hop(
        &self,
        running: &Running,
        scope: Option<&str>,
        method: &str,
        url: &url::Url,
        given: &[(String, String)],
        body: Option<&str>,
        timeout: Duration,
    ) -> Result<Answer, ErrorInfo> {
        let mut headers = given.to_vec();
        if header(&headers, "cookie").is_none() {
            let cookies = self.cookie_header(running, url.as_str(), timeout).await?;
            if !cookies.is_empty() {
                headers.push(("Cookie".into(), cookies));
            }
        }
        if !matches!(method, "GET" | "HEAD") && header(&headers, "origin").is_none() {
            headers.push(("Origin".into(), url.origin().ascii_serialization()));
        }
        let json: serde_json::Map<String, Value> = headers
            .iter()
            .map(|(n, v)| (n.clone(), Value::String(v.clone())))
            .collect();
        let json = Value::Object(json);
        let (decision, _) = self.settings.routes.decide(&route::Request {
            url: url.as_str(),
            method,
            resource_type: "Other",
            headers: &json,
            context: scope,
            tab: None,
            body: body.map(str::as_bytes),
            now_ms: self.now_ms(scope),
        });
        let (handler, by) = match decision {
            Decision::Pass | Decision::Preflight => {
                return self
                    .send(method, url.as_str(), &headers, body, By::Network)
                    .await;
            }
            Decision::Route { rule, handler, .. } => (handler, By::Route(route_name(rule))),
            Decision::Mock { handler, seq } => {
                let by = match self.settings.routes.entry(seq) {
                    Some(entry) => match entry.unmatched {
                        Some(set) => By::Unmatched(set),
                        None => By::Mock(entry.mock),
                    },
                    None => By::Mock(None),
                };
                (handler, by)
            }
        };
        self.carry_out(&handler, method, url, headers, body, by)
            .await
    }

    /// What a rule or mock does with the request.
    async fn carry_out(
        &self,
        handler: &Handler,
        method: &str,
        url: &url::Url,
        mut headers: Vec<(String, String)>,
        body: Option<&str>,
        by: By,
    ) -> Result<Answer, ErrorInfo> {
        if !handler.delay.is_zero() {
            tokio::time::sleep(handler.delay).await;
        }
        match &handler.act {
            Act::Fulfill(f) => Ok(Answer {
                status: f.status,
                headers: f.headers.clone(),
                body: base64::engine::general_purpose::STANDARD
                    .decode(&f.body)
                    .unwrap_or_default(),
                by,
            }),
            Act::Abort(reason) => Err(ErrorInfo::new(
                ErrorKind::NavigationFailed,
                format!(
                    "{method} {url} failed: the {} made it fail with {}",
                    by.text(),
                    reason_name(*reason)
                ),
            )
            .hint(Hint::run(
                Cmd::new("mock")
                    .arg("explain")
                    .arg(method)
                    .arg(url.as_str()),
            ))),
            Act::Continue {
                set_headers,
                remove_headers,
                url: to,
            } => {
                headers.retain(|(n, _)| {
                    !remove_headers.iter().any(|r| r.eq_ignore_ascii_case(n))
                        && !set_headers.iter().any(|(s, _)| s.eq_ignore_ascii_case(n))
                });
                headers.extend(set_headers.iter().cloned());
                let target = to.as_deref().unwrap_or(url.as_str());
                self.send(method, target, &headers, body, by).await
            }
            Act::PatchJson(patch) => {
                let mut answer = self.send(method, url.as_str(), &headers, body, by).await?;
                if let Ok(mut value) = serde_json::from_slice::<Value>(&answer.body) {
                    merge_patch(&mut value, patch);
                    answer.body = serde_json::to_vec(&value).unwrap_or_default();
                }
                Ok(answer)
            }
        }
    }

    /// Sends the request over the network. A 401 is asked again with the
    /// session's HTTP credentials, if it has some.
    async fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: Option<&str>,
        by: By,
    ) -> Result<Answer, ErrorInfo> {
        let client = client(self.settings.ignores_https_errors())?;
        let method_value = reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("`{method}` is not an HTTP method"),
            )
        })?;
        let build = || {
            let mut request = client.request(method_value.clone(), url);
            for (name, value) in headers {
                request = request.header(name, value);
            }
            if let Some(body) = body {
                request = request.body(body.to_owned());
            }
            request
        };
        let failed = |e: reqwest::Error| {
            let mut cause = String::new();
            let mut source: Option<&dyn std::error::Error> = Some(&e);
            while let Some(s) = source {
                cause = s.to_string();
                source = s.source();
            }
            ErrorInfo::new(
                ErrorKind::NavigationFailed,
                format!("{method} {url} failed: {cause}"),
            )
            .hint(Hint::text(
                "check that the server is up and the URL right (base_url joins paths)",
            ))
        };
        let mut response = build().send().await.map_err(failed)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            && header(headers, "authorization").is_none()
            && response.headers().contains_key("www-authenticate")
            && let Some(credentials) = self
                .settings
                .http_credentials()
                .filter(|c| c.applies_to(response.url().as_str()))
        {
            response = build()
                .basic_auth(&credentials.username, Some(&credentials.password))
                .send()
                .await
                .map_err(failed)?;
        }
        let status = response.status().as_u16();
        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(n, v)| {
                (
                    n.as_str().to_owned(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(failed)? {
            if body.len() + chunk.len() > MAX_BODY {
                return Err(ErrorInfo::new(
                    ErrorKind::NavigationFailed,
                    format!(
                        "{method} {url}: the answer is larger than {}",
                        format::size(MAX_BODY as u64)
                    ),
                ));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Answer {
            status,
            headers,
            body,
            by,
        })
    }

    /// The `Cookie` header a request from the context's pages to `url`
    /// would carry.
    async fn cookie_header(
        &self,
        running: &Running,
        url: &str,
        timeout: Duration,
    ) -> Result<String, ErrorInfo> {
        let page = self.page(running, None, timeout).await?;
        let cookies = page
            .session
            .with_timeout(timeout)
            .call(network::GetCookiesParams {
                urls: Some(vec![url.to_owned()]),
            })
            .await
            .map_err(|e| from_cdp(&e, &self.session))?
            .cookies;
        Ok(cookies
            .iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; "))
    }

    /// Gives the context the cookies `headers` set; returns their names.
    /// A cookie Chrome refuses is left out (a warning in the daemon log).
    async fn keep_cookies(
        &self,
        running: &Running,
        scope: Option<&str>,
        url: &url::Url,
        headers: &[(String, String)],
        timeout: Duration,
    ) -> Vec<String> {
        let params: Vec<CookieParam> = headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case("set-cookie"))
            .flat_map(|(_, v)| v.lines())
            .filter_map(|line| set_cookie(line, url.as_str(), now_secs()))
            .collect();
        let mut kept = Vec::new();
        let browser = running.conn.browser().with_timeout(timeout);
        for param in params {
            let name = param.name.clone();
            let mut set = storage::SetCookiesParams::new(vec![param]);
            set.browser_context_id = scope.map(str::to_owned);
            match browser.call(set).await {
                Ok(_) => kept.push(name),
                Err(e) => tracing::warn!("http: Chrome refused cookie {name}: {e}"),
            }
        }
        kept
    }

    /// Now for mock templates in `scope`: its frozen time, or the real
    /// one.
    fn now_ms(&self, scope: Option<&str>) -> i64 {
        self.settings.emulation.get_in(scope).time.map_or_else(
            || {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
            },
            |t| t.epoch_ms,
        )
    }
}

/// The HTTP client; one that accepts bad certificates when the session
/// ignores HTTPS errors (`open --ignore-https-errors`).
fn client(lenient: bool) -> Result<&'static reqwest::Client, ErrorInfo> {
    static STRICT: OnceLock<reqwest::Client> = OnceLock::new();
    static LENIENT: OnceLock<reqwest::Client> = OnceLock::new();
    let cell = if lenient { &LENIENT } else { &STRICT };
    if let Some(client) = cell.get() {
        return Ok(client);
    }
    // reqwest is built without a default provider (no aws-lc).
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .tls_danger_accept_invalid_certs(lenient)
        .user_agent(concat!("chrome-cdp/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| {
            ErrorInfo::new(
                ErrorKind::LaunchFailed,
                format!("cannot make an HTTP client: {e}"),
            )
        })?;
    Ok(cell.get_or_init(|| client))
}

/// The value of header `name` (any case).
fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// A `Set-Cookie` line as `Storage.setCookies` takes it, for a response
/// from `url` at time `now` (seconds). `None` when it has no name.
// Cookie times are whole seconds, far within f64's exact range.
#[allow(clippy::cast_precision_loss)]
fn set_cookie(line: &str, url: &str, now: f64) -> Option<CookieParam> {
    let mut parts = line.split(';');
    let (name, value) = parts.next()?.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let mut param = CookieParam {
        name: name.to_owned(),
        value: value.trim().trim_matches('"').to_owned(),
        url: Some(url.to_owned()),
        domain: None,
        path: None,
        secure: None,
        http_only: None,
        same_site: None,
        expires: None,
        priority: None,
        source_scheme: None,
        source_port: None,
        partition_key: None,
    };
    let mut max_age = None;
    for part in parts {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        let value = value.trim();
        match key.trim().to_ascii_lowercase().as_str() {
            "domain" if !value.is_empty() => {
                param.domain = Some(format!(".{}", value.trim_start_matches('.')));
            }
            "path" if value.starts_with('/') => param.path = Some(value.to_owned()),
            "secure" => param.secure = Some(true),
            "httponly" => param.http_only = Some(true),
            "samesite" => param.same_site = parse_same_site(value),
            "max-age" => max_age = value.parse::<i64>().ok(),
            "expires" if param.expires.is_none() => param.expires = http_date(value),
            _ => {}
        }
    }
    // Max-Age wins over Expires; 0 or less removes the cookie.
    if let Some(age) = max_age {
        param.expires = Some(if age <= 0 { 1.0 } else { now + age as f64 });
    }
    Some(param)
}

/// Seconds since the epoch of an HTTP date (`Wed, 21 Oct 2015 07:28:00
/// GMT`, dashes allowed between the date's parts).
#[allow(clippy::cast_precision_loss)]
fn http_date(text: &str) -> Option<f64> {
    let text = text.split_once(',').map_or(text, |(_, rest)| rest);
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == '-')
        .filter(|w| !w.is_empty())
        .collect();
    let [day, month, year, time, ..] = words.as_slice() else {
        return None;
    };
    let day: i64 = day.parse().ok()?;
    let month = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .position(|m| month.to_ascii_lowercase().starts_with(m))
    .and_then(|m| i64::try_from(m).ok())?
        + 1;
    let mut year: i64 = year.parse().ok()?;
    if year < 100 {
        year += if year < 70 { 2000 } else { 1900 };
    }
    let mut hms = time.split(':').map(|n| n.parse::<i64>().ok());
    let (h, m, s) = (hms.next()??, hms.next()??, hms.next()??);
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days * 86_400 + h * 3600 + m * 60 + s) as f64)
}

/// The command's output.
fn output(
    req: &HttpRequest,
    url: &str,
    answer: Answer,
    cookies_set: Vec<String>,
    duration: Duration,
) -> Output {
    let mut headers: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for (name, value) in &answer.headers {
        let name = name.to_ascii_lowercase();
        let joiner = if name == "set-cookie" { "\n" } else { ", " };
        headers
            .entry(name)
            .and_modify(|v| {
                v.push_str(joiner);
                v.push_str(value);
            })
            .or_insert_with(|| value.clone());
    }
    let size = answer.body.len();
    let (body, json) = match serde_json::from_slice::<Value>(&answer.body) {
        Ok(value) if !answer.body.is_empty() => (value, true),
        _ => match String::from_utf8(answer.body) {
            Ok(text) if !text.is_empty() => (Value::String(text), false),
            _ => (Value::Null, false),
        },
    };
    let shown = match &body {
        Value::Null if size > 0 => format!("({} of binary data)", format::size(size as u64)),
        Value::Null => String::new(),
        Value::String(text) if !json => text.clone(),
        value => value.to_string(),
    };
    let mut text = format!(
        "{} {} {url} ({}, {}, {})",
        answer.status,
        req.method,
        format::duration(duration),
        answer.by.text(),
        format::size(size as u64)
    );
    if !cookies_set.is_empty() {
        let _ = write!(text, "\nset cookies: {}", cookies_set.join(", "));
    }
    if !shown.is_empty() {
        let _ = write!(text, "\n{shown}");
    }
    let (mock, route, unmatched) = match &answer.by {
        By::Mock(mock) => (mock.clone(), None, None),
        By::Route(rule) => (None, Some(rule.clone()), None),
        By::Unmatched(set) => (None, None, Some(set.clone())),
        By::Network => (None, None, None),
    };
    let data = HttpData {
        method: req.method.clone(),
        url: url.to_owned(),
        status: answer.status,
        headers,
        body,
        json,
        size,
        answered_by: answer.by.name().to_owned(),
        mock,
        route,
        unmatched,
        cookies_set,
        elapsed_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
    };
    Output::typed(&data, text)
        .quiet(shown)
        .limit(req.max_chars, "--max-chars 0 for all of it, or --json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_cookie_lines_become_cookie_params() {
        let c = set_cookie(
            "sid=abc; Path=/app; Domain=example.com; HttpOnly; Secure; SameSite=Strict; Max-Age=60",
            "https://www.example.com/login",
            1000.0,
        )
        .unwrap();
        assert_eq!((c.name.as_str(), c.value.as_str()), ("sid", "abc"));
        assert_eq!(c.domain.as_deref(), Some(".example.com"));
        assert_eq!(c.path.as_deref(), Some("/app"));
        assert_eq!((c.secure, c.http_only), (Some(true), Some(true)));
        assert_eq!(c.same_site, parse_same_site("Strict"));
        assert_eq!(c.expires, Some(1060.0));
        assert_eq!(c.url.as_deref(), Some("https://www.example.com/login"));

        // Host-only, a session cookie; Max-Age 0 removes one.
        let c = set_cookie("a=1", "http://localhost:1/", 0.0).unwrap();
        assert_eq!((c.domain, c.expires), (None, None));
        let c = set_cookie(
            "a=; Max-Age=0; Expires=Thu, 01 Jan 2099 00:00:00 GMT",
            "http://h/",
            5.0,
        )
        .unwrap();
        assert_eq!(c.expires, Some(1.0));
        assert!(set_cookie("novalue", "http://h/", 0.0).is_none());
        assert!(set_cookie("=x", "http://h/", 0.0).is_none());
    }

    #[test]
    fn http_dates_parse() {
        assert_eq!(
            http_date("Wed, 21 Oct 2015 07:28:00 GMT"),
            Some(1_445_412_480.0)
        );
        assert_eq!(http_date("Thu, 01-Jan-1970 00:00:10 GMT"), Some(10.0));
        assert_eq!(
            http_date("Sun, 06-Nov-94 08:49:37 GMT"),
            Some(784_111_777.0)
        );
        assert_eq!(http_date("soon"), None);
    }
}
