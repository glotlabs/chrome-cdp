//! A mock's `respond`, compiled, and the answer it gives a request.

use std::fmt::Write as _;
use std::path::{Component, Path};
use std::time::Duration;

use protocol::format;
use serde_json::Value;

use crate::template::{self, is_template};

/// The largest data file a mock serves.
pub const MAX_FILE: u64 = 50 * 1024 * 1024;

/// Chrome's network errors (`Network.ErrorReason`), as `abort` and
/// `route --abort` take them (any case, `-` and `_` ignored).
pub const NETWORK_ERRORS: &[&str] = &[
    "Failed",
    "Aborted",
    "TimedOut",
    "AccessDenied",
    "ConnectionClosed",
    "ConnectionReset",
    "ConnectionRefused",
    "ConnectionAborted",
    "ConnectionFailed",
    "NameNotResolved",
    "InternetDisconnected",
    "AddressUnreachable",
    "BlockedByClient",
    "BlockedByResponse",
];

/// The network error `text` names (`connection-reset` → `ConnectionReset`).
pub fn network_error(text: &str) -> Option<&'static str> {
    let key: String = text
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | ' '))
        .collect();
    NETWORK_ERRORS
        .iter()
        .find(|r| r.eq_ignore_ascii_case(&key))
        .copied()
}

/// The `Content-Type` of a file, by its extension.
pub fn content_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "json" => "application/json",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "txt" | "text" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Whether `name` may be an HTTP header name.
pub fn is_header_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// What a mock does with a request.
#[derive(Debug, Clone)]
pub enum Response {
    /// Answer it.
    Fulfill {
        status: u16,
        /// With `Content-Type` when there is a body.
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        /// Add CORS headers for a cross-origin request (the set's `cors`).
        cors: bool,
    },
    /// Fail it with network error `reason` (one of [`NETWORK_ERRORS`]).
    Abort { reason: &'static str },
    /// Send it to the real server, then deep-merge `patch` into the JSON
    /// response, if any.
    Passthrough { patch: Option<Value> },
}

impl Response {
    /// A JSON answer.
    pub(crate) fn json(status: u16, value: &Value, cors: bool) -> Self {
        Self::Fulfill {
            status,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: serde_json::to_vec(value).unwrap_or_default(),
            cors,
        }
    }

