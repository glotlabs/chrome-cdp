//! Request routing (`route`, `routes`, `unroute`; plan 07, milestone 7.1)
//! against a real Chrome (`CHROME_PATH`; skipped without it unless
//! `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture site's `route/`
//! pages.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::Host;
use engine::{
    Engine, EvalRequest, EvalWorld, HttpCredentials, NavigateRequest, Navigation, OpenRequest,
    RouteAction, RouteBody, RouteRequest, RouteSpec, RoutesRequest, TabsRequest, UnrouteRequest,
    WaitUntil,
};
use fixtures::{AUTH_PASSWORD, AUTH_USER, Fixtures};
use protocol::schema::{EvalData, NavigateData, RouteData, RoutesData, TabsData, UnrouteData};
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

async fn open(engine: &Arc<Engine>, path: &Path, credentials: Option<HttpCredentials>) {
    let chrome = browser::discover(Some(path), &Host::current()).unwrap();
    engine
        .open(OpenRequest {
            chrome: Some(chrome),
            http_credentials: credentials,
            timeout_ms: TIMEOUT_MS,
            ..OpenRequest::default()
        })
        .await
        .unwrap();
}

async fn start(path: &Path, name: &str) -> Arc<Engine> {
    let engine = Engine::new(name, None, None);
    open(&engine, path, None).await;
    engine
}

async fn goto(engine: &Engine, url: &str) -> NavigateData {
    let out = engine
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
    serde_json::from_value(out.data).unwrap()
}

fn eval_request(script: &str) -> EvalRequest {
    EvalRequest {
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
    }
}

/// `script`'s value in the active tab.
async fn eval(engine: &Engine, script: &str) -> Value {
    eval_with(engine, eval_request(script)).await
}

async fn eval_with(engine: &Engine, req: EvalRequest) -> Value {
    let script = req.script.clone();
    let out = engine
        .eval(req)
        .await
        .unwrap_or_else(|e| panic!("{script}: {}", e.to_text()));
    let data: EvalData = serde_json::from_value(out.data).unwrap();
    data.value
}

