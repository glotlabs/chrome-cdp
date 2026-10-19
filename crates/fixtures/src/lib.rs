//! The fixture web site, `testdata/site`, served on two origins, plus an
//! HTTPS one (plan 03, "Tests").
//!
//! Every integration test that needs a web page uses [`Fixtures::shared`]:
//! one server per test process, started on first use on its own thread, so
//! sync and async tests alike can use it. `cargo xtask fixtures` runs the
//! same server (the `fixtures` binary) for manual debugging.
//!
//! The two origins are `http://127.0.0.1:<port>` ([`Fixtures::a`]) and
//! `http://localhost:<port>` ([`Fixtures::b`]). They are different *sites*,
//! not just different ports, so Chrome's site isolation puts an iframe from
//! one into the other in its own process (an OOPIF).
//!
//! [`Fixtures::https`] serves the same site at `https://127.0.0.1:<port>`
//! with a self-signed certificate (`crates/fixtures/tls`), which Chrome
//! rejects unless told to ignore certificate errors.
//!
//! What the server does, for every path on both origins:
//!
//! - Serves the file under `testdata/site` (`/` and directories serve their
//!   `index.html`), or a 404.
//! - Replaces `{{ORIGIN_A}}`, `{{ORIGIN_B}}` and `{{ORIGIN}}` (the origin
//!   serving the request) in `.html`, `.js` and `.css` files, so a page can
//!   link to the other origin.
//! - `?delay=<ms>` answers after that long.
//! - `?status=<code>` answers with that HTTP status (and the file, or a
//!   line of text if there is none).
//! - `/redirect?to=<path>&n=<hops>` redirects (302) `n` times (default 1)
//!   and then to `to`.
//! - `?download=<name>` sends the response as an attachment named `name`
//!   (`Content-Disposition`), so Chrome downloads it.
//! - `?set-cookie=<cookie>` (percent-encoded) sends it as a `Set-Cookie`
//!   header, for a cookie scripts cannot set (`HttpOnly`).
//! - `?header=<Name: value>` (percent-encoded, repeatable) adds that
//!   response header, e.g. a `Content-Security-Policy`.
//! - Paths under `/auth/` need HTTP basic auth as [`AUTH_USER`] with
//!   [`AUTH_PASSWORD`]; without it they answer 401 with a
//!   `WWW-Authenticate` challenge.
//! - Sends `Cache-Control: no-store`, so a reload always hits the server.
//! - `/ws` is a WebSocket that echoes every message (for pages that keep
//!   one open).
//! - `/live/ws` is a WebSocket that says `{"type":"hello"}`, then answers
//!   `{"type":"ping","id":N}` with `{"type":"pong","id":N}`; and
//!   `/live/events` an event stream of two events (for `mock record` of
//!   streaming traffic, plan 19.2).
//! - `/hmr/vite` and `/hmr/webpack` are fake dev servers' hot-reload
//!   sockets, and `POST /hmr/send` pushes a message to them (see [`hmr`]).
//! - `/items?n=<count>` answers a JSON array of `count` items (default
//!   10), for pages that need a large JSON response.
//! - `/echo` (any method, any path under it) answers with the request as
//!   JSON: `method`, `path`, `query`, `origin` (the serving origin) and
//!   `headers` (lower-case names), for checking what a request carried.
//!   It allows cross-origin requests from the other origin.
//!
//! - `/e2e/api/…` is a small stateful REST API (see [`api`]), the real
//!   backend of the test runner's shop page; each server has its own
//!   state.
//! - `/e2e/ssr?api=<origin>` stands for a server-rendered page (plan
//!   11.9): the server GETs `<origin>/e2e/api/greeting` (or `&path=…`)
//!   itself, with the request's `x-chrome-cdp-test` header, and shows the
//!   answer.
//! - `POST /bench/record` keeps its JSON body (both origins share the
//!   list), and `GET /bench/records` answers the list: what the agent
//!   benchmark's pages submitted (plan 10.4), which its verifier reads with
//!   [`Fixtures::records`].
//!
//! [`Shop`] is another server: the real backend of `examples/e2e-shop`
//! (its built app and its API on one origin, and a payment provider on
//! another).
//!
//! Add pages under `testdata/site` (and dynamic routes in [`respond`]) as
//! milestones need them, rather than starting another server.

mod api;
mod hmr;
mod payments;
mod shop;

pub use shop::{Shop, ShopRoutes};

/// Starts the shop's payment provider on `http://127.0.0.1:<port>` (0 =
/// any free port) and returns its URL; it stops with the process.
pub fn start_payments(port: u16) -> std::io::Result<String> {
    payments::start(port)
}

