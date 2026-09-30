//! The paused clock (`time pause`, `run-for`, `pause-at`, `resume`; plan
//! 14.2) against a real Chrome (`CHROME_PATH`; skipped without it unless
//! `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture site's
//! `emulate/timers.html`: a countdown (an interval), a debounced save (a
//! timeout) and an animation frame counter.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use browser::Host;
use engine::{
    EmulateRequest, EmulationChange, Engine, EvalRequest, EvalWorld, NavigateRequest, Navigation,
    OpenRequest, TabNewRequest, TabsRequest, WaitUntil,
};
use fixtures::Fixtures;
use protocol::schema::{ClockFired, EmulateData, EvalData, TabsData};
use protocol::{ErrorInfo, Output};
use serde_json::{Value, json};

const TIMEOUT_MS: u64 = 30_000;
/// 2026-01-31T09:30:00Z.
const T0: i64 = 1_769_851_800_000;

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
            wait: WaitUntil::Load,
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("goto {url}: {}", e.to_text()));
}

/// `script`'s value in `tab` (the active one with `None`).
async fn eval_in(engine: &Engine, tab: Option<&str>, script: &str) -> Value {
    let out = engine
        .eval(EvalRequest {
            script: script.to_owned(),
            locator: None,
            frame: None,
            world: EvalWorld::Main,
            tab: tab.map(str::to_owned),
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

async fn eval(engine: &Engine, script: &str) -> Value {
    eval_in(engine, None, script).await
}

async fn time(engine: &Engine, change: EmulationChange) -> Result<Output, ErrorInfo> {
    engine
        .emulate(EmulateRequest {
            change,
            timeout_ms: TIMEOUT_MS,
        })
        .await
}

/// Changes the clock; its data and warnings.
async fn clock(engine: &Engine, change: EmulationChange) -> (EmulateData, Vec<String>) {
    let what = format!("{change:?}");
    let out = time(engine, change)
        .await
        .unwrap_or_else(|e| panic!("{what}: {}", e.to_text()));
    (serde_json::from_value(out.data).unwrap(), out.warnings)
}

async fn run_for(engine: &Engine, ms: u64) -> ClockFired {
    let (data, _) = clock(engine, EmulationChange::TimeRunFor { ms }).await;
    data.fired.unwrap()
}

/// What the page shows: countdown, saved note and frames.
async fn shown(engine: &Engine, tab: Option<&str>) -> Value {
    eval_in(
        engine,
        tab,
        "[left, document.getElementById('saved').textContent, frames_]",
    )
    .await
}

#[tokio::test]
async fn paused_timers_run_only_when_told() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "clock-paused").await;
    let (data, _) = clock(&engine, EmulationChange::TimePause { epoch_ms: Some(T0) }).await;
    let paused = data.emulation.time.unwrap();
    assert!(paused.paused);
    assert_eq!(paused.iso, "2026-01-31T09:30:00.000Z");
    goto(&engine, &site.url("/emulate/timers.html")).await;

    // Nothing runs, and performance.now() stands still.
    let before = eval(&engine, "performance.now()").await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(shown(&engine, None).await, json!([50, "nothing", 0]));
    assert_eq!(eval(&engine, "performance.now()").await, before);
    assert_eq!(eval(&engine, "Date.now()").await, json!(T0));

    // Every interval tick and every 16ms frame over 1s.
    let fired = run_for(&engine, 1000).await;
    assert_eq!((fired.timers, fired.frames), (10, 62));
    assert_eq!(shown(&engine, None).await, json!([40, "nothing", 62]));
    assert_eq!(eval(&engine, "Date.now()").await, json!(T0 + 1000));
    let moved =
        eval(&engine, "performance.now()").await.as_f64().unwrap() - before.as_f64().unwrap();
    assert!((moved - 1000.0).abs() < 1e-6, "{moved}");
    let times = eval(
        &engine,
        "frameTimes.map((t, i, all) => i ? t - all[i - 1] : 0)",
    )
    .await;
    assert_eq!(times, json!([0, 16, 16, 16, 16]));

    // The debounce: a save 300ms after the last keystroke.
    eval(&engine, "typed('a'); 0").await;
    run_for(&engine, 200).await;
    eval(&engine, "typed('ab'); 0").await;
    run_for(&engine, 299).await;
    assert_eq!(shown(&engine, None).await[1], json!("nothing"));
    run_for(&engine, 1).await;
    assert_eq!(shown(&engine, None).await[1], json!("ab"));

    // A callback's own timers run in the window too, in due order, with
    // the time of each.
    eval(
        &engine,
        "log = []; setTimeout(() => { log.push(['outer', Date.now()]); \
           setTimeout(() => log.push(['inner', Date.now()]), 50); \
           setTimeout(() => log.push(['late', Date.now()]), 500); }, 100); \
         setTimeout(() => log.push(['between', Date.now()]), 120); 0",
    )
    .await;
    let start = eval(&engine, "Date.now()").await.as_i64().unwrap();
    run_for(&engine, 200).await;
    assert_eq!(
        eval(&engine, "log").await,
        json!([
            ["outer", start + 100],
            ["between", start + 120],
            ["inner", start + 150]
        ])
    );
    run_for(&engine, 400).await;
    assert_eq!(eval(&engine, "log.length").await, json!(4));

    // `advance` while paused fires each due callback once.
    eval(
        &engine,
        "window.ticks = 0; window.every = setInterval(() => ticks++, 100); 0",
    )
    .await;
    let (data, _) = clock(&engine, EmulationChange::TimeAdvance { ms: 1000 }).await;
    assert!(data.emulation.time.unwrap().paused);
    assert_eq!(data.fired.unwrap().frames, 1);
    assert_eq!(eval(&engine, "ticks").await, json!(1));
    eval(&engine, "clearInterval(every); 0").await;

    // pause-at: forward only, to the time asked.
    let now = eval(&engine, "Date.now()").await.as_i64().unwrap();
    let err = time(&engine, EmulationChange::TimePauseAt { epoch_ms: now - 1 })
        .await
        .unwrap_err();
    assert_eq!(err.exit_code(), 2);
    assert!(err.to_text().contains("time pause"), "{}", err.to_text());
    let left = eval(&engine, "left").await.as_i64().unwrap();
    let (data, _) = clock(
        &engine,
        EmulationChange::TimePauseAt {
            epoch_ms: now + 500,
        },
    )
    .await;
    assert_eq!(data.emulation.time.unwrap().epoch_ms, now + 500);
    assert_eq!(eval(&engine, "left").await.as_i64().unwrap(), left - 5);

    // resume: timers and frames run in real time, and Date stays frozen.
    let frozen = now + 500;
    let (data, _) = clock(&engine, EmulationChange::TimeResume).await;
    assert!(!data.emulation.time.as_ref().unwrap().paused);
    let frames = eval(&engine, "frames_").await.as_i64().unwrap();
    let left = eval(&engine, "left").await.as_i64().unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(eval(&engine, "left").await.as_i64().unwrap() < left);
    assert!(eval(&engine, "frames_").await.as_i64().unwrap() > frames);
    assert_eq!(eval(&engine, "Date.now()").await, json!(frozen));
    assert_eq!(
        eval(&engine, "window.requestAnimationFrame.name").await,
        json!("requestAnimationFrame")
    );
    let err = time(&engine, EmulationChange::TimeRunFor { ms: 10 })
        .await
        .unwrap_err();
    assert!(err.to_text().contains("time pause"), "{}", err.to_text());

    // reset gives everything back.
    clock(&engine, EmulationChange::TimeReset).await;
    assert!(eval(&engine, "Date.now()").await.as_f64().unwrap() > 1_780_000_000_000.0);
    engine.close().await;
}

#[tokio::test]
async fn a_runaway_loop_is_cut_short() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "clock-runaway").await;
    clock(&engine, EmulationChange::TimePause { epoch_ms: Some(T0) }).await;
    goto(&engine, &site.url("/emulate/timers.html")).await;
    eval(
        &engine,
        "window.n = 0; window.stop = false; \
         (function again() { if (stop) return; n++; setTimeout(again, 0); })(); 0",
    )
    .await;
    let (data, warnings) = clock(&engine, EmulationChange::TimeRunFor { ms: 1 }).await;
    let fired = data.fired.unwrap();
    assert_eq!(fired.timers, 10_000);
    assert!(
        warnings.iter().any(|w| w.contains("10,000 callbacks")),
        "{warnings:?}"
    );
    assert_eq!(eval(&engine, "n").await, json!(10_001));
    // The rest stay queued, and run first next time.
    eval(&engine, "stop = true; 0").await;
    let fired = run_for(&engine, 1).await;
    assert_eq!(fired.timers, 1);
    assert_eq!(eval(&engine, "n").await, json!(10_001));
    engine.close().await;
}

