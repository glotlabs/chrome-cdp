//! The load server (plan 23, "Rules for every milestone"): the built app
//! of `examples/e2e-shop` and its API ([`fixtures::ShopRoutes`]) on one
//! origin, with latency and errors you set per endpoint, so `chrome-cdp
//! load`'s tests and benchmarks run offline against a server whose
//! behaviour they know.
//!
//! - `--latency [METHOD ]PATH=200ms±50` delays the answers to the
//!   requests it matches by 200 ms, give or take up to 50 ms (uniform);
//!   `1.5s` or a bare `200` (ms) work too.
//! - `--errors [METHOD ]PATH=2%` answers that share of them with a 500
//!   (`{"error": "injected by the load server"}`) instead of the shop's
//!   answer, after the latency.
//!
//! A PATH is matched against the request's path without its query: `*`
//! stands for any part of one segment, `**` for anything. For each kind,
//! the first rule that matches applies. `GET /__load/stats` answers what
//! the server saw, per `METHOD /path`: requests and injected errors
//! (`POST /__load/reset` starts the counts over); [`LoadServer::stats`]
//! reads the same in-process.
//!
//! The shop's state (carts, sessions, orders) lives as long as the
//! server. It runs on a multi-threaded runtime of its own, and stops with
//! the process.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};

/// The body of an injected error.
pub const INJECTED: &str = "injected by the load server";

/// What a [`Fault`] does to the requests it matches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Effect {
    /// Answer `base` later, give or take up to `jitter` (uniform).
    Latency { base: Duration, jitter: Duration },
    /// Answer this share (0 to 1) of them with a 500.
    Errors { rate: f64 },
}

/// A `--latency` or `--errors` rule.
#[derive(Debug, Clone, PartialEq)]
pub struct Fault {
    /// `GET`, `POST`, …; `None` matches every method.
    pub method: Option<String>,
    /// The path pattern (`*` within a segment, `**` across them).
    pub path: String,
    pub effect: Effect,
}

impl Fault {
    /// `--latency [METHOD ]PATH=200ms±50`.
    pub fn latency(spec: &str) -> Result<Self, String> {
        let (method, path, value) = split(spec, "--latency", "/api/cart=200ms±50")?;
        let (base, jitter) = match value.split_once('±').or_else(|| value.split_once("+-")) {
            Some((base, jitter)) => (base, Some(jitter)),
            None => (value.as_str(), None),
        };
        let base = duration(base.trim())
            .ok_or_else(|| format!("--latency {spec}: `{base}` is not a duration, e.g. 200ms"))?;
        let jitter = match jitter {
            Some(j) => duration(j.trim())
                .ok_or_else(|| format!("--latency {spec}: `{j}` is not a duration, e.g. 50ms"))?,
            None => Duration::ZERO,
        };
        if jitter > base {
            return Err(format!(
                "--latency {spec}: the jitter is larger than the latency"
            ));
        }
        Ok(Self {
            method,
            path,
            effect: Effect::Latency { base, jitter },
        })
    }

    /// `--errors [METHOD ]PATH=2%`.
    pub fn errors(spec: &str) -> Result<Self, String> {
        let (method, path, value) = split(spec, "--errors", "/api/checkout=2%")?;
        let rate = value
            .trim()
            .strip_suffix('%')
            .and_then(|n| n.trim().parse::<f64>().ok())
            .filter(|n| (0.0..=100.0).contains(n))
            .ok_or_else(|| {
                format!("--errors {spec}: `{value}` is not a percentage from 0% to 100%")
            })?;
        Ok(Self {
            method,
            path,
            effect: Effect::Errors { rate: rate / 100.0 },
        })
    }

    /// Whether the rule applies to a request.
    pub fn matches(&self, method: &str, path: &str) -> bool {
        self.method
            .as_deref()
            .is_none_or(|m| m.eq_ignore_ascii_case(method))
            && glob(self.path.as_bytes(), path.as_bytes())
    }
}

/// `[METHOD ]PATH=VALUE` → its parts.
fn split(
    spec: &str,
    flag: &str,
    example: &str,
) -> Result<(Option<String>, String, String), String> {
    let (target, value) = spec
        .rsplit_once('=')
        .ok_or_else(|| format!("{flag} {spec}: give PATH=VALUE, e.g. {flag} {example}"))?;
    let target = target.trim();
    let (method, path) = match target.split_once(' ') {
        Some((method, path)) => (Some(method.to_ascii_uppercase()), path.trim()),
        None => (None, target),
    };
    if !path.starts_with('/') {
        return Err(format!(
            "{flag} {spec}: the path must start with /, e.g. {flag} {example}"
        ));
    }
    if method
        .as_deref()
        .is_some_and(|m| !m.chars().all(|c| c.is_ascii_alphabetic()))
    {
        return Err(format!("{flag} {spec}: `{target}` is not [METHOD ]PATH"));
    }
    Ok((method, path.to_owned(), value.to_owned()))
}

