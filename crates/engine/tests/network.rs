//! `network`, `network show` and `network clear` against a real Chrome
//! (`CHROME_PATH`; skipped without it unless `CHROME_CDP_REQUIRE_CHROME`
//! is set) and the fixture site's `network/` page (plan 06, milestone
//! 6.2).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::Host;
use engine::{
    Action, ActionRequest, Button, Engine, NavigateRequest, Navigation, NetworkClearRequest,
    NetworkRequest, NetworkShowRequest, OpenRequest, Since, StatusRange, WaitUntil,
};
use fixtures::Fixtures;
use protocol::schema::{NetworkData, NetworkEntry, NetworkShowData, RequestState};
use protocol::{ErrorKind, Output};

const TIMEOUT_MS: u64 = 30_000;

fn chrome_path() -> Option<PathBuf> {
    match std::env::var_os("CHROME_PATH") {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => {
            assert!(
                std::env::var_os("CHROME_CDP_REQUIRE_CHROME").is_none(),
                "CHROME_CDP_REQUIRE_CHROME is set but CHROME_PATH is not"
            );
            eprintln!("skipping: set CHROME_PATH to run tests against a real Chrome");
            None
        }
    }
}

async fn start(path: &Path, name: &str, buffer: Option<usize>) -> Arc<Engine> {
    let engine = Engine::new(name, None, None);
    let chrome = browser::discover(Some(path), &Host::current()).unwrap();
    engine
        .open(OpenRequest {
            chrome: Some(chrome),
            network_buffer: buffer,
            timeout_ms: TIMEOUT_MS,
            ..OpenRequest::default()
        })
        .await
        .unwrap();
    engine
}

async fn goto(engine: &Engine, url: &str) {
    engine
        .navigate(NavigateRequest {
            navigation: Navigation::Goto {
                url: url.to_owned(),
                base_url: None,
            },
            wait: WaitUntil::Settle,
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("goto {url}: {}", e.to_text()));
}

async fn click_with(engine: &Engine, locator: &str, no_wait_after: bool) -> Output {
    engine
        .act(ActionRequest {
            action: Action::Click {
                locator: locator.to_owned(),
                button: Button::Left,
                count: 1,
                modifiers: Vec::new(),
                position: None,
                force: false,
                dblclick: false,
            },
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: TIMEOUT_MS,
            no_wait_after,
            snap: false,
            max_chars: 20_000,
        })
        .await
        .unwrap_or_else(|e| panic!("click {locator}: {}", e.to_text()))
}

async fn click(engine: &Engine, locator: &str) -> Output {
    click_with(engine, locator, false).await
}

fn request() -> NetworkRequest {
    NetworkRequest {
        tab: None,
        failed: false,
        types: Vec::new(),
        url: None,
        method: None,
        status: Vec::new(),
        since: Since::Last,
        limit: 50,
        timeout_ms: TIMEOUT_MS,
        max_chars: 0,
    }
}

fn all() -> NetworkRequest {
    NetworkRequest {
        since: Since::Seq { seq: 0 },
        limit: 0,
        ..request()
    }
}

async fn list(engine: &Engine, req: NetworkRequest) -> (Output, NetworkData) {
    let out = engine
        .network(req)
        .await
        .unwrap_or_else(|e| panic!("network: {}", e.to_text()));
    let data: NetworkData = serde_json::from_value(out.data.clone()).unwrap();
    (out, data)
}

fn show_request(id: u64) -> NetworkShowRequest {
    NetworkShowRequest {
        tab: None,
        id,
        headers: false,
        body: false,
        request_body: false,
        timeout_ms: TIMEOUT_MS,
        max_chars: 0,
    }
}

async fn show(engine: &Engine, req: NetworkShowRequest) -> (Output, NetworkShowData) {
    let out = engine
        .network_show(req)
        .await
        .unwrap_or_else(|e| panic!("network show: {}", e.to_text()));
    let data: NetworkShowData = serde_json::from_value(out.data.clone()).unwrap();
    (out, data)
}

/// The one request in `data` whose URL contains `part`.
fn find<'a>(data: &'a NetworkData, part: &str) -> &'a NetworkEntry {
    let found: Vec<_> = data
        .requests
        .iter()
        .filter(|r| r.url.contains(part))
        .collect();
    assert_eq!(found.len(), 1, "{part}: {:#?}", data.requests);
    found[0]
}

