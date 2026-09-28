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
//! - Paths under `/auth/` need HTTP basic auth as [`AUTH_USER`] with
//!   [`AUTH_PASSWORD`]; without it they answer 401 with a
//!   `WWW-Authenticate` challenge.
//! - Sends `Cache-Control: no-store`, so a reload always hits the server.
//! - `/ws` is a WebSocket that echoes every message (for pages that keep
//!   one open).
//! - `/items?n=<count>` answers a JSON array of `count` items (default
//!   10), for pages that need a large JSON response.
//! - `/echo` (any method, any path under it) answers with the request as
//!   JSON: `method`, `path`, `query`, `origin` (the serving origin) and
//!   `headers` (lower-case names), for checking what a request carried.
//!   It allows cross-origin requests from the other origin.
//!
//! Add pages under `testdata/site` (and dynamic routes in [`respond`]) as
//! milestones need them, rather than starting another server.

use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt as _, StreamExt as _};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::{
    AUTHORIZATION, CACHE_CONTROL, CONNECTION, CONTENT_DISPOSITION, CONTENT_TYPE, HeaderValue,
    LOCATION, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, UPGRADE, WWW_AUTHENTICATE,
};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::WebSocketStream;
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
}

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
        };
        let site = Arc::new(Site {
            root,
            a: fixtures.a.clone(),
            b: fixtures.b.clone(),
        });
        let https = fixtures.https.clone();
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
                    tokio::join!(a, b, https);
                });
            })?;
        Ok(fixtures)
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

    /// `path` (starting with `/`) on origin A.
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.a)
    }

    /// `path` (starting with `/`) on origin B.
    pub fn url_b(&self, path: &str) -> String {
        format!("{}{path}", self.b)
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
            if req.uri().path() == "/ws" {
                return Ok::<_, Infallible>(websocket(&mut req));
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
    let Some(key) = req.headers().get(SEC_WEBSOCKET_KEY) else {
        return response(
            StatusCode::BAD_REQUEST,
            "text/plain; charset=utf-8",
            Bytes::from_static(b"/ws needs a WebSocket upgrade\n"),
        );
    };
    let accept = derive_accept_key(key.as_bytes());
    let upgrade = hyper::upgrade::on(req);
    tokio::spawn(async move {
        let Ok(upgraded) = upgrade.await else {
            return;
        };
        let mut ws =
            WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None).await;
        while let Some(Ok(message)) = ws.next().await {
            if (message.is_text() || message.is_binary()) && ws.send(message).await.is_err() {
                break;
            }
        }
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
    response
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

/// The value of `name` in a query string (no percent-decoding; fixture
/// params are plain).
fn param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)
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
    fn query_params() {
        assert_eq!(param("a=1&delay=20", "delay"), Some("20"));
        assert_eq!(param("delay", "delay"), None);
        assert_eq!(param("", "delay"), None);
    }
}
