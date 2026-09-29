//! Navigations Chrome could not complete (`Page.navigate`'s `errorText`,
//! or the error page a reload or history step ends on): one
//! `NavigationFailed` error per kind of failure, each with its own
//! `details.reason` and hint (plan 03, milestone 3.3).

use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};
use serde_json::json;
use url::Url;

use crate::context::Scoped as _;

/// Why a navigation failed, from Chrome's `net::ERR_*` code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reason {
    Dns,
    ConnectionRefused,
    ConnectionFailed,
    TimedOut,
    Offline,
    Certificate,
    Tls,
    TooManyRedirects,
    UnsafePort,
    Blocked,
    FileNotFound,
    InvalidUrl,
    BadResponse,
    Auth,
    Download,
    Aborted,
    Other,
}

impl Reason {
    /// Classifies `net::ERR_CONNECTION_REFUSED` (or just
    /// `ERR_CONNECTION_REFUSED`).
    pub(crate) fn of(error: &str, is_download: bool) -> Self {
        if is_download {
            return Self::Download;
        }
        let code = error.trim().trim_start_matches("net::");
        match code {
            "ERR_NAME_NOT_RESOLVED" | "ERR_NAME_RESOLUTION_FAILED" => Self::Dns,
            "ERR_CONNECTION_REFUSED" => Self::ConnectionRefused,
            "ERR_CONNECTION_RESET"
            | "ERR_CONNECTION_CLOSED"
            | "ERR_CONNECTION_ABORTED"
            | "ERR_CONNECTION_FAILED"
            | "ERR_EMPTY_RESPONSE"
            | "ERR_SOCKET_NOT_CONNECTED" => Self::ConnectionFailed,
            "ERR_CONNECTION_TIMED_OUT" | "ERR_TIMED_OUT" => Self::TimedOut,
            "ERR_INTERNET_DISCONNECTED"
            | "ERR_NETWORK_CHANGED"
            | "ERR_ADDRESS_UNREACHABLE"
            | "ERR_PROXY_CONNECTION_FAILED"
            | "ERR_TUNNEL_CONNECTION_FAILED" => Self::Offline,
            "ERR_TOO_MANY_REDIRECTS" => Self::TooManyRedirects,
            "ERR_UNSAFE_PORT" => Self::UnsafePort,
            "ERR_BLOCKED_BY_CLIENT"
            | "ERR_BLOCKED_BY_RESPONSE"
            | "ERR_BLOCKED_BY_ADMINISTRATOR"
            | "ERR_BLOCKED_BY_CSP"
            | "ERR_BLOCKED_BY_ORB"
            | "ERR_UNSAFE_REDIRECT"
            | "ERR_ACCESS_DENIED" => Self::Blocked,
            "ERR_FILE_NOT_FOUND" => Self::FileNotFound,
            "ERR_INVALID_URL" | "ERR_UNKNOWN_URL_SCHEME" | "ERR_DISALLOWED_URL_SCHEME" => {
                Self::InvalidUrl
            }
            "ERR_INVALID_RESPONSE"
            | "ERR_INVALID_HTTP_RESPONSE"
            | "ERR_CONTENT_DECODING_FAILED"
            | "ERR_CONTENT_LENGTH_MISMATCH"
            | "ERR_INCOMPLETE_CHUNKED_ENCODING"
            | "ERR_INVALID_CHUNKED_ENCODING"
            | "ERR_HTTP2_PROTOCOL_ERROR"
            | "ERR_RESPONSE_HEADERS_TOO_BIG"
            | "ERR_RESPONSE_HEADERS_MULTIPLE_LOCATION"
            | "ERR_RESPONSE_HEADERS_MULTIPLE_CONTENT_LENGTH"
            | "ERR_RESPONSE_HEADERS_MULTIPLE_CONTENT_DISPOSITION" => Self::BadResponse,
            "ERR_INVALID_AUTH_CREDENTIALS" | "ERR_UNEXPECTED_PROXY_AUTH" => Self::Auth,
            "ERR_ABORTED" => Self::Aborted,
            _ if code.starts_with("ERR_CERT_") => Self::Certificate,
            _ if code.starts_with("ERR_SSL_") || code == "ERR_BAD_SSL_CLIENT_AUTH_CERT" => {
                Self::Tls
            }
            _ => Self::Other,
        }
    }

    /// `details.reason`.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Dns => "dns",
            Self::ConnectionRefused => "connection_refused",
            Self::ConnectionFailed => "connection_failed",
            Self::TimedOut => "timed_out",
            Self::Offline => "offline",
            Self::Certificate => "certificate",
            Self::Tls => "tls",
            Self::TooManyRedirects => "too_many_redirects",
            Self::UnsafePort => "unsafe_port",
            Self::Blocked => "blocked",
            Self::FileNotFound => "file_not_found",
            Self::InvalidUrl => "invalid_url",
            Self::BadResponse => "bad_response",
            Self::Auth => "auth",
            Self::Download => "download",
            Self::Aborted => "aborted",
            Self::Other => "other",
        }
    }
}

