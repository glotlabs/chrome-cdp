//! `mock serve --upstream` (plan 19, milestone 19.3): the requests a
//! `passthrough` answer (a mock's, or a set's `unmatched`) sends to the
//! real server go to the upstream instead of a 502.
//!
//! A forwarded request keeps its method, path, query and body; its
//! headers lose the hop-by-hop ones, and `Host` becomes the upstream's.
//! The answer comes back as it is (its body streamed), unless a `patch`
//! is merged into its JSON. A connection error or a timeout answers 502
//! with a JSON body naming the upstream and the error.

use std::convert::Infallible;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt as _, Full, StreamBody};
use hyper::body::Frame;
use serde_json::{Value, json};

use super::Body;

/// How long a forwarded request may take, unless set.
pub const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(30);

/// Headers of one connection, never forwarded (RFC 9110 §7.6.1), with
/// `Host` (the upstream's own is sent) and `Content-Length` (the client
/// sets it from the body).
const HOP_BY_HOP: [&str; 11] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
];

/// Where `passthrough` requests go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    /// The server, e.g. `http://localhost:8080` or `http://api:8080/v2`
    /// (its path is a prefix of every forwarded path), without a trailing
    /// slash.
    url: String,
    /// How long one request may take.
    pub timeout: Duration,
}

impl Upstream {
    /// `url` must be an absolute `http(s)` URL with no query; the error
    /// says why it is not.
    pub fn new(url: &str, timeout: Duration) -> Result<Self, String> {
        let parsed = reqwest::Url::parse(url)
            .map_err(|e| format!("`{url}` is not a URL ({e}), e.g. http://localhost:8080"))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(format!(
                "`{url}` is not an http(s) URL, e.g. http://localhost:8080"
            ));
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(format!(
                "`{url}` has a query or fragment; give the server's URL, with a path prefix at most"
            ));
        }
        if timeout.is_zero() {
            return Err("the upstream timeout is 0; give one such as 30s".into());
        }
        Ok(Self {
            url: url.trim_end_matches('/').to_owned(),
            timeout,
        })
    }

    /// The server's URL, as given (no trailing slash).
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Where `target` (a path and query) goes on the upstream.
    pub fn join(&self, target: &str) -> String {
        format!("{}{target}", self.url)
    }
}

/// An upstream and the client that talks to it.
pub(super) struct Forwarder {
    pub(super) upstream: Upstream,
    client: reqwest::Client,
}

impl Forwarder {
    pub(super) fn new(upstream: Upstream) -> std::io::Result<Self> {
        // reqwest is built without a default provider (no aws-lc).
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .no_proxy()
            // The client gets redirects as they are, as a browser would.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(upstream.timeout)
            .build()
            .map_err(std::io::Error::other)?;
        Ok(Self { upstream, client })
    }