#[tokio::test]
async fn new_tabs_open_paused_and_loaded_pages_warn() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "clock-tabs").await;
    let page = site.url("/emulate/timers.html");

    // Paused on a loaded page: its own timers keep running.
    goto(&engine, &page).await;
    let (_, warnings) = clock(&engine, EmulationChange::TimePause { epoch_ms: None }).await;
    assert!(
        warnings
            .iter()
            .any(|w| w == "timers set before now keep running; reload for full control"),
        "{warnings:?}"
    );
    let left = eval(&engine, "left").await.as_i64().unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(eval(&engine, "left").await.as_i64().unwrap() < left);

    // A reload gives full control, and a new tab opens paused.
    goto(&engine, &page).await;
    let out = engine
        .tabs(TabsRequest {
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap();
    let tabs: TabsData = serde_json::from_value(out.data).unwrap();
    let first = Some(tabs.tabs[0].id.clone());
    let first = first.as_deref();
    let out = engine
        .tab_new(TabNewRequest {
            url: Some(page.clone()),
            base_url: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
    let second = out.data["tab"]["id"].as_str().unwrap().to_owned();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(shown(&engine, first).await, json!([50, "nothing", 0]));
    assert_eq!(
        shown(&engine, Some(&second)).await,
        json!([50, "nothing", 0])
    );
    let fired = run_for(&engine, 500).await;
    assert_eq!(fired.timers, 10, "5 countdown ticks in each tab");
    assert_eq!(eval_in(&engine, Some(&second), "left").await, json!(45));
    assert_eq!(eval_in(&engine, first, "left").await, json!(45));
    // Pausing again warns no more.
    let (_, warnings) = clock(&engine, EmulationChange::TimePause { epoch_ms: None }).await;
    assert!(warnings.is_empty(), "{warnings:?}");
    engine.close().await;
}