    /// What the journal says: `fulfill 200`, `abort connectionreset`,
    /// `passthrough`.
    pub fn outcome(&self) -> String {
        match self {
            Self::Fulfill { status, .. } => format!("fulfill {status}"),
            Self::Abort { reason } => format!("abort {}", reason.to_ascii_lowercase()),
            Self::Passthrough { patch: None } => "passthrough".into(),
            Self::Passthrough { patch: Some(_) } => "passthrough, patch json".into(),
        }
    }
}

/// Where a body comes from.
#[derive(Debug, Clone)]
pub(crate) enum Body {
    Empty,
    Json(Value),
    JsonFile(String),
    Text(String),
    TextFile(String),
    File(String),
}

/// A mock's `respond`, checked.
#[derive(Debug, Clone)]
pub(crate) enum Respond {
    Fulfill {
        status: u16,
        headers: Vec<(String, String)>,
        body: Body,
    },
    Abort(&'static str),
    Passthrough(Option<Value>),
}

/// A mock's answer, rendered for one request.
#[derive(Debug)]
pub(crate) struct Rendered {
    pub(crate) response: Response,
    /// Worth saying in the journal: `no data/products/9.json`.
    pub(crate) note: Option<String>,
}

impl Respond {
    /// The answer to a request whose template context is `ctx`; data files
    /// are in `base`. A template error is an `Err`.
    pub(crate) fn render(
        &self,
        ctx: &minijinja::Value,
        base: &Path,
        cors: bool,
    ) -> Result<Rendered, String> {
        let (status, headers, body) = match self {
            Self::Abort(reason) => {
                return Ok(Rendered {
                    response: Response::Abort { reason },
                    note: None,
                });
            }
            Self::Passthrough(patch) => {
                let patch = patch
                    .as_ref()
                    .map(|p| template::render_json(p, ctx))
                    .transpose()?;
                return Ok(Rendered {
                    response: Response::Passthrough { patch },
                    note: None,
                });
            }
            Self::Fulfill {
                status,
                headers,
                body,
            } => (*status, headers, body),
        };
        let mut headers: Vec<(String, String)> = headers
            .iter()
            .map(|(n, v)| Ok((n.clone(), template::render(v, ctx)?)))
            .collect::<Result<_, String>>()?;
        let (bytes, kind): (Vec<u8>, Option<&str>) = match body {
            Body::Empty => (Vec::new(), None),
            Body::Json(value) => (
                serde_json::to_vec(&template::render_json(value, ctx)?).unwrap_or_default(),
                Some("application/json"),
            ),
            Body::Text(text) => (
                template::render(text, ctx)?.into_bytes(),
                Some("text/plain; charset=utf-8"),
            ),
            Body::JsonFile(path) | Body::TextFile(path) | Body::File(path) => {
                let rel = data_path(path, ctx)?;
                let file = base.join(&rel);
                let bytes = match read(&file) {
                    Ok(bytes) => bytes,
                    Err(Missing::NotFound) => {
                        let body = serde_json::json!({ "error": format!("not found: {rel}") });
                        return Ok(Rendered {
                            response: Response::json(404, &body, cors),
                            note: Some(format!("no {rel}")),
                        });
                    }
                    Err(Missing::Unreadable(why)) => return Err(format!("{rel}: {why}")),
                };
                match body {
                    Body::JsonFile(_) => {
                        let bytes = if rel.ends_with(".tmpl.json") {
                            let value: Value = serde_json::from_slice(&bytes)
                                .map_err(|e| format!("{rel} is not JSON: {e}"))?;
                            serde_json::to_vec(&template::render_json(&value, ctx)?)
                                .unwrap_or_default()
                        } else {
                            bytes
                        };
                        (bytes, Some("application/json"))
                    }
                    Body::TextFile(_) if rel.contains(".tmpl.") => {
                        let text = String::from_utf8(bytes)
                            .map_err(|_| format!("{rel} is a template but not UTF-8 text"))?;
                        (
                            template::render(&text, ctx)?.into_bytes(),
                            Some(content_type(Path::new(&rel))),
                        )
                    }
                    _ => (bytes, Some(content_type(Path::new(&rel)))),
                }
            }
        };
        let typed = headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("Content-Type"));
        if let (Some(kind), false) = (kind, typed) {
            headers.push(("Content-Type".into(), kind.into()));
        }
        Ok(Rendered {
            response: Response::Fulfill {
                status,
                headers,
                body: bytes,
                cors,
            },
            note: None,
        })
    }

    pub(crate) fn is_passthrough(&self) -> bool {
        matches!(self, Self::Passthrough(_))
    }

    /// What it answers, in a few words: `200 json_file data/items.json`.
    /// With `ctx`, a templated file path is shown rendered.
    pub(crate) fn summary(&self, ctx: Option<&minijinja::Value>) -> String {
        match self {
            Self::Abort(reason) => format!("abort {}", reason.to_ascii_lowercase()),
            Self::Passthrough(None) => "passthrough".into(),
            Self::Passthrough(Some(patch)) => {
                format!("passthrough, patch {}", cut(&patch.to_string()))
            }
            Self::Fulfill { status, body, .. } => {
                let mut text = status.to_string();
                let path = |key: &str, path: &str| {
                    let shown = ctx
                        .and_then(|ctx| data_path(path, ctx).ok())
                        .unwrap_or_else(|| path.to_owned());
                    format!(" {key} {shown}")
                };
                let more = match body {
                    Body::Empty => String::new(),
                    Body::Json(value) => format!(" json {}", cut(&value.to_string())),
                    Body::Text(text) => format!(" body {}", cut(&format!("{text:?}"))),
                    Body::JsonFile(p) => path("json_file", p),
                    Body::TextFile(p) => path("body_file", p),
                    Body::File(p) => path("file", p),
                };
                text.push_str(&more);
                text
            }
        }
    }
}

fn cut(text: &str) -> String {
    let shown: String = text.chars().take(60).collect();
    if shown.len() < text.len() {
        format!("{shown}…")
    } else {
        shown
    }
}

/// A data file's path for this request. A path whose template put a `..`
/// in it is refused, so a request cannot reach files outside the mocks
/// directory.
fn data_path(path: &str, ctx: &minijinja::Value) -> Result<String, String> {
    if !is_template(path) {
        return Ok(path.to_owned());
    }
    let rendered = template::render(path, ctx)?;
    let before = Path::new(path)
        .components()
        .filter(|c| matches!(c, Component::ParentDir))
        .count();
    let after = Path::new(&rendered)
        .components()
        .filter(|c| matches!(c, Component::ParentDir | Component::RootDir))
        .count();
    if after > before {
        return Err(format!(
            "`{path}` became `{rendered}`, which leaves the mocks directory"
        ));
    }
    Ok(rendered)
}

enum Missing {
    NotFound,
    Unreadable(String),
}

fn read(file: &Path) -> Result<Vec<u8>, Missing> {
    let meta = std::fs::metadata(file).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Missing::NotFound,
        _ => Missing::Unreadable(e.to_string()),
    })?;
    if meta.is_dir() {
        return Err(Missing::NotFound);
    }
    if meta.len() > MAX_FILE {
        return Err(Missing::Unreadable(format!(
            "it is {}, over the {} limit",
            format::size(meta.len()),
            format::size(MAX_FILE)
        )));
    }
    std::fs::read(file).map_err(|e| Missing::Unreadable(e.to_string()))
}

