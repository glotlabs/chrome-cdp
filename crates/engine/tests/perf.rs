//! `perf` against a real Chrome (`CHROME_PATH`; skipped without it unless
//! `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture site (plan 06,
//! milestone 6.5).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::Host;
use engine::{
    Action, ActionRequest, Button, EmulateRequest, EmulationChange, Engine, ExpectRequest,
    Expectation, NavigateRequest, Navigation, OpenRequest, PerfRequest, WaitUntil,
};
use fixtures::Fixtures;
use protocol::schema::{ExpectData, ExpectPerf, PerfData};
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
    goto_until(engine, url, WaitUntil::Settle).await;
}

async fn goto_until(engine: &Engine, url: &str, wait: WaitUntil) {
    engine
        .navigate(NavigateRequest {
            navigation: Navigation::Goto {
                url: url.to_owned(),
                base_url: None,
            },
            wait,
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("goto {url}: {}", e.to_text()));
}

async fn click(engine: &Engine, locator: &str) {
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
        .unwrap_or_else(|e| panic!("click {locator}: {}", e.to_text()));
}

async fn perf(engine: &Engine) -> (Output, PerfData) {
    let out = engine
        .perf(PerfRequest {
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("perf: {}", e.to_text()));
    let data = serde_json::from_value(out.data.clone()).unwrap();
    (out, data)
}

#[tokio::test(flavor = "multi_thread")]
async fn perf_reports_load_timing_and_web_vitals() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "perf-vitals").await;
    let url = site.url("/perf/index.html");
    goto(&engine, &url).await;

    // Before any interaction, and before the banner moved the page.
    let (out, data) = perf(&engine).await;
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert_eq!((data.tab.as_str(), data.url.as_str()), ("t1", url.as_str()));
    assert!(data.from_start);
    let nav = data.navigation.as_ref().unwrap();
    assert_eq!(nav.kind, "navigate");
    assert!(nav.ttfb_ms.is_some() && nav.load_ms.is_some(), "{nav:?}");
    assert!(data.fcp_ms.is_some());
    let lcp = data.lcp.as_ref().unwrap();
    assert_eq!(lcp.element.as_deref(), Some("p#hero"));
    assert_eq!(data.requests, 1);
    assert!(data.inp.is_none());
    assert!(
        data.metrics.contains_key("JSHeapUsedSize"),
        "{:?}",
        data.metrics
    );
    assert!(out.text.contains("INP none yet"), "{}", out.text);

    // The banner shifts the page; the slow click is a long task and a
    // slow interaction.
    tokio::time::sleep(Duration::from_millis(1000)).await;
    click(&engine, "#slow").await;
    let (out, data) = perf(&engine).await;
    eprintln!("{}", out.text);
    assert!(
        data.cls.value > 0.0 && data.cls.shifts >= 1,
        "{:?}",
        data.cls
    );
    let inp = data.inp.as_ref().unwrap();
    assert!(inp.ms >= 250, "{inp:?}");
    assert_eq!(inp.rating, "needs improvement");
    assert_eq!(inp.target.as_deref(), Some("button#slow"));
    assert!(data.interactions >= 1);
    assert!(data.long_tasks >= 1 && data.tbt_ms >= 150, "{data:?}");
    for line in ["load: TTFB ", "LCP ", "CLS 0.", "INP ", "TBT ", "page: "] {
        assert!(out.text.contains(line), "{line}: {}", out.text);
    }
    assert!(
        out.text.contains("slowest: click on button#slow"),
        "{}",
        out.text
    );
    assert!(out.text.contains(" listeners · JS heap "), "{}", out.text);
    engine.close().await;
}

struct DebugChrome {
    child: std::process::Child,
    profile: PathBuf,
}

