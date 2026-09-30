//! The `security` and `meta` categories of `audit`, from what was read of
//! the page (its document's facts, its response headers, its cookies):
//! pure functions, so they are tested without a browser. The engine reads
//! the page and names the forms' elements ([`super`]).

use std::fmt::Write as _;

use protocol::schema::{Finding, FindingCookie, FindingTarget};
use serde::Deserialize;

use super::rules;
use crate::input::origin;
use crate::observe::issues::key_url;
use crate::tabs::commands::path_of;

/// What `auditPage` (`injected/src/audit.ts`) read of a document.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct DocFacts {
    /// The `<title>`'s text; `None` without one.
    #[serde(default)]
    pub(crate) title: Option<String>,
    /// `<html lang>`; `None` without the attribute.
    #[serde(default)]
    pub(crate) lang: Option<String>,
    /// The contents of the meta tags named so.
    #[serde(default)]
    pub(crate) viewport: Option<String>,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) robots: Option<String>,
    /// A `<meta http-equiv=Content-Security-Policy>`.
    #[serde(default)]
    pub(crate) csp_meta: bool,
    /// A password field.
    #[serde(default)]
    pub(crate) password: bool,
    #[serde(default)]
    pub(crate) forms: Vec<FormFact>,
}

/// A form of the document.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct FormFact {
    pub(crate) candidate: Described,
    /// `get` or `post` (as written, lower case).
    pub(crate) method: String,
    /// Where it sends its data, resolved.
    pub(crate) action: String,
    /// Its place in `els`, for its ref.
    pub(crate) index: usize,
    /// Its ref, once given.
    #[serde(skip)]
    pub(crate) reference: Option<String>,
    /// Its stable locator, once found.
    #[serde(skip)]
    pub(crate) locator: Option<String>,
}

/// An element as `describe` (`injected/src/locator.ts`) names it.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Described {
    #[serde(default)]
    pub(crate) role: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) tag: String,
}

impl Described {
    /// `form "Checkout"`, or the tag for an element without a role.
    pub(crate) fn text(&self) -> String {
        let mut text = if self.role.is_empty() || self.role == "generic" {
            self.tag.clone()
        } else {
            self.role.clone()
        };
        if !self.name.is_empty() {
            let _ = write!(text, " \"{}\"", self.name.replace('"', "\\\""));
        }
        text
    }
}

/// A cookie the page's requests carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CookieFacts {
    pub(crate) name: String,
    pub(crate) domain: String,
    pub(crate) path: String,
    pub(crate) secure: bool,
    pub(crate) http_only: bool,
    /// `SameSite` was set (to anything).
    pub(crate) same_site: bool,
}

/// Names that look like a session's cookie: one without `HttpOnly` is a
/// session scripts (and injected ones) can steal.
const SESSION_WORDS: [&str; 4] = ["sid", "session", "token", "auth"];

/// A finding of `category`'s rule `rule`, with its severity from the
/// table, and its key `<category>/<rule>/<place>`.
pub(crate) fn finding(
    category: &str,
    rule: &str,
    message: String,
    target: FindingTarget,
    place: &str,
) -> Finding {
    Finding {
        category: category.to_owned(),
        rule: rule.to_owned(),
        severity: rules::severity(category, rule),
        message,
        target,
        key: format!("{category}/{rule}/{place}"),
        count: 1,
        docs: None,
        wcag: None,
        tab: None,
        accepted: false,
    }
}

/// The page's path, what a page-wide finding's key ends with.
pub(crate) fn page_place(url: &str) -> String {
    path_of(url.split(['?', '#']).next().unwrap_or_default()).to_owned()
}

/// The host of `url`, without its port or brackets.
fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    if let Some(inner) = host.strip_prefix('[') {
        return inner.split(']').next().unwrap_or_default();
    }
    host.split(':').next().unwrap_or_default()
}

/// Whether `url` is on this machine: `localhost`, `*.localhost`,
/// `127.x.x.x` or `::1`, which browsers treat as secure over http: too.
pub(crate) fn is_local(url: &str) -> bool {
    let host = host_of(url).to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host == "::1"
        || host.starts_with("127.") && host.split('.').count() == 4
}

fn scheme_of(url: &str) -> &str {
    url.split_once(':').map_or("", |(scheme, _)| scheme)
}

