//! Init scripts and exposed functions (`init-script`, `expose`,
//! `unexpose`; plan 07, milestone 7.3) against a real Chrome
//! (`CHROME_PATH`; skipped without it unless `CHROME_CDP_REQUIRE_CHROME`
//! is set) and the fixture site's `inject/` pages.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::Host;
use engine::{
    ConsoleRequest, Engine, EvalRequest, EvalWorld, ExposeRequest, InitScriptAddRequest,
    InitScriptListRequest, InitScriptRemoveRequest, NavigateRequest, Navigation, OpenRequest,
    Since, TabsRequest, WaitUntil,
};
use fixtures::Fixtures;
use protocol::ErrorInfo;
use protocol::schema::{
    ConsoleData, EvalData, ExposeData, InitScriptAddData, InitScriptRemoveData, InitScriptsData,
    Level, TabsData,
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

async fn open(engine: &Arc<Engine>, path: &Path) {
    let chrome = browser::discover(Some(path), &Host::current()).unwrap();
    engine
        .open(OpenRequest {
            chrome: Some(chrome),
            timeout_ms: TIMEOUT_MS,
            ..OpenRequest::default()
        })
        .await
        .unwrap();
}

async fn start(path: &Path, name: &str) -> Arc<Engine> {
    let engine = Engine::new(name, None, None);
    open(&engine, path).await;
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

async fn eval_with(engine: &Engine, req: EvalRequest) -> Value {
    let script = req.script.clone();
    let out = engine
        .eval(req)
        .await
        .unwrap_or_else(|e| panic!("{script}: {}", e.to_text()));
    let data: EvalData = serde_json::from_value(out.data).unwrap();
    data.value
}

async fn eval(engine: &Engine, script: &str) -> Value {
    eval_with(engine, eval_request(script)).await
}

/// `window.first` (what the first script saw) in `tab` (or the active
/// one), in its frame `frame` if given.
async fn first(engine: &Engine, tab: Option<&str>, frame: Option<&str>) -> Value {
    let mut req = eval_request("window.first");
    req.tab = tab.map(str::to_owned);
    req.frame = frame.map(str::to_owned);
    eval_with(engine, req).await
}

fn add_request(source: &str, world: EvalWorld, now: bool) -> InitScriptAddRequest {
    InitScriptAddRequest {
        source: source.to_owned(),
        file: None,
        world,
        now,
        timeout_ms: TIMEOUT_MS,
    }
}

async fn try_add(
    engine: &Engine,
    req: InitScriptAddRequest,
) -> Result<InitScriptAddData, ErrorInfo> {
    let out = engine.init_script_add(req).await?;
    Ok(serde_json::from_value(out.data).unwrap())
}

async fn add(engine: &Engine, source: &str, world: EvalWorld, now: bool) -> InitScriptAddData {
    try_add(engine, add_request(source, world, now))
        .await
        .unwrap_or_else(|e| panic!("{source}: {}", e.to_text()))
}

async fn try_remove(
    engine: &Engine,
    ids: &[&str],
    all: bool,
) -> Result<InitScriptRemoveData, ErrorInfo> {
    let out = engine
        .init_script_remove(InitScriptRemoveRequest {
            ids: ids.iter().map(|&s| s.to_owned()).collect(),
            all,
            timeout_ms: TIMEOUT_MS,
        })
        .await?;
    Ok(serde_json::from_value(out.data).unwrap())
}

fn list(engine: &Engine) -> InitScriptsData {
    let out = engine
        .init_script_list(&InitScriptListRequest { max_chars: 0 })
        .unwrap();
    serde_json::from_value(out.data).unwrap()
}

async fn try_expose(
    engine: &Engine,
    names: &[&str],
    remove: bool,
    all: bool,
) -> Result<ExposeData, ErrorInfo> {
    let out = engine
        .expose(ExposeRequest {
            names: names.iter().map(|&s| s.to_owned()).collect(),
            remove,
            all,
            returns: Vec::new(),
            timeout_ms: TIMEOUT_MS,
        })
        .await?;
    Ok(serde_json::from_value(out.data).unwrap())
}

/// `expose NAME --returns-seq` with `returns` (`--returns X` is `[X]`).
async fn expose_returning(engine: &Engine, name: &str, returns: Vec<Value>) -> ExposeData {
    let out = engine
        .expose(ExposeRequest {
            names: vec![name.to_owned()],
            remove: false,
            all: false,
            returns,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("expose {name}: {}", e.to_text()));
    serde_json::from_value(out.data).unwrap()
}

async fn expose(engine: &Engine, names: &[&str], remove: bool) -> ExposeData {
    try_expose(engine, names, remove, false)
        .await
        .unwrap_or_else(|e| panic!("expose {names:?}: {}", e.to_text()))
}

/// The console texts of `tab` (or the active one) that start with
/// `binding`.
async fn binding_calls(engine: &Engine, tab: Option<&str>) -> Vec<String> {
    let out = engine
        .console(ConsoleRequest {
            errors: false,
            tab: tab.map(str::to_owned),
            level: Level::Debug,
            since: Since::Seq { seq: 0 },
            grep: None,
            limit: 0,
            wait_ms: 0,
            sourcemaps: false,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap();
    let data: ConsoleData = serde_json::from_value(out.data).unwrap();
    data.entries
        .into_iter()
        .filter(|e| e.source == "binding")
        .map(|e| e.text)
        .collect()
}

/// Polls `check` until it returns a value, for up to 10 s.
async fn eventually<T>(what: &str, mut check: impl AsyncFnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(value) = check().await {
            return value;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Opens a popup at `path` from the active tab and returns its id once it
/// has loaded.
async fn popup(engine: &Engine, path: &str) -> String {
    eval(engine, &format!("window.open('{path}'); true")).await;
    eventually("the popup", async || {
        let out = engine
            .tabs(TabsRequest {
                timeout_ms: TIMEOUT_MS,
                max_chars: 0,
            })
            .await
            .unwrap();
        let tabs: TabsData = serde_json::from_value(out.data).unwrap();
        tabs.tabs
            .iter()
            .find(|t| t.id != "t1" && t.url.ends_with(path) && t.title == "first script")
            .map(|t| t.id.clone())
    })
    .await
}

/// Init scripts and exposed functions set before the browser starts run
/// before a page's first script: in the first tab, in a popup, and in an
/// out-of-process iframe. Scripts run in the order they were added.
#[tokio::test]
async fn scripts_and_functions_come_before_first_scripts_everywhere() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = Engine::new("inject-first", None, None);
    // They belong to the session: they wait for its browser.
    let one = add(
        &engine,
        "window.__init = (window.__init || []).concat('one');",
        EvalWorld::Main,
        false,
    )
    .await;
    assert_eq!(one.added.id, "s1");
    add(
        &engine,
        "window.__init = window.__init.concat('two');",
        EvalWorld::Main,
        false,
    )
    .await;
    add(
        &engine,
        "window.__isolated = 1;",
        EvalWorld::Isolated,
        false,
    )
    .await;
    expose(&engine, &["report"], false).await;
    open(&engine, &path).await;

    let want = json!({
        "init": ["one", "two"],
        // The isolated world's globals are out of the page's reach.
        "isolated": "undefined",
        "report": "function",
        // The binding behind the function is hidden.
        "binding": "undefined",
    });
    goto(&engine, &site.url("/inject/first.html")).await;
    assert_eq!(first(&engine, None, None).await, want, "the first tab");
    let mut isolated = eval_request("window.__isolated");
    isolated.world = EvalWorld::Isolated;
    assert_eq!(eval_with(&engine, isolated).await, json!(1));
    let calls = eventually("the first tab's call", async || {
        Some(binding_calls(&engine, None).await).filter(|c| !c.is_empty())
    })
    .await;
    assert_eq!(
        calls,
        vec![r#"binding report("first", {"n":1,"url":"/inject/first.html"})"#]
    );

    let tab = popup(&engine, "/inject/first.html").await;
    assert_eq!(first(&engine, Some(&tab), None).await, want, "the popup");
    eventually("the popup's call", async || {
        Some(binding_calls(&engine, Some(&tab)).await).filter(|c| c.len() == 1)
    })
    .await;

    // An iframe of the other origin has its own process and session.
    goto(&engine, &site.url("/inject/frame.html")).await;
    assert_eq!(
        first(&engine, None, Some("child")).await,
        want,
        "the cross-origin iframe"
    );
    let calls = eventually("the frame's call", async || {
        Some(binding_calls(&engine, None).await).filter(|c| c.len() == 2)
    })
    .await;
    assert!(
        calls[1].starts_with("binding report(\"first\""),
        "{calls:?}"
    );
    engine.close().await;
}

/// A script added to a running browser runs from the next load, or at once
/// with `now`; removing it keeps it out of new documents. Ids are never
/// reused, and a script that does not compile is refused.
#[tokio::test]
async fn scripts_are_added_listed_and_removed_while_pages_run() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "inject-change").await;
    goto(&engine, &site.url("/inject/first.html")).await;

    add(&engine, "window.__later = 1;", EvalWorld::Main, false).await;
    assert_eq!(
        eval(&engine, "typeof window.__later").await,
        json!("undefined")
    );
    reload(&engine).await;
    assert_eq!(eval(&engine, "window.__later").await, json!(1));

    let now = add(&engine, "window.__now = 2;", EvalWorld::Main, true).await;
    assert!(now.ran_now);
    assert_eq!(now.active, 2);
    assert_eq!(eval(&engine, "window.__now").await, json!(2));

    let scripts = list(&engine).scripts;
    let ids: Vec<&str> = scripts.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["s1", "s2"]);
    assert_eq!(scripts[0].preview, "window.__later = 1;");
    assert_eq!(scripts[0].world, "main");

    let removed = try_remove(&engine, &["s1"], false).await.unwrap();
    assert_eq!(removed.removed, ["s1"]);
    assert_eq!(removed.active, 1);
    reload(&engine).await;
    assert_eq!(
        eval(&engine, "typeof window.__later").await,
        json!("undefined")
    );
    assert_eq!(eval(&engine, "window.__now").await, json!(2));

    let err = try_remove(&engine, &["s1"], false).await.unwrap_err();
    assert_eq!(err.exit_code(), 1, "{}", err.to_text());
    assert!(
        err.to_text().contains("no init script s1"),
        "{}",
        err.to_text()
    );

    let err = try_add(
        &engine,
        add_request("window.x = {;", EvalWorld::Main, false),
    )
    .await
    .unwrap_err();
    assert_eq!(err.exit_code(), 2, "{}", err.to_text());
    assert!(
        err.to_text().contains("does not compile: SyntaxError"),
        "{}",
        err.to_text()
    );
    assert!(err.to_text().contains("line 1"), "{}", err.to_text());
    let err = try_add(&engine, add_request("  \n", EvalWorld::Main, false))
        .await
        .unwrap_err();
    assert_eq!(err.exit_code(), 2, "{}", err.to_text());

    // A script's errors name it.
    add(&engine, "\nnotDefinedAnywhere();", EvalWorld::Main, false).await;
    let three = list(&engine).scripts.last().unwrap().id.clone();
    assert_eq!(three, "s3");
    reload(&engine).await;
    let out = engine
        .console(ConsoleRequest {
            errors: true,
            tab: None,
            level: Level::Debug,
            since: Since::Seq { seq: 0 },
            grep: None,
            limit: 0,
            wait_ms: 0,
            sourcemaps: false,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap();
    let data: ConsoleData = serde_json::from_value(out.data).unwrap();
    let error = data
        .entries
        .iter()
        .find(|e| e.text.contains("notDefinedAnywhere"))
        .unwrap_or_else(|| panic!("{:#?}", data.entries));
    assert_eq!(error.url.as_deref(), Some("init-script-s3.js"), "{error:?}");
    assert_eq!(error.line, Some(2), "{error:?}");

    let removed = try_remove(&engine, &[], true).await.unwrap();
    assert_eq!(removed.removed, ["s2", "s3"]);
    assert_eq!(removed.active, 0);
    reload(&engine).await;
    assert_eq!(
        eval(&engine, "typeof window.__now").await,
        json!("undefined")
    );
    assert_eq!(
        add(&engine, "1", EvalWorld::Main, false).await.added.id,
        "s4"
    );
    engine.close().await;
}

/// `expose` puts a function in the pages there are at once; its calls
/// take any arguments and return a promise. `unexpose` takes it out of
/// them.
#[tokio::test]
async fn functions_are_exposed_and_taken_away_while_pages_run() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "inject-expose").await;
    goto(&engine, &site.url("/inject/first.html")).await;
    assert_eq!(
        first(&engine, None, None).await["report"],
        json!("undefined")
    );

    let data = expose(&engine, &["report", "done"], false).await;
    assert_eq!(data.changed, ["report", "done"]);
    assert_eq!(eval(&engine, "typeof report").await, json!("function"));
    assert_eq!(
        eval(
            &engine,
            "report(1, 'two', [3], {four: 4}, undefined) instanceof Promise"
        )
        .await,
        json!(true)
    );
    eval(&engine, "done()").await;
    let calls = eventually("the calls", async || {
        Some(binding_calls(&engine, None).await).filter(|c| c.len() == 2)
    })
    .await;
    assert_eq!(
        calls,
        [
            r#"binding report(1, "two", [3], {"four":4}, null)"#,
            "binding done()"
        ]
    );

    // Exposed again: nothing changes.
    let data = expose(&engine, &["report"], false).await;
    assert!(data.changed.is_empty());
    assert_eq!(data.bindings, ["report", "done"]);

    // After a load the first script calls it.
    reload(&engine).await;
    eventually("the first script's call", async || {
        Some(binding_calls(&engine, None).await).filter(|c| c.len() == 3)
    })
    .await;

    let data = expose(&engine, &["report"], true).await;
    assert_eq!(data.changed, ["report"]);
    assert_eq!(data.bindings, ["done"]);
    assert_eq!(eval(&engine, "typeof report").await, json!("undefined"));
    reload(&engine).await;
    assert_eq!(
        first(&engine, None, None).await["report"],
        json!("undefined")
    );
    assert_eq!(eval(&engine, "typeof done").await, json!("function"));

    // Exposed again after being taken away.
    expose(&engine, &["report"], false).await;
    assert_eq!(eval(&engine, "typeof report").await, json!("function"));

    let err = try_expose(&engine, &["nope"], true, false)
        .await
        .unwrap_err();
    assert_eq!(err.exit_code(), 1, "{}", err.to_text());
    let err = try_expose(&engine, &["not-a-name"], false, false)
        .await
        .unwrap_err();
    assert_eq!(err.exit_code(), 2, "{}", err.to_text());
    let data = try_expose(&engine, &[], true, true).await.unwrap();
    assert!(data.bindings.is_empty());
    assert_eq!(eval(&engine, "typeof done").await, json!("undefined"));
    engine.close().await;
}

/// `expose --returns`: page code that awaits the function gets the fixed
/// value; `--returns-seq` gives the calls its values in order, the last
/// repeating, across reloads. The calls are still logged, and exposing
/// the name again with other answers starts over.
#[tokio::test]
async fn exposed_functions_return_their_answers() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "inject-expose-returns").await;
    goto(&engine, &site.url("/inject/first.html")).await;

    let data = expose_returning(&engine, "getConfig", vec![json!({"flags": ["beta"]})]).await;
    assert_eq!(data.changed, ["getConfig"]);
    assert_eq!(data.returns.len(), 1);
    assert_eq!(data.returns[0].values, [json!({"flags": ["beta"]})]);
    assert_eq!(
        eval(&engine, "(async () => (await getConfig('x')).flags[0])()").await,
        json!("beta")
    );
    assert_eq!(
        eval(
            &engine,
            "Promise.all([getConfig(), getConfig()]).then(a => a.length)"
        )
        .await,
        json!(2)
    );
    let calls = eventually("the calls", async || {
        Some(binding_calls(&engine, None).await).filter(|c| c.len() == 3)
    })
    .await;
    assert_eq!(calls[0], r#"binding getConfig("x")"#);

    let data = expose_returning(
        &engine,
        "nextToken",
        vec![json!("a"), json!("b"), json!(null)],
    )
    .await;
    assert_eq!(data.bindings, ["getConfig", "nextToken"]);
    assert_eq!(
        eval(
            &engine,
            "(async () => [await nextToken(), await nextToken()])()"
        )
        .await,
        json!(["a", "b"])
    );
    // The sequence is the session's, not the document's.
    reload(&engine).await;
    assert_eq!(
        eval(
            &engine,
            "(async () => [await nextToken(), await nextToken()])()"
        )
        .await,
        json!([null, null])
    );

    // The same answers again change nothing; others start over.
    let data = expose_returning(
        &engine,
        "nextToken",
        vec![json!("a"), json!("b"), json!(null)],
    )
    .await;
    assert!(data.changed.is_empty());
    let data = expose_returning(&engine, "nextToken", vec![json!(1), json!(2)]).await;
    assert_eq!(data.changed, ["nextToken"]);
    assert_eq!(
        eval(
            &engine,
            "(async () => [await nextToken(), await nextToken(), await nextToken()])()"
        )
        .await,
        json!([1, 2, 2])
    );
    // Without answers again: it resolves at once to undefined.
    expose(&engine, &["nextToken"], false).await;
    assert_eq!(
        eval(&engine, "nextToken().then(v => v === undefined)").await,
        json!(true)
    );
    engine.close().await;
}