/// A delay as written: `0ms`, `300ms`, `2s`.
pub(crate) fn parse_delay(text: &str) -> Result<Duration, String> {
    let t = text.trim();
    let num: String = t
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let zero = num.parse::<f64>().is_ok_and(|n| n == 0.0);
    if zero && matches!(&t[num.len()..], "" | "ms" | "s" | "m") {
        return Ok(Duration::ZERO);
    }
    format::parse_duration(t)
}

/// `[settings] latency`: fixed, or a seeded random range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Latency {
    Fixed(Duration),
    Range(Duration, Duration),
}

impl Latency {
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        match text.split_once("..") {
            None => parse_delay(text).map(Self::Fixed),
            Some((low, high)) => {
                let low = parse_delay(low)?;
                let high = parse_delay(high)?;
                if high < low {
                    return Err(format!("`{text}`: the range ends before it starts"));
                }
                Ok(Self::Range(low, high))
            }
        }
    }

    pub(crate) fn text(self) -> String {
        match self {
            Self::Fixed(d) if d.is_zero() => "0ms".into(),
            Self::Fixed(d) => format::duration(d),
            Self::Range(a, b) => {
                let mut text = String::new();
                let _ = write!(text, "{}..{}", show(a), show(b));
                text
            }
        }
    }

    /// A delay; `pick` gives a number in `0..n`.
    pub(crate) fn sample(self, pick: impl FnOnce(u64) -> u64) -> Duration {
        match self {
            Self::Fixed(d) => d,
            Self::Range(a, b) => {
                let span = u64::try_from(b.saturating_sub(a).as_millis()).unwrap_or(u64::MAX);
                a + Duration::from_millis(pick(span.saturating_add(1)))
            }
        }
    }
}

fn show(d: Duration) -> String {
    if d.is_zero() {
        "0ms".into()
    } else {
        format::duration(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_and_latency() {
        assert_eq!(parse_delay("0ms").unwrap(), Duration::ZERO);
        assert_eq!(parse_delay("0").unwrap(), Duration::ZERO);
        assert_eq!(parse_delay("250ms").unwrap(), Duration::from_millis(250));
        assert!(parse_delay("fast").is_err());
        assert!(parse_delay("100").is_err());
        let range = Latency::parse("50ms..200ms").unwrap();
        assert_eq!(
            range,
            Latency::Range(Duration::from_millis(50), Duration::from_millis(200))
        );
        assert_eq!(range.text(), "50ms..200ms");
        assert_eq!(range.sample(|n| n - 1), Duration::from_millis(200));
        assert_eq!(range.sample(|_| 0), Duration::from_millis(50));
        assert!(Latency::parse("2s..1s").is_err());
        assert_eq!(Latency::parse("0ms").unwrap().text(), "0ms");
    }

    #[test]
    fn network_errors_take_any_case() {
        assert_eq!(network_error("connectionreset"), Some("ConnectionReset"));
        assert_eq!(
            network_error("connection-refused"),
            Some("ConnectionRefused")
        );
        assert_eq!(network_error("TimedOut"), Some("TimedOut"));
        assert_eq!(network_error("nope"), None);
    }
}
