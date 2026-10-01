//! `mock serve` (plan 11, milestone 11.9): mock sets as a real HTTP
//! server, with state partitioned by the `x-chrome-cdp-test` header; its
//! WebSockets and pushes (plan 19, milestone 19.2).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use e2e::mock_server::{MockServer, Push, Served, SocketLog, TEST_HEADER, post_push};
use futures_util::{SinkExt as _, StreamExt as _};
use mock::{MockSet, Source};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

const BASE: &str = r#"
[settings]
scope = ["/api/**"]

[[mock]]
name = "hello"
match = { method = "GET", path = "/api/hello/:name" }
respond = { json = { hello = "{{params.name}}", n = "{{seq}}" }, headers = { "x-mock" = "yes" } }

[[mock]]
name = "down"
match = { path = "/api/down" }
respond = { abort = "connectionreset" }

[[mock]]
name = "real"
match = { path = "/api/real" }
respond = { passthrough = true }

[[mock]]
name = "prices"
match = { path = "/api/prices" }
respond = { sse = [{ data = { price = 1 } }, { delay = "200ms", event = "price", data = { price = 2 } }] }

[[collection]]
name = "todos"
path = "/api/todos"
items = [{ id = 1, title = "Buy milk" }]

[[websocket]]
name = "prices ws"
url = "/ws/prices/:symbol"
protocol = "v1"
on_open = [{ send = { type = "hello", symbol = "{{params.symbol}}", n = "{{seq}}" } }]
on_message = [
  { match = { type = "ping" }, delay = "50ms", send = { type = "pong", id = "{{json.id}}" } },
  { match = "bin", send_base64 = "AQID" },
  { match = "bye", close = 4000, reason = "bye" },
]
"#;

const OTHER: &str = r#"
[[mock]]
name = "other hello"
match = { method = "GET", path = "/api/hello/:name" }
respond = { json = { other = true } }
"#;

fn sets(dir: &Path, names: &[&str]) -> Vec<MockSet> {
    names
        .iter()
        .map(|n| MockSet::load(&Source::find(dir, dir, n).unwrap()).unwrap())
        .collect()
}

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("base.toml"), BASE).unwrap();
    std::fs::write(dir.path().join("other.toml"), OTHER).unwrap();
    dir
}

fn client() -> reqwest::Client {
    // reqwest is built without a default provider (no aws-lc).
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder().no_proxy().build().unwrap()
}

fn local() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