/// Waits until `script`'s value is `want` (it changes as the page loads).
async fn eventually(engine: &Engine, req: EvalRequest, want: &Value) {
    let start = Instant::now();
    loop {
        let value = eval_with(engine, req.clone()).await;
        if &value == want {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "{}: {value} is not {want}",
            req.script
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The text `fetch(url)` gets, or `failed: <message>`.
async fn fetch_text(engine: &Engine, url: &str) -> String {
    let script = format!("fetch({url:?}).then((r) => r.text(), (e) => 'failed: ' + e.message)");
    eval(engine, &script).await.as_str().unwrap().to_owned()
}

fn rule(pattern: &str, action: RouteAction) -> RouteSpec {
    RouteSpec {
        pattern: pattern.into(),
        label: None,
        methods: Vec::new(),
        resource_types: Vec::new(),
        times: None,
        delay_ms: 0,
        context: None,
        action,
    }
}

fn text(body: &str) -> RouteAction {
    RouteAction::Fulfill {
        status: 200,
        headers: Vec::new(),
        body: RouteBody::Text { text: body.into() },
    }
}

fn continue_with(
    set_headers: &[(&str, &str)],
    remove: &[&str],
    url: Option<String>,
) -> RouteAction {
    RouteAction::Continue {
        set_headers: set_headers
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect(),
        remove_headers: remove.iter().map(|&h| h.to_owned()).collect(),
        url,
    }
}

async fn add(engine: &Engine, rules: Vec<RouteSpec>) -> RouteData {
    let out = engine
        .route(RouteRequest {
            rules,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("route: {}", e.to_text()));
    serde_json::from_value(out.data).unwrap()
}

fn routes(engine: &Engine, log: bool) -> (String, RoutesData) {
    let out = engine
        .routes(&RoutesRequest {
            log,
            limit: 0,
            max_chars: 0,
        })
        .unwrap();
    (out.text, serde_json::from_value(out.data).unwrap())
}

async fn unroute(engine: &Engine, ids: &[&str], all: bool) -> UnrouteData {
    let out = engine
        .unroute(UnrouteRequest {
            ids: ids.iter().map(|&i| i.to_owned()).collect(),
            all,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("unroute: {}", e.to_text()));
    serde_json::from_value(out.data).unwrap()
}

#[tokio::test]
async fn fulfil_and_abort() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "route-fulfil").await;
    goto(&engine, &site.url("/route/index.html")).await;

    let added = add(
        &engine,
        vec![
            rule(
                "**/route/api/items",
                RouteAction::Fulfill {
                    status: 201,
                    headers: vec![("X-Mock".into(), "yes".into())],
                    body: RouteBody::Json {
                        value: json!({"items": [1, 2]}),
                    },
                },
            ),
            rule(
                "/route/api/",
                RouteAction::Abort {
                    reason: "blockedbyclient".into(),
                },
            ),
        ],
    )
    .await;
    assert_eq!(added.active, 2);
    assert_eq!(added.added[0].summary, "fulfill 201 application/json 15B");

    // The first matching rule answers, without the server.
    let got = eval(
        &engine,
        "fetch('/route/api/items').then(async (r) => [r.status, r.headers.get('x-mock'), \
         r.headers.get('content-type'), await r.json()])",
    )
    .await;
    assert_eq!(
        got,
        json!([201, "yes", "application/json", {"items": [1, 2]}])
    );
    // The second rule fails the rest of /route/api/.
    assert_eq!(
        fetch_text(&engine, "/route/api/other").await,
        "failed: Failed to fetch"
    );
    // Other requests go on.
    assert!(
        fetch_text(&engine, "/route/index.html")
            .await
            .contains("<h1>Routes</h1>")
    );

    let (text, data) = routes(&engine, false);
    assert_eq!(
        text,
        "r1 **/route/api/items → fulfill 201 application/json 15B (1 hit)\n\
         r2 /route/api/ → abort blockedbyclient (1 hit)"
    );
    assert_eq!(data.routes.len(), 2);

    // The journal has every request, with the rule that handled it.
    let (text, data) = routes(&engine, true);
    let log = data.log.unwrap();
    let items = log.iter().find(|e| e.url.ends_with("/api/items")).unwrap();
    assert_eq!(items.route.as_deref(), Some("r1"));
    assert_eq!(items.outcome, "fulfill 201");
    // Chrome's `Fetch` domain calls a `fetch()` an XHR.
    assert_eq!(items.resource_type, "xhr");
    assert_eq!(items.tab.as_deref(), Some("t1"));
    let page = log.iter().find(|e| e.url.ends_with("/index.html")).unwrap();
    assert_eq!(page.route, None);
    assert!(text.contains(" GET "), "{text}");
    assert!(
        text.contains("/route/api/other xhr → r2 abort blockedbyclient"),
        "{text}"
    );
}

#[tokio::test]
async fn continue_changed_delayed_and_patched() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "route-continue").await;
    goto(&engine, &site.url("/route/index.html")).await;

    let mut slow = rule("/echo/slow", continue_with(&[], &[], None));
    slow.delay_ms = 600;
    add(
        &engine,
        vec![
            rule(
                "/echo/headers",
                continue_with(
                    &[("X-Added", "1"), ("X-Page", "replaced")],
                    &["x-drop"],
                    None,
                ),
            ),
            rule(
                "**/route/api/moved",
                continue_with(&[], &[], Some(site.url("/echo/rewritten"))),
            ),
            slow,
            rule(
                "/echo/patched",
                RouteAction::PatchJson {
                    patch: json!({"method": "PATCHED", "headers": null, "extra": [1]}),
                },
            ),
            rule(
                "/route/index.html?patch",
                RouteAction::PatchJson {
                    patch: json!({"a": 1}),
                },
            ),
        ],
    )
    .await;

    // Headers set, replaced and removed on the way to the server.
    let echoed = eval(
        &engine,
        "fetch('/echo/headers', {headers: {'X-Page': 'mine', 'X-Drop': 'gone'}}).then((r) => r.json())",
    )
    .await;
    let headers = &echoed["headers"];
    assert_eq!(headers["x-added"], "1");
    assert_eq!(headers["x-page"], "replaced");
    assert_eq!(headers.get("x-drop"), None, "{headers}");

    // A rewritten URL goes to another path; the page does not see it.
    let echoed = eval(
        &engine,
        "fetch('/route/api/moved').then(async (r) => [r.url, await r.json()])",
    )
    .await;
    assert!(echoed[0].as_str().unwrap().ends_with("/route/api/moved"));
    assert_eq!(echoed[1]["path"], "/echo/rewritten");

    // A delay holds the request.
    let took = eval(
        &engine,
        "(async () => { const t = performance.now(); await (await fetch('/echo/slow')).json(); \
         return performance.now() - t; })()",
    )
    .await;
    assert!(took.as_f64().unwrap() >= 550.0, "{took}");

    // The real response, with the patch merged in.
    let patched = eval(&engine, "fetch('/echo/patched?q=1').then((r) => r.json())").await;
    assert_eq!(patched["method"], "PATCHED");
    assert_eq!(patched["path"], "/echo/patched");
    assert_eq!(patched["query"], "q=1");
    assert_eq!(patched["extra"], json!([1]));
    assert_eq!(patched.get("headers"), None);

    // A response that is no JSON goes on as it is, and the journal says so.
    assert!(
        fetch_text(&engine, "/route/index.html?patch")
            .await
            .contains("<h1>Routes</h1>")
    );
    let (_, data) = routes(&engine, true);
    let log = data.log.unwrap();
    let html = log.iter().find(|e| e.url.ends_with("?patch")).unwrap();
    assert_eq!(html.outcome, "patch json (not JSON; not patched)");
}

#[tokio::test]
async fn times_and_unroute() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "route-times").await;
    goto(&engine, &site.url("/route/index.html")).await;

    let mut once = rule("/echo/once", text("mocked"));
    once.times = Some(1);
    let added = add(
        &engine,
        vec![once, rule("/echo/a", text("a")), rule("/echo/b", text("b"))],
    )
    .await;
    assert_eq!(added.added[0].left, Some(1));
    assert_eq!(fetch_text(&engine, "/echo/once").await, "mocked");
    // Spent: the server answers now.
    assert!(
        fetch_text(&engine, "/echo/once")
            .await
            .contains("\"path\":\"/echo/once\"")
    );
    let (_, data) = routes(&engine, false);
    let ids: Vec<&str> = data.routes.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["r2", "r3"]);

    let removed = unroute(&engine, &["r2"], false).await;
    assert_eq!(
        (removed.removed, removed.active),
        (vec!["r2".to_owned()], 1)
    );
    assert!(fetch_text(&engine, "/echo/a").await.contains("\"path\""));
    assert_eq!(fetch_text(&engine, "/echo/b").await, "b");
    let missing = engine
        .unroute(UnrouteRequest {
            ids: vec!["r2".into()],
            all: false,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_err();
    assert_eq!(missing.message, "there is no route r2");

    // Without routes, nothing is intercepted: the journal stops growing.
    let removed = unroute(&engine, &[], true).await;
    assert_eq!(
        (removed.removed, removed.active),
        (vec!["r3".to_owned()], 0)
    );
    let (_, before) = routes(&engine, true);
    assert!(fetch_text(&engine, "/echo/b").await.contains("\"path\""));
    let (_, after) = routes(&engine, true);
    assert_eq!(before.log.unwrap().len(), after.log.unwrap().len());
}

/// Rules apply before a page's first script runs: added before the
/// browser starts, in a popup opened later, and in an out-of-process
/// iframe.
#[tokio::test]
async fn routes_apply_to_first_scripts_popups_and_frames() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = Engine::new("route-first", None, None);
    // Rules belong to the session: they wait for its browser.
    add(
        &engine,
        vec![
            rule("/route/api/first", text("mocked first")),
            rule("/route/api/frame", text("mocked frame")),
        ],
    )
    .await;
    open(&engine, &path, None).await;

    goto(&engine, &site.url("/route/inline.html")).await;
    eventually(
        &engine,
        eval_request("document.title"),
        &json!("mocked first"),
    )
    .await;

    // A popup's inline script.
    eval(&engine, "window.open('/route/inline.html'); true").await;
    let start = Instant::now();
    loop {
        let out = engine
            .tabs(TabsRequest {
                timeout_ms: TIMEOUT_MS,
                max_chars: 0,
            })
            .await
            .unwrap();
        let tabs: TabsData = serde_json::from_value(out.data).unwrap();
        if tabs
            .tabs
            .iter()
            .any(|t| t.id != "t1" && t.title == "mocked first")
        {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "{:#?}",
            tabs.tabs
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // An iframe of the other origin (its own process and session).
    goto(&engine, &site.url("/route/frame.html")).await;
    let mut in_frame = eval_request("document.getElementById('out').textContent");
    in_frame.frame = Some("child".into());
    eventually(&engine, in_frame, &json!("mocked frame")).await;
    let (_, data) = routes(&engine, false);
    assert!(
        data.routes.iter().all(|r| r.hits >= 1),
        "{:#?}",
        data.routes
    );

    // Sessions set up with the rules stop intercepting once they are gone.
    unroute(&engine, &[], true).await;
    let (_, before) = routes(&engine, true);
    goto(&engine, &site.url("/route/inline.html")).await;
    eventually(
        &engine,
        eval_request("document.title"),
        &json!("no fixture at /route/api/first"),
    )
    .await;
    let (_, after) = routes(&engine, true);
    assert_eq!(before.log.unwrap().len(), after.log.unwrap().len());
}

#[tokio::test]
async fn service_workers_are_bypassed_while_routes_exist() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "route-sw").await;
    goto(&engine, &site.url("/route/sw.html")).await;
    assert_eq!(eval(&engine, "window.controlled").await, json!(true));
    assert_eq!(
        fetch_text(&engine, "/route/api/sw").await,
        "from service worker"
    );

    add(&engine, vec![rule("/route/api/sw", text("from the route"))]).await;
    assert_eq!(fetch_text(&engine, "/route/api/sw").await, "from the route");

    unroute(&engine, &[], true).await;
    assert_eq!(
        fetch_text(&engine, "/route/api/sw").await,
        "from service worker"
    );

    // `open --bypass-service-worker` without routes: the server answers.
    engine
        .open(OpenRequest {
            bypass_service_worker: Some(true),
            timeout_ms: TIMEOUT_MS,
            ..OpenRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(
        fetch_text(&engine, "/route/api/sw").await,
        "no fixture at /route/api/sw\n"
    );
}

/// A mocked API on another origin answers CORS preflights and allows the
/// page's origin.
#[tokio::test]
async fn cross_origin_mocks_pass_cors() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "route-cors").await;
    goto(&engine, &site.url("/route/index.html")).await;
    let mut post = rule(
        &format!("{}/api/cart", site.b()),
        RouteAction::Fulfill {
            status: 200,
            headers: Vec::new(),
            body: RouteBody::Json {
                value: json!({"ok": true}),
            },
        },
    );
    post.methods = vec!["POST".into()];
    add(&engine, vec![post]).await;
    let script = format!(
        "fetch('{}/api/cart', {{method: 'POST', credentials: 'include', \
         headers: {{'Content-Type': 'application/json', 'X-Token': 't'}}, body: '{{}}'}}) \
         .then((r) => r.json(), (e) => 'failed: ' + e.message)",
        site.b()
    );
    assert_eq!(eval(&engine, &script).await, json!({"ok": true}));
    let (_, data) = routes(&engine, true);
    let log = data.log.unwrap();
    let outcomes: Vec<&str> = log
        .iter()
        .filter(|e| e.url.ends_with("/api/cart"))
        .map(|e| e.outcome.as_str())
        .collect();
    assert_eq!(outcomes, ["preflight allowed", "fulfill 200"]);
}

