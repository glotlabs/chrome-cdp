//! Logpoints (plan 15, milestone 15.4) against a real Chrome
//! (`CHROME_PATH`; skipped without it unless `CHROME_CDP_REQUIRE_CHROME`
//! is set) and the fixture apps' dev builds (`testdata/site/apps/dev`,
//! on the localhost origin).
//!
//! A logpoint on a line of `src/cart.ts` reports its values after a
//! click, in the report and the console; one on a line with no code
//! moves to the next line and says so; one set before a reload binds
//! again after it; one on a lazily loaded module is pending, then bound;
//! an expression that throws shows its error and the others still show;
//! `--limit 3` stops after three hits; an ambiguous place lists the
//! candidates; a page with a `debugger;` statement keeps running; `debug
//! clear --all` turns the debugger off.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use browser::Host;
use engine::{
    Action, ActionRequest, Button, ConsoleRequest, DebugClearRequest, DebugListRequest,
    DebugLogRequest, Engine, EvalRequest, EvalWorld, NavigateRequest, Navigation, OpenRequest,
    Since, WaitFor, WaitRequest, WaitUntil,
};
use fixtures::Fixtures;
use protocol::schema::{
    ConsoleData, DebugClearData, DebugListData, DebugLogData, Event, Level, Logpoint, LogpointHit,
};
use protocol::{ErrorInfo, ErrorKind, Output};

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

/// The sources of fixture app `name`: the project root its places print
/// relative to.
fn app_root(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/apps-src")
        .join(name)
        .canonicalize()
        .unwrap()
}

/// The 1-based line of the app's `path` that contains `needle`.
fn line_of(app: &str, path: &str, needle: &str) -> u32 {
    let text = std::fs::read_to_string(app_root(app).join(path)).unwrap();
    let index = text
        .lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} is not in {path}"));
    u32::try_from(index).unwrap() + 1
}