use std::convert::Infallible;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt as _, StreamExt as _};
use http_body_util::{BodyExt as _, Full};
use hyper::body::Incoming;
use hyper::header::{
    AUTHORIZATION, CACHE_CONTROL, CONNECTION, CONTENT_DISPOSITION, CONTENT_TYPE, HeaderValue,
    LOCATION, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_PROTOCOL, SET_COOKIE, UPGRADE,
    WWW_AUTHENTICATE,
};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;

/// The user name `/auth/` pages accept.
pub const AUTH_USER: &str = "ada";

/// The password `/auth/` pages accept.
pub const AUTH_PASSWORD: &str = "s3cret";

/// `Authorization` for [`AUTH_USER`] and [`AUTH_PASSWORD`]: base64 of
/// `ada:s3cret`.
const AUTH_HEADER: &str = "Basic YWRhOnMzY3JldA==";

/// The site's files: `testdata/site` in the repository.
pub fn site_dir() -> PathBuf {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    // crates/fixtures → the workspace root.
    let root = crate_dir.ancestors().nth(2).unwrap_or(crate_dir);
    root.join("testdata/site")
}

/// A running fixture server. It stops with the process.
#[derive(Debug, Clone)]
pub struct Fixtures {
    a: String,
    b: String,
    https: String,
    records: Records,
    /// Ends the server ([`Fixtures::stop`]).
    stop: Arc<tokio::sync::Notify>,
}

/// The bodies `POST /bench/record` kept, in order.
type Records = Arc<std::sync::Mutex<Vec<serde_json::Value>>>;

impl Fixtures {
    /// The process's server, started on first use on free ports.
    pub fn shared() -> &'static Self {
        static SHARED: OnceLock<Fixtures> = OnceLock::new();
        SHARED.get_or_init(|| {
            Self::start(0, 0).unwrap_or_else(|e| panic!("cannot start the fixture server: {e}"))
        })
    }

    /// Starts a server for [`site_dir`] on the given ports (0 = any free
    /// one) and returns once both origins accept connections.
    pub fn start(port_a: u16, port_b: u16) -> std::io::Result<Self> {
        Self::start_in(site_dir(), port_a, port_b)
    }

    /// Like [`Fixtures::start`], serving `root`.
    pub fn start_in(root: PathBuf, port_a: u16, port_b: u16) -> std::io::Result<Self> {
        let listener_a = bind(port_a)?;
        let listener_b = bind(port_b)?;
        let listener_https = bind(0)?;
        let tls = tls_acceptor()?;
        let fixtures = Self {
            a: format!("http://127.0.0.1:{}", listener_a.local_addr()?.port()),
            b: format!("http://localhost:{}", listener_b.local_addr()?.port()),
            https: format!("https://127.0.0.1:{}", listener_https.local_addr()?.port()),
            records: Records::default(),
            stop: Arc::new(tokio::sync::Notify::new()),
        };
        let site = Arc::new(Site {
            root,
            a: fixtures.a.clone(),
            b: fixtures.b.clone(),
            api: std::sync::Mutex::default(),
            records: fixtures.records.clone(),
            hmr: hmr::Channels::default(),
        });
        let https = fixtures.https.clone();
        let stop = fixtures.stop.clone();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        std::thread::Builder::new()
            .name("fixtures".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let a = serve(listener_a, site.clone(), site.a.clone());
                    let b = serve(listener_b, site.clone(), site.b.clone());
                    let https = serve_tls(listener_https, tls, site.clone(), https);
                    tokio::select! {
                        _ = async { tokio::join!(a, b, https) } => {}
                        () = stop.notified() => {}
                    }
                });
                // Dropping the runtime closes the listeners and every
                // connection.
                drop(runtime);
            })?;
        Ok(fixtures)
    }

    /// Stops the server: its ports refuse connections from now on (a
    /// server that went down).
    pub fn stop(&self) {
        self.stop.notify_one();
        let addr = self.a.trim_start_matches("http://").to_owned();
        for _ in 0..200 {
            if std::net::TcpStream::connect(&addr).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Origin A, `http://127.0.0.1:<port>`.
    pub fn a(&self) -> &str {
        &self.a
    }

    /// Origin B, `http://localhost:<port>`: another site than A.
    pub fn b(&self) -> &str {
        &self.b
    }

    /// The HTTPS origin, `https://127.0.0.1:<port>`, with a self-signed
    /// certificate.
    pub fn https(&self) -> &str {
        &self.https
    }

    /// The JSON bodies pages sent to `POST /bench/record`, in order.
    pub fn records(&self) -> Vec<serde_json::Value> {
        self.records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// `path` (starting with `/`) on origin A.
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.a)
    }

    /// `path` (starting with `/`) on origin B.
    pub fn url_b(&self, path: &str) -> String {
        format!("{}{path}", self.b)
    }

    /// Sends `message` to the fake hot-reload sockets (`/hmr/vite`,
    /// `/hmr/webpack`) of `channel`, as a dev server would after an edit;
    /// returns how many sockets got it (0 until the page connected).
    pub fn hmr_send(&self, channel: &str, message: &str) -> usize {
        use std::io::{Read as _, Write as _};
        let host = self.b.strip_prefix("http://").unwrap_or(&self.b);
        let Ok(mut stream) = std::net::TcpStream::connect(host) else {
            return 0;
        };
        let request = format!(
            "POST /hmr/send?channel={channel} HTTP/1.1\r\nHost: {host}\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{message}",
            message.len()
        );
        let mut text = String::new();
        if stream.write_all(request.as_bytes()).is_err()
            || stream.read_to_string(&mut text).is_err()
        {
            return 0;
        }
        text.split_once("\r\n\r\n")
            .and_then(|(_, body)| body.trim().parse().ok())
            .unwrap_or(0)
    }
}