/// `200ms`, `1.5s`, or `200` (ms).
fn duration(text: &str) -> Option<Duration> {
    let (number, scale) = if let Some(n) = text.strip_suffix("ms") {
        (n, 1.0)
    } else if let Some(n) = text.strip_suffix('s') {
        (n, 1000.0)
    } else {
        (text, 1.0)
    };
    let ms: f64 = number.trim().parse().ok()?;
    (ms.is_finite() && ms >= 0.0).then(|| Duration::from_secs_f64(ms * scale / 1000.0))
}

/// Whether `text` matches `pattern`: `*` is any run of bytes but `/`,
/// `**` any run at all.
fn glob(pattern: &[u8], text: &[u8]) -> bool {
    match pattern {
        [] => text.is_empty(),
        [b'*', b'*', rest @ ..] => (0..=text.len()).any(|i| glob(rest, &text[i..])),
        [b'*', rest @ ..] => {
            let segment = text.iter().position(|&b| b == b'/').unwrap_or(text.len());
            (0..=segment).any(|i| glob(rest, &text[i..]))
        }
        [first, rest @ ..] => text.first() == Some(first) && glob(rest, &text[1..]),
    }
}

/// How a load server starts.
#[derive(Debug, Clone)]
pub struct Options {
    /// The app's build: `examples/e2e-shop/dist` by default.
    pub dist: PathBuf,
    /// 0 = any free port.
    pub port: u16,
    /// Also start the shop's payment provider on this port (0 = any).
    pub payments: Option<u16>,
    pub faults: Vec<Fault>,
    /// The seed of the jitter and the errors, so a run can be repeated.
    pub seed: u64,
    /// The runtime's worker threads.
    pub threads: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            dist: default_dist(),
            port: 0,
            payments: None,
            faults: Vec::new(),
            seed: 1,
            threads: std::thread::available_parallelism().map_or(2, |n| n.get().min(4)),
        }
    }
}

/// `examples/e2e-shop/dist` of this repository.
pub fn default_dist() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/e2e-shop/dist")
}

/// What the server saw of one `METHOD /path`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Seen {
    pub requests: u64,
    /// Answered with an injected 500.
    pub errors: u64,
}

/// A running load server.
#[derive(Debug, Clone)]
pub struct LoadServer {
    url: String,
    payments: Option<String>,
    state: Arc<Server>,
}

#[derive(Debug)]
struct Server {
    routes: fixtures::ShopRoutes,
    faults: Vec<Fault>,
    random: AtomicU64,
    seen: Mutex<BTreeMap<String, Seen>>,
}

impl LoadServer {
    /// Serves on `http://localhost:<port>`; returns once it accepts
    /// connections.
    pub fn start(options: Options) -> std::io::Result<Self> {
        let payments = options.payments.map(fixtures::start_payments).transpose()?;
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, options.port)))?;
        listener.set_nonblocking(true)?;
        let url = format!("http://localhost:{}", listener.local_addr()?.port());
        let state = Arc::new(Server {
            routes: fixtures::ShopRoutes::new(options.dist),
            faults: options.faults,
            random: AtomicU64::new(options.seed),
            seen: Mutex::new(BTreeMap::new()),
        });
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(options.threads.max(1))
            .thread_name("load-server")
            .enable_all()
            .build()?;
        let serving = state.clone();
        std::thread::Builder::new()
            .name("load-server".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                        return;
                    };
                    loop {
                        let Ok((stream, _)) = listener.accept().await else {
                            continue;
                        };
                        let _ = stream.set_nodelay(true);
                        tokio::spawn(serve_connection(stream, serving.clone()));
                    }
                });
            })?;
        Ok(Self {
            url,
            payments,
            state,
        })
    }

    /// `http://localhost:<port>`.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The payment provider's URL, if it runs.
    pub fn payments(&self) -> Option<&str> {
        self.payments.as_deref()
    }

    /// What the server saw so far, by `METHOD /path`.
    pub fn stats(&self) -> BTreeMap<String, Seen> {
        self.state.seen().clone()
    }

    /// Starts the counts over.
    pub fn reset(&self) {
        self.state.seen().clear();
    }
}

impl Server {
    fn seen(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Seen>> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A number from 0 to 1 (SplitMix64).
    fn random(&self) -> f64 {
        let mut z = self
            .random
            .fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed)
            .wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        #[allow(clippy::cast_precision_loss)]
        let unit = (z >> 11) as f64 / (1u64 << 53) as f64;
        unit
    }