/// Runs `fut` as a call of a CLI in app `name`'s directory.
async fn in_app<F: Future>(name: &str, fut: F) -> F::Output {
    engine::in_root(Some(app_root(name)), Box::pin(fut)).await
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

/// Opens `path` of the fixture site on the localhost origin.
async fn goto(engine: &Engine, path: &str) {
    let url = Fixtures::shared().url_b(path);
    engine
        .navigate(NavigateRequest {
            navigation: Navigation::Goto {
                url: url.clone(),
                base_url: None,
            },
            wait: WaitUntil::Settle,
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("goto {url}: {}", e.to_text()));
}

/// Opens app `name` and waits for its items.
async fn open_app(engine: &Engine, name: &str) {
    goto(engine, &format!("/apps/dev/{name}/index.html")).await;
    wait(
        engine,
        WaitFor::Function {
            expression: "!!document.querySelector('[data-testid=add-3]')".into(),
        },
    )
    .await;
}

async fn wait(engine: &Engine, condition: WaitFor) {
    engine
        .wait(WaitRequest {
            condition,
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap_or_else(|e| panic!("wait: {}", e.to_text()));
}

async fn reload(engine: &Engine) {
    engine
        .navigate(NavigateRequest {
            navigation: Navigation::Reload { hard: false },
            wait: WaitUntil::Settle,
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("reload: {}", e.to_text()));
}

async fn click(engine: &Engine, locator: &str) -> Output {
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
            no_wait_after: false,
            snap: false,
            max_chars: 20_000,
        })
        .await
        .unwrap_or_else(|e| panic!("click {locator}: {}", e.to_text()))
}

async fn eval(engine: &Engine, script: &str) -> Output {
    engine
        .eval(EvalRequest {
            script: script.into(),
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
        .unwrap_or_else(|e| panic!("eval {script}: {}", e.to_text()))
}

/// `debug log PATH:LINE EXPR…` in app `app`'s directory.
async fn log(
    engine: &Engine,
    app: &str,
    path: &str,
    line: u32,
    exprs: &[&str],
    limit: u64,
) -> Result<(Output, Logpoint), ErrorInfo> {
    let file = app_root(app).join(path);
    let output = in_app(
        app,
        engine.debug_log(DebugLogRequest {
            tab: None,
            path: path.into(),
            file: file.is_file().then(|| file.to_string_lossy().into_owned()),
            line,
            column: None,
            exprs: exprs.iter().map(|&e| e.to_owned()).collect(),
            limit,
            timeout_ms: TIMEOUT_MS,
        }),
    )
    .await?;
    let data: DebugLogData = serde_json::from_value(output.data.clone()).unwrap();
    Ok((output, data.logpoint))
}

async fn list(engine: &Engine, app: &str) -> Vec<Logpoint> {
    let output = engine::in_root(Some(app_root(app)), async {
        engine.debug_list(DebugListRequest {
            timeout_ms: TIMEOUT_MS,
        })
    })
    .await
    .unwrap();
    serde_json::from_value::<DebugListData>(output.data)
        .unwrap()
        .logpoints
}

async fn clear(engine: &Engine, id: Option<u32>) -> (Output, DebugClearData) {
    let output = engine
        .debug_clear(DebugClearRequest {
            id,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("debug clear: {}", e.to_text()));
    let data = serde_json::from_value(output.data.clone()).unwrap();
    (output, data)
}

/// The tab's console entries, as `console` prints them and as data.
async fn console(engine: &Engine, app: &str) -> (String, ConsoleData) {
    let output = in_app(
        app,
        engine.console(ConsoleRequest {
            errors: false,
            tab: None,
            level: Level::Debug,
            since: Since::Seq { seq: 0 },
            grep: None,
            limit: 0,
            wait_ms: 0,
            sourcemaps: false,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        }),
    )
    .await
    .unwrap();
    let data = serde_json::from_value(output.data).unwrap();
    (output.text, data)
}

fn hits(output: &Output) -> Vec<&LogpointHit> {
    output
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Logpoint { hit, .. } => Some(hit),
            _ => None,
        })
        .collect()
}

/// A logpoint in `countClick` reports the values of its line on the next
/// click, in the report and the console, and its expressions see the
/// function's scope; one that throws shows its error beside the others.
/// Clearing it turns the debugger off.
#[tokio::test(flavor = "multi_thread")]
async fn a_logpoint_reports_its_values() {
    let Some(chrome) = chrome_path() else { return };
    for app in ["plain", "react19"] {
        let engine = start(&chrome, &format!("debug-values-{app}")).await;
        open_app(&engine, app).await;
        let line = line_of(app, "src/cart.ts", "clicks += 1");
        let (output, logpoint) = log(
            &engine,
            app,
            "src/cart.ts",
            line,
            &["what", "clicks", "missing"],
            1000,
        )
        .await
        .unwrap_or_else(|e| panic!("{app}: {}", e.to_text()));
        assert_eq!(logpoint.state, "bound", "{app}: {logpoint:?}");
        assert_eq!(logpoint.place, format!("src/cart.ts:{line}"));
        assert_eq!(logpoint.bound_line, None);
        assert_eq!(
            logpoint.file.as_deref(),
            Some(app_root(app).join("src/cart.ts").to_str().unwrap())
        );
        assert!(
            output.text.starts_with(&format!(
                "logpoint 1 at src/cart.ts:{line} (bound in /apps/dev/{app}/assets/index.js)"
            )),
            "{app}: {}",
            output.text
        );
        assert_eq!(output.quiet.as_deref(), Some("1"));

        let clicked = in_app(app, click(&engine, "testid=add-1")).await;
        let expected = format!(
            "! logpoint 1 src/cart.ts:{line}: what=\"add\", clicks=0, missing=<ReferenceError: missing is not defined>"
        );
        assert!(clicked.text.contains(&expected), "{app}: {}", clicked.text);
        let hit = hits(&clicked);
        assert_eq!(hit.len(), 1, "{app}: {:?}", clicked.events);
        assert_eq!(hit[0].values[1].value.as_deref(), Some("0"));
        assert_eq!(
            hit[0].values[2].error.as_deref(),
            Some("ReferenceError: missing is not defined")
        );

        let (text, data) = console(&engine, app).await;
        assert!(
            text.contains(&format!(
                "info logpoint 1 src/cart.ts:{line}: what=\"add\", clicks=0"
            )),
            "{app}: {text}"
        );
        let entry = data
            .entries
            .iter()
            .find(|e| e.source == "logpoint")
            .unwrap();
        assert_eq!(entry.logpoint.as_ref().unwrap().line, line);

        let listed = list(&engine, app).await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].hits, 1);

        let (output, cleared) = clear(&engine, None).await;
        assert_eq!(cleared.cleared, [1]);
        assert!(!cleared.debugger, "{app}");
        assert_eq!(output.text, "cleared logpoint 1; the debugger is off");
        // No more hits.
        let clicked = in_app(app, click(&engine, "testid=add-1")).await;
        assert!(hits(&clicked).is_empty(), "{app}: {}", clicked.text);
        engine.close().await;
    }
}

/// A line with no code (a comment) uses the next line that has some, and
/// says which.
#[tokio::test(flavor = "multi_thread")]
async fn a_line_without_code_moves_on() {
    let Some(chrome) = chrome_path() else { return };
    let engine = start(&chrome, "debug-moves").await;
    open_app(&engine, "plain").await;
    let comment = line_of("plain", "src/cart.ts", "// Called on every click");
    let (output, logpoint) = log(&engine, "plain", "src/cart.ts", comment, &["what"], 1000)
        .await
        .unwrap();
    let bound = logpoint.bound_line.expect("moved");
    assert!(bound > comment, "{logpoint:?}");
    assert!(
        output
            .text
            .contains(&format!("(line {comment} has no code; bound in")),
        "{}",
        output.text
    );
    let clicked = in_app("plain", click(&engine, "testid=add-2")).await;
    assert!(
        clicked
            .text
            .contains(&format!("! logpoint 1 src/cart.ts:{bound}: what=\"add\"")),
        "{}",
        clicked.text
    );
    engine.close().await;
}

/// A logpoint set before a reload is set again in the reloaded page's
/// script.
#[tokio::test(flavor = "multi_thread")]
async fn a_logpoint_survives_a_reload() {
    let Some(chrome) = chrome_path() else { return };
    let engine = start(&chrome, "debug-reload").await;
    open_app(&engine, "plain").await;
    let line = line_of("plain", "src/cart.ts", "return clicks;");
    log(&engine, "plain", "src/cart.ts", line, &["clicks"], 1000)
        .await
        .unwrap();
    in_app("plain", click(&engine, "testid=add-1")).await;
    reload(&engine).await;
    wait(
        &engine,
        WaitFor::Function {
            expression: "!!document.querySelector('[data-testid=add-3]')".into(),
        },
    )
    .await;
    let clicked = in_app("plain", click(&engine, "testid=remove-2")).await;
    assert!(
        clicked
            .text
            .contains(&format!("! logpoint 1 src/cart.ts:{line}: clicks=1")),
        "{}",
        clicked.text
    );
    assert_eq!(list(&engine, "plain").await[0].hits, 2);
    engine.close().await;
}

/// A logpoint on a module the page loads later is pending, then bound
/// when its chunk loads, before it runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_logpoint_waits_for_its_script() {
    let Some(chrome) = chrome_path() else { return };
    let engine = start(&chrome, "debug-lazy").await;
    open_app(&engine, "plain").await;
    let line = line_of("plain", "src/later.ts", "document.title = note");
    let (output, logpoint) = log(
        &engine,
        "plain",
        "src/later.ts",
        line,
        &["note", "why"],
        1000,
    )
    .await
    .unwrap();
    assert_eq!(logpoint.state, "pending");
    assert_eq!(
        output.text,
        format!(
            "logpoint 1 at src/later.ts:{line} is pending: no script of t1 has src/later.ts yet; it binds when one loads"
        )
    );
    eval(&engine, "window.dispatchEvent(new Event('later'))").await;
    wait(
        &engine,
        WaitFor::Function {
            expression: "document.title === 'later: asked'".into(),
        },
    )
    .await;
    let listed = &list(&engine, "plain").await[0];
    assert_eq!(listed.state, "bound", "{listed:?}");
    assert_eq!(listed.hits, 1);
    assert_eq!(listed.scripts.len(), 1);
    assert!(
        listed.scripts[0].ends_with("/assets/later.js"),
        "{listed:?}"
    );
    let (text, _) = console(&engine, "plain").await;
    assert!(
        text.contains(&format!(
            "logpoint 1 src/later.ts:{line}: note=\"later: asked\", why=\"asked\""
        )),
        "{text}"
    );
    engine.close().await;
}

/// `--limit 3` stops a logpoint after three hits, and the console says
/// so once.
#[tokio::test(flavor = "multi_thread")]
async fn a_logpoint_stops_at_its_limit() {
    let Some(chrome) = chrome_path() else { return };
    let engine = start(&chrome, "debug-limit").await;
    open_app(&engine, "plain").await;
    let line = line_of("plain", "src/cart.ts", "clicks += 1");
    log(&engine, "plain", "src/cart.ts", line, &["clicks"], 3)
        .await
        .unwrap();
    for _ in 0..5 {
        in_app("plain", click(&engine, "testid=add-1")).await;
    }
    let listed = &list(&engine, "plain").await[0];
    assert_eq!((listed.hits, listed.state.as_str()), (3, "stopped"));
    let (text, data) = console(&engine, "plain").await;
    let logged: Vec<_> = data
        .entries
        .iter()
        .filter(|e| e.source == "logpoint")
        .collect();
    assert_eq!(logged.len(), 4, "{text}");
    assert_eq!(
        logged.iter().filter(|e| e.logpoint.is_some()).count(),
        3,
        "{text}"
    );
    assert!(
        text.contains("logpoint 1 stopped after 3 hits (its --limit)"),
        "{text}"
    );
    engine.close().await;
}

/// A place that names several sources lists them.
#[tokio::test(flavor = "multi_thread")]
async fn an_ambiguous_place_lists_the_candidates() {
    let Some(chrome) = chrome_path() else { return };
    let engine = start(&chrome, "debug-ambiguous").await;
    open_app(&engine, "react19").await;
    let err = log(&engine, "react19", "index.js", 1, &[], 1000)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Ambiguous, "{}", err.to_text());
    let candidates: Vec<String> =
        serde_json::from_value(err.details.clone().unwrap()["candidates"].clone()).unwrap();
    assert!(
        candidates.contains(&"node_modules/react/index.js".to_owned()),
        "{candidates:?}"
    );
    assert!(
        candidates.contains(&"node_modules/react-dom/index.js".to_owned()),
        "{candidates:?}"
    );
    assert!(err.to_text().contains("candidate: "), "{}", err.to_text());
    // Nothing was set.
    assert!(list(&engine, "react19").await.is_empty());
    engine.close().await;
}

