//! `connect <endpoint>`: from what the user typed to a DevTools WebSocket
//! URL.
//!
//! Accepted: `9222` (a port on 127.0.0.1), `host:port`, `http://host:port`
//! (both ask `/json/version` for the URL) and `ws://…` as is. `wss://` and
//! `https://` need TLS, which the WebSocket transport is built without.

use std::time::Duration;

use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};
use serde_json::Value;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

/// Where the DevTools URL comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// A `ws://` URL, used as is.
    WebSocket(String),
    /// `host:port` of Chrome's HTTP endpoint; `/json/version` names the
    /// WebSocket URL.
    Http(String),
}

/// Parses what the user typed. Fails with `BadArgs` for anything else.
pub fn parse_endpoint(input: &str) -> Result<Endpoint, ErrorInfo> {
    let input = input.trim();
    let bad = |why: &str| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("`{input}` is not a DevTools endpoint: {why}"),
        )
        .hint(
            Hint::run(Cmd::new("connect").arg("9222"))
                .why("Chrome started with --remote-debugging-port=9222")
                .or(format!(
                    "{} with the URL from chrome://inspect or /json/version",
                    Cmd::new("connect").raw("ws://127.0.0.1:9222/devtools/browser/<id>")
                )),
        )
    };
    if let Some(rest) = input.strip_prefix("ws://") {
        if rest.is_empty() {
            return Err(bad("no host"));
        }
        return Ok(Endpoint::WebSocket(input.to_owned()));
    }
    if input.starts_with("wss://") || input.starts_with("https://") {
        return Err(bad("TLS (wss://, https://) is not supported yet"));
    }
    let authority = input.strip_prefix("http://").unwrap_or(input);
    let authority = authority.trim_end_matches('/');
    if authority.bytes().all(|b| b.is_ascii_digit()) {
        return port(authority)
            .map(|p| Endpoint::Http(format!("127.0.0.1:{p}")))
            .ok_or_else(|| bad("not a port number"));
    }
    match authority.rsplit_once(':') {
        Some((host, p)) if !host.is_empty() && !host.contains('/') && port(p).is_some() => {
            Ok(Endpoint::Http(authority.to_owned()))
        }
        _ => Err(bad("use a port, host:port, http://host:port or ws://…")),
    }
}

/// The warning for a browser that is not on this machine, or `None` when
/// `url` (a `ws://` URL or `host:port`) is on a loopback address. The
/// DevTools protocol has neither authentication nor encryption, so whoever
/// can see or reach that address can read and control the browser.
pub(crate) fn remote_warning(url: &str) -> Option<String> {
    let host = host(url);
    if is_loopback(host) {
        return None;
    }
    Some(format!(
        "{host} is not this machine: the DevTools protocol is unencrypted and \
         unauthenticated, so anyone on the network between can read and control \
         the browser"
    ))
}

/// The host of a `ws://` or `http://` URL or a `host:port`, without
/// brackets around an IPv6 address.
fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    if let Some(v6) = authority.strip_prefix('[') {
        return v6.split(']').next().unwrap_or_default();
    }
    authority.split(':').next().unwrap_or_default()
}