fn bind(port: u16) -> std::io::Result<TcpListener> {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// What every request handler needs.
#[derive(Debug)]
struct Site {
    root: PathBuf,
    a: String,
    b: String,
    api: std::sync::Mutex<api::Api>,
    records: Records,
    /// The fake dev servers' hot-reload sockets, by channel.
    hmr: hmr::Channels,
}

async fn serve(listener: TcpListener, site: Arc<Site>, origin: String) {
    let Some(listener) = listen(listener, &origin) else {
        return;
    };
    let origin: Arc<str> = origin.into();
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        tokio::spawn(serve_connection(stream, site.clone(), origin.clone()));
    }
}

/// Like [`serve`], over TLS.
async fn serve_tls(listener: TcpListener, tls: TlsAcceptor, site: Arc<Site>, origin: String) {
    let Some(listener) = listen(listener, &origin) else {
        return;
    };
    let origin: Arc<str> = origin.into();
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let (tls, site, origin) = (tls.clone(), site.clone(), origin.clone());
        tokio::spawn(async move {
            // A failed handshake is Chrome rejecting the certificate.
            if let Ok(stream) = tls.accept(stream).await {
                serve_connection(stream, site, origin).await;
            }
        });
    }
}

fn listen(listener: TcpListener, origin: &str) -> Option<tokio::net::TcpListener> {
    match tokio::net::TcpListener::from_std(listener) {
        Ok(listener) => Some(listener),
        Err(e) => {
            report(&format!("fixtures: cannot listen for {origin}: {e}"));
            None
        }
    }
}

async fn serve_connection<S>(stream: S, site: Arc<Site>, origin: Arc<str>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let service = service_fn(move |mut req: Request<Incoming>| {
        let (site, origin) = (site.clone(), origin.clone());
        async move {
            match req.uri().path() {
                "/ws" => return Ok::<_, Infallible>(websocket(&mut req)),
                "/live/ws" => return Ok::<_, Infallible>(live_socket(&mut req)),
                "/hmr/vite" => return Ok(hmr::socket(&site, &mut req, hmr::Kind::Vite)),
                "/hmr/webpack" => return Ok(hmr::socket(&site, &mut req, hmr::Kind::Webpack)),
                "/hmr/send" => return Ok(hmr::send(&site, req).await),
                _ => {}
            }
            if req.uri().path() == "/e2e/ssr" {
                return Ok::<_, Infallible>(ssr(&req).await);
            }
            if matches!(req.uri().path(), "/bench/record" | "/bench/records") {
                return Ok::<_, Infallible>(record(&site, req).await);
            }
            if req.uri().path().starts_with(api::PREFIX) {
                let (parts, body) = req.into_parts();
                let body = body
                    .collect()
                    .await
                    .map(http_body_util::Collected::to_bytes)
                    .unwrap_or_default();
                let cookie = parts
                    .headers
                    .get(hyper::header::COOKIE)
                    .and_then(|v| v.to_str().ok());
                return Ok::<_, Infallible>(api::respond(
                    &site.api,
                    &parts.method,
                    parts.uri.path(),
                    cookie,
                    &body,
                ));
            }
            Ok::<_, Infallible>(respond(&site, &origin, &req).await)
        }
    });
    // Errors here are clients going away mid-response.
    let _ = http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades()
        .await;
}

