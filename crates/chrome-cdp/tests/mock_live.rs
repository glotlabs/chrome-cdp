//! Mocks beyond fetches, end to end (plan 11, milestone 11.9): the page's
//! WebSockets and EventSources answered by `[[websocket]]` and `sse`
//! mocks, `mock ws send`, and `mock serve`.
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::time::Duration;

use common::{Sandbox, chrome_path, cli, code, json_out, stderr, stdout};
use fixtures::Fixtures;

fn mocks_dir() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/site/mock/mocks")
        .canonicalize()
        .unwrap()
        .display()
        .to_string()
}

/// `window.seen` of the page once it has `n` entries.
fn seen(sandbox: &Sandbox, n: usize) -> Vec<String> {
    let out = sandbox.run(&[
        "wait",
        "--fn",
        &format!("window.seen && seen.length >= {n}"),
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["eval", "seen", "--json"]);
    serde_json::from_value(json_out(&out)["data"]["value"].clone()).unwrap()
}

#[test]
fn websocket_and_event_source_mocks() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let site = Fixtures::shared();
    let mocks = mocks_dir();
    let out = sandbox.run(&["open"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["mock", "use", "--dir", &mocks, "live"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["goto", &site.url("/mock/live.html")]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    // The socket opens with the page's protocol and gets on_open's
    // message; the EventSource gets both events, the second by its type.
    let mut got = seen(&sandbox, 4);
    got.sort();
    assert_eq!(
        got,
        [
            r#"headline {"title":"Second"} #2"#,
            "open v1",
            "sse first",
            // Keys come in order, as in a JSON body.
            r#"ws {"symbol":"ACME","type":"hello"}"#,
        ]
    );

    // The page's message gets its on_message answer.
    let out = sandbox.run(&["click", "#ping"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let got = seen(&sandbox, 5);
    assert_eq!(got[4], r#"ws {"id":7,"type":"pong"}"#);

    // A scenario pushes a message.
    let out = sandbox.run(&[
        "mock",
        "ws",
        "send",
        "/mock/ws/**",
        r#"{"type":"price","value":42}"#,
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).starts_with("sent 27B to 1 socket:\n  t1 ws://"),
        "{}",
        stdout(&out)
    );
    assert!(
        stdout(&out).contains(r#"(live.toml:6 "prices")"#),
        "{}",
        stdout(&out)
    );
    let out = sandbox.run(&["expect", "#price", "text", "42"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    // The journal has the socket and the stream, as mocked requests.
    let out = sandbox.run(&["mock", "log"]);
    let log = stdout(&out);
    assert!(
        log.contains(r#"websocket → live.toml:6 "prices" websocket open, sends 1 message"#),
        "{log}"
    );
    assert!(
        log.contains(r#"eventsource → live.toml:15 "news" fulfill 200 (2 events)"#),
        "{log}"
    );

    // A socket in the set's scope that no mock answers fails, and is
    // unmatched; one outside it is real (the fixture server echoes).
    let out = sandbox.run(&[
        "eval",
        "new Promise((done) => { const s = new WebSocket('/mock/ws/other'); \
         s.onerror = () => done('error ' + s.readyState); s.onopen = () => done('open'); })",
    ]);
    assert_eq!(stdout(&out), "error 3\n", "{}", stderr(&out));
    let out = sandbox.run(&["mock", "log", "--unmatched"]);
    assert!(
        stdout(&out).contains("/mock/ws/other websocket → unmatched, socket fails (scope of live)"),
        "{}",
        stdout(&out)
    );
    let out = sandbox.run(&[
        "eval",
        "new Promise((done) => { const s = new WebSocket('/ws'); s.binaryType = 'arraybuffer'; \
         s.onopen = () => s.send('echo me'); s.onmessage = (e) => done(e.data + ' ' + (s instanceof WebSocket)); })",
    ]);
    assert_eq!(stdout(&out), "echo me true\n", "{}", stderr(&out));

    // The mock closes the socket when the page says bye; then there is
    // nothing to send to.
    let out = sandbox.run(&["eval", "socket.send('bye')"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let got = seen(&sandbox, 7);
    assert_eq!(got[6], "close 4000 bye");
    let out = sandbox.run(&["mock", "ws", "send", "*", "hi", "--timeout", "300ms"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("no mocked WebSocket matching * opened within 300ms"),
        "{}",
        stderr(&out)
    );

    // Without mock sets, sockets are the browser's own again.
    let out = sandbox.run(&["mock", "off"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["eval", "String(WebSocket).includes('[native code]')"]);
    assert_eq!(stdout(&out), "true\n", "{}", stderr(&out));
    let out = sandbox.run(&["reload"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&[
        "eval",
        "String(WebSocket).includes('[native code]') && String(EventSource).includes('[native code]')",
    ]);
    assert_eq!(stdout(&out), "true\n", "{}", stderr(&out));
}

/// A `mock serve` process, stopped (SIGINT) when dropped.
struct Served(Child);

impl Drop for Served {
    fn drop(&mut self) {
        let _ = std::process::Command::new("kill")
            .args(["-INT", &self.0.id().to_string()])
            .status();
        let _ = self.0.wait();
    }
}

/// A raw HTTP/1.1 GET, its status line and body.
fn get(addr: &str, path: &str, headers: &str) -> (String, String) {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n{headers}\r\n"
    )
    .unwrap();
    let mut text = String::new();
    stream.read_to_string(&mut text).unwrap();
    let status = text.lines().next().unwrap_or_default().to_owned();
    let body = text.split("\r\n\r\n").nth(1).unwrap_or_default().to_owned();
    (status, body)
}

#[test]
fn mock_serve_runs_until_interrupted() {
    let mocks = mocks_dir();
    let mut child = cli()
        .args(["mock", "serve", "--dir", &mocks, "base", "--port", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let served = Served(child);
    let first = lines.next().unwrap().unwrap();
    let url = first
        .strip_prefix("mock server on ")
        .and_then(|rest| rest.split(": ").next())
        .unwrap_or_else(|| panic!("{first}"))
        .to_owned();
    assert!(first.ends_with(": base (Ctrl-C stops it)"), "{first}");
    let addr = url.trim_start_matches("http://").to_owned();

    let (status, body) = get(&addr, "/mock/api/products/1", "");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(body.contains("\"id\""), "{body}");
    let line = lines.next().unwrap().unwrap();
    assert!(
        line.starts_with(&format!(
            "#1 GET {url}/mock/api/products/1 server → base.toml:"
        )),
        "{line}"
    );
    let (status, _) = get(&addr, "/mock/api/nothing", "x-chrome-cdp-test: t7\r\n");
    assert_eq!(status, "HTTP/1.1 501 Not Implemented");
    let line = lines.next().unwrap().unwrap();
    assert!(line.ends_with("(scope of base) [test t7]"), "{line}");

    drop(served);
    let rest: Vec<String> = lines.map_while(Result::ok).collect();
    assert_eq!(
        rest.last().map(String::as_str),
        Some("mock server stopped after 2 requests")
    );
}