fn is_loopback(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn port(text: &str) -> Option<u16> {
    text.parse().ok().filter(|&p| p != 0)
}

/// The WebSocket URL to connect to.
pub(crate) async fn websocket_url(
    endpoint: &Endpoint,
    timeout: Duration,
) -> Result<String, ErrorInfo> {
    let authority = match endpoint {
        Endpoint::WebSocket(url) => return Ok(url.clone()),
        Endpoint::Http(authority) => authority,
    };
    let failed = |why: String| {
        ErrorInfo::new(
            ErrorKind::LaunchFailed,
            format!("cannot reach a browser at http://{authority}: {why}"),
        )
        .hint(Hint::text(format!(
            "start Chrome with --remote-debugging-port={}, or check the address",
            authority.rsplit_once(':').map_or("9222", |(_, p)| p)
        )))
    };
    let body = tokio::time::timeout(timeout, get(authority, "/json/version"))
        .await
        .map_err(|_| failed("no answer".into()))?
        .map_err(failed)?;
    let version: Value =
        serde_json::from_str(&body).map_err(|e| failed(format!("bad /json/version: {e}")))?;
    let url = version["webSocketDebuggerUrl"]
        .as_str()
        .ok_or_else(|| failed("/json/version has no webSocketDebuggerUrl".into()))?;
    Ok(url.to_owned())
}

/// A plain HTTP/1.1 GET; returns the body of a 200 response.
async fn get(authority: &str, path: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect(authority)
        .await
        .map_err(|e| e.to_string())?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = stream.read(&mut chunk).await.map_err(|e| e.to_string())?;
        buf.extend_from_slice(&chunk[..n]);
        if n == 0 || complete(&buf) {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let (head, body) = text.split_once("\r\n\r\n").ok_or("not an HTTP response")?;
    let status = head.lines().next().unwrap_or_default();
    if status.split_whitespace().nth(1) != Some("200") {
        return Err(format!("{path} answered `{status}`"));
    }
    Ok(body.to_owned())
}

/// Whether `buf` holds the whole response, going by `Content-Length`.
fn complete(buf: &[u8]) -> bool {
    let text = String::from_utf8_lossy(buf);
    let Some((head, body)) = text.split_once("\r\n\r\n") else {
        return false;
    };
    head.lines()
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .is_some_and(|len| body.len() >= len)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::{Endpoint, parse_endpoint, remote_warning, websocket_url};

    #[test]
    fn endpoints() {
        let p = |s: &str| parse_endpoint(s).unwrap();
        assert_eq!(p("9222"), Endpoint::Http("127.0.0.1:9222".into()));
        assert_eq!(p("localhost:9333"), Endpoint::Http("localhost:9333".into()));
        assert_eq!(
            p("http://10.0.0.2:9222/"),
            Endpoint::Http("10.0.0.2:9222".into())
        );
        let ws = "ws://127.0.0.1:9222/devtools/browser/abc";
        assert_eq!(p(ws), Endpoint::WebSocket(ws.into()));
        for bad in [
            "",
            "0",
            "99999",
            "wss://x/y",
            "https://x:1",
            "chrome",
            "ws://",
            ":9",
        ] {
            let err = parse_endpoint(bad).unwrap_err();
            assert_eq!(err.exit_code(), 2, "{bad}");
        }
        assert!(
            parse_endpoint("wss://x")
                .unwrap_err()
                .message
                .ends_with("TLS (wss://, https://) is not supported yet")
        );
    }

    #[test]
    fn only_remote_browsers_warn() {
        for local in [
            "ws://127.0.0.1:9222/devtools/browser/abc",
            "ws://127.8.0.1:9222/x",
            "ws://localhost:9222/x",
            "ws://LOCALHOST./x",
            "ws://app.localhost:9222",
            "ws://[::1]:9222/devtools/browser/abc",
            "ws://user@127.0.0.1:9222/x",
            "127.0.0.1:9222",
            "localhost:9333",
        ] {
            assert_eq!(remote_warning(local), None, "{local}");
        }
        for (remote, host) in [
            ("ws://10.0.0.2:9222/devtools/browser/abc", "10.0.0.2"),
            ("ws://chrome.example:9222", "chrome.example"),
            ("ws://[fe80::1]:9222/x", "fe80::1"),
            ("ws://127.0.0.1@evil.example:9222/x", "evil.example"),
            ("ws://localhost.evil.example/x", "localhost.evil.example"),
            ("0.0.0.0:9222", "0.0.0.0"),
            ("10.0.0.2:9222", "10.0.0.2"),
        ] {
            let warning = remote_warning(remote).expect(remote);
            assert!(
                warning.starts_with(&format!("{host} is not this machine")),
                "{warning}"
            );
        }
    }

    #[tokio::test]
    async fn json_version_names_the_websocket() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut req = [0u8; 1024];
            let n = sock.read(&mut req).await.unwrap();
            assert!(String::from_utf8_lossy(&req[..n]).starts_with("GET /json/version HTTP/1.1"));
            let body = r#"{"webSocketDebuggerUrl":"ws://x/devtools/browser/1"}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            // Keep the connection open: the length ends the read.
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let url = websocket_url(&Endpoint::Http(addr.to_string()), Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(url, "ws://x/devtools/browser/1");

        let err = websocket_url(
            &Endpoint::Http("127.0.0.1:1".into()),
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
        assert!(
            err.message
                .starts_with("cannot reach a browser at http://127.0.0.1:1: "),
            "{err}"
        );
    }
}
