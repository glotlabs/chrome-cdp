//! Mocks beyond fetches, end to end (plan 11, milestone 11.9): the page's
//! WebSockets and EventSources answered by `[[websocket]]` and `sse`
//! mocks, `mock ws send`, `mock sse send` (plan 19.2), and `mock serve`
//! (with a set `mock gen` wrote from the Petstore spec, plan 19.4).
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

    // A scenario pushes an event to the stream, after the mock's own; a
    // `.cdp` file runs the same command.
    let out = sandbox.run(&[
        "mock",
        "sse",
        "send",
        "/mock/api/news",
        "third",
        "--event",
        "headline",
        "--id",
        "3",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).starts_with("sent to 1 stream (/mock/api/news)\n  t1 http://"),
        "{}",
        stdout(&out)
    );
    assert!(
        stdout(&out).contains(r#"(live.toml:15 "news")"#),
        "{}",
        stdout(&out)
    );
    let got = seen(&sandbox, 7);
    assert_eq!(got[6], "headline third #3");
    let script = sandbox.tmp().join("push.cdp");
    std::fs::write(
        &script,
        "mock sse send '/mock/api/**' \"line 1\\nline 2\"\nwait --fn 'seen.length >= 8'\n",
    )
    .unwrap();
    let out = sandbox.run(&["run", script.to_str().unwrap()]);
    assert_eq!(code(&out), 0, "{}{}", stdout(&out), stderr(&out));
    let got = seen(&sandbox, 8);
    assert_eq!(got[7], "sse line 1\nline 2");
    let out = sandbox.run(&["mock", "sse", "send", "/nope/**", "x", "--timeout", "200ms"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("no mocked EventSource matching /nope/** opened within 200ms"),
        "{}",
        stderr(&out)
    );

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
    let got = seen(&sandbox, 9);
    assert_eq!(got[8], "close 4000 bye");
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

/// A `mock serve` of set `set`, its URL and its output's lines.
fn serve(
    set: &str,
) -> (
    Served,
    String,
    std::io::Lines<BufReader<std::process::ChildStdout>>,
) {
    let mut child = cli()
        .args(["mock", "serve", "--dir", &mocks_dir(), set, "--port", "0"])
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
    (served, url, lines)
}

#[test]
fn mock_serve_talks_websocket_to_a_client_that_is_not_a_browser() {
    use tokio_tungstenite::tungstenite::{self, Message};
    let (served, url, mut lines) = serve("live");
    let addr = url.trim_start_matches("http://").to_owned();
    let stream = TcpStream::connect(&addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let (mut socket, _) =
        tungstenite::client(format!("ws://{addr}/mock/ws/prices/ACME"), stream).unwrap();
    assert_eq!(
        socket.read().unwrap(),
        Message::text(r#"{"symbol":"ACME","type":"hello"}"#)
    );
    assert_eq!(
        lines.next().unwrap().unwrap(),
        r#"WS /mock/ws/prices/ACME open (live.toml:6 "prices")"#
    );
    assert_eq!(
        lines.next().unwrap().unwrap(),
        r#"WS /mock/ws/prices/ACME → {"symbol":"ACME","type":"hello"}"#
    );

    // `mock ws send --serve` reaches it, without a browser.
    let out = cli()
        .args(["mock", "ws", "send", "--serve", &url, "/mock/ws/**"])
        .arg(r#"{"type":"price","value":42}"#)
        .output()
        .unwrap();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!(
            "sent 27B to 1 socket of mock server {url}:\n  ws://{addr}/mock/ws/prices/ACME (live.toml:6 \"prices\")\n"
        )
    );
    assert_eq!(
        socket.read().unwrap(),
        Message::text(r#"{"type":"price","value":42}"#)
    );
    assert_eq!(
        lines.next().unwrap().unwrap(),
        r#"WS /mock/ws/prices/ACME → {"type":"price","value":42}"#
    );
    let out = cli()
        .args(["mock", "sse", "send", "--serve", &url, "*", "x"])
        .args(["--timeout", "200ms"])
        .output()
        .unwrap();
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!(
            "no EventSource stream of mock server {url} matching * opened within 200ms"
        )),
        "{}",
        stderr(&out)
    );

    // The page's message gets its rule's answer; bye closes it.
    socket
        .send(Message::text(r#"{"type":"ping","id":3}"#))
        .unwrap();
    assert_eq!(
        socket.read().unwrap(),
        Message::text(r#"{"id":3,"type":"pong"}"#)
    );
    socket.send(Message::text("bye")).unwrap();
    match socket.read().unwrap() {
        Message::Close(Some(frame)) => assert_eq!(u16::from(frame.code), 4000),
        other => panic!("not a close: {other:?}"),
    }
    let rest: Vec<String> = (0..3).map(|_| lines.next().unwrap().unwrap()).collect();
    assert_eq!(
        rest,
        [
            r#"WS /mock/ws/prices/ACME ← {"type":"ping","id":3}"#,
            r#"WS /mock/ws/prices/ACME → {"id":3,"type":"pong"}"#,
            "WS /mock/ws/prices/ACME ← bye",
        ]
    );
    assert_eq!(
        lines.next().unwrap().unwrap(),
        "WS /mock/ws/prices/ACME close 4000 bye"
    );
    drop(served);
}

/// `window.seen` of the page once it has `n` entries, sorted, with JSON
/// messages in one form.
fn seen_sorted(sandbox: &Sandbox, n: usize) -> Vec<String> {
    let mut got: Vec<String> = seen(sandbox, n)
        .into_iter()
        .map(|s| match s.split_once(' ') {
            Some((kind, rest)) => match serde_json::from_str::<serde_json::Value>(rest) {
                Ok(v) if v.is_object() => format!("{kind} {v}"),
                _ => s,
            },
            None => s,
        })
        .collect();
    got.sort();
    got
}

#[test]
fn a_recorded_websocket_and_event_source_replay() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let site = Fixtures::shared();
    let out = sandbox.run(&["open", &site.url("/mock/real-live.html")]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    seen(&sandbox, 3);
    let out = sandbox.run(&["click", "#ping"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let real = seen_sorted(&sandbox, 4);
    assert_eq!(
        real,
        [
            r#"headline {"title":"Second"} #2"#,
            "sse first",
            r#"ws {"id":7,"type":"pong"}"#,
            r#"ws {"type":"hello"}"#,
        ]
    );

    let file = sandbox.tmp().join("mocks/live.toml");
    let file = file.to_str().unwrap();
    let out = sandbox.run(&["mock", "record", "-o", file, "--force"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains(": 1 mock from 1 request, 1 [[websocket]] from 1 socket"),
        "{}",
        stdout(&out)
    );
    let toml = std::fs::read_to_string(file).unwrap();
    assert!(toml.contains("scope = [\"/live/**\"]"), "{toml}");
    assert!(
        toml.contains("[[websocket]]\nurl = \"/live/ws\"\n"),
        "{toml}"
    );
    assert!(toml.contains("respond = { sse = ["), "{toml}");

    // Replayed: the page gets the same messages and events, from the
    // mocks.
    let out = sandbox.run(&["mock", "use", file]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["reload"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    seen(&sandbox, 3);
    let out = sandbox.run(&["click", "#ping"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(seen_sorted(&sandbox, 4), real);
    let out = sandbox.run(&["mock", "log"]);
    let log = stdout(&out);
    assert!(log.contains("websocket → live.toml:"), "{log}");
    assert!(log.contains("eventsource → live.toml:"), "{log}");
}

/// Plan 19.4: `mock gen` over the Petstore writes a set `mock serve`
/// answers the spec's requests with.
#[test]
fn a_set_generated_from_the_petstore_spec_is_served() {
    let dir = tempfile::tempdir().unwrap();
    let mocks = dir.path().join("mocks");
    let spec = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/openapi/petstore.yaml")
        .canonicalize()
        .unwrap();
    let out = cli()
        .current_dir(dir.path())
        .args(["mock", "gen", "--openapi"])
        .arg(&spec)
        .arg("--dir")
        .arg(&mocks)
        .output()
        .unwrap();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("generated.toml: 19 mocks, 0 collections, 11 data files\n"),
        "{}",
        stdout(&out)
    );
    // No problem of mock check is left to warn about.
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));

    let mut child = cli()
        .current_dir(dir.path())
        .args(["mock", "serve", "--dir"])
        .arg(&mocks)
        .args(["generated", "--no-defaults", "--port", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let _served = Served(child);
    let first = lines.next().unwrap().unwrap();
    let addr = first
        .strip_prefix("mock server on http://")
        .and_then(|rest| rest.split(": ").next())
        .unwrap_or_else(|| panic!("{first}"))
        .to_owned();
    let (status, body) = get(&addr, "/api/v3/pet/10", "");
    assert_eq!(status, "HTTP/1.1 200 OK");
    let pet: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(pet["name"], "doggie", "{body}");
    let (status, body) = get(&addr, "/api/v3/pet/findByStatus?status=sold", "");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body)
            .unwrap()
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    let (status, _) = get(&addr, "/api/v3/user/logout", "");
    assert_eq!(status, "HTTP/1.1 200 OK");
}
