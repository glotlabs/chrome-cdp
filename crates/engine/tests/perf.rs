//! `perf` against a real Chrome (`CHROME_PATH`; skipped without it unless
//! `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture site (plan 06,
//! milestone 6.5).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use browser::Host;
use engine::{
    Action, ActionRequest, Button, Engine, NavigateRequest, Navigation, OpenRequest, PerfRequest,
    WaitUntil,
};
use fixtures::Fixtures;
use protocol::Output;
use protocol::schema::PerfData;

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
    tokio::time::sleep(Duration::from_millis(500)).await;
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
