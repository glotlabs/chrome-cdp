//! Cookies, storage and auth state (plan 07, milestone 7.4): `cookies`,
//! `storage`, `state save` and `state load` (and `open --state`).
//!
//! Cookies are the browser's (`Storage.getCookies`/`setCookies` on the
//! browser session), so they belong to every tab. Web Storage and
//! IndexedDB belong to an origin, so they are read and written by a script
//! ([`LIBRARY`]) in chrome-cdp's isolated world of a frame of that origin:
//! it shares the page's storage but none of its globals, so a page that
//! replaced `localStorage` cannot fool it.
//!
//! A state file is Playwright's `storageState` JSON ([`StorageState`]):
//! `cookies` and `origins` with their `localStorage` (and `indexedDB` with
//! `--indexeddb`, as Playwright's `storageState({ indexedDB: true })` has
//! it). chrome-cdp adds `sessionStorage` to each origin; Playwright
//! ignores it. `state save` reads every origin the context's frames
//! visited: in a frame of it where one is open, else in a hidden page (see
//! [`visited`]), as Playwright does. `state load` gives an origin with a frame open its storage at
//! once, and seeds the others: an init script for the origin's next
//! document, which takes itself out once it ran (see
//! [`crate::inject::Injector::seed`]).

pub(crate) mod commands;
mod visited;

use cdp::domains::network::{Cookie, CookieParam, CookiePartitionKey, CookieSameSite};
use protocol::schema::{CookieInfo, StorageItem};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The script behind `storage`, `state save` and `state load`: an
/// expression whose value has `collect`, `restore` and `seed`.
pub(crate) const LIBRARY: &str = include_str!("storage.js");

/// A storage state file: Playwright's `storageState` format, plus
/// `sessionStorage` per origin.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StorageState {
    #[serde(default)]
    pub cookies: Vec<StateCookie>,
    #[serde(default)]
    pub origins: Vec<OriginState>,
}

/// A cookie as Playwright writes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateCookie {
    pub name: String,
    pub value: String,
    /// `.example.com` for subdomains too; `example.com` for the host only.
    pub domain: String,
    #[serde(default = "root_path")]
    pub path: String,
    /// Seconds since 1970; -1 for a session cookie.
    #[serde(default = "session_expiry")]
    pub expires: f64,
    #[serde(default)]
    pub http_only: bool,
    #[serde(default)]
    pub secure: bool,
    /// `Strict`, `Lax` or `None`.
    #[serde(default = "lax")]
    pub same_site: String,
    /// The top-level site of a partitioned cookie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition_key: Option<String>,
}

/// One origin's storage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginState {
    /// `https://example.com` (scheme, host and port).
    pub origin: String,
    #[serde(default)]
    pub local_storage: Vec<StorageItem>,
    /// chrome-cdp's extension: the saving tab's `sessionStorage`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub session_storage: Vec<StorageItem>,
    /// Playwright's IndexedDB databases (`--indexeddb`), passed through to
    /// the page as they are.
    #[serde(default, rename = "indexedDB", skip_serializing_if = "Option::is_none")]
    pub indexed_db: Option<Vec<Value>>,
}

fn root_path() -> String {
    "/".to_owned()
}

fn session_expiry() -> f64 {
    -1.0
}

fn lax() -> String {
    "Lax".to_owned()
}

impl StorageState {
    /// Parses a state file's text; the error says what is wrong and
    /// where.
    pub fn parse(text: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if !value.is_object() {
            return Err("a storage state is a JSON object with `cookies` and `origins`".to_owned());
        }
        let state: Self = serde_json::from_value(value).map_err(|e| e.to_string())?;
        for origin in &state.origins {
            if origin_of(&origin.origin).as_deref() != Some(origin.origin.trim_end_matches('/')) {
                return Err(format!(
                    "`{}` is not an origin (scheme://host[:port])",
                    origin.origin
                ));
            }
        }
        Ok(state)
    }
}

