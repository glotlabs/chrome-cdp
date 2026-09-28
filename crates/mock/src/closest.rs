//! How close a request comes to a `match` (plan 11, milestone 11.5): the
//! closest mocks of an unmatched request, and [`RequestFilter`], the
//! conditions of `expect request`, whose failure lists the requests sent
//! that come closest.
//!
//! Closeness weighs the path most (segment by segment, so `/api/cart` is
//! close to `/api/carts`), then the method, then the query, header and
//! body conditions of a request whose method and path match.

use protocol::schema::MockCheck;
use serde_json::Value;

use crate::glob::{Glob, PathGlob};
use crate::matcher::{BodyMatcher, Matcher, Parsed, Request, Target, check_body};

/// How close a path must come (0 to 100) for a mock or request to be
/// named as close: more than a shared first segment (`/api`).
pub(crate) const CLOSE_PATH: u32 = 60;

/// How many close mocks an unmatched request names.
pub(crate) const CLOSEST: usize = 2;

/// How close a request comes to a matcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Closeness {
    /// How alike the path (or URL) is, 0 to 100 (100: it matches).
    pub(crate) path: u32,
    /// Higher is closer; ranks candidates.
    pub(crate) score: u32,
}

impl Closeness {
    /// Close enough to be named.
    pub(crate) fn is_close(self) -> bool {
        self.path >= CLOSE_PATH
    }
}

impl Matcher {
    /// How close `req` comes to matching.
    pub(crate) fn closeness(&self, req: &Parsed) -> Closeness {
        let path = match &self.target {
            None => 100,
            Some(Target::Path(glob)) => glob_similarity(glob, &req.path),
            Some(Target::Url(glob)) => glob_similarity(glob, &req.base),
            Some(Target::Regex(regex)) => {
                if regex.is_match(&req.url) {
                    100
                } else {
                    0
                }
            }
        };
        let method = self.methods.is_empty() || self.methods.iter().any(|m| *m == req.method);
        // The same path is closer than a path a letter away, whatever the
        // method.
        let mut score = path * 4 + if path == 100 { 100 } else { 0 } + if method { 50 } else { 0 };
        if path == 100 && method {
            score += [
                self.check_query(req).is_ok(),
                self.check_headers(req).is_ok(),
                self.body
                    .as_ref()
                    .is_none_or(|b| check_body(b, req).is_ok()),
            ]
            .into_iter()
            .map(|ok| if ok { 30 } else { 0 })
            .sum::<u32>();
        }
        Closeness { path, score }
    }
}

/// How alike `subject` is to `glob` (100 when it matches).
pub(crate) fn glob_similarity(glob: &PathGlob, subject: &str) -> u32 {
    if glob.matches(subject) {
        100
    } else {
        path_similarity(&glob.to_string(), subject).min(99)
    }
}

/// How alike two paths (or URLs) are, 0 to 100, segment by segment: a
/// segment the pattern's matches (`:id`, a glob, the same text) counts
/// one, a segment a few letters away counts less, others nothing.
pub(crate) fn path_similarity(pattern: &str, subject: &str) -> u32 {
    let segments = |text: &str| -> Vec<String> {
        let text = text.split_once("://").map_or(text, |(_, rest)| rest);
        text.split('/')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let (want, got) = (segments(pattern), segments(subject));
    let n = want.len().max(got.len());
    if n == 0 {
        return 100;
    }
    let total: f64 = want
        .iter()
        .zip(&got)
        .map(|(w, g)| {
            if w == g || w.starts_with(':') || (w.contains('*') && Glob::new(w).matches(g)) {
                return 1.0;
            }
            let longest = w.chars().count().max(g.chars().count()).max(1);
            #[allow(clippy::cast_precision_loss)]
            let alike = 1.0 - strsim::levenshtein(w, g) as f64 / longest as f64;
            if alike >= 0.5 { alike } else { 0.0 }
        })
        .sum();
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let similarity = (total / n as f64 * 100.0).round() as u32;
    similarity.min(100)
}

/// The closest of `candidates` (each with its closeness), best first:
/// those close enough, at most `max`; ties keep their order.
pub(crate) fn closest(mut candidates: Vec<(Closeness, MockCheck)>, max: usize) -> Vec<MockCheck> {
    candidates.retain(|(c, _)| c.is_close());
    candidates.sort_by_key(|(c, _)| std::cmp::Reverse(c.score));
    candidates.into_iter().take(max).map(|(_, m)| m).collect()
}

/// The conditions of `expect request` on the requests a page sent: a
/// method, a path or URL glob, query parameters, headers and a JSON body,
/// with the same globs and JSON subset matching as a mock's `match`.
#[derive(Debug, Clone)]
pub struct RequestFilter {
    matcher: Matcher,
}

/// Whether a request passes a [`RequestFilter`], or why not, and how
/// close it comes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// The first condition that fails; `None` when it matches.
    pub reason: Option<String>,
    /// How alike its path is, 0 to 100.
    pub path: u32,
    /// Higher is closer.
    pub score: u32,
}

impl Verdict {
    pub fn matches(&self) -> bool {
        self.reason.is_none()
    }

    /// Close enough to be listed when nothing matches.
    pub fn is_close(&self) -> bool {
        self.path >= CLOSE_PATH
    }
}

