//! What `goto`, `open` and `tab new` make of a typed URL: a scheme is
//! added when there is none, and a relative one is resolved against
//! `base_url` or the page's current URL.

use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};
use serde_json::json;
use url::Url;

use crate::context::Scoped as _;

/// Schemes (and scheme-like prefixes) that are complete without `//`.
const OPAQUE: &[&str] = &[
    "about:",
    "data:",
    "file:",
    "javascript:",
    "chrome:",
    "blob:",
];

/// Adds a scheme to a URL typed without one: `http://` for local hosts,
/// else `https://`. `about:`, `data:`, `file:` and the like pass as is,
/// and so does a relative URL (`/login`, `./next`, `?q=1`, `#top`), which
/// [`resolve`] completes once the page is known.
pub fn normalize_url(url: &str) -> Result<String, ErrorInfo> {
    let url = url.trim();
    if url.is_empty() {
        return Err(ErrorInfo::new(ErrorKind::BadArgs, "the URL is empty")
            .hint(Hint::run(Cmd::new("goto").arg("https://example.com"))));
    }
    if url.contains("://") || OPAQUE.iter().any(|s| url.starts_with(s)) || is_relative(url) {
        return Ok(url.to_owned());
    }
    let host = url.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit_once(':').map_or(host, |(h, port)| {
        if port.bytes().all(|b| b.is_ascii_digit()) {
            h
        } else {
            host
        }
    });
    let local = matches!(host, "localhost" | "[::1]")
        || host.ends_with(".localhost")
        || host.starts_with("127.")
        || host == "0.0.0.0";
    Ok(format!("{}://{url}", if local { "http" } else { "https" }))
}

/// Whether `url` is relative to the page (or `base_url`): a path, a query
/// or a fragment.
pub(crate) fn is_relative(url: &str) -> bool {
    url.starts_with(['/', '?', '#']) || url.starts_with("./") || url.starts_with("../")
}

/// The URL to load for `input` (already [`normalize_url`]d): a relative
/// one is resolved against `base_url` when set, else against `current`,
/// the page's URL.
pub(crate) fn resolve(
    input: &str,
    base_url: Option<&str>,
    current: &str,
    session: &str,
) -> Result<String, ErrorInfo> {
    if !is_relative(input) {
        return Ok(input.to_owned());
    }
    let (base, from) = match base_url {
        Some(base) => (base, "base_url"),
        None => (current, "the current page"),
    };
    let resolved = Url::parse(base)
        .ok()
        .filter(|base| !base.cannot_be_a_base())
        .and_then(|base| base.join(input).ok());
    match resolved {
        Some(url) => Ok(url.into()),
        None => Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot resolve the relative URL `{input}` against {from} ({base})"),
        )
        .hint(
            Hint::run(
                Cmd::new("goto")
                    .arg(format!("http://localhost:3000{}", example_path(input)))
                    .scoped(session),
            )
            .why("a full URL")
            .or("set base_url in .chrome-cdp.toml"),
        )
        .details(json!({ "url": input, "base": base }))),
    }
}

/// `input` as a path for the hint's example URL.
fn example_path(input: &str) -> String {
    let trimmed = input.trim_start_matches("./").trim_start_matches("../");
    if trimmed.starts_with(['/', '?', '#']) {
        trimmed.to_owned()
    } else {
        format!("/{trimmed}")
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_url, resolve};

    #[test]
    fn urls_get_a_scheme() {
        let n = |u: &str| normalize_url(u).unwrap();
        assert_eq!(n("https://example.com"), "https://example.com");
        assert_eq!(n("example.com/a?b"), "https://example.com/a?b");
        assert_eq!(n("localhost:8080/x"), "http://localhost:8080/x");
        assert_eq!(n("127.0.0.1:3000"), "http://127.0.0.1:3000");
        assert_eq!(n("app.localhost"), "http://app.localhost");
        assert_eq!(n("about:blank"), "about:blank");
        assert_eq!(n("data:text/html,hi"), "data:text/html,hi");
        assert_eq!(n("file:///tmp/a.html"), "file:///tmp/a.html");
        assert!(normalize_url(" ").is_err());
    }

    #[test]
    fn relative_urls_stay_relative_until_resolved() {
        for url in ["/login", "./next", "../up", "?q=1", "#top", "//cdn.test/x"] {
            assert_eq!(normalize_url(url).unwrap(), url);
        }
    }

    #[test]
    fn relative_urls_resolve_against_base_url_then_the_page() {
        let r = |input: &str, base: Option<&str>, current: &str| {
            resolve(input, base, current, "default").unwrap()
        };
        let page = "http://127.0.0.1:3000/app/list?page=2#x";
        assert_eq!(r("/login", None, page), "http://127.0.0.1:3000/login");
        assert_eq!(r("./item", None, page), "http://127.0.0.1:3000/app/item");
        assert_eq!(
            r("?page=3", None, page),
            "http://127.0.0.1:3000/app/list?page=3"
        );
        assert_eq!(
            r("#top", None, page),
            "http://127.0.0.1:3000/app/list?page=2#top"
        );
        assert_eq!(
            r("/login", Some("https://staging.test/"), page),
            "https://staging.test/login"
        );
        assert_eq!(
            r("./b.html", None, "file:///tmp/a.html"),
            "file:///tmp/b.html"
        );
        assert_eq!(r("https://x.test/", None, "about:blank"), "https://x.test/");
    }

    #[test]
    fn a_relative_url_on_a_blank_page_needs_a_base_url() {
        let err = resolve("/login", None, "about:blank", "default").unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert_eq!(
            err.to_text(),
            "error: cannot resolve the relative URL `/login` against the current page (about:blank)\n  \
             hint: chrome-cdp goto http://localhost:3000/login   (a full URL), or set base_url in .chrome-cdp.toml\n"
        );
    }
}