    /// Sends `request` upstream and makes its answer, with `patch` merged
    /// into a JSON body. The text says what happened, for the journal
    /// (`upstream 200`, `upstream 200, patched`, `upstream error: …`).
    pub(super) async fn forward(
        &self,
        request: Forward<'_>,
        patch: Option<&Value>,
    ) -> (hyper::Response<Body>, String) {
        let url = self.upstream.join(request.target);
        let Ok(method) = reqwest::Method::from_bytes(request.method.as_bytes()) else {
            return self.failed(&format!("`{}` is not an HTTP method", request.method));
        };
        let mut sent = self.client.request(method, &url);
        for (name, value) in request.headers {
            let lower = name.to_ascii_lowercase();
            if HOP_BY_HOP.contains(&lower.as_str()) || request.connection.contains(&lower) {
                continue;
            }
            // A patch reads the body: ask for it plain.
            if patch.is_some() && lower == "accept-encoding" {
                continue;
            }
            sent = sent.header(name.as_str(), value.as_str());
        }
        if !request.body.is_empty() {
            sent = sent.body(request.body.clone());
        }
        let mut answer = match sent.send().await {
            Ok(answer) => answer,
            Err(e) => return self.failed(&error_text(&e)),
        };
        let status = answer.status().as_u16();
        let mut response = hyper::Response::builder().status(status);
        let connection = connection_names(
            answer
                .headers()
                .get_all(hyper::header::CONNECTION)
                .iter()
                .filter_map(|v| v.to_str().ok()),
        );
        let kept = |name: &str| {
            let lower = name.to_ascii_lowercase();
            !HOP_BY_HOP.contains(&lower.as_str()) && !connection.contains(&lower)
        };
        let Some(patch) = patch else {
            for (name, value) in answer.headers() {
                if kept(name.as_str()) {
                    response = response.header(name, value);
                }
            }
            // Streamed as it comes, so an upstream event stream stays one.
            let frames = futures_util::stream::unfold(Some(answer), |answer| async move {
                let mut answer = answer?;
                match answer.chunk().await {
                    Ok(Some(chunk)) => {
                        Some((Ok::<_, Infallible>(Frame::data(chunk)), Some(answer)))
                    }
                    _ => None,
                }
            });
            let body = StreamBody::new(frames).boxed();
            return (
                response.body(body).unwrap_or_else(|_| bad_headers()),
                format!("upstream {status}"),
            );
        };
        let mut bytes = Vec::new();
        loop {
            match answer.chunk().await {
                Ok(Some(chunk)) => bytes.extend_from_slice(&chunk),
                Ok(None) => break,
                Err(e) => return self.failed(&error_text(&e)),
            }
        }
        let (body, note) = if (300..400).contains(&status) {
            (bytes, "redirect; not patched")
        } else {
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(mut value) => {
                    mock::merge_patch(&mut value, patch);
                    (serde_json::to_vec(&value).unwrap_or_default(), "patched")
                }
                Err(_) => (bytes, "not JSON; not patched"),
            }
        };
        let patched = note == "patched";
        for (name, value) in answer.headers() {
            // The body is now plain and of another length.
            let encoding = name == hyper::header::CONTENT_ENCODING;
            if kept(name.as_str()) && !(patched && encoding) {
                response = response.header(name, value);
            }
        }
        (
            response
                .body(Full::new(Bytes::from(body)).boxed())
                .unwrap_or_else(|_| bad_headers()),
            format!("upstream {status}, {note}"),
        )
    }

    /// The 502 for an upstream that did not answer.
    fn failed(&self, error: &str) -> (hyper::Response<Body>, String) {
        let body = json!({
            "error": format!(
                "chrome-cdp mock serve: the upstream {} did not answer: {error}",
                self.upstream.url
            ),
            "upstream": self.upstream.url,
        });
        (
            super::plain(502, &body),
            format!("upstream error: {error}, 502"),
        )
    }
}

/// What a forwarded request is made of.
pub(super) struct Forward<'a> {
    pub(super) method: &'a str,
    /// The path and query.
    pub(super) target: &'a str,
    pub(super) headers: &'a [(String, String)],
    /// The names the request's `Connection` header lists (lower case):
    /// hop-by-hop too.
    pub(super) connection: Vec<String>,
    pub(super) body: &'a Bytes,
}

/// The header names `Connection` values list, lower case.
pub(super) fn connection_names<'a>(values: impl Iterator<Item = &'a str>) -> Vec<String> {
    values
        .flat_map(|v| v.split(','))
        .map(|n| n.trim().to_ascii_lowercase())
        .filter(|n| !n.is_empty())
        .collect()
}

/// A reqwest error in a few words: a timeout says so, a connection error
/// gives its cause.
fn error_text(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        return "timed out".into();
    }
    let mut text = if e.is_connect() {
        "cannot connect".to_owned()
    } else {
        "the request failed".to_owned()
    };
    let mut source = std::error::Error::source(e);
    let mut last = None;
    while let Some(s) = source {
        last = Some(s.to_string());
        source = s.source();
    }
    if let Some(cause) = last {
        text.push_str(" (");
        text.push_str(&cause);
        text.push(')');
    }
    text
}

fn bad_headers() -> hyper::Response<Body> {
    super::plain(
        502,
        &json!({"error": "chrome-cdp mock serve: the upstream's headers cannot be sent on"}),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn new_checks_the_url() {
        let up = Upstream::new("http://localhost:8080/v2/", UPSTREAM_TIMEOUT).unwrap();
        assert_eq!(up.url(), "http://localhost:8080/v2");
        assert_eq!(up.join("/api/x?a=1"), "http://localhost:8080/v2/api/x?a=1");
        let root = Upstream::new("https://api.example.com", UPSTREAM_TIMEOUT).unwrap();
        assert_eq!(root.join("/a"), "https://api.example.com/a");
        for bad in [
            "localhost:8080",
            "ftp://x",
            "http://x/?q=1",
            "/api",
            "http://x/#a",
        ] {
            assert!(Upstream::new(bad, UPSTREAM_TIMEOUT).is_err(), "{bad}");
        }
        assert!(Upstream::new("http://x", Duration::ZERO).is_err());
    }

    #[test]
    fn connection_lists_more_hop_by_hop_headers() {
        let names = connection_names(["close, X-Secret", " keep-alive"].into_iter());
        assert_eq!(names, ["close", "x-secret", "keep-alive"]);
    }
}
