//! Mock sets (`mock use|off|list|log|reset|explain`; plan 11, milestone
//! 11.2) and their state (`mock state|seed`, `expect mock-state`;
//! milestone 11.3), `expect request` and unmatched diagnostics
//! (milestone 11.5) against a real Chrome (`CHROME_PATH`; skipped without it unless
//! `CHROME_CDP_REQUIRE_CHROME` is set), the fixture site's `mock/` page and
//! its mock files in `mock/mocks/`.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use browser::Host;
use engine::{
    EmulateRequest, EmulationChange, Engine, EvalRequest, EvalWorld, ExpectRequest, Expectation,
    MockExplainRequest, MockListRequest, MockLogRequest, MockOffRequest, MockSeedRequest,
    MockStateCheck, MockStateRequest, MockUseRequest, NavigateRequest, Navigation, OpenRequest,
    RequestCheck, RouteAction, RouteBody, RouteRequest, RouteSpec, WaitUntil,
};
use fixtures::Fixtures;
use protocol::ErrorKind;
use protocol::schema::{
    EvalData, ExpectData, MockExplainData, MockLogData, MockOffData, MockSeedData, MockSetsData,
    MockStateData,
};
use serde_json::{Value, json};

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

async fn start(path: &Path, name: &str) -> Arc<Engine> {
    let engine = Engine::new(name, None, None);
    let chrome = browser::discover(Some(path), &Host::current()).unwrap();
    engine
        .open(OpenRequest {
            chrome: Some(chrome),
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

/// `script`'s value in the active tab.
async fn eval(engine: &Engine, script: &str) -> Value {
    let out = engine
        .eval(EvalRequest {
            script: script.to_owned(),
            locator: None,
            frame: None,
            world: EvalWorld::Main,
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            sourcemaps: false,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap_or_else(|e| panic!("{script}: {}", e.to_text()));
    let data: EvalData = serde_json::from_value(out.data).unwrap();
    data.value
}

/// `[status, body]` of a fetch (the body as JSON if it is), or
/// `failed: <message>`.
async fn fetch(engine: &Engine, url: &str, init: &str) -> Value {
    let script = format!(
        "fetch({url:?}, {init}).then(async (r) => {{ const t = await r.text(); \
         let b; try {{ b = JSON.parse(t) }} catch {{ b = t }} return [r.status, b] }}, \
         (e) => 'failed: ' + e.message)"
    );
    eval(engine, &script).await
}

fn mocks_dir() -> PathBuf {
    fixtures::site_dir().join("mock/mocks")
}

fn source(dir: &Path, name: &str) -> mock::Source {
    mock::Source {
        name: name.to_owned(),
        file: dir.join(format!("{name}.toml")),
        base: dir.to_path_buf(),
    }
}

async fn use_sets(engine: &Engine, dir: &Path, names: &[&str], add: bool) -> MockSetsData {
    let out = engine
        .mock_use(MockUseRequest {
            sets: names.iter().map(|n| source(dir, n)).collect(),
            add,
            seed: 0,
            test: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("mock use: {}", e.to_text()));
    serde_json::from_value(out.data).unwrap()
}

fn log(engine: &Engine, unmatched: bool) -> (String, MockLogData) {
    let out = engine
        .mock_log(&MockLogRequest {
            unmatched,
            all: false,
            limit: 0,
            max_chars: 0,
        })
        .unwrap();
    (out.text, serde_json::from_value(out.data).unwrap())
}

fn explain(engine: &Engine, method: &str, url: &str) -> (String, MockExplainData) {
    let out = engine
        .mock_explain(&MockExplainRequest {
            method: method.into(),
            url: url.into(),
            headers: Vec::new(),
            body: None,
            limit: 0,
            max_chars: 0,
        })
        .unwrap();
    (out.text, serde_json::from_value(out.data).unwrap())
}

#[tokio::test]
async fn mock_sets_answer_the_page() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "mock-answers").await;
    goto(&engine, &site.url("/mock/index.html")).await;
    let dir = mocks_dir();

    let used = use_sets(&engine, &dir, &["base"], false).await;
    assert_eq!(used.sets.len(), 1);
    assert_eq!(used.sets[0].count, 6);
    assert_eq!(used.sets[0].scope, ["/mock/api/**"]);

    // Answers from data files, a templated path, and a missing one.
    assert_eq!(
        fetch(&engine, "/mock/api/products", "{}").await,
        json!([200, [{"id": 1, "name": "Keyboard"}, {"id": 2, "name": "Mouse"}]])
    );
    assert_eq!(
        fetch(&engine, "/mock/api/products/1", "{}").await,
        json!([200, {"id": 1, "name": "Keyboard", "price": 49}])
    );
    assert_eq!(
        fetch(&engine, "/mock/api/products/9", "{}").await,
        json!([404, {"error": "not found: data/products/9.json"}])
    );
    // The body is matched and templated, and a count kept.
    let post = |body: &str| {
        format!(
            "{{method: 'POST', headers: {{'content-type': 'application/json'}}, body: {body:?}}}"
        )
    };
    assert_eq!(
        fetch(&engine, "/mock/api/orders", &post(r#"{"qty":2}"#)).await[1]["n"],
        json!(1)
    );
    let second = fetch(&engine, "/mock/api/orders", &post(r#"{"qty":3}"#)).await;
    assert_eq!(second[0], json!(201));
    assert_eq!(
        (second[1]["n"].clone(), second[1]["qty"].clone()),
        (json!(2), json!(3))
    );
    // A network error.
    assert_eq!(
        fetch(&engine, "/mock/api/down", "{}").await,
        json!("failed: Failed to fetch")
    );
    // Unmatched in scope: a 501 that says so; out of scope: the server.
    let unmatched = fetch(&engine, "/mock/api/cart?user=7", "{}").await;
    assert_eq!(unmatched[0], json!(501));
    assert!(
        unmatched[1]["error"]
            .as_str()
            .unwrap()
            .contains("no mock for GET /mock/api/cart"),
        "{unmatched}"
    );
    let items = fetch(&engine, "/items?n=1", "{}").await;
    assert_eq!(
        (items[0].clone(), items[1][0]["id"].clone()),
        (json!(200), json!(1))
    );
    // Passthrough, with the real response patched.
    let echo = fetch(&engine, "/echo/patched", "{}").await;
    assert_eq!(echo[1]["patched"], json!(true));
    assert_eq!(echo[1]["path"], json!("/echo/patched"));

    // The journal says which mock answered what.
    let (text, data) = log(&engine, false);
    assert!(
        text.contains("GET ")
            && text
                .contains("/mock/api/products/1 xhr → base.toml:10 \"product by id\" fulfill 200"),
        "{text}"
    );
    assert!(
        text.contains("fulfill 404 (no data/products/9.json)"),
        "{text}"
    );
    assert!(
        text.contains("→ unmatched, fulfill 501 (scope of base)"),
        "{text}"
    );
    assert!(!text.contains("/items"), "{text}");
    assert!(
        data.log
            .iter()
            .all(|e| e.mock.is_some() || e.unmatched.is_some())
    );
    let (text, data) = log(&engine, true);
    assert_eq!(data.log.len(), 1, "{text}");

    // Hits per mock.
    let out = engine.mock_list(&MockListRequest { max_chars: 0 }).unwrap();
    assert!(
        out.text
            .contains("base.toml:20 \"order\" POST /mock/api/orders → fulfill 201 json "),
        "{}",
        out.text
    );
    assert!(out.text.contains("(2 hits)"), "{}", out.text);

    // `{{seq}}` starts over after a reset.
    engine.mock_reset().unwrap();
    assert_eq!(
        fetch(&engine, "/mock/api/orders", &post(r#"{"qty":1}"#)).await[1]["n"],
        json!(1)
    );

    // Off: the requests reach the server again.
    let out = engine
        .mock_off(MockOffRequest {
            sets: Vec::new(),
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
    let off: MockOffData = serde_json::from_value(out.data).unwrap();
    assert_eq!(off.removed, ["base"]);
    assert_eq!(out.text, "turned off base; no mock sets in use");
    assert_eq!(
        fetch(&engine, "/mock/api/products", "{}").await[0],
        json!(404)
    );
}

#[tokio::test]
async fn layers_routes_cors_and_explain() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "mock-layers").await;
    goto(&engine, &site.url("/mock/index.html")).await;
    let dir = mocks_dir();
    use_sets(&engine, &dir, &["base", "sale"], false).await;

    // The later set wins where both match; the earlier one does the rest.
    assert_eq!(
        fetch(&engine, "/mock/api/products", "{}").await[1][0]["sale"],
        json!(true)
    );
    assert_eq!(
        fetch(&engine, "/mock/api/products/1", "{}").await[0],
        json!(200)
    );

    // A cross-origin JSON POST: the preflight is allowed, the answer
    // carries CORS headers.
    let login = format!(
        "{{method: 'POST', headers: {{'content-type': 'application/json'}}, body: {:?}}}",
        r#"{"email":"ada@example.com"}"#
    );
    let got = eval(
        &engine,
        &format!(
            "fetch({:?}, {login}).then(async (r) => [r.status, r.headers.get('x-mock'), await r.json()], (e) => 'failed: ' + e.message)",
            site.url_b("/mock/api/login")
        ),
    )
    .await;
    assert_eq!(
        got,
        json!([200, "login", {"ok": true, "user": "ada@example.com"}])
    );
    let (text, _) = log(&engine, false);
    assert!(text.contains("OPTIONS "), "{text}");
    assert!(text.contains("\"login\" preflight allowed"), "{text}");

    // A body the mock does not match: unmatched.
    let other = format!(
        "{{method: 'POST', headers: {{'content-type': 'application/json'}}, body: {:?}}}",
        r#"{"email":"ada@corp.test"}"#
    );
    assert_eq!(
        fetch(&engine, "/mock/api/login", &other).await[0],
        json!(501)
    );

    // Rules come before mocks.
    engine
        .route(RouteRequest {
            rules: vec![RouteSpec {
                pattern: "**/mock/api/products".into(),
                label: None,
                methods: Vec::new(),
                resource_types: Vec::new(),
                times: None,
                delay_ms: 0,
                context: None,
                action: RouteAction::Fulfill {
                    status: 200,
                    headers: Vec::new(),
                    body: RouteBody::Json {
                        value: json!("route"),
                    },
                },
            }],
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
    assert_eq!(
        fetch(&engine, "/mock/api/products", "{}").await,
        json!([200, "route"])
    );
    let (text, data) = explain(&engine, "GET", &site.url("/mock/api/products"));
    assert_eq!(data.route.as_deref(), Some("r1"));
    assert_eq!(data.answer.unwrap().at, "sale.toml:2");
    assert!(text.contains("→ route r1 answers it first"), "{text}");

    let (text, data) = explain(&engine, "GET", &site.url("/mock/api/products/7"));
    assert_eq!(data.answer.unwrap().name, "product by id");
    assert_eq!(data.params["id"], "7");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[1],
        "→ base.toml:10 \"product by id\": fulfill 200 json_file data/products/7.json"
    );
    assert_eq!(lines[2], "  params: id=7");
    assert_eq!(
        lines[4],
        "  ✗ sale.toml:2 \"on sale\": path differs: expects /mock/api/products, got /mock/api/products/7"
    );
    let (text, data) = explain(&engine, "DELETE", &site.url("/mock/api/cart"));
    assert!(data.answer.is_none());
    assert!(
        text.contains("→ no mock matches; unmatched: in scope of base"),
        "{text}"
    );
}

#[tokio::test]
async fn frozen_time_and_changed_files() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "mock-reload").await;
    goto(&engine, &site.url("/mock/index.html")).await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("live.toml");
    std::fs::write(
        &file,
        "[[mock]]\nmatch = { path = \"/live\" }\nrespond = { json = { at = \"{{now}}\", v = 1 } }\n",
    )
    .unwrap();
    use_sets(&engine, dir.path(), &["live"], false).await;
    engine
        .emulate(EmulateRequest {
            change: EmulationChange::TimeFreeze {
                epoch_ms: 1_767_261_600_000,
            },
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
    assert_eq!(
        fetch(&engine, "/live", "{}").await,
        json!([200, {"at": "2026-01-01T10:00:00.000Z", "v": 1}])
    );
    // An edit is picked up without `mock use` again.
    std::fs::write(
        &file,
        "[[mock]]\nmatch = { path = \"/live\" }\nrespond = { json = { v = 2 } }\n",
    )
    .unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(later)
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(fetch(&engine, "/live", "{}").await, json!([200, {"v": 2}]));
    // A bad mock file is refused whole, at its line.
    std::fs::write(
        dir.path().join("bad.toml"),
        "[[mock]]\nmatch = { path = \"/x\" }\n",
    )
    .unwrap();
    let err = engine
        .mock_use(MockUseRequest {
            sets: vec![source(dir.path(), "bad")],
            add: true,
            seed: 0,
            test: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_err();
    assert!(
        err.message
            .ends_with("bad.toml:1:1: the mock has no `respond` or `sequence`"),
        "{}",
        err.message
    );
    assert_eq!(err.exit_code(), 2);
}

/// `expect mock-state <name> <check>` with `timeout_ms`.
async fn expect_state(
    engine: &Engine,
    name: &str,
    check: MockStateCheck,
    not: bool,
    timeout_ms: u64,
) -> Result<ExpectData, protocol::ErrorInfo> {
    let out = Box::pin(engine.expect(ExpectRequest {
        expectation: Expectation::MockState {
            name: name.into(),
            check,
        },
        not,
        tab: None,
        strict_refs: false,
        testid_attribute: None,
        timeout_ms,
        max_chars: 0,
    }))
    .await?;
    Ok(serde_json::from_value(out.data).unwrap())
}

fn count(n: u64) -> MockStateCheck {
    MockStateCheck::Count {
        count: Some(n),
        min: None,
        max: None,
    }
}

fn state(engine: &Engine, name: Option<&str>, set: &[(&str, Value)]) -> (String, MockStateData) {
    let out = engine
        .mock_state(&MockStateRequest {
            name: name.map(str::to_owned),
            set: set
                .iter()
                .map(|(n, v)| ((*n).to_owned(), v.clone()))
                .collect(),
            max_chars: 0,
        })
        .unwrap_or_else(|e| panic!("mock state: {}", e.to_text()));
    (out.text, serde_json::from_value(out.data).unwrap())
}

#[tokio::test]
async fn stateful_mocks_follow_the_page() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "mock-stateful").await;
    let dir = mocks_dir();

    // Without mock sets there is no state to look at.
    let err = expect_state(&engine, "todos", count(2), false, 100)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);

    let used = use_sets(&engine, &dir, &["stateful"], false).await;
    assert_eq!((used.sets[0].count, used.sets[0].collections), (5, 1));
    goto(&engine, &site.url("/mock/todos.html")).await;
    expect_state(&engine, "todos", count(2), false, 5000)
        .await
        .unwrap();
    assert_eq!(
        eval(&engine, "document.querySelectorAll('li').length").await,
        json!(2)
    );

    // The page adds a todo; it is in the next list, and in the state.
    eval(&engine, "addTodo('Ship it')").await;
    assert_eq!(
        eval(
            &engine,
            "[...document.querySelectorAll('li')].map(l => l.textContent)"
        )
        .await,
        json!(["Buy milk", "Write tests", "Ship it"])
    );
    let passed = expect_state(
        &engine,
        "todos",
        MockStateCheck::Contains {
            value: json!({"id": 3, "title": "Ship it"}),
        },
        false,
        5000,
    )
    .await
    .unwrap();
    assert_eq!(passed.count, Some(3));
    let (text, data) = state(&engine, Some("todos"), &[]);
    assert_eq!(data.collection.unwrap().count, 3);
    assert!(text.contains("\"title\": \"Ship it\""), "{text}");

    // `expect` waits for requests the page sends later.
    eval(&engine, "setTimeout(() => addTodo('Later'), 300), 0").await;
    let passed = expect_state(&engine, "todos", count(4), false, 5000)
        .await
        .unwrap();
    assert!(passed.retries > 0, "{passed:?}");
    assert!(passed.tab.is_empty());
    let err = expect_state(&engine, "todos", count(9), false, 200)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::ExpectFailed);
    let text = err.to_text();
    assert!(text.contains("expected: todos with 9 items"), "{text}");
    assert!(text.contains("actual: 4 items (after 200ms"), "{text}");
    assert!(
        text.contains("hint: chrome-cdp mock state todos --session mock-stateful"),
        "{text}"
    );

    // A sequence: busy once, then saved.
    let post = "{method: 'POST'}";
    assert_eq!(
        fetch(&engine, "/mock/api/save", post).await,
        json!([503, {"error": "busy"}])
    );
    assert_eq!(
        fetch(&engine, "/mock/api/save", post).await,
        json!([200, {"saved": true, "try": 2}])
    );

    // Named state: log in, look, log out.
    assert_eq!(fetch(&engine, "/mock/api/me", "{}").await[0], json!(401));
    let login = "{method: 'POST', headers: {'content-type': 'application/json'}, \
                 body: JSON.stringify({email: 'ada@example.com'})}";
    fetch(&engine, "/mock/api/login", login).await;
    assert_eq!(
        fetch(&engine, "/mock/api/me", "{}").await,
        json!([200, {"email": "ada@example.com"}])
    );
    expect_state(
        &engine,
        "logged_in",
        MockStateCheck::Value { value: json!(true) },
        false,
        1000,
    )
    .await
    .unwrap();
    let (text, data) = state(&engine, None, &[]);
    assert_eq!(data.values["user"], json!("ada@example.com"));
    assert!(
        text.starts_with("logged_in = true\nuser = \"ada@example.com\"\ncollections:\ntodos  4 items  /mock/api/todos  (stateful.toml:"),
        "{text}"
    );
    // --set changes a value by hand.
    let (text, _) = state(&engine, None, &[("logged_in", json!(false))]);
    assert!(
        text.starts_with("set logged_in\nlogged_in = false"),
        "{text}"
    );
    assert_eq!(fetch(&engine, "/mock/api/me", "{}").await[0], json!(401));
    let err = engine
        .mock_state(&MockStateRequest {
            name: Some("loged_in".into()),
            set: Vec::new(),
            max_chars: 0,
        })
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert!(
        err.to_text().contains("mock state logged_in"),
        "{}",
        err.to_text()
    );

    // The journal and the list say what the collection did.
    let (text, _) = log(&engine, false);
    assert!(
        text.contains("stateful.toml:41 \"todos\" fulfill 201 (todos: created 3)"),
        "{text}"
    );
    let out = engine.mock_list(&MockListRequest { max_chars: 0 }).unwrap();
    assert!(
        out.text
            .contains("stateful  5 mocks, 1 collection  scope /mock/api/**"),
        "{}",
        out.text
    );
    assert!(
        out.text.contains(
            "\"save: busy once, then ok\" POST /mock/api/save → sequence 2/2 (then repeat-last)"
        ),
        "{}",
        out.text
    );

    // Seed from a data file, then start over.
    let out = engine
        .mock_seed(&MockSeedRequest {
            name: "todos".into(),
            file: Some("data/more-todos.json".into()),
            items: None,
            cwd: None,
        })
        .unwrap_or_else(|e| panic!("{}", e.to_text()));
    let seeded: MockSeedData = serde_json::from_value(out.data).unwrap();
    assert_eq!(seeded.collection.count, 5);
    assert!(
        out.text.starts_with("seeded todos with 5 items from /"),
        "{}",
        out.text
    );
    assert_eq!(
        fetch(&engine, "/mock/api/todos?done=true", "{}").await,
        json!([200, [{"id": 12, "title": "Three", "done": true}]])
    );
    engine.mock_reset().unwrap();
    expect_state(&engine, "todos", count(2), false, 1000)
        .await
        .unwrap();
    expect_state(
        &engine,
        "logged_in",
        MockStateCheck::Value {
            value: json!(false),
        },
        false,
        1000,
    )
    .await
    .unwrap();
    assert_eq!(fetch(&engine, "/mock/api/save", post).await[0], json!(503));
}

/// `expect request` (or `no-request` with `none`) with `timeout_ms`.
async fn expect_request(
    engine: &Engine,
    check: RequestCheck,
    timeout_ms: u64,
) -> Result<(String, ExpectData), protocol::ErrorInfo> {
    let out = Box::pin(engine.expect(ExpectRequest {
        expectation: Expectation::Request(check),
        not: false,
        tab: None,
        strict_refs: false,
        testid_attribute: None,
        timeout_ms,
        max_chars: 0,
    }))
    .await?;
    Ok((out.text, serde_json::from_value(out.data).unwrap()))
}

fn sent(method: &str, target: &str) -> RequestCheck {
    RequestCheck {
        method: method.into(),
        target: target.into(),
        query: Vec::new(),
        headers: Vec::new(),
        json: None,
        count: None,
        min: None,
        max: None,
        since: None,
        none: false,
    }
}

#[tokio::test]
async fn expect_request_and_unmatched_diagnostics() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "mock-requests").await;
    goto(&engine, &site.url("/mock/index.html")).await;
    let dir = mocks_dir();
    use_sets(&engine, &dir, &["base", "cart"], false).await;
    let post = |body: &str, headers: &str| {
        format!(
            "{{method: 'POST', headers: {{'content-type': 'application/json'{headers}}}, body: {body:?}}}"
        )
    };
    fetch(&engine, "/mock/api/orders", &post(r#"{"qty":2}"#, "")).await;
    fetch(
        &engine,
        "/mock/api/orders",
        &post(r#"{"qty":3}"#, ", 'x-src': 'web'"),
    )
    .await;
    fetch(&engine, "/mock/api/products?page=2", "{}").await;
    // A body too large for the network log is read from Chrome.
    let big = format!(r#"{{"qty":9,"note":"{}"}}"#, "x".repeat(20_000));
    fetch(&engine, "/mock/api/orders", &post(&big, "")).await;

    let (text, data) = expect_request(
        &engine,
        RequestCheck {
            count: Some(3),
            ..sent("POST", "/mock/api/orders")
        },
        2_000,
    )
    .await
    .unwrap_or_else(|e| panic!("{}", e.to_text()));
    assert!(
        text.starts_with("passed: request POST /mock/api/orders --count 3: 3 requests ("),
        "{text}"
    );
    assert_eq!(data.count, Some(3));
    for check in [
        RequestCheck {
            json: Some(json!({"qty": 3})),
            headers: vec![("X-Src".into(), "w*".into())],
            count: Some(1),
            ..sent("post", "**/orders")
        },
        RequestCheck {
            json: Some(json!({"qty": 9})),
            ..sent("POST", "/mock/api/orders")
        },
        RequestCheck {
            query: vec![("page".into(), "2".into())],
            ..sent("*", "/mock/api/products")
        },
        sent("GET", "/mock/api/products?page=*"),
        RequestCheck {
            none: true,
            ..sent("DELETE", "/mock/api/**")
        },
    ] {
        expect_request(&engine, check.clone(), 2_000)
            .await
            .unwrap_or_else(|e| panic!("{check:?}: {}", e.to_text()));
    }

    // Nothing matches: the closest requests, with the condition each
    // fails.
    let err = expect_request(
        &engine,
        RequestCheck {
            json: Some(json!({"qty": 4})),
            ..sent("POST", "/mock/api/orders")
        },
        300,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::ExpectFailed);
    let text = err.to_text();
    assert!(
        text.contains("expected: a request POST /mock/api/orders with json {\"qty\":4}"),
        "{text}"
    );
    assert!(text.contains("actual: 0 requests (after 300ms, "), "{text}");
    assert!(
        text.contains("similar: t1 #")
            && text.contains(" POST ")
            && text.contains("/mock/api/orders → 201: json $.qty: expects 4, got 2"),
        "{text}"
    );
    assert!(text.contains("hint: chrome-cdp network show "), "{text}");
    // A path a letter off is listed; an unrelated one is not.
    let err = expect_request(&engine, sent("POST", "/mock/api/order"), 100)
        .await
        .unwrap_err();
    let text = err.to_text();
    assert!(
        text.contains("path differs: expects /mock/api/order, got /mock/api/orders"),
        "{text}"
    );
    let err = expect_request(&engine, sent("GET", "/mock/api/users"), 100)
        .await
        .unwrap_err();
    let text = err.to_text();
    assert!(
        text.contains("similar: none: no request sent has a path like /mock/api/users ("),
        "{text}"
    );
    assert!(text.contains("hint: chrome-cdp network "), "{text}");

    // Too many fails at once, without waiting.
    let started = std::time::Instant::now();
    let err = expect_request(
        &engine,
        RequestCheck {
            count: Some(1),
            ..sent("POST", "/mock/api/orders")
        },
        10_000,
    )
    .await
    .unwrap_err();
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    let text = err.to_text();
    assert!(text.contains("actual: 3 requests\n"), "{text}");
    assert_eq!(text.matches("matching: t1 #").count(), 3, "{text}");
    let err = expect_request(
        &engine,
        RequestCheck {
            none: true,
            ..sent("GET", "/mock/api/products")
        },
        10_000,
    )
    .await
    .unwrap_err();
    assert!(
        err.to_text()
            .contains("expected: no request GET /mock/api/products")
    );

    // --since a mark; a request sent later is waited for.
    engine
        .mark(engine::MarkRequest {
            name: "later".into(),
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
    eval(
        &engine,
        "setTimeout(() => fetch('/mock/api/orders', {method: 'POST', body: '{\"qty\":1}'}), 300); 1",
    )
    .await;
    let (_, data) = expect_request(
        &engine,
        RequestCheck {
            since: Some("later".into()),
            count: Some(1),
            ..sent("POST", "/mock/api/orders")
        },
        5_000,
    )
    .await
    .unwrap_or_else(|e| panic!("{}", e.to_text()));
    assert!(data.retries > 0, "{data:?}");
    let err = expect_request(
        &engine,
        RequestCheck {
            since: Some("nope".into()),
            ..sent("POST", "/mock/api/orders")
        },
        100,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadArgs);

    // An unmatched request names the mock that comes closest.
    let unmatched = fetch(&engine, "/mock/api/cart?user=7", "{}").await;
    assert_eq!(unmatched[0], json!(501));
    let (text, data) = log(&engine, true);
    assert_eq!(data.log.len(), 1, "{text}");
    let entry = &data.log[0];
    assert_eq!(entry.policy.as_deref(), Some("fail"));
    assert_eq!(entry.closest.len(), 1, "{entry:?}");
    assert_eq!(entry.closest[0].mock.at, "cart.toml:3");
    assert!(
        text.ends_with(
            "(scope of base)\n  closest mock: cart.toml:3 \"cart\": path differs: expects /mock/api/carts, got /mock/api/cart"
        ),
        "{text}"
    );
}

#[tokio::test]
async fn mocks_match_large_request_bodies() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "mock-large-bodies").await;
    goto(&engine, &site.url("/mock/index.html")).await;
    use_sets(&engine, &mocks_dir(), &["base"], false).await;
    // Chrome leaves a body this large out of `Fetch.requestPaused`.
    let answer = eval(
        &engine,
        "fetch('/mock/api/login', {method: 'POST', headers: {'content-type': 'application/json'}, \
         body: JSON.stringify({email: 'ada@example.com', pad: 'x'.repeat(2000000)})}) \
         .then(async (r) => [r.status, await r.json()])",
    )
    .await;
    assert_eq!(
        answer,
        json!([200, {"ok": true, "user": "ada@example.com"}]),
        "{answer}"
    );
    // A Blob body, which Chrome also leaves out.
    let answer = eval(
        &engine,
        "fetch('/mock/api/login', {method: 'POST', headers: {'content-type': 'application/json'}, \
         body: new Blob([JSON.stringify({email: 'bob@example.com'})])}) \
         .then(async (r) => [r.status, await r.json()])",
    )
    .await;
    assert_eq!(
        answer,
        json!([200, {"ok": true, "user": "bob@example.com"}]),
        "{answer}"
    );
}
