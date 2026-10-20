//! Endpoints, not URLs (plan 23.4): a load run's requests are grouped by
//! method and a path pattern, so `GET /api/products/17` and `GET
//! /api/products/18` are one endpoint, `GET /api/products/:id`.
//!
//! The automatic pattern drops the query string and makes each numeric,
//! UUID or long hex segment `:id`. The patterns of `[load] endpoints`
//! (`GET /api/search?q=*`, `/blog/:slug`) win over it: a `:name` segment
//! is any one segment, `*` any text (a lone last `*`, any rest of the
//! path), and `?q=*` asks for a `q` parameter. Static files (scripts,
//! stylesheets, images, fonts, media) group by type, `static js` and so
//! on, unless `group_assets = false`. A request to another origin than the
//! backend's keeps its host: `GET cdn.example.com/lib/:id`.

use url::Url;

/// The group of the requests above [`MOST_GROUPS`].
pub(crate) const OTHER: &str = "other";

/// How many endpoint groups a run keeps; later ones go into [`OTHER`].
pub(crate) const MOST_GROUPS: usize = 500;

/// How long a hex segment must be to be an id.
const LONG_HEX: usize = 16;

/// Groups requests.
#[derive(Debug, Clone)]
pub(crate) struct Grouper {
    patterns: Vec<Pattern>,
    assets: bool,
    /// The backend's origin (`http://localhost:4174`).
    origin: Option<String>,
}

/// A pattern of `[load] endpoints`.
#[derive(Debug, Clone)]
struct Pattern {
    written: String,
    method: Option<String>,
    path: Vec<String>,
    /// `name=value` pairs of the query (`*` for any value).
    query: Vec<(String, String)>,
}

impl Grouper {
    /// A grouper with `[load] endpoints` `patterns` (checked by the
    /// config), static files by type unless not `assets`, for a backend at
    /// `base_url`.
    pub(crate) fn new(patterns: &[String], assets: bool, base_url: Option<&str>) -> Self {
        Self {
            patterns: patterns.iter().map(|p| Pattern::parse(p)).collect(),
            assets,
            origin: base_url
                .and_then(|u| Url::parse(u).ok())
                .map(|u| u.origin().ascii_serialization()),
        }
    }

    /// The group of a `method` request of resource type `kind` to `url`;
    /// `None` for what is no request to a server (`data:`, `blob:`).
    pub(crate) fn group(&self, method: &str, url: &str, kind: &str) -> Option<String> {
        let url = Url::parse(url).ok()?;
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        let own = self
            .origin
            .as_ref()
            .is_none_or(|o| *o == url.origin().ascii_serialization());
        if own && let Some(pattern) = self.patterns.iter().find(|p| p.matches(method, &url)) {
            return Some(match &pattern.method {
                Some(_) => pattern.written.clone(),
                None => format!("{method} {}", pattern.written),
            });
        }
        if self.assets
            && let Some(kind) = asset(kind)
        {
            return Some(format!("static {kind}"));
        }
        let mut path: String = url
            .path_segments()
            .into_iter()
            .flatten()
            .map(|s| if is_id(s) { ":id" } else { s })
            .fold(String::new(), |mut path, s| {
                path.push('/');
                path.push_str(s);
                path
            });
        if path.is_empty() {
            path.push('/');
        }
        Some(if own {
            format!("{method} {path}")
        } else {
            let host = url.host_str().unwrap_or_default();
            match url.port() {
                Some(port) => format!("{method} {host}:{port}{path}"),
                None => format!("{method} {host}{path}"),
            }
        })
    }

    /// The pattern a request that went into [`OTHER`] most likely misses:
    /// its group with the last literal segment as `:name`.
    pub(crate) fn likely_pattern(group: &str) -> String {
        let Some(slash) = group.rfind('/') else {
            return group.to_owned();
        };
        let last = &group[slash + 1..];
        if last.is_empty() || last.starts_with(':') {
            return group.to_owned();
        }
        format!("{}:name", &group[..=slash])
    }
}