/// Waits until request `id` is no longer pending.
async fn ended(engine: &Engine, id: u64) -> NetworkShowData {
    let start = Instant::now();
    loop {
        let (_, data) = show(engine, show_request(id)).await;
        if data.request.state != RequestState::Pending {
            return data;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "{data:#?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn requests_and_their_bodies() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "network-bodies", None).await;
    goto(&engine, &site.url("/network/index.html")).await;

    // What the page loaded, retroactively.
    let (out, data) = list(&engine, request()).await;
    let text = &out.text;
    assert!(
        text.contains(" GET 200 /network/index.html document "),
        "{text}"
    );
    assert!(
        text.contains(" GET 200 /network/style.css stylesheet "),
        "{text}"
    );
    assert!(text.contains(" GET 200 /network/app.js script "), "{text}");
    assert!(
        text.ends_with("chrome-cdp network show 3 --session network-bodies shows one in full"),
        "{text}"
    );
    let document = find(&data, "index.html");
    assert_eq!(document.kind, "document");
    assert_eq!(document.mime.as_deref(), Some("text/html"));
    assert_eq!(document.protocol.as_deref(), Some("http/1.1"));
    assert!(document.size.unwrap() > 500, "{document:?}");
    let script = find(&data, "app.js");
    assert_eq!(script.initiator.as_ref().unwrap().kind, "parser");

    // Nothing new: nothing repeated.
    let (out, _) = list(&engine, request()).await;
    assert_eq!(
        out.text,
        "no new requests in t1; chrome-cdp network --session network-bodies --since 0 shows the earlier ones"
    );

    // A large JSON response, its body fetched on request.
    click(&engine, "#items").await;
    let (out, data) = list(&engine, request()).await;
    assert_eq!(data.requests.len(), 1, "{}", out.text);
    let items = &data.requests[0];
    assert_eq!(
        (items.method.as_str(), items.status, items.kind.as_str()),
        ("GET", Some(200), "fetch")
    );
    let initiator = items.initiator.as_ref().unwrap();
    assert_eq!(initiator.kind, "script");
    assert_eq!(initiator.function.as_deref(), Some("loadItems"));
    assert!(items.body_size.unwrap() > 10_000, "{items:?}");
    let (out, shown) = show(
        &engine,
        NetworkShowRequest {
            body: true,
            ..show_request(items.id)
        },
    )
    .await;
    let body = shown.response_body.unwrap();
    assert!(
        body.text
            .as_deref()
            .unwrap()
            .starts_with(r#"[{"id":1,"name":"item 1""#)
    );
    assert_eq!(body.size, items.body_size);
    // Pretty-printed in the text form.
    assert!(out.text.contains("\n    \"id\": 1,\n"), "{}", out.text);
    assert!(
        out.text
            .contains("initiator: script /network/app.js:6 (loadItems)"),
        "{}",
        out.text
    );
    assert!(shown.timing.is_some());
    // `-q` is the body.
    assert!(out.quiet.unwrap().starts_with("[{"));

    // A POST: its headers and body.
    click(&engine, "#login").await;
    let (out, data) = list(&engine, request()).await;
    let login = find(&data, "/api/login");
    assert!(
        out.text.starts_with(&format!(
            "#{} POST 500 /api/login?status=500 fetch",
            login.id
        )),
        "{}",
        out.text
    );
    let (out, shown) = show(
        &engine,
        NetworkShowRequest {
            headers: true,
            request_body: true,
            ..show_request(login.id)
        },
    )
    .await;
    let headers = shown.request_headers.unwrap();
    assert!(
        headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("x-test") && h.value == "yes"),
        "{headers:?}"
    );
    assert!(!shown.response_headers.unwrap().is_empty());
    assert_eq!(
        shown.request_body.unwrap().text.as_deref(),
        Some(r#"{"user":"ada","password":"s3cret"}"#)
    );
    assert!(out.text.contains("request headers:\n"), "{}", out.text);
    // The page never reads the body, so Chrome never ends the request.
    assert!(
        out.text
            .contains("type: fetch, mime: text/plain, protocol: http/1.1, body still loading"),
        "{}",
        out.text
    );

    // A request body too large to keep in the log comes from Chrome.
    click(&engine, "#upload").await;
    let (_, data) = list(&engine, request()).await;
    let upload = find(&data, "/items?n=1");
    assert_eq!(upload.kind, "xhr");
    let (_, shown) = show(
        &engine,
        NetworkShowRequest {
            request_body: true,
            ..show_request(upload.id)
        },
    )
    .await;
    let body = shown.request_body.unwrap();
    assert_eq!(body.size, Some(20_000), "{body:?}");
    engine.close().await;
}

#[tokio::test]
async fn failures_and_filters() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "network-failures", None).await;
    goto(&engine, &site.url("/network/index.html")).await;
    for button in ["#missing", "#refused", "#cors", "#abort", "#login"] {
        click(&engine, button).await;
    }
    let failed = NetworkRequest {
        failed: true,
        ..all()
    };
    let (out, data) = list(&engine, failed).await;
    let text = &out.text;
    assert_eq!(data.requests.len(), 4, "{text}");
    assert!(
        text.contains(" GET 404 /network/missing.json fetch"),
        "{text}"
    );
    assert!(
        text.contains(" GET failed http://127.0.0.1:2/refused fetch ")
            && text.contains(" net::ERR_CONNECTION_REFUSED"),
        "{text}"
    );
    let cors = find(&data, &site.url_b("/items"));
    assert_eq!(cors.state, RequestState::Failed);
    assert_eq!(cors.cors.as_deref(), Some("MissingAllowOriginHeader"));
    assert!(text.contains("(CORS: MissingAllowOriginHeader)"), "{text}");
    // Canceled is not failed.
    assert!(!text.contains("delay=2000"), "{text}");
    let (out, data) = list(&engine, all()).await;
    let aborted = find(&data, "delay=2000");
    assert_eq!(aborted.state, RequestState::Canceled, "{}", out.text);
    assert!(
        out.text
            .contains(" GET canceled /items?n=1&delay=2000 fetch"),
        "{}",
        out.text
    );

    // A failed request has no body.
    let refused = find(&data, "127.0.0.1:2");
    let (out, _) = show(
        &engine,
        NetworkShowRequest {
            body: true,
            ..show_request(refused.id)
        },
    )
    .await;
    assert!(
        out.text.contains("error: net::ERR_CONNECTION_REFUSED"),
        "{}",
        out.text
    );
    assert!(
        out.text
            .contains("response body: not available: the request failed"),
        "{}",
        out.text
    );

    let ids = |data: &NetworkData| {
        data.requests
            .iter()
            .map(|r| r.url.clone())
            .collect::<Vec<_>>()
    };
    let status = NetworkRequest {
        status: vec![StatusRange::parse("4xx").unwrap()],
        ..all()
    };
    let (_, data) = list(&engine, status).await;
    assert_eq!(ids(&data), [site.url("/network/missing.json")]);
    let post = NetworkRequest {
        method: Some("post".into()),
        ..all()
    };
    let (_, data) = list(&engine, post).await;
    assert_eq!(ids(&data), [site.url("/api/login?status=500")]);
    let types = NetworkRequest {
        types: vec!["script".into(), "stylesheet".into()],
        ..all()
    };
    let (_, data) = list(&engine, types).await;
    assert_eq!(data.requests.len(), 2);
    let url = NetworkRequest {
        url: Some("**/network/*.json".into()),
        ..all()
    };
    let (_, data) = list(&engine, url).await;
    assert_eq!(ids(&data), [site.url("/network/missing.json")]);
    engine.close().await;
}

#[tokio::test]
async fn pending_requests_redirects_and_websockets() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "network-pending", None).await;
    goto(&engine, &site.url("/network/index.html")).await;
    list(&engine, request()).await;

    // Shown pending, and once more when it ends.
    click_with(&engine, "#slow", true).await;
    let (out, data) = list(&engine, request()).await;
    let slow = find(&data, "delay=1500");
    assert_eq!(slow.state, RequestState::Pending, "{}", out.text);
    assert!(
        out.text
            .contains(" GET pending /items?n=1&delay=1500 fetch"),
        "{}",
        out.text
    );
    ended(&engine, slow.id).await;
    let (out, data) = list(&engine, request()).await;
    assert_eq!(data.requests.len(), 1, "{}", out.text);
    assert_eq!(data.requests[0].status, Some(200));
    assert!(data.requests[0].duration_ms.unwrap() >= 1500);

    // Each hop of a redirect is its own request.
    click(&engine, "#redirect").await;
    let (out, data) = list(&engine, request()).await;
    assert_eq!(data.requests.len(), 2, "{}", out.text);
    assert_eq!(data.requests[0].status, Some(302));
    assert!(
        out.text.contains(" GET 302 /redirect?to=/items?n=1 fetch ")
            && out.text.contains(" -> /items?n=1\n"),
        "{}",
        out.text
    );
    assert_eq!(data.requests[1].status, Some(200));

    // A WebSocket's frames, summed up.
    click(&engine, "#ws").await;
    let (_, data) = list(&engine, request()).await;
    let ws = find(&data, "/ws");
    let shown = ended(&engine, ws.id).await;
    let summary = shown.request.websocket.unwrap();
    assert_eq!((summary.sent, summary.received), (1, 1), "{summary:?}");
    assert!(summary.closed);
    assert_eq!(shown.request.status, Some(101));
    assert_eq!(shown.request.kind, "websocket");
    assert_eq!(shown.frames[0].text.as_deref(), Some("hello"));
    let (out, _) = show(&engine, show_request(ws.id)).await;
    assert!(
        out.text
            .contains("frames: 1 sent (5B), 1 received (5B), closed"),
        "{}",
        out.text
    );
    assert!(
        out.text.contains("\n  sent text 5B hello\n"),
        "{}",
        out.text
    );
    engine.close().await;
}

#[tokio::test]
async fn the_log_is_bounded_and_clears() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "network-bounded", Some(3)).await;
    goto(&engine, &site.url("/network/index.html")).await;
    click(&engine, "#items").await;
    click(&engine, "#missing").await;
    let (out, data) = list(&engine, request()).await;
    assert_eq!(data.requests.len(), 3, "{}", out.text);
    assert!(data.dropped >= 2, "{data:?}");
    assert!(
        out.warnings[0].contains("dropped from t1's network log before they were read; it keeps the newest 3 (open --network-buffer N keeps more)"),
        "{:?}",
        out.warnings
    );
    let err = engine.network_show(show_request(1)).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert!(
        err.message.contains("no longer in t1's network log"),
        "{}",
        err.message
    );
    let err = engine.network_show(show_request(999)).await.unwrap_err();
    assert_eq!(err.message, "t1's network log has no request #999");

    let out = engine
        .network_clear(NetworkClearRequest {
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
    assert_eq!(out.text, "cleared 3 requests from t1's network log");
    let (out, data) = list(&engine, all()).await;
    assert!(data.requests.is_empty());
    assert_eq!(data.dropped, 0);
    assert_eq!(out.text, "no requests in t1");
    engine.close().await;
}