/// With the debugger on, a `debugger;` statement does not stop the page:
/// it is resumed and noted. Once the logpoints are cleared, the debugger
/// is off and the statement does nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_debugger_statement_does_not_stop_the_page() {
    let Some(chrome) = chrome_path() else { return };
    let engine = start(&chrome, "debug-statement").await;
    goto(&engine, "/debug/debugger.html").await;
    let (output, logpoint) = log(&engine, "plain", "debug/debugger.html", 10, &["runs"], 1000)
        .await
        .unwrap();
    assert_eq!(logpoint.state, "bound", "{}", output.text);
    let clicked = in_app("plain", click(&engine, "#go")).await;
    assert!(
        clicked
            .text
            .contains("! logpoint 1 /debug/debugger.html:10: runs=1"),
        "{}",
        clicked.text
    );
    wait(
        &engine,
        WaitFor::Function {
            expression: "document.getElementById('out').textContent === 'ran 1'".into(),
        },
    )
    .await;
    let (text, _) = console(&engine, "plain").await;
    assert!(
        text.contains(
            "[debugger] paused at a debugger statement (/debug/debugger.html:9); resumed"
        ),
        "{text}"
    );

    let (_, cleared) = clear(&engine, None).await;
    assert!(!cleared.debugger);
    in_app("plain", click(&engine, "#go")).await;
    wait(
        &engine,
        WaitFor::Function {
            expression: "document.getElementById('out').textContent === 'ran 2'".into(),
        },
    )
    .await;
    let (text, _) = console(&engine, "plain").await;
    assert_eq!(
        text.matches("paused at a debugger statement").count(),
        1,
        "{text}"
    );
    engine.close().await;
}