impl Pattern {
    fn parse(written: &str) -> Self {
        let written = written.trim();
        let (method, rest) = match written.split_once(' ') {
            Some((m, rest)) if m.bytes().all(|b| b.is_ascii_uppercase()) => {
                (Some(m.to_owned()), rest.trim())
            }
            _ => (None, written),
        };
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        Self {
            written: written.to_owned(),
            method,
            path: segments(path).map(str::to_owned).collect(),
            query: query
                .split('&')
                .filter(|p| !p.is_empty())
                .map(|p| {
                    let (name, value) = p.split_once('=').unwrap_or((p, "*"));
                    (name.to_owned(), value.to_owned())
                })
                .collect(),
        }
    }

    fn matches(&self, method: &str, url: &Url) -> bool {
        if self.method.as_deref().is_some_and(|m| m != method) {
            return false;
        }
        let path: Vec<&str> = segments(url.path()).collect();
        if !segments_match(&self.path, &path) {
            return false;
        }
        self.query.iter().all(|(name, value)| {
            url.query_pairs()
                .any(|(n, v)| n == name.as_str() && glob(value, &v))
        })
    }
}

/// A path's segments, without the empty ones around slashes.
fn segments(path: &str) -> impl Iterator<Item = &str> {
    path.split('/').filter(|s| !s.is_empty())
}

fn segments_match(pattern: &[String], path: &[&str]) -> bool {
    match (pattern.split_first(), path.split_first()) {
        (None, None) => true,
        (Some((last, [])), _) if last == "*" => true,
        (Some((p, rest)), Some((s, path_rest))) => {
            let one = if p.starts_with(':') && p.len() > 1 {
                true
            } else {
                glob(p, s)
            };
            one && segments_match(rest, path_rest)
        }
        _ => false,
    }
}

/// Whether `text` matches `pattern`, whose `*` is any text.
fn glob(pattern: &str, text: &str) -> bool {
    let Some((first, rest)) = pattern.split_once('*') else {
        return pattern == text;
    };
    let Some(mut text) = text.strip_prefix(first) else {
        return false;
    };
    let mut parts: Vec<&str> = rest.split('*').collect();
    let last = parts.pop().unwrap_or_default();
    for part in parts {
        match text.find(part) {
            Some(at) => text = &text[at + part.len()..],
            None => return false,
        }
    }
    text.len() >= last.len() && text.ends_with(last)
}

/// A static file's type, by Chrome's resource type.
fn asset(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "script" => "js",
        "stylesheet" => "css",
        "image" => "img",
        "font" => "font",
        "media" => "media",
        _ => return None,
    })
}

