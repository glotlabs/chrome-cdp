//! Cookies, storage and storage state files (`cookies`, `storage`, `state
//! save`, `state load`, `open --state`; plan 07, milestone 7.4) against a
//! real Chrome (`CHROME_PATH`; skipped without it unless
//! `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture site's `state/`
//! pages.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use browser::Host;
use engine::{
    CookieFilter, CookieSpec, CookiesOp, CookiesRequest, Engine, EvalRequest, EvalWorld,
    InitScriptListRequest, NavigateRequest, Navigation, OpenRequest, StateLoadRequest,
    StateSaveRequest, StorageArea, StorageOp, StorageRequest, StorageState, WaitUntil,
};
use fixtures::Fixtures;
use protocol::schema::{
    CookieSetData, CookiesClearData, CookiesData, EvalData, InitScriptsData, StateData, StorageData,
};
use protocol::{ErrorInfo, ErrorKind};
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

fn open_request(path: &Path) -> OpenRequest {
    OpenRequest {
        chrome: Some(browser::discover(Some(path), &Host::current()).unwrap()),
        timeout_ms: TIMEOUT_MS,
        ..OpenRequest::default()
    }
}

async fn start(path: &Path, name: &str) -> Arc<Engine> {
    let engine = Engine::new(name, None, None);
    engine.open(open_request(path)).await.unwrap();
    engine
}