/// The `security` findings: the page's scheme, its CSP and HSTS
/// (`headers`: the document's response headers, `None` if chrome-cdp did
/// not see the response), its cookies, and its forms. `page_only`: the
/// checks of the page as a whole run (not with a LOCATOR, unless asked
/// for).
pub(crate) fn security(
    url: &str,
    doc: &DocFacts,
    headers: Option<&[(String, String)]>,
    cookies: &[CookieFacts],
    page_only: bool,
) -> Vec<Finding> {
    let mut out = Vec::new();
    let place = page_place(url);
    let scheme = scheme_of(url);
    let local = is_local(url);
    if scheme == "http" && !local {
        if doc.password {
            out.push(finding(
                "security",
                "http-password",
                "a password field on an http: page: the password crosses the network in clear"
                    .to_owned(),
                FindingTarget::default(),
                &place,
            ));
        } else if page_only {
            out.push(finding(
                "security",
                "http-page",
                "the page is served over http:, not https:".to_owned(),
                FindingTarget::default(),
                &place,
            ));
        }
    }
    for form in &doc.forms {
        if scheme_of(&form.action) != "http" || is_local(&form.action) {
            continue;
        }
        let element = form.candidate.text();
        let shown = match &form.reference {
            Some(reference) => format!("{element} [{reference}]"),
            None => element.clone(),
        };
        let action = form.action.split(['?', '#']).next().unwrap_or_default();
        let target = FindingTarget {
            reference: form.reference.clone(),
            element: Some(element.clone()),
            locator: form.locator.clone(),
            url: Some(action.to_owned()),
            ..FindingTarget::default()
        };
        let where_ = form.locator.clone().unwrap_or(element);
        out.push(finding(
            "security",
            "form-http-action",
            format!(
                "{shown} sends its data to {action} over http: ({})",
                form.method.to_uppercase()
            ),
            target,
            &where_,
        ));
    }
    if !page_only {
        return out;
    }
    let header = |name: &str| {
        headers.and_then(|headers| {
            headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        })
    };
    if headers.is_some()
        && header("content-security-policy").is_none()
        && !doc.csp_meta
        && matches!(scheme, "http" | "https")
    {
        out.push(finding(
            "security",
            "no-csp",
            "the document has no Content-Security-Policy".to_owned(),
            FindingTarget::default(),
            &place,
        ));
    }
    if headers.is_some()
        && scheme == "https"
        && !local
        && header("strict-transport-security").is_none()
    {
        out.push(finding(
            "security",
            "no-hsts",
            "the https: page has no Strict-Transport-Security header".to_owned(),
            FindingTarget::default(),
            &place,
        ));
    }
    for cookie in cookies {
        let shown = format!("cookie {:?} ({})", cookie.name, cookie.domain);
        let target = FindingTarget {
            cookie: Some(FindingCookie {
                name: cookie.name.clone(),
                domain: cookie.domain.clone(),
                path: cookie.path.clone(),
            }),
            ..FindingTarget::default()
        };
        let where_ = format!("cookie {}@{}", cookie.name, cookie.domain);
        let name = cookie.name.to_ascii_lowercase();
        if scheme == "https" && !cookie.secure {
            out.push(finding(
                "security",
                "cookie-no-secure",
                format!("{shown} has no Secure: it may be sent over http:"),
                target.clone(),
                &where_,
            ));
        }
        if !cookie.http_only && SESSION_WORDS.iter().any(|w| name.contains(w)) {
            out.push(finding(
                "security",
                "cookie-no-httponly",
                format!("{shown} looks like a session, and scripts can read it (no HttpOnly)"),
                target.clone(),
                &where_,
            ));
        }
        if !cookie.same_site {
            out.push(finding(
                "security",
                "cookie-no-samesite",
                format!("{shown} has no SameSite"),
                target,
                &where_,
            ));
        }
    }
    out
}

/// The `meta` findings of the document.
pub(crate) fn meta(url: &str, doc: &DocFacts) -> Vec<Finding> {
    let mut out = Vec::new();
    let place = page_place(url);
    let mut add = |rule: &str, message: &str| {
        out.push(finding(
            "meta",
            rule,
            message.to_owned(),
            FindingTarget::default(),
            &place,
        ));
    };
    if doc.title.as_deref().is_none_or(str::is_empty) {
        add("no-title", "the document has no <title>");
    }
    if doc.lang.as_deref().is_none_or(str::is_empty) {
        add("no-lang", "<html> has no lang attribute");
    }
    match doc.viewport.as_deref() {
        None => add("no-viewport", "there is no <meta name=viewport>"),
        Some(content) => {
            if let Some(why) = blocks_zoom(content) {
                add(
                    "viewport-blocks-zoom",
                    &format!("the viewport meta tag blocks zooming ({why})"),
                );
            }
        }
    }
    if doc
        .description
        .as_deref()
        .is_none_or(|d| d.trim().is_empty())
    {
        add("no-description", "there is no <meta name=description>");
    }
    if doc.robots.as_deref().is_some_and(|r| {
        r.to_ascii_lowercase()
            .split([',', ' '])
            .any(|w| w == "noindex" || w == "none")
    }) {
        add(
            "noindex",
            "a robots meta tag keeps search engines out (noindex)",
        );
    }
    out
}

