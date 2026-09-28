//! `[e2e.web_server] static = "dist"` (plan 11, milestone 11.10): a
//! prebuilt single-page app served by chrome-cdp itself, so a project
//! that commits its build (or builds it in an earlier CI step) needs no
//! Node or other server to run its tests.
//!
//! [`StaticServer`] serves the files below a directory on a thread of its
//! own, as [`crate::mock_server::MockServer`] does:
//!
//! - `GET` and `HEAD` only (else 405); a directory serves its
//!   `index.html`;
//! - a path that names no file and whose last segment has no `.` gets the
//!   root `index.html` (200), so the app's client-side routes load on a
//!   reload or a `goto`; a missing file with an extension (`/app.js`) is
//!   404, so a broken build shows as a failed request;
//! - `Cache-Control: no-cache`, content types by extension (the mock
//!   engine's), and no path outside the directory (`..` is 404).

use std::convert::Infallible;
use std::net::{SocketAddr, TcpListener};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::{ALLOW, CACHE_CONTROL, CONTENT_TYPE, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;

/// A running static file server; dropping it stops it.
#[derive(Debug)]
pub struct StaticServer {
    addr: SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl StaticServer {
    /// Starts serving `dir` on `addr`. An address that cannot be bound is
    /// the error.
    pub fn start(addr: SocketAddr, dir: PathBuf) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("static-server".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                        return;
                    };
                    tokio::select! {
                        () = accept(listener, dir) => {}
                        _ = stopped => {}
                    }
                });
            })?;
        Ok(Self {
            addr,
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    /// Where it listens.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    fn halt(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for StaticServer {
    fn drop(&mut self) {
        self.halt();
    }
}

async fn accept(listener: tokio::net::TcpListener, dir: PathBuf) {
    let dir = std::sync::Arc::new(dir);
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        };
        let dir = dir.clone();
        tokio::spawn(async move {
            let service = service_fn(move |req: Request<Incoming>| {
                let dir = dir.clone();
                async move { Ok::<_, Infallible>(respond(&dir, req.method(), req.uri().path())) }
            });
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

/// The answer to `method path` from the files below `dir`.
fn respond(dir: &Path, method: &Method, path: &str) -> Response<Full<Bytes>> {
    if method != Method::GET && method != Method::HEAD {
        let mut response = text(StatusCode::METHOD_NOT_ALLOWED, "only GET and HEAD\n");
        response
            .headers_mut()
            .insert(ALLOW, HeaderValue::from_static("GET, HEAD"));
        return response;
    }
    let Some(file) = resolve(dir, path) else {
        return text(StatusCode::NOT_FOUND, &format!("not found: {path}\n"));
    };
    let Ok(bytes) = std::fs::read(&file) else {
        return text(StatusCode::NOT_FOUND, &format!("not found: {path}\n"));
    };
    let body = if method == Method::HEAD {
        Bytes::new()
    } else {
        Bytes::from(bytes)
    };
    let mut response = Response::new(Full::new(body));
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static(mock::content_type(&file)),
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// The file that answers `path`: the file itself, a directory's
/// `index.html`, or, for a path whose last segment has no `.`, the root
/// `index.html`. `None` for a path that would leave `dir`.
fn resolve(dir: &Path, path: &str) -> Option<PathBuf> {
    let decoded = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .ok()?;
    let relative = Path::new(decoded.trim_start_matches('/'));
    if !relative
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    let file = dir.join(relative);
    if file.is_file() {
        return Some(file);
    }
    let index = file.join("index.html");
    if index.is_file() {
        return Some(index);
    }
    let last = decoded.rsplit('/').next().unwrap_or_default();
    if last.contains('.') {
        return None;
    }
    Some(dir.join("index.html")).filter(|i| i.is_file())
}

fn text(status: StatusCode, body: &str) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(body.to_owned())));
    *response.status_mut() = status;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

#[cfg(test)]
mod tests {
    use hyper::{Method, StatusCode};

    use super::{resolve, respond};

    #[test]
    fn files_directories_and_client_side_routes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("index.html"), "<p>app</p>").unwrap();
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/app.js"), "1").unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/index.html"), "docs").unwrap();

        assert_eq!(resolve(root, "/"), Some(root.join("index.html")));
        assert_eq!(
            resolve(root, "/assets/app.js"),
            Some(root.join("assets/app.js"))
        );
        assert_eq!(resolve(root, "/docs/"), Some(root.join("docs/index.html")));
        // Client-side routes get the app; missing assets do not.
        assert_eq!(resolve(root, "/products/2"), Some(root.join("index.html")));
        assert_eq!(resolve(root, "/assets/gone.js"), None);
        // Nothing outside the directory.
        assert_eq!(resolve(root, "/../secret"), None);
        assert_eq!(resolve(root, "/assets/%2e%2e/%2e%2e/secret"), None);

        let js = respond(root, &Method::GET, "/assets/app.js");
        assert_eq!(js.status(), StatusCode::OK);
        assert_eq!(
            js.headers()["content-type"],
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            respond(root, &Method::GET, "/gone.css").status(),
            StatusCode::NOT_FOUND
        );
        let post = respond(root, &Method::POST, "/");
        assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(post.headers()["allow"], "GET, HEAD");
    }
}
