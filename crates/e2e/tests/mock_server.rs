//! `mock serve` (plan 11, milestone 11.9): mock sets as a real HTTP
//! server, with state partitioned by the `x-chrome-cdp-test` header.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use e2e::mock_server::{MockServer, Served, TEST_HEADER};
use mock::{MockSet, Source};
use serde_json::{Value, json};

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