#[tokio::test]
async fn resource_types_narrow_a_rule() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "route-types").await;
    let loaded = "new Promise((ok) => { const img = document.getElementById('pic'); \
                  if (img.complete) ok(img.naturalWidth); else img.onload = img.onerror = () => ok(img.naturalWidth); })";
    goto(&engine, &site.url("/route/index.html")).await;
    assert_eq!(eval(&engine, loaded).await, json!(20));

    let mut images = rule(
        "**",
        RouteAction::Abort {
            reason: "blockedbyclient".into(),
        },
    );
    images.label = Some("images".into());
    images.resource_types = vec!["image".into()];
    add(&engine, vec![images]).await;
    goto(&engine, &site.url("/route/index.html")).await;
    assert_eq!(eval(&engine, loaded).await, json!(0));
    // The page itself is no image.
    let (text, _) = routes(&engine, false);
    assert_eq!(text, "r1 images → abort blockedbyclient (1 hit)");
}

/// Routes and `--http-credentials` share `Fetch`: removing the routes
/// keeps answering challenges.
#[tokio::test]
async fn credentials_still_work_with_and_after_routes() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = Engine::new("route-auth", None, None);
    open(
        &engine,
        &path,
        Some(HttpCredentials {
            username: AUTH_USER.into(),
            password: AUTH_PASSWORD.into(),
            origin: None,
        }),
    )
    .await;
    add(&engine, vec![rule("/route/api/", text("mocked"))]).await;
    let page = goto(&engine, &site.url("/auth/")).await;
    assert_eq!((page.status, page.title.as_str()), (Some(200), "Members"));
    assert_eq!(fetch_text(&engine, "/route/api/x").await, "mocked");
    unroute(&engine, &[], true).await;
    let page = goto(&engine, &site.url("/auth/index.html")).await;
    assert_eq!((page.status, page.title.as_str()), (Some(200), "Members"));
}