/// The error for a navigation to `url` that failed with Chrome's `error`
/// (`net::ERR_*`). Hints name `session`.
pub(crate) fn navigation_failed(
    url: &str,
    error: &str,
    is_download: bool,
    session: &str,
) -> ErrorInfo {
    let reason = Reason::of(error, is_download);
    let parsed = Url::parse(url).ok();
    let host = parsed
        .as_ref()
        .and_then(Url::host_str)
        .unwrap_or(url)
        .to_owned();
    let host_port = match parsed.as_ref().and_then(Url::port_or_known_default) {
        Some(port) => format!("{host}:{port}"),
        None => host.clone(),
    };
    let retry = Cmd::new("goto").arg(url).scoped(session);
    let (message, hint) = match reason {
        Reason::Dns => (
            format!("cannot resolve the host {host}"),
            Hint::text("check the host name in the URL (a typo, or a VPN-only host?)"),
        ),
        Reason::ConnectionRefused => (
            format!("connection refused on {host_port}"),
            Hint::text(format!(
                "is your dev server running? start it, then: {retry}"
            )),
        ),
        Reason::ConnectionFailed => (
            format!("the connection to {host_port} broke ({error})"),
            Hint::text(format!(
                "the server closed the connection; check its logs, then: {retry}"
            )),
        ),
        Reason::TimedOut => (
            format!("connecting to {host_port} timed out"),
            Hint::text("check that the host is reachable from this machine (firewall, VPN)"),
        ),
        Reason::Offline => (
            format!("the network is unreachable ({error})"),
            Hint::text("check this machine's network connection and proxy settings"),
        ),
        Reason::Certificate => (
            format!("the certificate of {host} is not trusted ({error})"),
            Hint::run(Cmd::new("open").switch("ignore-https-errors").session(session))
                .why("for test servers; then goto again"),
        ),
        Reason::Tls => {
            let http = parsed
                .as_ref()
                .filter(|u| u.scheme() == "https")
                .map(|_| url.replacen("https://", "http://", 1));
            let hint = match http {
                Some(http) => Hint::run(Cmd::new("goto").arg(http).scoped(session))
                    .why("if the server speaks plain HTTP"),
                None => Hint::text("check that the server speaks HTTPS on this port"),
            };
            (
                format!("the TLS handshake with {host_port} failed ({error})"),
                hint,
            )
        }
        Reason::TooManyRedirects => (
            format!("{url} redirects in a loop"),
            Hint::text(
                "check the server's redirects (a login loop often means a missing cookie)",
            ),
        ),
        Reason::UnsafePort => (
            format!(
                "Chrome refuses to load port {}",
                parsed
                    .as_ref()
                    .and_then(Url::port_or_known_default)
                    .map_or_else(|| "?".to_owned(), |p| p.to_string())
            ),
            Hint::text("serve the page on another port (Chrome blocks ports of well-known services)"),
        ),
        Reason::Blocked => (
            format!("Chrome blocked {url} ({error})"),
            Hint::text(
                "a policy, header (CSP, X-Frame-Options, Cross-Origin-*) or extension blocked it; check the server's headers",
            ),
        ),
        Reason::FileNotFound => (
            format!(
                "no such file: {}",
                parsed
                    .as_ref()
                    .and_then(|u| u.to_file_path().ok())
                    .map_or_else(|| url.to_owned(), |p| p.display().to_string())
            ),
            Hint::text("check the path; file URLs need an absolute one, e.g. file:///home/me/page.html"),
        ),
        Reason::InvalidUrl => (
            format!("Chrome cannot open {url} ({error})"),
            Hint::run(Cmd::new("goto").arg("https://example.com").scoped(session))
                .why("URLs need a scheme Chrome knows"),
        ),
        Reason::BadResponse => (
            format!("{host_port} sent an invalid response ({error})"),
            Hint::text("the server's response is malformed; check its logs"),
        ),
        Reason::Auth => (
            format!(
                "{} asks for a user name and password (HTTP authentication)",
                parsed
                    .as_ref()
                    .map_or_else(|| host_port.clone(), |u| u.origin().ascii_serialization())
            ),
            Hint::run(
                Cmd::new("open")
                    .flag("http-credentials", "USER:PASSWORD")
                    .session(session),
            )
            .why("then goto again; if the session has some, the server refused them"),
        ),
        Reason::Download => (
            format!("{url} is a download, not a page, and the browser did not save it"),
            Hint::text(format!("fetch the file directly, e.g. curl -O {url}")),
        ),
        Reason::Aborted => (
            format!("the navigation to {url} was cancelled ({error})"),
            Hint::run(Cmd::new("tabs").scoped(session)).why(
                "the server sent no page (204?), or something else navigated the tab; look at where it is",
            ),
        ),
        Reason::Other => (
            format!("cannot open {url}: {error}"),
            Hint::text(format!(
                "check the URL and that the server is up; the session stays open. Retry: {retry}"
            )),
        ),
    };
    ErrorInfo::new(ErrorKind::NavigationFailed, message)
        .hint(hint)
        .details(json!({ "url": url, "reason": reason.as_str(), "error": error }))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use protocol::ErrorKind;

    use super::{Reason, navigation_failed};

    /// One Chrome error per reason, with the URL it fails on.
    const CASES: &[(&str, &str, &str)] = &[
        ("dns", "net::ERR_NAME_NOT_RESOLVED", "https://nope.invalid/"),
        (
            "connection_refused",
            "net::ERR_CONNECTION_REFUSED",
            "http://localhost:3000/",
        ),
        (
            "connection_failed",
            "net::ERR_EMPTY_RESPONSE",
            "http://localhost:3000/",
        ),
        (
            "timed_out",
            "net::ERR_CONNECTION_TIMED_OUT",
            "https://10.0.0.1/",
        ),
        (
            "offline",
            "net::ERR_INTERNET_DISCONNECTED",
            "https://example.com/",
        ),
        (
            "certificate",
            "net::ERR_CERT_AUTHORITY_INVALID",
            "https://127.0.0.1:8443/",
        ),
        (
            "tls",
            "net::ERR_SSL_PROTOCOL_ERROR",
            "https://localhost:3000/login",
        ),
        (
            "too_many_redirects",
            "net::ERR_TOO_MANY_REDIRECTS",
            "https://example.com/loop",
        ),
        (
            "unsafe_port",
            "net::ERR_UNSAFE_PORT",
            "http://127.0.0.1:6000/",
        ),
        (
            "blocked",
            "net::ERR_BLOCKED_BY_CLIENT",
            "https://ads.example.com/",
        ),
        (
            "file_not_found",
            "net::ERR_FILE_NOT_FOUND",
            "file:///tmp/missing.html",
        ),
        ("invalid_url", "net::ERR_UNKNOWN_URL_SCHEME", "foo://bar"),
        (
            "bad_response",
            "net::ERR_INVALID_HTTP_RESPONSE",
            "http://localhost:3000/",
        ),
        ("aborted", "net::ERR_ABORTED", "http://localhost:3000/empty"),
        (
            "auth",
            "net::ERR_INVALID_AUTH_CREDENTIALS",
            "http://localhost:3000/admin",
        ),
        ("other", "net::ERR_SOMETHING_NEW", "https://example.com/"),
    ];

    #[test]
    fn every_reason_has_its_own_hint() {
        let mut reasons = HashSet::new();
        let mut hints = HashSet::new();
        for (reason, error, url) in CASES {
            let info = navigation_failed(url, error, false, "default");
            let details = info.details.clone().unwrap();
            assert_eq!(info.kind, ErrorKind::NavigationFailed);
            assert_eq!(info.exit_code(), 1);
            assert_eq!(details["reason"], *reason, "{error}");
            assert_eq!(details["error"], *error);
            assert_eq!(details["url"], *url);
            assert!(reasons.insert(*reason), "{reason} twice");
            assert!(
                hints.insert(info.hint.clone().unwrap()),
                "{reason}: same hint"
            );
        }
        let download =
            navigation_failed("https://x.test/a.pdf", "net::ERR_ABORTED", true, "default");
        assert_eq!(download.details.unwrap()["reason"], "download");
        assert!(hints.insert(download.hint.unwrap()));
    }

    #[test]
    fn codes_without_the_net_prefix() {
        assert_eq!(
            Reason::of("ERR_CONNECTION_REFUSED", false),
            Reason::ConnectionRefused
        );
        assert_eq!(
            Reason::of("net::ERR_CERT_DATE_INVALID", false),
            Reason::Certificate
        );
        assert_eq!(
            Reason::of("net::ERR_SSL_VERSION_OR_CIPHER_MISMATCH", false),
            Reason::Tls
        );
    }

    #[test]
    fn hints_name_the_next_command() {
        let text = |url: &str, error: &str, session: &str| {
            navigation_failed(url, error, false, session).to_text()
        };
        assert_eq!(
            text(
                "http://localhost:3000/",
                "net::ERR_CONNECTION_REFUSED",
                "default"
            ),
            "error: connection refused on localhost:3000\n  \
             hint: is your dev server running? start it, then: chrome-cdp goto http://localhost:3000/\n"
        );
        assert_eq!(
            text(
                "https://127.0.0.1:8443/",
                "net::ERR_CERT_AUTHORITY_INVALID",
                "shop"
            ),
            "error: the certificate of 127.0.0.1 is not trusted (net::ERR_CERT_AUTHORITY_INVALID)\n  \
             hint: chrome-cdp open --ignore-https-errors --session shop   (for test servers; then goto again)\n"
        );
        assert_eq!(
            text(
                "https://localhost:3000/login",
                "net::ERR_SSL_PROTOCOL_ERROR",
                "default"
            ),
            "error: the TLS handshake with localhost:3000 failed (net::ERR_SSL_PROTOCOL_ERROR)\n  \
             hint: chrome-cdp goto http://localhost:3000/login   (if the server speaks plain HTTP)\n"
        );
        assert_eq!(
            text(
                "https://nope.invalid/",
                "net::ERR_NAME_NOT_RESOLVED",
                "default"
            ),
            "error: cannot resolve the host nope.invalid\n  \
             hint: check the host name in the URL (a typo, or a VPN-only host?)\n"
        );
    }
}