/// The origin (`scheme://host[:port]`) of `url`, for http and https URLs;
/// `None` for others (`about:blank`, `data:`, …), which keep no storage of
/// their own.
pub(crate) fn origin_of(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

/// A cookie as chrome-cdp shows it.
pub(crate) fn cookie_info(cookie: &Cookie) -> CookieInfo {
    CookieInfo {
        name: cookie.name.clone(),
        value: cookie.value.clone(),
        domain: cookie.domain.clone(),
        path: cookie.path.clone(),
        expires: (!cookie.session && cookie.expires >= 0.0).then_some(cookie.expires),
        http_only: cookie.http_only,
        secure: cookie.secure,
        same_site: cookie.same_site.map(|s| same_site_name(s).to_owned()),
        partition_key: cookie
            .partition_key
            .as_ref()
            .map(|k| k.top_level_site.clone()),
    }
}

/// A cookie as a state file has it. Playwright writes `Lax` for a cookie
/// that did not say, since that is how Chrome treats it.
pub(crate) fn state_cookie(cookie: &Cookie) -> StateCookie {
    StateCookie {
        name: cookie.name.clone(),
        value: cookie.value.clone(),
        domain: cookie.domain.clone(),
        path: cookie.path.clone(),
        expires: if cookie.session || cookie.expires < 0.0 {
            -1.0
        } else {
            cookie.expires
        },
        http_only: cookie.http_only,
        secure: cookie.secure,
        same_site: cookie.same_site.map_or("Lax", same_site_name).to_owned(),
        partition_key: cookie
            .partition_key
            .as_ref()
            .map(|k| k.top_level_site.clone()),
    }
}

/// What `Storage.setCookies` takes for a state file's cookie. A domain
/// without a leading dot stays host-only.
pub(crate) fn cookie_param(cookie: &StateCookie) -> CookieParam {
    CookieParam {
        name: cookie.name.clone(),
        value: cookie.value.clone(),
        url: None,
        domain: Some(cookie.domain.clone()),
        path: Some(cookie.path.clone()),
        secure: Some(cookie.secure),
        http_only: Some(cookie.http_only),
        same_site: parse_same_site(&cookie.same_site),
        expires: (cookie.expires >= 0.0).then_some(cookie.expires),
        priority: None,
        source_scheme: None,
        source_port: None,
        partition_key: cookie
            .partition_key
            .as_ref()
            .map(|site| partition_key(site)),
    }
}

/// What `Storage.setCookies` takes to remove `cookie`: the same cookie,
/// expired.
pub(crate) fn expired(cookie: &Cookie) -> CookieParam {
    CookieParam {
        name: cookie.name.clone(),
        value: String::new(),
        url: None,
        domain: Some(cookie.domain.clone()),
        path: Some(cookie.path.clone()),
        secure: Some(cookie.secure),
        http_only: Some(cookie.http_only),
        same_site: cookie.same_site,
        expires: Some(1.0),
        priority: None,
        source_scheme: None,
        source_port: None,
        partition_key: cookie.partition_key.clone(),
    }
}

pub(crate) fn partition_key(site: &str) -> CookiePartitionKey {
    CookiePartitionKey {
        top_level_site: site.to_owned(),
        has_cross_site_ancestor: false,
    }
}

fn same_site_name(same_site: CookieSameSite) -> &'static str {
    match same_site {
        CookieSameSite::Strict => "Strict",
        CookieSameSite::None => "None",
        _ => "Lax",
    }
}

/// `Strict`, `Lax` or `None`, in any case.
pub(crate) fn parse_same_site(text: &str) -> Option<CookieSameSite> {
    match text.to_ascii_lowercase().as_str() {
        "strict" => Some(CookieSameSite::Strict),
        "lax" => Some(CookieSameSite::Lax),
        "none" => Some(CookieSameSite::None),
        _ => None,
    }
}

/// Whether `cookie` belongs to `domain` or one of its subdomains.
pub(crate) fn in_domain(cookie: &str, domain: &str) -> bool {
    let cookie = cookie.trim_start_matches('.').to_ascii_lowercase();
    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
    cookie == domain || cookie.ends_with(&format!(".{domain}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_are_scheme_host_and_port() {
        assert_eq!(
            origin_of("http://127.0.0.1:4000/a/b?c#d").as_deref(),
            Some("http://127.0.0.1:4000")
        );
        assert_eq!(
            origin_of("https://Example.com:443/").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(origin_of("about:blank"), None);
        assert_eq!(origin_of("data:text/html,x"), None);
    }

    #[test]
    fn playwright_files_parse() {
        let state = StorageState::parse(
            r#"{"cookies":[{"name":"a","value":"1","domain":"example.com","path":"/",
                "expires":-1,"httpOnly":true,"secure":false,"sameSite":"Lax"}],
               "origins":[{"origin":"https://example.com","localStorage":[{"name":"k","value":"v"}]}]}"#,
        )
        .unwrap();
        assert_eq!(state.cookies[0].name, "a");
        assert!(state.cookies[0].http_only);
        assert_eq!(state.origins[0].local_storage[0].value, "v");
        let param = cookie_param(&state.cookies[0]);
        assert_eq!(param.expires, None);
        assert_eq!(param.same_site, Some(CookieSameSite::Lax));
        let saved = serde_json::to_value(&state).unwrap();
        assert_eq!(saved["cookies"][0]["httpOnly"], true);
        assert_eq!(saved["origins"][0]["localStorage"][0]["name"], "k");
        assert!(saved["origins"][0].get("sessionStorage").is_none());
        assert!(saved["origins"][0].get("indexedDB").is_none());
    }

    #[test]
    fn bad_files_say_why() {
        assert!(StorageState::parse("[]").is_err());
        let err = StorageState::parse(r#"{"origins":[{"origin":"example.com"}]}"#).unwrap_err();
        assert!(err.contains("not an origin"), "{err}");
        assert!(StorageState::parse(r#"{"cookies":[{"name":"a"}]}"#).is_err());
        assert_eq!(StorageState::parse("{}").unwrap(), StorageState::default());
    }

    #[test]
    fn domains_match_their_subdomains() {
        assert!(in_domain(".example.com", "example.com"));
        assert!(in_domain("app.example.com", "example.com"));
        assert!(in_domain("example.com", ".example.com"));
        assert!(!in_domain("badexample.com", "example.com"));
        assert!(!in_domain("example.com", "app.example.com"));
    }
}