/// Plan 07's goal: routing adds under 1 ms per request at p95 with 20
/// rules. The page times 300 sequential fetches of `/echo` without routes
/// and with 20 rules that match other URLs, so every request is paused,
/// checked against all 20 and sent on.
///
/// Ignored: benchmarks run on a quiet machine after all plans are in. Run
/// it with `CHROME_PATH=… cargo test -p chrome-cdp-engine --release --test route --
/// --ignored`.
#[tokio::test]
#[ignore = "benchmark; run after all plans are implemented"]
async fn overhead_with_20_rules() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "route-bench").await;
    goto(&engine, &site.url("/route/index.html")).await;
    let timings = "(async () => { const t = []; for (let i = 0; i < 300; i++) { \
                   const s = performance.now(); await (await fetch('/echo?i=' + i)).text(); \
                   t.push(performance.now() - s); } t.sort((a, b) => a - b); \
                   return [t[150], t[285]]; })()";
    // Warm up the connection and the page's code.
    eval(&engine, timings).await;
    let without = eval(&engine, timings).await;
    let rules = (0..20)
        .map(|i| rule(&format!("**/api/v{i}/**"), text("mocked")))
        .collect();
    add(&engine, rules).await;
    let with = eval(&engine, timings).await;
    let ms = |v: &Value, i: usize| v[i].as_f64().unwrap();
    let overhead = ms(&with, 1) - ms(&without, 1);
    eprintln!(
        "p50 {:.2} → {:.2} ms, p95 {:.2} → {:.2} ms: {overhead:.2} ms a request at p95",
        ms(&without, 0),
        ms(&with, 0),
        ms(&without, 1),
        ms(&with, 1)
    );
    assert!(overhead < 1.0, "routing added {overhead:.2} ms at p95");
}