    /// How long to wait before answering `method path`, and whether the
    /// answer is an injected error.
    fn faults(&self, method: &str, path: &str) -> (Duration, bool) {
        let mut latency = None;
        let mut error = None;
        for fault in self.faults.iter().filter(|f| f.matches(method, path)) {
            match fault.effect {
                Effect::Latency { base, jitter } if latency.is_none() => {
                    let offset = (self.random() * 2.0 - 1.0) * jitter.as_secs_f64();
                    latency = Some(Duration::from_secs_f64(
                        (base.as_secs_f64() + offset).max(0.0),
                    ));
                }
                Effect::Errors { rate } if error.is_none() => {
                    error = Some(self.random() < rate);
                }
                _ => {}
            }
        }
        (latency.unwrap_or_default(), error.unwrap_or(false))
    }

    async fn respond(&self, req: Request<Incoming>) -> Response<Full<Bytes>> {
        let method = req.method().as_str().to_owned();
        let path = req.uri().path().to_owned();
        match (method.as_str(), path.as_str()) {
            ("GET", "/__load/stats") => {
                let endpoints: serde_json::Map<String, Value> = self
                    .seen()
                    .iter()
                    .map(|(k, s)| {
                        (
                            k.clone(),
                            json!({ "requests": s.requests, "errors": s.errors }),
                        )
                    })
                    .collect();
                return answer(StatusCode::OK, &json!({ "endpoints": endpoints }));
            }
            ("POST", "/__load/reset") => {
                self.seen().clear();
                return answer(StatusCode::OK, &json!({ "ok": true }));
            }
            _ => {}
        }
        let (latency, error) = self.faults(&method, &path);
        if !latency.is_zero() {
            tokio::time::sleep(latency).await;
        }
        {
            let mut seen = self.seen();
            let entry = seen.entry(format!("{method} {path}")).or_default();
            entry.requests += 1;
            entry.errors += u64::from(error);
        }
        if error {
            return answer(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": INJECTED }),
            );
        }
        self.routes.respond(req).await
    }
}

fn answer(status: StatusCode, body: &Value) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(body.to_string())));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(
        hyper::header::CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("application/json"),
    );
    headers.insert(
        hyper::header::CACHE_CONTROL,
        hyper::header::HeaderValue::from_static("no-store"),
    );
    response
}

async fn serve_connection(stream: tokio::net::TcpStream, server: Arc<Server>) {
    let service = service_fn(move |req: Request<Incoming>| {
        let server = server.clone();
        async move { Ok::<_, Infallible>(server.respond(req).await) }
    });
    let _ = http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades()
        .await;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::time::Duration;

    use super::{Effect, Fault, glob};

    #[test]
    fn latency_rules_parse() {
        let f = Fault::latency("/api/cart=200ms±50").unwrap();
        assert_eq!(f.method, None);
        assert_eq!(f.path, "/api/cart");
        assert_eq!(
            f.effect,
            Effect::Latency {
                base: Duration::from_millis(200),
                jitter: Duration::from_millis(50)
            }
        );
        let f = Fault::latency("post /api/cart=1.5s+-100ms").unwrap();
        assert_eq!(f.method.as_deref(), Some("POST"));
        assert_eq!(
            f.effect,
            Effect::Latency {
                base: Duration::from_millis(1500),
                jitter: Duration::from_millis(100)
            }
        );
        assert!(Fault::latency("/api/cart").is_err());
        assert!(Fault::latency("api/cart=200ms").is_err());
        assert!(Fault::latency("/api/cart=fast").is_err());
        assert!(Fault::latency("/api/cart=50ms±100").is_err());
    }

    #[test]
    fn error_rules_parse() {
        let f = Fault::errors("/api/checkout=2%").unwrap();
        assert_eq!(f.effect, Effect::Errors { rate: 0.02 });
        assert!(Fault::errors("/api/checkout=2").is_err());
        assert!(Fault::errors("/api/checkout=200%").is_err());
    }

    #[test]
    fn paths_match_by_segment() {
        assert!(glob(b"/api/cart", b"/api/cart"));
        assert!(!glob(b"/api/cart", b"/api/cart/1"));
        assert!(glob(b"/api/cart/*", b"/api/cart/1"));
        assert!(!glob(b"/api/*", b"/api/cart/1"));
        assert!(glob(b"/api/**", b"/api/cart/1"));
        assert!(glob(b"/assets/*.js", b"/assets/index-abc.js"));
        let f = Fault::errors("GET /api/products/*=10%").unwrap();
        assert!(f.matches("get", "/api/products/3"));
        assert!(!f.matches("POST", "/api/products/3"));
    }
}