/// `POST /bench/record`: keeps the JSON body; `GET /bench/records`: the
/// bodies kept. Pages of either origin may post (CORS).
async fn record(site: &Site, req: Request<Incoming>) -> Response<Full<Bytes>> {
    let records = || {
        site.records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    let post = req.method() == hyper::Method::POST && req.uri().path() == "/bench/record";
    let get = req.method() == hyper::Method::GET && req.uri().path() == "/bench/records";
    let mut answer = if post {
        let body = req
            .into_body()
            .collect()
            .await
            .map(http_body_util::Collected::to_bytes)
            .unwrap_or_default();
        match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(value) => {
                records().push(value);
                response(StatusCode::NO_CONTENT, "text/plain", Bytes::new())
            }
            Err(e) => response(
                StatusCode::BAD_REQUEST,
                "text/plain; charset=utf-8",
                Bytes::from(format!("not JSON: {e}\n")),
            ),
        }
    } else if get {
        let list = serde_json::Value::from(records().clone());
        response(
            StatusCode::OK,
            "application/json",
            Bytes::from(list.to_string()),
        )
    } else {
        response(
            StatusCode::METHOD_NOT_ALLOWED,
            "text/plain; charset=utf-8",
            Bytes::from_static(b"POST /bench/record or GET /bench/records\n"),
        )
    };
    answer
        .headers_mut()
        .insert("access-control-allow-origin", HeaderValue::from_static("*"));
    answer
}

/// The self-signed certificate for `127.0.0.1` in `crates/fixtures/tls`
/// (valid for 100 years).
fn tls_acceptor() -> std::io::Result<TlsAcceptor> {
    let invalid = |e: &dyn std::fmt::Display| std::io::Error::other(e.to_string());
    let cert = CertificateDer::from_pem_slice(include_bytes!("../tls/cert.pem"))
        .map_err(|e| invalid(&e))?;
    let key =
        PrivateKeyDer::from_pem_slice(include_bytes!("../tls/key.pem")).map_err(|e| invalid(&e))?;
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| invalid(&e))?
    .with_no_client_auth()
    .with_single_cert(vec![cert], key)
    .map_err(|e| invalid(&e))?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

#[allow(clippy::print_stderr)]
fn report(message: &str) {
    eprintln!("{message}");
}

/// Answers one request.
async fn respond(site: &Site, origin: &str, req: &Request<Incoming>) -> Response<Full<Bytes>> {
    let query = req.uri().query().unwrap_or_default();
    if let Some(ms) = param(query, "delay").and_then(|v| v.parse::<u64>().ok()) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
    let path = req.uri().path();
    if path == "/redirect" {
        return redirect(query);
    }
    if path == "/echo" || path.starts_with("/echo/") {
        return echo(origin, req);
    }
    if path.starts_with("/auth/")
        && req.headers().get(AUTHORIZATION).map(HeaderValue::as_bytes)
            != Some(AUTH_HEADER.as_bytes())
    {
        let mut response = response(
            StatusCode::UNAUTHORIZED,
            "text/plain; charset=utf-8",
            Bytes::from_static(b"log in first\n"),
        );
        response.headers_mut().insert(
            WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"fixtures\""),
        );
        return response;
    }
    let status = param(query, "status")
        .and_then(|v| v.parse::<u16>().ok())
        .and_then(|code| StatusCode::from_u16(code).ok());
    let mut response = match file(site, path) {
        None if path == "/items" => items(query),
        None if path == "/live/events" => live_events(),
        None if path == "/targets/api" => targets_api(query),
        Some(file) => {
            let Ok(body) = std::fs::read(&file) else {
                return not_found(path);
            };
            let kind = content_type(&file);
            let body = if kind.starts_with("text/") || kind.starts_with("application/javascript") {
                Bytes::from(expand(&String::from_utf8_lossy(&body), site, origin))
            } else {
                Bytes::from(body)
            };
            response(StatusCode::OK, kind, body)
        }
        None if status.is_some() => response(
            StatusCode::OK,
            "text/plain; charset=utf-8",
            Bytes::from(format!("{path}\n")),
        ),
        None => not_found(path),
    };
    if let Some(status) = status {
        *response.status_mut() = status;
    }
    if let Some(cookie) = param(query, "set-cookie")
        && let Ok(value) = HeaderValue::from_str(&percent_decode(cookie))
    {
        response.headers_mut().append(SET_COOKIE, value);
    }
    for header in params(query, "header") {
        let header = percent_decode(header);
        if let Some((name, value)) = header.split_once(':')
            && let (Ok(name), Ok(value)) = (
                hyper::header::HeaderName::from_bytes(name.trim().as_bytes()),
                HeaderValue::from_str(value.trim()),
            )
        {
            response.headers_mut().append(name, value);
        }
    }
    if let Some(name) = param(query, "download")
        && let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
    {
        response.headers_mut().insert(CONTENT_DISPOSITION, value);
    }
    response
}