async fn navigate(engine: &Engine, navigation: Navigation) {
    let what = format!("{navigation:?}");
    engine
        .navigate(NavigateRequest {
            navigation,
            wait: WaitUntil::Settle,
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("{what}: {}", e.to_text()));
}

async fn goto(engine: &Engine, url: &str) {
    navigate(
        engine,
        Navigation::Goto {
            url: url.to_owned(),
            base_url: None,
        },
    )
    .await;
}

async fn reload(engine: &Engine) {
    navigate(engine, Navigation::Reload { hard: false }).await;
}

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

async fn cookies(engine: &Engine, op: CookiesOp) -> Result<protocol::Output, ErrorInfo> {
    engine
        .cookies(CookiesRequest {
            op,
            max_chars: 0,
            timeout_ms: TIMEOUT_MS,
        })
        .await
}

async fn list(engine: &Engine, filter: CookieFilter) -> CookiesData {
    let out = cookies(engine, CookiesOp::List { filter }).await.unwrap();
    serde_json::from_value(out.data).unwrap()
}

async fn set(engine: &Engine, cookie: CookieSpec) -> Result<CookieSetData, ErrorInfo> {
    let out = cookies(engine, CookiesOp::Set { cookie }).await?;
    Ok(serde_json::from_value(out.data).unwrap())
}

async fn clear(engine: &Engine, filter: CookieFilter) -> usize {
    let out = cookies(engine, CookiesOp::Clear { filter }).await.unwrap();
    let data: CookiesClearData = serde_json::from_value(out.data).unwrap();
    data.removed
}

async fn storage(
    engine: &Engine,
    area: StorageArea,
    op: StorageOp,
    origin: Option<&str>,
) -> Result<(StorageData, String), ErrorInfo> {
    let out = engine
        .storage(StorageRequest {
            area,
            op,
            origin: origin.map(str::to_owned),
            tab: None,
            max_chars: 0,
            timeout_ms: TIMEOUT_MS,
        })
        .await?;
    Ok((serde_json::from_value(out.data).unwrap(), out.text))
}

fn names(data: &CookiesData) -> Vec<&str> {
    data.cookies.iter().map(|c| c.name.as_str()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn cookies_are_listed_set_and_cleared() {
    let Some(path) = chrome_path() else { return };
    let f = Fixtures::shared();
    let engine = start(&path, "state-cookies").await;
    goto(&engine, &f.url("/state/login.html")).await;
    assert!(
        list(&engine, CookieFilter::default())
            .await
            .cookies
            .is_empty()
    );

    // Without --url or --domain, the cookie is the active tab's (host only).
    let set_one = set(
        &engine,
        CookieSpec {
            name: "theme".into(),
            value: "dark".into(),
            ..CookieSpec::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(set_one.cookie.domain, "127.0.0.1");
    assert_eq!(set_one.cookie.path, "/");
    assert_eq!(set_one.cookie.expires, None);
    assert_eq!(eval(&engine, "document.cookie").await, json!("theme=dark"));

    // A --domain with a leading dot makes a cookie for subdomains too;
    // flags and expiry stick.
    let expires = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
        + 3600.0;
    let set_two = set(
        &engine,
        CookieSpec {
            name: "sid".into(),
            value: "s-1".into(),
            domain: Some(".example.com".into()),
            path: Some("/app".into()),
            http_only: true,
            same_site: Some("strict".into()),
            expires: Some(expires.floor()),
            ..CookieSpec::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(set_two.cookie.domain, ".example.com");
    assert!(set_two.cookie.http_only);
    assert_eq!(set_two.cookie.same_site.as_deref(), Some("Strict"));
    assert_eq!(set_two.cookie.expires, Some(expires.floor()));
    set(
        &engine,
        CookieSpec {
            name: "lang".into(),
            value: "nb".into(),
            url: Some(f.url_b("/")),
            ..CookieSpec::default()
        },
    )
    .await
    .unwrap();

    let all = list(&engine, CookieFilter::default()).await;
    assert_eq!(names(&all), vec!["theme", "sid", "lang"]);
    let domain = CookieFilter {
        domain: Some("example.com".into()),
        ..CookieFilter::default()
    };
    assert_eq!(names(&list(&engine, domain.clone()).await), vec!["sid"]);
    // --url: what a request there carries (subdomain and path count).
    let url = |u: &str| CookieFilter {
        url: Some(u.to_owned()),
        ..CookieFilter::default()
    };
    let for_url = |u: &'static str| list(&engine, url(u));
    assert_eq!(
        names(&for_url("http://app.example.com/app/x").await),
        vec!["sid"]
    );
    assert!(for_url("http://app.example.com/").await.cookies.is_empty());
    assert_eq!(
        names(&list(&engine, url(&f.url_b("/"))).await),
        vec!["lang"]
    );

    // Chrome refuses SameSite=None without Secure; the hint says so.
    let err = set(
        &engine,
        CookieSpec {
            name: "x".into(),
            value: "1".into(),
            same_site: Some("none".into()),
            ..CookieSpec::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadArgs, "{}", err.to_text());
    assert!(
        err.to_text().contains("SameSite=None needs --secure"),
        "{}",
        err.to_text()
    );

    let named = CookieFilter {
        name: Some("sid".into()),
        ..CookieFilter::default()
    };
    assert_eq!(clear(&engine, named).await, 1);
    assert!(list(&engine, domain).await.cookies.is_empty());
    assert_eq!(clear(&engine, CookieFilter::default()).await, 2);
    assert!(
        list(&engine, CookieFilter::default())
            .await
            .cookies
            .is_empty()
    );
    engine.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn storage_is_read_and_changed_in_a_frame_of_its_origin() {
    let Some(path) = chrome_path() else { return };
    let f = Fixtures::shared();
    let engine = start(&path, "state-storage").await;

    // A page with no origin keeps no storage.
    let err = storage(&engine, StorageArea::Local, StorageOp::List, None)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound, "{}", err.to_text());

    goto(&engine, &f.url("/frames/cross-origin.html")).await;
    let set_op = |key: &str, value: &str| StorageOp::Set {
        key: key.into(),
        value: value.into(),
    };
    let (data, text) = storage(&engine, StorageArea::Local, set_op("b", "2"), None)
        .await
        .unwrap();
    assert_eq!(data.origin, f.a());
    assert_eq!(data.length, Some(1));
    assert!(
        text.starts_with("set b in localStorage of http://127.0.0.1:"),
        "{text}"
    );
    storage(&engine, StorageArea::Local, set_op("a", "1"), None)
        .await
        .unwrap();
    storage(&engine, StorageArea::Session, set_op("s", "x"), None)
        .await
        .unwrap();
    let (data, text) = storage(&engine, StorageArea::Local, StorageOp::List, None)
        .await
        .unwrap();
    let items: Vec<(&str, &str)> = data
        .items
        .iter()
        .map(|i| (i.name.as_str(), i.value.as_str()))
        .collect();
    assert_eq!(items, vec![("a", "1"), ("b", "2")]);
    assert!(text.contains(": 2 items\na  1\nb  2"), "{text}");
    // The page sees what `storage` set.
    assert_eq!(
        eval(
            &engine,
            "localStorage.getItem('a') + sessionStorage.getItem('s')"
        )
        .await,
        json!("1x")
    );

    let get = |key: &str| StorageOp::Get { key: key.into() };
    let (data, text) = storage(&engine, StorageArea::Session, get("s"), None)
        .await
        .unwrap();
    assert_eq!((data.value.as_deref(), text.as_str()), (Some("x"), "x"));
    let err = storage(&engine, StorageArea::Session, get("nope"), None)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert!(
        err.to_text().contains("storage session"),
        "{}",
        err.to_text()
    );

    // --origin: the cross-site iframe's own storage.
    let (data, _) = storage(
        &engine,
        StorageArea::Local,
        set_op("frame", "y"),
        Some(f.b()),
    )
    .await
    .unwrap();
    assert_eq!(data.origin, f.b());
    let (data, _) = storage(
        &engine,
        StorageArea::Local,
        StorageOp::List,
        Some(&f.url_b("/x")),
    )
    .await
    .unwrap();
    assert_eq!(data.items.len(), 1);
    let err = storage(
        &engine,
        StorageArea::Local,
        StorageOp::List,
        Some("https://example.com"),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert!(
        err.to_text().contains("goto https://example.com"),
        "{}",
        err.to_text()
    );

    let (data, text) = storage(
        &engine,
        StorageArea::Local,
        StorageOp::Remove { key: "a".into() },
        None,
    )
    .await
    .unwrap();
    assert_eq!(data.length, Some(1));
    assert!(text.starts_with("removed a from"), "{text}");
    let (_, text) = storage(&engine, StorageArea::Local, StorageOp::Clear, None)
        .await
        .unwrap();
    assert!(text.contains("(1 item removed)"), "{text}");
    assert_eq!(eval(&engine, "localStorage.length").await, json!(0));
    engine.close().await;
}

/// Logs in on the fixture: every kind of state the login page leaves.
async fn log_in(engine: &Engine, f: &Fixtures) {
    goto(engine, &f.url("/state/login.html")).await;
    // Not awaited: logIn() navigates away when it is done, and an eval
    // whose page is gone before it answers fails. The wait below sees it.
    eval(engine, "logIn(), null").await;
    engine
        .wait(engine::WaitRequest {
            condition: engine::WaitFor::Text {
                text: "Logged in as Ada".into(),
                within: None,
                gone: false,
            },
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap_or_else(|e| panic!("{}", e.to_text()));
}

#[tokio::test(flavor = "multi_thread")]
async fn state_survives_a_new_session() {
    let Some(path) = chrome_path() else { return };
    let f = Fixtures::shared();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("auth/state.json");

    let first = start(&path, "state-save").await;
    log_in(&first, f).await;
    let out = first
        .state_save(StateSaveRequest {
            path: file.clone(),
            indexed_db: true,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("{}", e.to_text()));
    assert!(
        out.warnings[0].contains("readable by you only"),
        "{:?}",
        out.warnings
    );
    let saved: StateData = serde_json::from_value(out.data).unwrap();
    assert_eq!(saved.cookies, 2);
    assert_eq!(saved.origins.len(), 1);
    assert_eq!(saved.origins[0].origin, f.a());
    assert_eq!(
        (
            saved.origins[0].local_storage,
            saved.origins[0].session_storage
        ),
        (1, 1)
    );
    assert_eq!(saved.origins[0].indexed_db, Some(1));
    first.close().await;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let text = std::fs::read_to_string(&file).unwrap();
    let json: Value = serde_json::from_str(&text).unwrap();
    let session = json["cookies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "session")
        .unwrap();
    assert_eq!(session["httpOnly"], true);
    assert_eq!(session["expires"], -1.0);
    assert_eq!(session["sameSite"], "Lax");
    let origin = &json["origins"][0];
    assert_eq!(
        origin["localStorage"],
        json!([{"name": "token", "value": "tok-ada-42"}])
    );
    assert_eq!(
        origin["sessionStorage"],
        json!([{"name": "draft", "value": "draft-7"}])
    );
    let db = &origin["indexedDB"][0];
    assert_eq!(
        (db["name"].clone(), db["version"].clone()),
        (json!("app"), json!(2))
    );
    let kv = db["stores"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "kv")
        .unwrap();
    // A Date and a Map are not JSON: Playwright's encoding, with the key.
    assert!(
        kv["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["keyEncoded"] == json!("saved")
                || r["key"] == json!("saved")
                    && r["valueEncoded"] == json!({"d": "2026-01-31T09:30:00.000Z"}))
    );

    // A new session, opened with the file: logged in on the first load,
    // with the storage there before the page's first script.
    let second = Engine::new("state-load", None, None);
    let state = StorageState::parse(&text).unwrap();
    let mut open = open_request(&path);
    open.url = Some(f.url("/state/app.html"));
    open.state = Some(StateLoadRequest {
        state,
        path: Some(file.clone()),
        timeout_ms: TIMEOUT_MS,
    });
    let opened = second
        .open(open)
        .await
        .unwrap_or_else(|e| panic!("{}", e.to_text()));
    let loaded = opened.data.state.unwrap();
    assert_eq!(loaded.cookies, 2);
    assert_eq!(loaded.origins[0].applied.as_deref(), Some("next-load"));
    assert_eq!(eval(&second, "window.checked").await, json!(true));
    assert_eq!(
        eval(&second, "window.first").await,
        json!({"token": "tok-ada-42", "draft": "draft-7"})
    );
    assert_eq!(
        eval(&second, "drafts()").await,
        json!({
            "version": 2,
            "draft": {"id": 1, "title": "Hello", "words": 2},
            "saved": "2026-01-31T09:30:00.000Z",
            "settings": {"theme": "dark"},
        })
    );
    // The seed did its work and is gone: the page's own changes stay, and
    // `init-script list` never showed it.
    eval(&second, "localStorage.setItem('token', 'changed')").await;
    reload(&second).await;
    assert_eq!(eval(&second, "window.first.token").await, json!("changed"));
    let scripts = second
        .init_script_list(&InitScriptListRequest { max_chars: 0 })
        .unwrap();
    let scripts: InitScriptsData = serde_json::from_value(scripts.data).unwrap();
    assert!(scripts.scripts.is_empty());
    second.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn state_load_gives_open_pages_their_storage_at_once() {
    let Some(path) = chrome_path() else { return };
    let f = Fixtures::shared();
    let engine = start(&path, "state-load-open").await;
    goto(&engine, &f.url("/state/app.html")).await;
    assert_eq!(eval(&engine, "window.checked").await, json!(false));
    let state: StorageState = serde_json::from_value(json!({
        "cookies": [{
            "name": "session", "value": "ada-42", "domain": "127.0.0.1", "path": "/",
            "expires": -1, "httpOnly": true, "secure": false, "sameSite": "Lax"
        }, {
            "name": "old", "value": "1", "domain": "127.0.0.1", "path": "/",
            "expires": 1000, "httpOnly": false, "secure": false, "sameSite": "Lax"
        }],
        "origins": [
            {"origin": f.a(), "localStorage": [{"name": "token", "value": "tok-ada-42"}]},
            {"origin": f.b(), "localStorage": [{"name": "b", "value": "2"}]}
        ]
    }))
    .unwrap();
    let out = engine
        .state_load(StateLoadRequest {
            state,
            path: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("{}", e.to_text()));
    assert!(
        out.warnings[0].contains("1 cookie had expired"),
        "{:?}",
        out.warnings
    );
    let data: StateData = serde_json::from_value(out.data).unwrap();
    assert_eq!(data.cookies, 1);
    let applied: Vec<Option<&str>> = data.origins.iter().map(|o| o.applied.as_deref()).collect();
    assert_eq!(applied, vec![Some("now"), Some("next-load")]);
    assert!(out.text.contains("given to the open page"), "{}", out.text);
    assert_eq!(
        eval(&engine, "localStorage.getItem('token')").await,
        json!("tok-ada-42")
    );
    reload(&engine).await;
    assert_eq!(eval(&engine, "window.checked").await, json!(true));

    // The other origin gets its storage when a page of it loads.
    goto(&engine, &f.url_b("/state/app.html")).await;
    assert_eq!(eval(&engine, "localStorage.getItem('b')").await, json!("2"));
    engine.close().await;
}

/// A state file real Playwright (1.64) saved after logging in on the
/// fixture (`bench/playwright/storage-state.mjs`) loads as it is; only its
/// origin is replaced by this run's fixture origin (the port differs).
#[tokio::test(flavor = "multi_thread")]
async fn playwright_state_files_load_unchanged() {
    let Some(path) = chrome_path() else { return };
    let f = Fixtures::shared();
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/state/playwright.json");
    let text = std::fs::read_to_string(&file).unwrap();
    let recorded: Value = serde_json::from_str(&text).unwrap();
    let origin = recorded["origins"][0]["origin"].as_str().unwrap();
    let state = StorageState::parse(&text.replace(origin, f.a())).unwrap();

    let engine = Engine::new("state-playwright", None, None);
    let mut open = open_request(&path);
    open.url = Some(f.url("/state/app.html"));
    open.state = Some(StateLoadRequest {
        state,
        path: None,
        timeout_ms: TIMEOUT_MS,
    });
    let opened = engine
        .open(open)
        .await
        .unwrap_or_else(|e| panic!("{}", e.to_text()));
    // The theme cookie expires a day after the file was made.
    assert!((1..=2).contains(&opened.data.state.unwrap().cookies));
    assert_eq!(eval(&engine, "window.checked").await, json!(true));
    assert_eq!(
        eval(&engine, "window.first").await,
        json!({"token": "tok-ada-42", "draft": null})
    );
    assert_eq!(
        eval(&engine, "drafts()").await,
        json!({
            "version": 2,
            "draft": {"id": 1, "title": "Hello", "words": 2},
            "saved": "2026-01-31T09:30:00.000Z",
            "settings": {"theme": "dark"},
        })
    );
    engine.close().await;
}
