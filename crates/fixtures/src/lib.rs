//! The fixture web site, `testdata/site`, served on two origins (plan 03,
//! "Tests").
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
//! What the server does, for every path on both origins:
//!
//! - Serves the file under `testdata/site` (`/` and directories serve their
//!   `index.html`), or a 404.
//! - Replaces `{{ORIGIN_A}}`, `{{ORIGIN_B}}` and `{{ORIGIN}}` (the origin
//!   serving the request) in `.html`, `.js` and `.css` files, so a page can
//!   link to the other origin.
//! - `?delay=<ms>` answers after that long.
//! - Sends `Cache-Control: no-store`, so a reload always hits the server.
//!
//! Add pages under `testdata/site` (and dynamic routes in [`respond`]) as
//! milestones need them, rather than starting another server.

use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::{CACHE_CONTROL, CONTENT_TYPE, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;

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
        let fixtures = Self {
            a: format!("http://127.0.0.1:{}", listener_a.local_addr()?.port()),
            b: format!("http://localhost:{}", listener_b.local_addr()?.port()),
        };
        let site = Arc::new(Site {
            root,
            a: fixtures.a.clone(),
            b: fixtures.b.clone(),
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        std::thread::Builder::new()
            .name("fixtures".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let a = serve(listener_a, site.clone(), site.a.clone());
                    let b = serve(listener_b, site.clone(), site.b.clone());
                    tokio::join!(a, b);
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
    let listener = match tokio::net::TcpListener::from_std(listener) {
        Ok(listener) => listener,
        Err(e) => {
            report(&format!("fixtures: cannot listen for {origin}: {e}"));
            return;
        }
    };
    let origin: Arc<str> = origin.into();
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let (site, origin) = (site.clone(), origin.clone());
        tokio::spawn(async move {
            let service = service_fn(move |req| {
                let (site, origin) = (site.clone(), origin.clone());
                async move { Ok::<_, Infallible>(respond(&site, &origin, &req).await) }
            });
            // Errors here are clients going away mid-response.
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades()
                .await;
        });
    }
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
    match file(site, path) {
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
        None => not_found(path),
    }
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
    fn query_params() {
        assert_eq!(param("a=1&delay=20", "delay"), Some("20"));
        assert_eq!(param("delay", "delay"), None);
        assert_eq!(param("", "delay"), None);
    }
}