impl Drop for DebugChrome {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

/// A Chrome started with a DevTools port on `url`, and that port.
fn debug_chrome(path: &Path, url: &str) -> (DebugChrome, String) {
    let profile = std::env::temp_dir().join(format!("engine-perf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&profile);
    std::fs::create_dir_all(&profile).unwrap();
    let child = std::process::Command::new(path)
        .args([
            "--headless",
            "--remote-debugging-port=0",
            "--no-first-run",
            "--no-default-browser-check",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(url)
        .env("TMPDIR", &profile)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let chrome = DebugChrome {
        child,
        profile: profile.clone(),
    };
    let active = profile.join("DevToolsActivePort");
    let start = Instant::now();
    let port = loop {
        if let Ok(text) = std::fs::read_to_string(&active)
            && text.lines().count() >= 2
        {
            break text.lines().next().unwrap().to_owned();
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "no DevTools port"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    (chrome, port)
}

#[tokio::test(flavor = "multi_thread")]
async fn perf_of_a_page_loaded_before_attaching() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let (_chrome, port) = debug_chrome(&path, &site.url("/perf/index.html"));
    // The page loads, paints and shifts while nobody watches.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let engine = Engine::new("perf-connect", None, None);
    engine
        .connect(engine::ConnectRequest {
            endpoint: port.clone(),
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
    let (out, data) = perf(&engine).await;
    assert!(!data.from_start);
    assert!(
        out.warnings.iter().any(|w| w.contains("104ms")),
        "{:?}",
        out.warnings
    );
    // What the browser buffered: the paints and the shift.
    assert_eq!(
        data.lcp.as_ref().unwrap().element.as_deref(),
        Some("p#hero")
    );
    assert!(data.cls.shifts >= 1, "{:?}", data.cls);
    assert!(data.navigation.is_some());
    // A slow interaction is still seen.
    click(&engine, "#slow").await;
    let (_, data) = perf(&engine).await;
    assert!(data.inp.as_ref().unwrap().ms >= 250, "{:?}", data.inp);
    engine.close().await;
}

/// `expect --perf <budgets>` (with `--perf-missing ok` if `missing_ok`).
async fn expect_perf(
    engine: &Engine,
    budgets: &str,
    missing_ok: bool,
) -> Result<Output, ErrorInfo> {
    expect(
        engine,
        Expectation::Perf {
            budgets: budgets.to_owned(),
            missing_ok,
        },
    )
    .await
}

async fn expect(engine: &Engine, expectation: Expectation) -> Result<Output, ErrorInfo> {
    Box::pin(engine.expect(ExpectRequest {
        expectation,
        not: false,
        tab: None,
        strict_refs: false,
        testid_attribute: None,
        timeout_ms: TIMEOUT_MS,
        max_chars: 20_000,
    }))
    .await
}

async fn perf_passes(engine: &Engine, budgets: &str, missing_ok: bool) -> (Output, ExpectPerf) {
    let out = expect_perf(engine, budgets, missing_ok)
        .await
        .unwrap_or_else(|e| panic!("--perf {budgets:?}: {}", e.to_text()));
    let data: ExpectData = serde_json::from_value(out.data.clone()).unwrap();
    (out, data.perf.unwrap())
}

async fn perf_fails(engine: &Engine, budgets: &str) -> (ErrorInfo, ExpectPerf) {
    let Err(err) = expect_perf(engine, budgets, false).await else {
        panic!("--perf {budgets:?} passed");
    };
    eprintln!("{}", err.to_text());
    assert_eq!(err.kind, ErrorKind::ExpectFailed, "{}", err.to_text());
    let perf = serde_json::from_value(err.details.as_ref().unwrap()["perf"].clone()).unwrap();
    (err, perf)
}

/// The text of the failure's note `label`.
fn note<'a>(err: &'a ErrorInfo, label: &str) -> &'a str {
    err.notes.iter().find(|n| n.label == label).map_or_else(
        || panic!("no note {label}: {}", err.to_text()),
        |n| n.text.as_str(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn expect_perf_waits_for_load_then_checks_every_budget() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "perf-expect-lcp").await;
    // Back before the held-back image arrives: --perf waits for load.
    goto_until(&engine, &site.url("/perf/slow-lcp.html"), WaitUntil::Commit).await;
    let all = "ttfb<60s fcp<60s dcl<60s load<60s lcp<60s cls<1 tbt<60s requests<=50 \
               transfer<10mb elements<5000 heap<1gb";
    let (out, perf) = perf_passes(&engine, all, false).await;
    assert!(out.text.starts_with("passed: --perf "), "{}", out.text);
    assert!(out.text.contains("(throttling: none)"), "{}", out.text);
    assert_eq!(perf.budgets.len(), 11);
    assert!(
        perf.budgets.iter().all(|b| b.held && b.value.is_some()),
        "{perf:?}"
    );
    let lcp = perf.budgets.iter().find(|b| b.metric == "lcp").unwrap();
    assert_eq!(lcp.element.as_deref(), Some("img#hero"));
    assert_eq!(
        (lcp.op.as_str(), lcp.limit, lcp.unit.as_str()),
        ("<", 60_000.0, "ms")
    );
    assert!(
        out.quiet.as_deref().unwrap().starts_with("ttfb="),
        "{:?}",
        out.quiet
    );

    // The image comes after 1.5s: far over a 100ms budget.
    let (err, perf) = perf_fails(&engine, "lcp<100 tbt<60s").await;
    assert_eq!(err.message, "expect failed: --perf 'lcp<100 tbt<60s'");
    assert_eq!(note(&err, "actual"), "1 budget over (of 2)");
    let lcp = note(&err, "lcp");
    assert!(lcp.contains(" > 100ms ✗ (element: img#hero [e"), "{lcp}");
    let reference = perf.budgets[0].reference.clone().unwrap();
    assert!(lcp.ends_with(&format!("[{reference}])")), "{lcp}");
    assert!(
        note(&err, "tbt").ends_with(" < 60000ms ✓"),
        "{}",
        note(&err, "tbt")
    );
    assert_eq!(note(&err, "throttling"), "none");
    assert!(
        err.hint.as_deref().unwrap().contains("chrome-cdp perf"),
        "{:?}",
        err.hint
    );
    assert_eq!(
        perf.budgets.iter().map(|b| b.held).collect::<Vec<_>>(),
        [false, true]
    );

    // INP before any interaction is not a pass, unless asked.
    let (err, perf) = perf_fails(&engine, "good").await;
    assert_eq!(
        note(&err, "inp"),
        "not measured ✗ (no interaction yet; interact first)"
    );
    assert!(
        err.hint.as_deref().unwrap().contains("--perf-missing ok"),
        "{:?}",
        err.hint
    );
    assert_eq!(perf.budgets.len(), 3);
    let (out, _) = perf_passes(&engine, "inp<200 cls<1", true).await;
    assert!(out.text.contains("inp not measured"), "{}", out.text);
    engine.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn expect_perf_counts_long_tasks_and_names_the_throttling() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "perf-expect-tbt").await;
    goto(&engine, &site.url("/perf/long-tasks.html")).await;
    expect(
        &engine,
        Expectation::Title {
            text: "Busy done".into(),
            exact: true,
        },
    )
    .await
    .unwrap_or_else(|e| panic!("{}", e.to_text()));
    // Two tasks of 300ms: 500ms of blocking time.
    perf_passes(&engine, "tbt<60s tbt>0", false).await;
    engine
        .emulate(EmulateRequest {
            change: EmulationChange::CpuThrottle { rate: Some(2.0) },
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
    let (err, perf) = perf_fails(&engine, "tbt<=100 cls<1").await;
    assert!(
        note(&err, "tbt").contains(" > 100ms ✗"),
        "{}",
        note(&err, "tbt")
    );
    assert_eq!(note(&err, "throttling"), "cpu 2x");
    assert_eq!(perf.throttling.cpu, Some(2.0));
    assert!(perf.budgets[0].value.unwrap() > 100.0, "{perf:?}");
    engine.close().await;
}
