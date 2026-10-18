//! Browser contexts (`context new|list|close` and `--context`; plan 11,
//! milestone 11.4) against a real Chrome (`CHROME_PATH`; skipped without
//! it unless `CHROME_CDP_REQUIRE_CHROME` is set): each context has its own
//! tabs, cookies, storage, routes, mock sets and state, init scripts and
//! emulation.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use browser::Host;
use engine::{
    CdpRequest, ContextCloseRequest, ContextListRequest, ContextNewRequest, CookieFilter,
    CookiesOp, CookiesRequest, EmulateRequest, EmulationChange, Engine, EvalRequest, EvalWorld,
    InitScriptAddRequest, MockStateRequest, MockUseRequest, NavigateRequest, Navigation,
    OpenRequest, PermissionsOp, PermissionsRequest, RouteAction, RouteRequest, RouteSpec,
    TabNewRequest, TabsRequest, Target, WaitUntil, in_context,
};
use fixtures::Fixtures;
use protocol::ErrorKind;
use protocol::schema::{
    ContextCloseData, ContextNewData, ContextsData, CookiesData, EvalData, MockStateData,
    PermissionsData, TabsData, Viewport,
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

/// Runs `fut` as a call with `--context context` would.
async fn within<F: Future>(context: &str, fut: F) -> F::Output {
    in_context(Some(context.to_owned()), Box::pin(fut)).await
}

async fn new_context(engine: &Engine, url: &str) -> ContextNewData {
    let out = engine
        .context_new(ContextNewRequest {
            url: Some(url.to_owned()),
            base_url: None,
            timeout_ms: TIMEOUT_MS,
            setup: Vec::new(),
            ahead: false,
        })
        .await
        .unwrap_or_else(|e| panic!("context new: {}", e.to_text()));
    serde_json::from_value(out.data).unwrap()
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

/// `script`'s value in the active tab (of the call's context).
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

/// The status of a fetch, or `failed`.
async fn status(engine: &Engine, url: &str) -> Value {
    let script = format!("fetch({url:?}).then((r) => r.status, () => 'failed')");
    eval(engine, &script).await
}

async fn use_sets(engine: &Engine, names: &[&str]) {
    let dir = fixtures::site_dir().join("mock/mocks");
    engine
        .mock_use(MockUseRequest {
            sets: names
                .iter()
                .map(|n| mock::Source {
                    name: (*n).to_owned(),
                    file: dir.join(format!("{n}.toml")),
                    base: dir.clone(),
                })
                .collect(),
            add: false,
            seed: 0,
            test: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("mock use: {}", e.to_text()));
}

fn state(engine: &Engine) -> MockStateData {
    let out = engine
        .mock_state(&MockStateRequest {
            name: None,
            set: Vec::new(),
            max_chars: 0,
        })
        .unwrap();
    serde_json::from_value(out.data).unwrap()
}

async fn tabs(engine: &Engine) -> TabsData {
    let out = engine
        .tabs(TabsRequest {
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap();
    serde_json::from_value(out.data).unwrap()
}

/// `cookies --url url`, as `name=value`s.
async fn cookies_for(engine: &Engine, url: &str) -> Vec<String> {
    let out = engine
        .cookies(CookiesRequest {
            op: CookiesOp::List {
                filter: CookieFilter {
                    url: Some(url.to_owned()),
                    ..CookieFilter::default()
                },
            },
            max_chars: 0,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("cookies --url: {}", e.to_text()));
    let data: CookiesData = serde_json::from_value(out.data).unwrap();
    data.cookies
        .into_iter()
        .map(|c| format!("{}={}", c.name, c.value))
        .collect()
}

#[tokio::test]
async fn contexts_keep_cookies_mocks_and_routes_apart() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "context-apart").await;
    let page = site.url("/mock/index.html");
    goto(&engine, &page).await;

    let c1 = new_context(&engine, &page).await;
    assert_eq!(c1.context.id, "c1");
    assert_eq!(c1.tab.context.as_deref(), Some("c1"));
    assert!(c1.tab.active);
    let c2 = new_context(&engine, &page).await;
    assert_eq!(c2.context.id, "c2");

    // Mock sets: c1's answer in c1 only.
    within("c1", use_sets(&engine, &["base"])).await;
    assert_eq!(
        within("c1", status(&engine, "/mock/api/products")).await,
        json!(200)
    );
    assert_ne!(
        within("c2", status(&engine, "/mock/api/products")).await,
        json!(200)
    );
    assert_ne!(status(&engine, "/mock/api/products").await, json!(200));

    // Mock state: each context's sets keep their own.
    within("c2", use_sets(&engine, &["stateful"])).await;
    within("c1", use_sets(&engine, &["stateful"])).await;
    let login = "fetch('/mock/api/login', {method: 'POST', headers: {'content-type': \
                 'application/json'}, body: '{\"email\":\"a@example.com\"}'}).then((r) => r.status)";
    assert_eq!(within("c1", eval(&engine, login)).await, json!(200));
    assert_eq!(
        within("c1", async { state(&engine) }).await.values["logged_in"],
        json!(true)
    );
    assert_eq!(
        within("c2", async { state(&engine) }).await.values["logged_in"],
        json!(false)
    );

    // Routes: c2's rule sees c2's requests only.
    within(
        "c2",
        engine.route(RouteRequest {
            rules: vec![RouteSpec {
                pattern: "**/items*".into(),
                label: None,
                methods: Vec::new(),
                resource_types: Vec::new(),
                times: None,
                delay_ms: 0,
                context: None,
                action: RouteAction::Abort {
                    reason: "failed".into(),
                },
            }],
            timeout_ms: TIMEOUT_MS,
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        within("c2", status(&engine, "/items?n=1")).await,
        json!("failed")
    );
    assert_eq!(
        within("c1", status(&engine, "/items?n=1")).await,
        json!(200)
    );
    assert_eq!(status(&engine, "/items?n=1").await, json!(200));

    // Cookies and storage are Chrome's per context.
    within(
        "c1",
        eval(&engine, "document.cookie = 'who=c1'; localStorage.x = 'c1'"),
    )
    .await;
    assert_eq!(
        within("c1", eval(&engine, "document.cookie")).await,
        json!("who=c1")
    );
    assert_eq!(
        within("c2", eval(&engine, "document.cookie")).await,
        json!("")
    );
    assert_eq!(eval(&engine, "localStorage.x ?? null").await, Value::Null);

    // `cookies --url` reads the context's cookies, not the active tab's.
    within("c2", eval(&engine, "document.cookie = 'who=c2'")).await;
    assert_eq!(within("c1", cookies_for(&engine, &page)).await, ["who=c1"]);
    assert_eq!(within("c2", cookies_for(&engine, &page)).await, ["who=c2"]);
    assert!(cookies_for(&engine, &page).await.is_empty());

    // The list says what each context has.
    let out = engine
        .context_list(ContextListRequest {
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap();
    let list: ContextsData = serde_json::from_value(out.data).unwrap();
    assert_eq!(list.contexts.len(), 2);
    assert_eq!(list.contexts[0].mocks, ["stateful"]);
    assert_eq!(list.contexts[1].routes, 1);
    assert!(out.text.contains("c1  tabs *t"), "{}", out.text);
}

#[tokio::test]
async fn a_context_has_its_own_tabs_emulation_and_init_scripts() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "context-tabs").await;
    let page = site.url("/tabs/popup-target.html");
    goto(&engine, &page).await;
    let c1 = new_context(&engine, &page).await;
    let first = c1.tab.id.clone();

    // A second tab in c1 becomes c1's active tab, not the session's.
    let out = within(
        "c1",
        engine.tab_new(TabNewRequest {
            url: Some(page.clone()),
            base_url: None,
            timeout_ms: TIMEOUT_MS,
        }),
    )
    .await
    .unwrap();
    let second = out.data["tab"]["id"].as_str().unwrap().to_owned();
    let in_c1 = within("c1", tabs(&engine)).await;
    let ids: Vec<&str> = in_c1.tabs.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, [first.as_str(), second.as_str()]);
    assert!(in_c1.tabs[1].active);
    let all = tabs(&engine).await;
    assert_eq!(all.tabs.len(), 3);
    assert!(all.tabs[0].active && all.tabs[0].context.is_none());

    // --tab of another context is refused.
    let err = within(
        "c1",
        engine.eval(EvalRequest {
            script: "1".into(),
            locator: None,
            frame: None,
            world: EvalWorld::Main,
            tab: Some("t1".into()),
            strict_refs: false,
            testid_attribute: None,
            sourcemaps: false,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert!(err.message.contains("not in c1"), "{}", err.message);

    // Emulation: c1's viewport, the session's elsewhere.
    within(
        "c1",
        engine.emulate(EmulateRequest {
            change: EmulationChange::Viewport {
                viewport: Some(Viewport {
                    width: 390,
                    height: 844,
                    dpr: 1.0,
                    mobile: false,
                    touch: false,
                    screen_width: None,
                    screen_height: None,
                }),
            },
            timeout_ms: TIMEOUT_MS,
        }),
    )
    .await
    .unwrap();
    assert_eq!(within("c1", eval(&engine, "innerWidth")).await, json!(390));
    assert_eq!(eval(&engine, "innerWidth").await, json!(1280));

    // Init scripts: c1's run in c1's documents only.
    within(
        "c1",
        engine.init_script_add(InitScriptAddRequest {
            source: "window.__ctx = 'c1'".into(),
            file: None,
            world: EvalWorld::Main,
            now: true,
            timeout_ms: TIMEOUT_MS,
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        within("c1", eval(&engine, "window.__ctx ?? null")).await,
        json!("c1")
    );
    assert_eq!(eval(&engine, "window.__ctx ?? null").await, Value::Null);

    // Closing c1 closes its tabs and forgets it.
    let out = engine
        .context_close(ContextCloseRequest {
            ids: vec!["c1".into()],
            all: false,
            timeout_ms: TIMEOUT_MS,
            background: false,
        })
        .await
        .unwrap();
    let closed: ContextCloseData = serde_json::from_value(out.data).unwrap();
    assert_eq!(closed.closed, ["c1"]);
    assert_eq!(closed.tabs, [first, second]);
    assert_eq!(tabs(&engine).await.tabs.len(), 1);
    let err = within(
        "c1",
        engine.tabs(TabsRequest {
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    // Ids are never reused.
    assert_eq!(new_context(&engine, &page).await.context.id, "c2");
}

/// `test`'s contexts (plan 22.2): made ahead while the last one is used,
/// out of sight until taken, and disposed of with `context close --all`.
#[tokio::test]
async fn contexts_made_ahead_stay_out_of_sight() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "context-ahead").await;
    let ahead = || async {
        let out = engine
            .context_new(ContextNewRequest {
                url: None,
                base_url: None,
                timeout_ms: TIMEOUT_MS,
                setup: Vec::new(),
                ahead: true,
            })
            .await
            .unwrap_or_else(|e| panic!("context new: {}", e.to_text()));
        // As the daemon does once the context has its settings.
        engine.make_ahead(TIMEOUT_MS);
        serde_json::from_value::<ContextNewData>(out.data).unwrap()
    };
    // Chrome's pages, the ones made ahead too.
    let pages = || async {
        let out = engine
            .cdp(CdpRequest {
                all_sessions: false,
                method: Some("Target.getTargets".into()),
                params: serde_json::Map::new(),
                target: Target::Browser,
                tab: None,
                frame: None,
                listen: Vec::new(),
                count: 0,
                timeout_ms: TIMEOUT_MS,
                max_chars: 0,
            })
            .await
            .unwrap();
        let infos = out.data["targetInfos"].as_array().unwrap().iter();
        infos.filter(|t| t["type"] == "page").count()
    };
    let settled = |n: usize| async move {
        let start = std::time::Instant::now();
        loop {
            let now = pages().await;
            if now == n {
                break;
            }
            assert!(start.elapsed().as_secs() < 15, "{now} pages, not {n}");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        // And it stays so.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert_eq!(pages().await, n);
    };
    let session = pages().await;
    let shown = tabs(&engine).await.tabs.len();

    assert_eq!(ahead().await.context.id, "c1");
    // c1's tab, and one of the next context's.
    settled(session + 2).await;
    let listed = || async {
        let out = engine
            .context_list(ContextListRequest {
                timeout_ms: TIMEOUT_MS,
                max_chars: 0,
            })
            .await
            .unwrap();
        let data: ContextsData = serde_json::from_value(out.data).unwrap();
        data.contexts.into_iter().map(|c| c.id).collect::<Vec<_>>()
    };
    assert_eq!(listed().await, ["c1"]);
    // The session's tabs and c1's, not the one made ahead.
    assert_eq!(tabs(&engine).await.tabs.len(), shown + 1);
    assert_eq!(
        engine.status().tabs,
        Some(u32::try_from(shown + 1).unwrap())
    );

    // c2 is the one made ahead (one more is made, not two).
    let c2 = ahead().await;
    assert_eq!(c2.context.id, "c2");
    assert_eq!(c2.tab.context.as_deref(), Some("c2"));
    settled(session + 3).await;
    assert_eq!(listed().await, ["c1", "c2"]);

    // Closed in the background: gone for every call at once, and
    // disposed of in Chrome when the next context is made ahead.
    let out = engine
        .context_close(ContextCloseRequest {
            ids: vec!["c2".into()],
            all: false,
            timeout_ms: TIMEOUT_MS,
            background: true,
        })
        .await
        .unwrap();
    let closed: ContextCloseData = serde_json::from_value(out.data).unwrap();
    assert_eq!(closed.closed, ["c2"]);
    assert_eq!(listed().await, ["c1"]);
    assert_eq!(
        engine.status().tabs,
        Some(u32::try_from(shown + 1).unwrap())
    );
    settled(session + 3).await;
    assert_eq!(ahead().await.context.id, "c3");
    // c1, c3 and the next one made ahead; c2 is gone.
    settled(session + 3).await;

    // --all disposes of the ones made ahead too.
    engine
        .context_close(ContextCloseRequest {
            ids: Vec::new(),
            all: true,
            timeout_ms: TIMEOUT_MS,
            background: false,
        })
        .await
        .unwrap();
    assert!(listed().await.is_empty());
    settled(session).await;
}

async fn permissions(engine: &Engine, op: PermissionsOp, names: &[&str]) -> PermissionsData {
    let out = engine
        .permissions(PermissionsRequest {
            op,
            names: names.iter().map(|&n| n.to_owned()).collect(),
            origin: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("permissions: {}", e.to_text()));
    serde_json::from_value(out.data).unwrap()
}

/// `permissions` applies in the call's context only (plan 13.3), and so
/// does `geo`'s grant.
#[tokio::test]
async fn a_context_has_its_own_permissions() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "context-permissions").await;
    let page = site.url("/emulate/permissions.html");
    goto(&engine, &page).await;
    new_context(&engine, &page).await;
    new_context(&engine, &page).await;
    let state = "state('notifications')";

    let granted = within(
        "c1",
        permissions(&engine, PermissionsOp::Grant, &["notifications"]),
    )
    .await;
    assert_eq!(granted.permissions.len(), 1);
    assert_eq!(within("c1", eval(&engine, state)).await, json!("granted"));
    assert_eq!(within("c2", eval(&engine, state)).await, json!("prompt"));
    assert_eq!(eval(&engine, state).await, json!("prompt"));
    let listed = within("c2", permissions(&engine, PermissionsOp::List, &[])).await;
    assert!(listed.permissions.is_empty(), "{listed:?}");
    assert!(
        permissions(&engine, PermissionsOp::List, &[])
            .await
            .permissions
            .is_empty()
    );

    // The session's own grant stays out of the contexts.
    permissions(&engine, PermissionsOp::Deny, &["notifications"]).await;
    assert_eq!(eval(&engine, state).await, json!("denied"));
    assert_eq!(within("c1", eval(&engine, state)).await, json!("granted"));
    assert_eq!(within("c2", eval(&engine, state)).await, json!("prompt"));

    // geo grants the position in its context.
    within(
        "c2",
        engine.emulate(EmulateRequest {
            change: EmulationChange::Geolocation {
                geolocation: Some(protocol::schema::Geolocation {
                    latitude: 59.91,
                    longitude: 10.75,
                    accuracy: 10.0,
                }),
            },
            timeout_ms: TIMEOUT_MS,
        }),
    )
    .await
    .unwrap();
    let geo = "state('geolocation')";
    assert_eq!(within("c2", eval(&engine, geo)).await, json!("granted"));
    assert_eq!(within("c1", eval(&engine, geo)).await, json!("prompt"));
    let listed = within("c2", permissions(&engine, PermissionsOp::List, &[])).await;
    assert_eq!(listed.permissions[0].name, "geolocation");

    // Reset in c1 leaves the others alone.
    within("c1", permissions(&engine, PermissionsOp::Reset, &[])).await;
    assert_eq!(within("c1", eval(&engine, state)).await, json!("prompt"));
    assert_eq!(eval(&engine, state).await, json!("denied"));
    assert_eq!(within("c2", eval(&engine, geo)).await, json!("granted"));
}