/// `/ws`: accepts the WebSocket upgrade and echoes every message until the
/// page closes it.
fn websocket(req: &mut Request<Incoming>) -> Response<Full<Bytes>> {
    upgrade(req, None, |mut ws| async move {
        while let Some(Ok(message)) = ws.next().await {
            if (message.is_text() || message.is_binary()) && ws.send(message).await.is_err() {
                break;
            }
        }
    })
}

/// The server end of a WebSocket.
type ServerSocket = WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>;

/// Accepts `req`'s WebSocket upgrade (with `protocol` as its
/// subprotocol) and runs `serve` on the socket; a 400 without one.
fn upgrade<F, Fut>(
    req: &mut Request<Incoming>,
    protocol: Option<&'static str>,
    serve: F,
) -> Response<Full<Bytes>>
where
    F: FnOnce(ServerSocket) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send,
{
    let Some(key) = req.headers().get(SEC_WEBSOCKET_KEY) else {
        return response(
            StatusCode::BAD_REQUEST,
            "text/plain; charset=utf-8",
            Bytes::from(format!("{} needs a WebSocket upgrade\n", req.uri().path())),
        );
    };
    let accept = derive_accept_key(key.as_bytes());
    let upgrade = hyper::upgrade::on(req);
    tokio::spawn(async move {
        let Ok(upgraded) = upgrade.await else {
            return;
        };
        let ws = WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None).await;
        serve(ws).await;
    });
    let mut response = response(
        StatusCode::SWITCHING_PROTOCOLS,
        "text/plain; charset=utf-8",
        Bytes::new(),
    );
    let headers = response.headers_mut();
    headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    if let Ok(accept) = HeaderValue::from_str(&accept) {
        headers.insert(SEC_WEBSOCKET_ACCEPT, accept);
    }
    if let Some(protocol) = protocol {
        headers.insert(SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static(protocol));
    }
    response
}

/// `/e2e/ssr?api=http://127.0.0.1:<port>`: a page whose server fetched
/// `/e2e/api/greeting` (or `path`) of `api` (forwarding
/// `x-chrome-cdp-test`).
async fn ssr(req: &Request<Incoming>) -> Response<Full<Bytes>> {
    let query = req.uri().query().unwrap_or_default();
    let api = param(query, "api").unwrap_or_default();
    let test = req
        .headers()
        .get("x-chrome-cdp-test")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let path = param(query, "path").unwrap_or("/e2e/api/greeting");
    let fetched = server_get(api, path, test.as_deref()).await;
    let escape = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let answer = match fetched {
        Ok((status, body)) => format!("{status} {}", escape(body.trim())),
        Err(e) => format!("failed: {}", escape(&e.to_string())),
    };
    let html = format!(
        "<!doctype html>\n<meta charset=\"utf-8\">\n<title>SSR</title>\n\
         <p id=\"answer\">The server got: {answer}</p>\n\
         <p id=\"test\">Test header: {}</p>\n",
        escape(test.as_deref().unwrap_or("none"))
    );
    response(
        StatusCode::OK,
        "text/html; charset=utf-8",
        Bytes::from(html),
    )
}

/// A GET the server itself sends (HTTP/1.1, no TLS): the status code
/// and body.
async fn server_get(
    origin: &str,
    path: &str,
    test: Option<&str>,
) -> std::io::Result<(u16, String)> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let host = origin.strip_prefix("http://").unwrap_or(origin);
    let mut stream =
        tokio::time::timeout(Duration::from_secs(5), tokio::net::TcpStream::connect(host))
            .await
            .map_err(|_| std::io::Error::other("connect timed out"))??;
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    if let Some(test) = test {
        request = format!("{request}x-chrome-cdp-test: {test}\r\n");
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await?;
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_string(&mut text))
        .await
        .map_err(|_| std::io::Error::other("read timed out"))??;
    let status = text
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b)
        .unwrap_or_default();
    Ok((status, body.to_owned()))
}

/// `/live/events`: two events, and a long `retry` so the page does not
/// come back for more when the (whole, not streamed) answer ends.
fn live_events() -> Response<Full<Bytes>> {
    response(
        StatusCode::OK,
        "text/event-stream",
        Bytes::from_static(
            b"retry: 3600000\n\ndata: first\n\nevent: headline\nid: 2\ndata: {\"title\":\"Second\"}\n\n",
        ),
    )
}