/// Bad input: an expression that is not JavaScript, a line 0, an unknown
/// id.
#[tokio::test(flavor = "multi_thread")]
async fn bad_input_is_refused() {
    let Some(chrome) = chrome_path() else { return };
    let engine = start(&chrome, "debug-bad").await;
    open_app(&engine, "plain").await;
    let err = log(&engine, "plain", "src/cart.ts", 14, &["a +"], 1000)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadArgs, "{}", err.to_text());
    assert!(
        err.message
            .contains("\"a +\" is not a JavaScript expression"),
        "{}",
        err.message
    );
    let err = log(&engine, "plain", "src/cart.ts", 0, &[], 1000)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadArgs);
    let err = engine
        .debug_clear(DebugClearRequest {
            id: Some(7),
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert_eq!(err.message, "no logpoint 7; the session has none");
    engine.close().await;
}

/// Two logpoints on one line share its breakpoint: both log, and
/// clearing one leaves the other.
#[tokio::test(flavor = "multi_thread")]
async fn two_logpoints_on_one_line() {
    let Some(chrome) = chrome_path() else { return };
    let engine = start(&chrome, "debug-shared").await;
    open_app(&engine, "plain").await;
    let line = line_of("plain", "src/cart.ts", "clicks += 1");
    let (_, first) = log(&engine, "plain", "src/cart.ts", line, &["what"], 1000)
        .await
        .unwrap();
    let (_, second) = log(&engine, "plain", "src/cart.ts", line, &["clicks"], 1000)
        .await
        .unwrap();
    assert_eq!(
        (first.state.as_str(), second.state.as_str()),
        ("bound", "bound")
    );
    let clicked = in_app("plain", click(&engine, "testid=add-1")).await;
    assert!(
        clicked
            .text
            .contains(&format!("! logpoint 1 src/cart.ts:{line}: what=\"add\"\n! logpoint 2 src/cart.ts:{line}: clicks=0")),
        "{}",
        clicked.text
    );
    let (output, cleared) = clear(&engine, Some(1)).await;
    assert_eq!((cleared.remaining, cleared.debugger), (1, true));
    assert_eq!(output.text, "cleared logpoint 1; 1 left");
    let clicked = in_app("plain", click(&engine, "testid=add-1")).await;
    let hits = hits(&clicked);
    assert_eq!(hits.len(), 1, "{}", clicked.text);
    assert_eq!(hits[0].id, 2);
    assert_eq!(hits[0].values[0].value.as_deref(), Some("1"));
    engine.close().await;
}
