//! `open <url>`: load a URL in the current page and wait for its load
//! event.
//!
//! This is the smallest navigation milestone 2.4 needs. Plan 03's `goto`
//! replaces it with lifecycle-aware waits, `base_url` and redirects.

use std::time::Duration;

use cdp::Session;
use cdp::domains::{page, runtime};
use protocol::schema::PageInfo;
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, format};
use serde_json::{Value, json};

use crate::errors::from_cdp;

/// Adds a scheme to a URL typed without one: `http://` for local hosts,
/// else `https://`. `about:`, `data:`, `file:` and the like pass as is.
pub fn normalize_url(url: &str) -> Result<String, ErrorInfo> {
    let url = url.trim();
    if url.is_empty() {
        return Err(ErrorInfo::new(ErrorKind::BadArgs, "the URL is empty")
            .hint(Hint::run(Cmd::new("open").arg("https://example.com"))));
    }
    if url.contains("://")
        || ["about:", "data:", "file:", "javascript:", "chrome:"]
            .iter()
            .any(|s| url.starts_with(s))
    {
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

/// Loads `url` in `page` and waits (up to `timeout`) for its load event.
pub(crate) async fn open_url(
    page: &Session,
    url: &str,
    timeout: Duration,
    session: &str,
) -> Result<PageInfo, ErrorInfo> {
    let page = page.with_timeout(timeout);
    let cdp_err = |e: cdp::CdpError| from_cdp(&e, session);
    let mut loaded = page.listen::<page::LoadEventFired>();
    page.call(page::EnableParams::default())
        .await
        .map_err(cdp_err)?;
    let nav = page
        .call(page::NavigateParams::new(url.to_owned()))
        .await
        .map_err(cdp_err)?;
    if let Some(error) = nav.error_text.filter(|e| !e.is_empty()) {
        return Err(ErrorInfo::new(
            ErrorKind::NavigationFailed,
            format!("cannot open {url}: {error}"),
        )
        .hint(Hint::text(
            "check the URL and that the server is up; the session stays open",
        ))
        .details(json!({ "url": url, "error": error })));
    }
    // A same-document navigation (`#anchor`) has no loader and no load event.
    if nav.loader_id.is_some() && tokio::time::timeout(timeout, loaded.next()).await.is_err() {
        let observed = describe(&page).await.ok();
        let mut info = ErrorInfo::new(
            ErrorKind::Timeout,
            format!(
                "{url} did not finish loading within {}",
                format::duration(timeout)
            ),
        )
        .hint(format!(
            "retry with a longer timeout, e.g. --timeout {}",
            format::duration(timeout * 2)
        ));
        if let Some(observed) = observed {
            info = info
                .note("page", format!("{} {:?}", observed.url, observed.title))
                .details(json!({ "url": observed.url, "title": observed.title }));
        }
        return Err(info);
    }
    describe(&page).await.map_err(cdp_err)
}

/// The page's URL and title.
async fn describe(page: &Session) -> cdp::Result<PageInfo> {
    let mut params = runtime::EvaluateParams::new("[location.href, document.title]".into());
    params.return_by_value = Some(true);
    let result = page.call(params).await?.result.value.unwrap_or(Value::Null);
    let field = |i: usize| result[i].as_str().unwrap_or_default().to_owned();
    Ok(PageInfo {
        url: field(0),
        title: field(1),
    })
}

#[cfg(test)]
mod tests {
    use super::normalize_url;

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
}