/// `/live/ws`: says hello, then answers pings with pongs.
fn live_socket(req: &mut Request<Incoming>) -> Response<Full<Bytes>> {
    upgrade(req, None, |mut ws| async move {
        let hello = Message::text(r#"{"type":"hello"}"#);
        if ws.send(hello).await.is_err() {
            return;
        }
        while let Some(Ok(message)) = ws.next().await {
            let Message::Text(text) = message else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(text.as_str()) else {
                continue;
            };
            if value["type"] == "ping" {
                let pong = serde_json::json!({ "type": "pong", "id": value["id"] });
                if ws.send(Message::text(pong.to_string())).await.is_err() {
                    break;
                }
            }
        }
    })
}

/// `/items?n=<count>`: a JSON array of `count` items.
fn items(query: &str) -> Response<Full<Bytes>> {
    let count = param(query, "n")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(10);
    let items: Vec<String> = (1..=count)
        .map(|i| {
            format!(
                "{{\"id\":{i},\"name\":\"item {i}\",\"done\":{}}}",
                i % 2 == 0
            )
        })
        .collect();
    response(
        StatusCode::OK,
        "application/json",
        Bytes::from(format!("[{}]", items.join(","))),
    )
}

/// `/targets/api?from=<who>`: `{"from": who}`, for the frames and
/// workers of `targets/matrix.html` (plan 13).
fn targets_api(query: &str) -> Response<Full<Bytes>> {
    let from = percent_decode(param(query, "from").unwrap_or_default());
    let body = serde_json::json!({ "from": from }).to_string();
    response(StatusCode::OK, "application/json", Bytes::from(body))
}

/// `/echo`: the request as JSON.
fn echo(origin: &str, req: &Request<Incoming>) -> Response<Full<Bytes>> {
    let headers: serde_json::Map<String, serde_json::Value> = req
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                String::from_utf8_lossy(value.as_bytes()).into(),
            )
        })
        .collect();
    let body = serde_json::json!({
        "method": req.method().as_str(),
        "path": req.uri().path(),
        "query": req.uri().query().unwrap_or_default(),
        "origin": origin,
        "headers": headers,
    });
    let mut response = response(
        StatusCode::OK,
        "application/json",
        Bytes::from(body.to_string()),
    );
    if let Some(from) = req.headers().get(hyper::header::ORIGIN) {
        let cors = response.headers_mut();
        cors.insert(hyper::header::ACCESS_CONTROL_ALLOW_ORIGIN, from.clone());
        cors.insert(
            hyper::header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("*"),
        );
        cors.insert(
            hyper::header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, PUT, PATCH, DELETE"),
        );
    }
    response
}

/// `/redirect?to=<path>&n=<hops>`: one 302 hop of the chain.
fn redirect(query: &str) -> Response<Full<Bytes>> {
    let to = param(query, "to").unwrap_or("/");
    let hops = param(query, "n")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(1);
    let location = if hops > 1 {
        format!("/redirect?to={to}&n={}", hops - 1)
    } else {
        to.to_owned()
    };
    let mut response = response(StatusCode::FOUND, "text/plain; charset=utf-8", Bytes::new());
    if let Ok(location) = HeaderValue::from_str(&location) {
        response.headers_mut().insert(LOCATION, location);
    }
    response
}

/// The file for a request path, if it is inside the site.
fn file(site: &Site, path: &str) -> Option<PathBuf> {
    let relative = Path::new(path.trim_start_matches('/'));
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let mut file = site.root.join(relative);
    if file.is_dir() {
        file.push("index.html");
    }
    file.is_file().then_some(file)
}

/// `{{ORIGIN_A}}`, `{{ORIGIN_B}}` and `{{ORIGIN}}` filled in.
fn expand(text: &str, site: &Site, origin: &str) -> String {
    text.replace("{{ORIGIN_A}}", &site.a)
        .replace("{{ORIGIN_B}}", &site.b)
        .replace("{{ORIGIN}}", origin)
}

fn content_type(file: &Path) -> &'static str {
    match file.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("txt") => "text/plain; charset=utf-8",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

fn not_found(path: &str) -> Response<Full<Bytes>> {
    response(
        StatusCode::NOT_FOUND,
        "text/plain; charset=utf-8",
        Bytes::from(format!("no fixture at {path}\n")),
    )
}