impl RequestFilter {
    /// `method` is an HTTP method or `*` (any). `target` is a path glob
    /// (`/api/orders/*`, `**/orders`), a full-URL glob
    /// (`https://api.example.com/**`), or `*` for any URL; a `?query`
    /// after it adds query conditions. `query` values and `headers`
    /// values are globs; `json` must be contained in the JSON body.
    pub fn new(
        method: &str,
        target: &str,
        query: &[(String, String)],
        headers: &[(String, String)],
        json: Option<Value>,
    ) -> Result<Self, String> {
        let mut matcher = Matcher::default();
        let method = method.trim().to_ascii_uppercase();
        if method != "*" && method != "ANY" {
            if method.is_empty() || !method.chars().all(|c| c.is_ascii_alphabetic()) {
                return Err(format!("{method:?} is not an HTTP method (or * for any)"));
            }
            matcher.methods.push(method);
        }
        let (path, inline) = match target.split_once('?') {
            Some((path, query)) => (path, Some(query)),
            None => (target, None),
        };
        matcher.target = match path {
            "" | "*" | "**" | "/**" => None,
            p if p.contains("://") => Some(Target::Url(PathGlob::url(p)?)),
            p if p.starts_with('/') => Some(Target::Path(PathGlob::path(p)?)),
            p if p.starts_with('*') => Some(Target::Path(PathGlob::path(&format!("/{p}"))?)),
            p => {
                return Err(format!(
                    "{p:?} is no path: start it with / (a path, e.g. /api/orders), ** (any prefix) or a scheme (a full URL)"
                ));
            }
        };
        for (name, value) in inline
            .into_iter()
            .flat_map(|q| url::form_urlencoded::parse(q.as_bytes()))
        {
            matcher.query.push((name.into_owned(), Glob::new(&value)));
        }
        for (name, value) in query {
            matcher.query.push((name.clone(), Glob::new(value)));
        }
        for (name, value) in headers {
            matcher
                .headers
                .push((name.trim().to_ascii_lowercase(), Glob::new(value.trim())));
        }
        matcher.body = json.map(|value| BodyMatcher::Json {
            value,
            contains: false,
        });
        Ok(Self { matcher })
    }

    /// Whether a request's body is needed to check it.
    pub fn needs_body(&self) -> bool {
        self.matcher.body.is_some()
    }

    /// Whether it matches everything but the body: a request that does
    /// not needs no body read.
    pub fn matches_without_body(&self, req: &Request<'_>) -> bool {
        let parsed = Parsed::new(req);
        self.matcher.check(&parsed, &parsed.method, true).is_ok()
            && self.matcher.check_headers(&parsed).is_ok()
    }

    /// Checks `req`.
    pub fn check(&self, req: &Request<'_>) -> Verdict {
        let parsed = Parsed::new(req);
        let closeness = self.matcher.closeness(&parsed);
        Verdict {
            reason: self.matcher.check(&parsed, &parsed.method, false).err(),
            path: closeness.path,
            score: closeness.score,
        }
    }

    /// What it matches, in a few words: `POST /api/orders (json)`.
    pub fn summary(&self) -> String {
        self.matcher.summary()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{RequestFilter, path_similarity};
    use crate::matcher::Request;

    fn req<'a>(method: &'a str, url: &'a str, body: Option<&'a [u8]>) -> Request<'a> {
        Request {
            method,
            url,
            headers: &[],
            body,
        }
    }

    #[test]
    fn paths_alike_segment_by_segment() {
        assert_eq!(path_similarity("/api/carts", "/api/cart"), 90);
        assert_eq!(path_similarity("/api/products/:id", "/api/products/7"), 100);
        assert_eq!(path_similarity("/api/users", "/api/cart"), 50);
        assert_eq!(path_similarity("/api/cart", "/api/cart/items/3"), 50);
        assert_eq!(path_similarity("/login", "/api/cart"), 0);
        assert_eq!(
            path_similarity(
                "https://api.example.com/v1/cart",
                "https://api.example.com/v1/carts"
            ),
            93
        );
    }

    #[test]
    fn a_filter_checks_like_a_mock_match() {
        let filter = RequestFilter::new(
            "post",
            "/api/orders?source=web",
            &[("coupon".into(), "*".into())],
            &[],
            Some(json!({"qty": 2})),
        )
        .unwrap();
        assert!(filter.needs_body());
        let url = "http://localhost/api/orders?source=web&coupon=X";
        let ok = filter.check(&req("POST", url, Some(br#"{"qty":2,"sku":"a"}"#)));
        assert!(ok.matches(), "{ok:?}");
        let body = filter.check(&req("POST", url, Some(br#"{"qty":3}"#)));
        assert_eq!(body.reason.as_deref(), Some("json $.qty: expects 2, got 3"));
        let method = filter.check(&req("GET", url, None));
        assert_eq!(
            method.reason.as_deref(),
            Some("method differs: expects POST, got GET")
        );
        let path = filter.check(&req("POST", "http://localhost/api/order", None));
        assert!(path.reason.as_ref().unwrap().starts_with("path differs"));
        // A body mismatch is closer than a method mismatch, which is
        // closer than a path mismatch.
        assert!(body.score > method.score && method.score > path.score);
        assert!(path.is_close());
    }

    #[test]
    fn filter_targets() {
        let any = RequestFilter::new("*", "**/orders/*", &[], &[], None).unwrap();
        assert!(
            any.check(&req("DELETE", "https://x.test/v2/orders/9", None))
                .matches()
        );
        let url = RequestFilter::new("GET", "https://api.test/**", &[], &[], None).unwrap();
        assert!(
            url.check(&req("GET", "https://api.test/a/b?c=1", None))
                .matches()
        );
        assert!(!url.check(&req("GET", "https://web.test/a", None)).matches());
        assert!(RequestFilter::new("GET", "api/orders", &[], &[], None).is_err());
        assert!(RequestFilter::new("G T", "/a", &[], &[], None).is_err());
    }
}