/// Why a viewport's `content` blocks zooming, if it does:
/// `user-scalable=no` (or 0), or a `maximum-scale` under 2.
fn blocks_zoom(content: &str) -> Option<String> {
    for part in content.split([',', ';']) {
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        let (name, value) = (
            name.trim().to_ascii_lowercase(),
            value.trim().to_ascii_lowercase(),
        );
        match name.as_str() {
            "user-scalable" if matches!(value.as_str(), "no" | "0") => {
                return Some(format!("user-scalable={value}"));
            }
            "maximum-scale" if value.parse::<f64>().is_ok_and(|v| v < 2.0) => {
                return Some(format!("maximum-scale={value}"));
            }
            _ => {}
        }
    }
    None
}

/// `url` for a key (see [`key_url`]), for requests and scripts.
pub(crate) fn url_place(url: &str, page_url: &str) -> String {
    key_url(url, origin(page_url))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|f| f.key.as_str()).collect()
    }

    #[test]
    fn local_hosts() {
        for url in [
            "http://localhost:3000/",
            "http://app.localhost/x",
            "http://127.0.0.1:8080/a",
            "http://[::1]:9/",
        ] {
            assert!(is_local(url), "{url}");
        }
        for url in [
            "http://shop.test/",
            "https://127.0.0.1.example.com/",
            "http://10.0.0.1/",
        ] {
            assert!(!is_local(url), "{url}");
        }
    }

    #[test]
    fn meta_rules() {
        let doc = DocFacts {
            title: Some(String::new()),
            viewport: Some("width=device-width, maximum-scale=1".into()),
            robots: Some("noindex, nofollow".into()),
            ..DocFacts::default()
        };
        let found = meta("http://x.test:81/shop/cart?x=1", &doc);
        assert_eq!(
            keys(&found),
            [
                "meta/no-title//shop/cart",
                "meta/no-lang//shop/cart",
                "meta/viewport-blocks-zoom//shop/cart",
                "meta/no-description//shop/cart",
                "meta/noindex//shop/cart",
            ]
        );
        assert!(found[2].message.contains("maximum-scale=1"));
        let clean = DocFacts {
            title: Some("Cart".into()),
            lang: Some("en".into()),
            viewport: Some("width=device-width, initial-scale=1".into()),
            description: Some("Your cart".into()),
            robots: Some("index".into()),
            ..DocFacts::default()
        };
        assert!(meta("http://x.test/", &clean).is_empty());
        assert_eq!(
            blocks_zoom("user-scalable=no"),
            Some("user-scalable=no".into())
        );
        assert_eq!(blocks_zoom("maximum-scale=5"), None);
    }

    #[test]
    fn security_rules() {
        let doc = DocFacts {
            password: true,
            forms: vec![FormFact {
                candidate: Described {
                    role: "form".into(),
                    name: "Login".into(),
                    tag: "form".into(),
                },
                method: "post".into(),
                action: "http://api.test:8080/login?next=1".into(),
                index: 0,
                reference: Some("e3".into()),
                locator: Some("role=form[name=\"Login\"]".into()),
            }],
            ..DocFacts::default()
        };
        let headers = vec![("Content-Type".to_owned(), "text/html".to_owned())];
        let cookies = vec![CookieFacts {
            name: "session_id".into(),
            domain: "shop.test".into(),
            path: "/".into(),
            secure: false,
            http_only: false,
            same_site: false,
        }];
        let found = security(
            "http://shop.test:81/login",
            &doc,
            Some(&headers),
            &cookies,
            true,
        );
        assert_eq!(
            keys(&found),
            [
                "security/http-password//login",
                "security/form-http-action/role=form[name=\"Login\"]",
                "security/no-csp//login",
                "security/cookie-no-httponly/cookie session_id@shop.test",
                "security/cookie-no-samesite/cookie session_id@shop.test",
            ]
        );
        assert!(found[1].message.contains("form \"Login\" [e3]"));
        assert!(
            found[1]
                .message
                .contains("http://api.test:8080/login over http:")
        );
        // https: no HSTS, a cookie without Secure; a CSP in a meta tag.
        let doc = DocFacts {
            csp_meta: true,
            ..DocFacts::default()
        };
        let found = security("https://shop.test/", &doc, Some(&headers), &cookies, true);
        assert_eq!(
            keys(&found),
            [
                "security/no-hsts//",
                "security/cookie-no-secure/cookie session_id@shop.test",
                "security/cookie-no-httponly/cookie session_id@shop.test",
                "security/cookie-no-samesite/cookie session_id@shop.test",
            ]
        );
        // On this machine, http: is fine; no response seen, no header
        // checks; a scoped audit only checks the forms.
        let found = security("http://127.0.0.1:9/", &DocFacts::default(), None, &[], true);
        assert!(found.is_empty());
        let found = security(
            "http://shop.test/",
            &DocFacts::default(),
            Some(&headers),
            &cookies,
            false,
        );
        assert!(found.is_empty());
    }
}