async fn json_of(response: reqwest::Response) -> (u16, Value) {
    let status = response.status().as_u16();
    let bytes = response.bytes().await.unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn mock_serve_answers_with_the_mocks() {
    let dir = project();
    let seen: Arc<Mutex<Vec<Served>>> = Arc::default();
    let log = seen.clone();
    let server = MockServer::start(
        local(),
        sets(dir.path(), &["base"]),
        0,
        Some(Box::new(move |s: &Served| {
            log.lock().unwrap().push(s.clone());
        })),
    )
    .unwrap();
    let url = server.url();
    let http = client();

    let response = http
        .get(format!("{url}/api/hello/ada"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["x-mock"], "yes");
    assert_eq!(
        json_of(response).await,
        (200, json!({"hello": "ada", "n": 1}))
    );

    // Collections keep their state across requests.
    let created = http
        .post(format!("{url}/api/todos"))
        .header("content-type", "application/json")
        .body(json!({"title": "Walk the dog"}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(created.status().as_u16(), 201);
    let (_, list) = json_of(http.get(format!("{url}/api/todos")).send().await.unwrap()).await;
    assert_eq!(list.as_array().unwrap().len(), 2);

    // In scope, but no mock: the set's `unmatched` answer.
    let (status, body) =
        json_of(http.get(format!("{url}/api/nothing")).send().await.unwrap()).await;
    assert_eq!(status, 501);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("no mock for GET /api/nothing")
    );
    // No set owns it.
    let (status, _) = json_of(http.get(format!("{url}/elsewhere")).send().await.unwrap()).await;
    assert_eq!(status, 404);
    // `abort` closes the connection; `passthrough` has no server behind.
    assert!(http.get(format!("{url}/api/down")).send().await.is_err());
    let (status, body) = json_of(http.get(format!("{url}/api/real")).send().await.unwrap()).await;
    assert_eq!(status, 502);
    assert!(body["error"].as_str().unwrap().contains("passthrough"));

    // CORS: a preflight is allowed, and answers allow the origin.
    let preflight = http
        .request(reqwest::Method::OPTIONS, format!("{url}/api/hello/x"))
        .header("Origin", "http://app.test")
        .header("Access-Control-Request-Method", "GET")
        .header("Access-Control-Request-Headers", "x-token")
        .send()
        .await
        .unwrap();
    assert_eq!(preflight.status().as_u16(), 204);
    assert_eq!(
        preflight.headers()["access-control-allow-origin"],
        "http://app.test"
    );
    assert_eq!(
        preflight.headers()["access-control-allow-headers"],
        "x-token"
    );
    let cross = http
        .get(format!("{url}/api/hello/x"))
        .header("Origin", "http://app.test")
        .send()
        .await
        .unwrap();
    assert_eq!(
        cross.headers()["access-control-allow-origin"],
        "http://app.test"
    );
    assert_eq!(cross.headers()["access-control-expose-headers"], "x-mock");

    let journal = server.journal(None);
    let outcomes: Vec<&str> = journal.iter().map(|e| e.outcome.as_str()).collect();
    assert_eq!(journal[0].mock.as_ref().unwrap().name, "hello");
    assert_eq!(journal[0].resource_type, "server");
    assert!(outcomes.contains(&"unmatched, fulfill 501"), "{outcomes:?}");
    assert!(outcomes.contains(&"no mock, 404"), "{outcomes:?}");
    assert!(
        outcomes.contains(&"abort connectionreset (connection closed)"),
        "{outcomes:?}"
    );
    assert_eq!(seen.lock().unwrap().len(), journal.len());
    server.stop();
}

#[tokio::test]
async fn mock_serve_streams_server_sent_events() {
    let dir = project();
    let server = MockServer::start(local(), sets(dir.path(), &["base"]), 0, None).unwrap();
    let mut response = client()
        .get(format!("{}/api/prices", server.url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let first = response.chunk().await.unwrap().unwrap();
    assert_eq!(&first[..], b"data: {\"price\":1}\n\n");
    let second = response.chunk().await.unwrap().unwrap();
    assert_eq!(&second[..], b"event: price\ndata: {\"price\":2}\n\n");
    // The stream stays open after the last event.
    let more = tokio::time::timeout(Duration::from_millis(300), response.chunk()).await;
    assert!(more.is_err(), "the stream ended: {more:?}");
}

#[tokio::test]
async fn the_test_header_partitions_the_state() {
    let dir = project();
    let server = MockServer::start(local(), sets(dir.path(), &["base"]), 0, None).unwrap();
    let url = server.url();
    let http = client();
    let hello = |test: Option<&'static str>| {
        let mut request = http.get(format!("{url}/api/hello/x"));
        if let Some(test) = test {
            request = request.header(TEST_HEADER, test);
        }
        async move { json_of(request.send().await.unwrap()).await.1 }
    };
    assert_eq!(hello(None).await["n"], 1);
    assert_eq!(hello(None).await["n"], 2);
    // A new test id starts from fresh state.
    assert_eq!(hello(Some("t1")).await["n"], 1);
    assert_eq!(hello(Some("t2")).await["n"], 1);
    assert_eq!(hello(Some("t1")).await["n"], 2);

    // A test's own sets, registered by the runner.
    server.partition("t3", sets(dir.path(), &["base", "other"]), None);
    assert_eq!(hello(Some("t3")).await, json!({"other": true}));
    assert_eq!(server.journal(Some("t3")).len(), 1);
    assert_eq!(server.journal(Some("t1")).len(), 2);
    server.close_partition("t1");
    assert!(server.journal(Some("t1")).is_empty());
    assert_eq!(hello(Some("t1")).await["n"], 1);
}

#[test]
fn a_taken_port_is_an_error() {
    let dir = project();
    let first = MockServer::start(local(), sets(dir.path(), &["base"]), 0, None).unwrap();
    let again = MockServer::start(first.addr(), sets(dir.path(), &["base"]), 0, None);
    assert!(again.is_err());
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A WebSocket to `path` of `server`, with the test header if given.
async fn ws(server: &MockServer, path: &str, test: Option<&str>) -> (Ws, Option<String>) {
    let url = format!("ws://{}{path}", server.addr());
    let mut request = url.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("sec-websocket-protocol", "v1, v2".parse().unwrap());
    if let Some(test) = test {
        request
            .headers_mut()
            .insert(TEST_HEADER, test.parse().unwrap());
    }
    let (socket, response) = tokio_tungstenite::connect_async(request).await.unwrap();
    let protocol = response
        .headers()
        .get("sec-websocket-protocol")
        .map(|v| v.to_str().unwrap().to_owned());
    (socket, protocol)
}

/// The next text message.
async fn text(socket: &mut Ws) -> String {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("a message in time")
            .unwrap()
            .unwrap();
        match message {
            Message::Text(text) => return text.as_str().to_owned(),
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("not text: {other:?}"),
        }
    }
}

/// A control request to `server`: status and JSON answer.
async fn control(server: &MockServer, kind: &str, body: Value) -> (u16, Value) {
    let response = client()
        .post(format!("{}/__chrome-cdp/{kind}/send", server.url()))
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    json_of(response).await
}

#[tokio::test]
async fn mock_serve_answers_websockets() {
    let dir = project();
    let logged: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = logged.clone();
    let server = MockServer::start(local(), sets(dir.path(), &["base"]), 0, None).unwrap();
    server.on_socket(Box::new(move |entry: &SocketLog| {
        log.lock().unwrap().push(entry.line());
    }));

    // It opens with the mock's protocol and gets on_open's message.
    let (mut socket, protocol) = ws(&server, "/ws/prices/ACME", None).await;
    assert_eq!(protocol.as_deref(), Some("v1"));
    assert_eq!(
        text(&mut socket).await,
        r#"{"n":1,"symbol":"ACME","type":"hello"}"#
    );

    // Messages get their rule's answer, after its delay; binary too.
    socket
        .send(Message::text(r#"{"type":"ping","id":7}"#))
        .await
        .unwrap();
    assert_eq!(text(&mut socket).await, r#"{"id":7,"type":"pong"}"#);
    socket.send(Message::text("bin")).await.unwrap();
    let message = socket.next().await.unwrap().unwrap();
    assert_eq!(message, Message::binary(vec![1, 2, 3]));

    // Pushes reach it; a glob that matches no socket is a 404 that names
    // the open ones.
    let (status, body) = control(
        &server,
        "ws",
        json!({ "url": "/ws/**", "data": "{\"type\":\"price\"}" }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["sent"], 1);
    assert_eq!(body["to"][0]["mock"]["name"], "prices ws");
    assert_eq!(text(&mut socket).await, r#"{"type":"price"}"#);
    let (status, body) = control(
        &server,
        "ws",
        json!({ "url": "/nope/**", "data": "x", "timeout_ms": 100 }),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(body["error"], "no WebSocket matching /nope/** is open");
    assert!(
        body["open"][0]
            .as_str()
            .unwrap()
            .ends_with("/ws/prices/ACME"),
        "{body}"
    );
    let pushed = server.push(
        &mock::SocketGlob::parse("*").unwrap(),
        &Push::Ws(mock::WsFrame::Text("direct".into())),
        None,
    );
    assert_eq!(pushed.len(), 1);
    assert_eq!(text(&mut socket).await, "direct");

    // The test header picks the partition: its own hit count, and pushes
    // for another test do not reach it.
    let (mut other, _) = ws(&server, "/ws/prices/XYZ", Some("t1")).await;
    assert_eq!(
        text(&mut other).await,
        r#"{"n":1,"symbol":"XYZ","type":"hello"}"#
    );
    let (status, _) = control(
        &server,
        "ws",
        json!({ "url": "*", "data": "for t2", "test": "t2" }),
    )
    .await;
    assert_eq!(status, 404);
    let (status, body) = control(
        &server,
        "ws",
        json!({ "url": "*", "data": "for t1", "test": "t1" }),
    )
    .await;
    assert_eq!((status, body["sent"].clone()), (200, json!(1)));
    assert_eq!(text(&mut other).await, "for t1");
    // Closing the partition closes its sockets.
    server.close_partition("t1");
    let closed = tokio::time::timeout(Duration::from_secs(5), other.next())
        .await
        .unwrap();
    assert!(
        matches!(closed, Some(Ok(Message::Close(_))) | None),
        "{closed:?}"
    );

    // The mock closes the socket when told bye.
    socket.send(Message::text("bye")).await.unwrap();
    match socket.next().await.unwrap().unwrap() {
        Message::Close(Some(frame)) => {
            assert_eq!(u16::from(frame.code), 4000);
            assert_eq!(frame.reason.as_str(), "bye");
        }
        other => panic!("not a close: {other:?}"),
    }

    // No [[websocket]] takes it: the HTTP answer, as before.
    let url = format!("ws://{}/ws/other", server.addr());
    assert!(tokio_tungstenite::connect_async(url).await.is_err());

    let journal = server.journal(None);
    assert!(
        journal
            .iter()
            .any(|e| e.resource_type == "websocket"
                && e.outcome == "websocket open, sends 1 message"),
        "{journal:?}"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    let lines = logged.lock().unwrap().clone();
    assert_eq!(
        lines[..4],
        [
            r#"WS /ws/prices/ACME open (base.toml:30 "prices ws")"#,
            r#"WS /ws/prices/ACME → {"n":1,"symbol":"ACME","type":"hello"}"#,
            r#"WS /ws/prices/ACME ← {"type":"ping","id":7}"#,
            r#"WS /ws/prices/ACME → {"id":7,"type":"pong"}"#,
        ],
        "{lines:#?}"
    );
    assert!(
        lines.contains(&"WS /ws/prices/ACME close 4000 bye".to_owned()),
        "{lines:#?}"
    );
}

#[tokio::test]
async fn mock_serve_pushes_to_streams() {
    let dir = project();
    let server = MockServer::start(local(), sets(dir.path(), &["base"]), 0, None).unwrap();
    let mut response = client()
        .get(format!("{}/api/prices", server.url()))
        .send()
        .await
        .unwrap();
    // A push waits for the stream's own events to go first.
    let (status, body) = control(
        &server,
        "sse",
        json!({ "url": "/api/prices", "data": "a\nb", "event": "price", "id": "9" }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["to"][0]["mock"]["name"], "prices");
    let mut got = Vec::new();
    while got.len() < 3 {
        let chunk = response.chunk().await.unwrap().unwrap();
        got.push(String::from_utf8(chunk.to_vec()).unwrap());
    }
    assert_eq!(
        got,
        [
            "data: {\"price\":1}\n\n",
            "event: price\ndata: {\"price\":2}\n\n",
            "event: price\nid: 9\ndata: a\ndata: b\n\n",
        ]
    );
    // A stream the client left takes no more.
    drop(response);
    let (status, _) = control(
        &server,
        "sse",
        json!({ "url": "*", "data": "x", "timeout_ms": 200 }),
    )
    .await;
    assert_eq!(status, 404);
    // Unknown control requests are refused.
    let (status, body) = control(&server, "nope", json!({})).await;
    assert_eq!(status, 404);
    assert!(
        body["error"].as_str().unwrap().contains("ws/send"),
        "{body}"
    );
}

#[test]
fn post_push_reports_what_it_reached() {
    let dir = project();
    let server = MockServer::start(local(), sets(dir.path(), &["base"]), 0, None).unwrap();
    let err = post_push(
        &server.url(),
        "ws",
        &json!({ "url": "/ws/**", "data": "x", "timeout_ms": 50 }),
    )
    .unwrap_err();
    assert!(err.none_matched);
    assert_eq!(err.message, "no WebSocket matching /ws/** is open");
    let err = post_push(
        "http://127.0.0.1:1",
        "ws",
        &json!({ "url": "*", "data": "x" }),
    )
    .unwrap_err();
    assert!(!err.none_matched);
    assert!(
        err.message
            .starts_with("cannot reach the mock server at http://127.0.0.1:1"),
        "{}",
        err.message
    );
}