fn response(status: StatusCode, kind: &'static str, body: Bytes) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(body));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(kind));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// `%XX` escapes (and `+` for a space) decoded; bad escapes stay.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => match text
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                Some(b) => {
                    out.push(b);
                    i += 3;
                    continue;
                }
                None => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The value of `name` in a query string (no percent-decoding; fixture
/// params are plain).
fn param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
}

/// Every value of query parameter `name`, in order.
fn params<'a>(query: &'a str, name: &'a str) -> impl Iterator<Item = &'a str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .filter(move |(key, _)| *key == name)
        .map(|(_, value)| value)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::io::{Read as _, Write as _};
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    use super::{Fixtures, param};

    /// A plain HTTP/1.1 GET; returns the status line and the body.
    fn get(url: &str) -> (String, String) {
        let rest = url.strip_prefix("http://").unwrap();
        let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        let port = authority.rsplit_once(':').unwrap().1;
        let mut stream = TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap())).unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        (head.lines().next().unwrap().to_owned(), body.to_owned())
    }

    #[test]
    fn serves_the_site_on_two_origins() {
        let f = Fixtures::shared();
        assert!(f.a().starts_with("http://127.0.0.1:"));
        assert!(f.b().starts_with("http://localhost:"));
        assert_ne!(f.a(), f.b());
        for url in [f.url("/"), f.url_b("/index.html")] {
            let (status, body) = get(&url);
            assert_eq!(status, "HTTP/1.1 200 OK");
            assert!(
                body.contains("<title>chrome-cdp fixtures</title>"),
                "{body}"
            );
        }
    }

    #[test]
    fn fills_in_the_origins() {
        let f = Fixtures::shared();
        let (_, body) = get(&f.url("/frames/cross-origin.html"));
        assert!(
            body.contains(&format!("{}/frames/child.html", f.b())),
            "{body}"
        );
        assert!(!body.contains("{{"), "{body}");
    }

    #[test]
    fn missing_files_and_escapes_are_404() {
        let f = Fixtures::shared();
        for path in ["/nope.html", "/../Cargo.toml", "/tabs/../../Cargo.toml"] {
            let (status, _) = get(&f.url(path));
            assert_eq!(status, "HTTP/1.1 404 Not Found", "{path}");
        }
    }

    #[test]
    fn delay_holds_the_response() {
        let f = Fixtures::shared();
        let start = Instant::now();
        let (status, _) = get(&f.url("/?delay=300"));
        assert_eq!(status, "HTTP/1.1 200 OK");
        assert!(start.elapsed() >= Duration::from_millis(300));
    }

    #[test]
    fn status_and_redirects() {
        let f = Fixtures::shared();
        let (status, body) = get(&f.url("/?status=500"));
        assert_eq!(status, "HTTP/1.1 500 Internal Server Error");
        assert!(body.contains("chrome-cdp fixtures"), "{body}");
        let (status, body) = get(&f.url("/gone?status=410"));
        assert_eq!(status, "HTTP/1.1 410 Gone");
        assert_eq!(body, "/gone\n");

        let mut stream = TcpStream::connect(f.a().trim_start_matches("http://")).unwrap();
        write!(
            stream,
            "GET /redirect?to=/nav/a.html&n=2 HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        assert!(text.starts_with("HTTP/1.1 302 Found"), "{text}");
        assert!(
            text.contains("location: /redirect?to=/nav/a.html&n=1"),
            "{text}"
        );
    }

    /// A GET with extra header lines; returns the whole response.
    fn get_raw(url: &str, headers: &str) -> String {
        let rest = url.strip_prefix("http://").unwrap();
        let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        let mut stream = TcpStream::connect(authority).unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: {authority}\r\n{headers}Connection: close\r\n\r\n"
        )
        .unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        text
    }

    #[test]
    fn downloads_and_basic_auth() {
        let f = Fixtures::shared();
        let text = get_raw(&f.url("/downloads/report.txt?download=report.txt"), "");
        assert!(
            text.contains("content-disposition: attachment; filename=\"report.txt\""),
            "{text}"
        );

        let text = get_raw(&f.url("/auth/"), "");
        assert!(text.starts_with("HTTP/1.1 401 Unauthorized"), "{text}");
        assert!(
            text.contains("www-authenticate: Basic realm=\"fixtures\""),
            "{text}"
        );
        let text = get_raw(
            &f.url("/auth/"),
            &format!("Authorization: {}\r\n", super::AUTH_HEADER),
        );
        assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
        assert!(text.contains("Signed in"), "{text}");
    }

    #[test]
    fn serves_https() {
        let f = Fixtures::shared();
        assert!(f.https().starts_with("https://127.0.0.1:"));
        let port = f.https().rsplit_once(':').unwrap().1;
        // A TLS server answers a plain-HTTP request with an alert, not a page.
        let mut stream = TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap())).unwrap();
        write!(stream, "GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut bytes = Vec::new();
        let _ = stream.read_to_end(&mut bytes);
        assert!(!bytes.starts_with(b"HTTP/"), "{bytes:?}");
    }

    #[test]
    fn echo_describes_the_request() {
        let f = Fixtures::shared();
        let text = get_raw(&f.url("/echo/x?a=1"), "X-Test: yes\r\n");
        assert!(text.starts_with("HTTP/1.1 200"), "{text}");
        let body = text.split("\r\n\r\n").nth(1).unwrap();
        let json: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(json["method"], "GET");
        assert_eq!(json["path"], "/echo/x");
        assert_eq!(json["query"], "a=1");
        assert_eq!(json["headers"]["x-test"], "yes");
    }

    #[test]
    fn targets_api_says_who_asked() {
        let f = Fixtures::shared();
        let (status, body) = get(&f.url_b("/targets/api?from=oopif-shared"));
        assert!(status.contains("200"), "{status}");
        assert_eq!(body, r#"{"from":"oopif-shared"}"#);
    }

    #[test]
    fn items_are_json() {
        let f = Fixtures::shared();
        let (status, body) = get(&f.url("/items?n=2"));
        assert_eq!(status, "HTTP/1.1 200 OK");
        assert_eq!(
            body,
            r#"[{"id":1,"name":"item 1","done":false},{"id":2,"name":"item 2","done":true}]"#
        );
        let (status, _) = get(&f.url("/items?n=1&status=503"));
        assert_eq!(status, "HTTP/1.1 503 Service Unavailable");
    }

    #[test]
    fn set_cookie_sends_the_header() {
        let f = Fixtures::shared();
        let text = get_raw(
            &f.url("/?set-cookie=session%3Dada-42%3B%20HttpOnly%3B%20Path%3D%2F"),
            "",
        );
        assert!(
            text.contains("set-cookie: session=ada-42; HttpOnly; Path=/"),
            "{text}"
        );
    }

    #[test]
    fn hmr_sockets_greet_and_relay_what_is_sent() {
        use tokio_tungstenite::tungstenite::{client, handshake::client::generate_key};
        let f = Fixtures::shared();
        assert_eq!(f.hmr_send("nobody", "{}"), 0);
        let host = f.b().strip_prefix("http://").unwrap();
        let request = hyper::Request::get(format!("ws://{host}/hmr/vite?channel=greet"))
            .header("Host", host)
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", generate_key())
            .header("Sec-WebSocket-Protocol", "vite-hmr")
            .body(())
            .unwrap();
        let stream = TcpStream::connect(host).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let (mut ws, response) = client::client(request, stream).unwrap();
        assert_eq!(
            response.headers()["sec-websocket-protocol"],
            "vite-hmr",
            "{response:?}"
        );
        assert_eq!(
            ws.read().unwrap().to_text().unwrap(),
            r#"{"type":"connected"}"#
        );
        // The socket joins its channel as the upgrade is answered.
        assert_eq!(f.hmr_send("greet", r#"{"type":"full-reload"}"#), 1);
        assert_eq!(
            ws.read().unwrap().to_text().unwrap(),
            r#"{"type":"full-reload"}"#
        );
        drop(ws);
    }

    #[test]
    fn bench_records_keep_posted_json() {
        // Its own server: the records list belongs to one.
        let f = Fixtures::start(0, 0).unwrap();
        let post = |url: &str, body: &str| {
            let rest = url.strip_prefix("http://").unwrap();
            let (authority, path) = rest.split_at(rest.find('/').unwrap());
            let mut stream = TcpStream::connect(authority).unwrap();
            write!(
                stream,
                "POST {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            let mut text = String::new();
            stream.read_to_string(&mut text).unwrap();
            text
        };
        let text = post(&f.url("/bench/record"), r#"{"form":"login","email":"a@b"}"#);
        assert!(text.starts_with("HTTP/1.1 204"), "{text}");
        // Origin B shares the list.
        post(&f.url_b("/bench/record"), r#"{"form":"frame"}"#);
        let text = post(&f.url("/bench/record"), "not json");
        assert!(text.starts_with("HTTP/1.1 400"), "{text}");
        assert_eq!(
            serde_json::Value::from(f.records()),
            serde_json::json!([{"form": "login", "email": "a@b"}, {"form": "frame"}])
        );
        let (status, body) = get(&f.url("/bench/records"));
        assert_eq!(status, "HTTP/1.1 200 OK");
        assert_eq!(body, r#"[{"email":"a@b","form":"login"},{"form":"frame"}]"#);
    }

    #[test]
    fn query_params() {
        assert_eq!(param("a=1&delay=20", "delay"), Some("20"));
        assert_eq!(param("delay", "delay"), None);
        assert_eq!(param("", "delay"), None);
    }
}