/// Whether a path segment is an id: a number, a UUID, or long hex.
fn is_id(segment: &str) -> bool {
    if !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    let hex = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit());
    let parts: Vec<&str> = segment.split('-').collect();
    if parts.iter().map(|p| p.len()).eq([8, 4, 4, 4, 12]) && parts.iter().all(|p| hex(p)) {
        return true;
    }
    segment.len() >= LONG_HEX && hex(segment) && segment.bytes().any(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::Grouper;

    fn grouper(patterns: &[&str]) -> Grouper {
        let patterns: Vec<String> = patterns.iter().map(|p| (*p).to_owned()).collect();
        Grouper::new(&patterns, true, Some("http://localhost:4174/"))
    }

    fn group(g: &Grouper, method: &str, url: &str, kind: &str) -> String {
        g.group(method, url, kind).unwrap()
    }

    #[test]
    fn ids_become_id_and_queries_go() {
        let g = grouper(&[]);
        let at = |path: &str| group(&g, "GET", &format!("http://localhost:4174{path}"), "fetch");
        assert_eq!(at("/api/products/17"), "GET /api/products/:id");
        assert_eq!(at("/api/products/17?view=full"), "GET /api/products/:id");
        assert_eq!(
            at("/api/orders/3f2c1a9e-7b4d-4e2a-9c1b-0d5e6f7a8b9c/items"),
            "GET /api/orders/:id/items"
        );
        assert_eq!(at("/api/blobs/5f8d0c2a9b7e4d1f3a6c"), "GET /api/blobs/:id");
        // Words are not ids, nor are short hex ones.
        assert_eq!(at("/api/products/deadbeef"), "GET /api/products/deadbeef");
        assert_eq!(at("/api/cart"), "GET /api/cart");
        assert_eq!(at("/"), "GET /");
        assert_eq!(at(""), "GET /");
        assert_eq!(
            group(&g, "POST", "http://localhost:4174/api/cart", "fetch"),
            "POST /api/cart"
        );
    }

    #[test]
    fn static_files_group_by_type_unless_told() {
        let g = grouper(&[]);
        let url = "http://localhost:4174/assets/index-3f2a.js";
        assert_eq!(group(&g, "GET", url, "script"), "static js");
        assert_eq!(
            group(&g, "GET", "http://x.test/a.css", "stylesheet"),
            "static css"
        );
        assert_eq!(
            group(&g, "GET", "http://localhost:4174/p.png", "image"),
            "static img"
        );
        assert_eq!(
            group(&g, "GET", "http://localhost:4174/", "document"),
            "GET /"
        );
        let by_path = Grouper::new(&[], false, Some("http://localhost:4174"));
        assert_eq!(
            group(&by_path, "GET", url, "script"),
            "GET /assets/index-3f2a.js"
        );
    }

    #[test]
    fn other_origins_keep_their_host() {
        let g = grouper(&[]);
        assert_eq!(
            group(&g, "GET", "https://api.example.com/v1/users/12", "fetch"),
            "GET api.example.com/v1/users/:id"
        );
        assert_eq!(
            group(&g, "GET", "http://localhost:9000/x", "fetch"),
            "GET localhost:9000/x"
        );
        assert!(
            g.group("GET", "data:image/png;base64,AAAA", "image")
                .is_none()
        );
        assert!(
            g.group("GET", "blob:http://localhost:4174/1", "fetch")
                .is_none()
        );
    }

    #[test]
    fn written_patterns_win() {
        let g = grouper(&[
            "GET /api/search?q=*",
            "/blog/:slug",
            "/files/*",
            "GET /api/*.json",
        ]);
        let at = |method: &str, path: &str| {
            group(&g, method, &format!("http://localhost:4174{path}"), "fetch")
        };
        assert_eq!(
            at("GET", "/api/search?q=shoes&page=2"),
            "GET /api/search?q=*"
        );
        // No q: not that endpoint.
        assert_eq!(at("GET", "/api/search?page=2"), "GET /api/search");
        assert_eq!(at("POST", "/api/search?q=x"), "POST /api/search");
        assert_eq!(at("GET", "/blog/hello-world"), "GET /blog/:slug");
        assert_eq!(at("PUT", "/blog/hello-world"), "PUT /blog/:slug");
        assert_eq!(at("GET", "/blog/a/comments"), "GET /blog/a/comments");
        assert_eq!(at("GET", "/files/a/b/c.txt"), "GET /files/*");
        assert_eq!(at("GET", "/api/config.json"), "GET /api/*.json");
        // A pattern beats static grouping too.
        assert_eq!(
            group(&g, "GET", "http://localhost:4174/files/app.js", "script"),
            "GET /files/*"
        );
    }

    #[test]
    fn the_likely_missing_pattern() {
        assert_eq!(
            Grouper::likely_pattern("GET /api/items/blue-shoes"),
            "GET /api/items/:name"
        );
        assert_eq!(Grouper::likely_pattern("GET /api/:id"), "GET /api/:id");
    }
}
