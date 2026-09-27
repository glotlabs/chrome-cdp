//! The tab manager against a real Chrome (`CHROME_PATH`; skipped without it
//! unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture site.
//!
//! Session settings must reach a new target before it runs any script:
//! these tests add an init script and check that a popup's and a
//! cross-site frame's inline scripts see what it set.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::Host;
use engine::{CdpRequest, Engine, OpenRequest, TabNewRequest, TabsRequest, Target};
use fixtures::Fixtures;
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

async fn open(path: &std::path::Path, url: String) -> Arc<Engine> {
    let engine = Engine::new("engine-tabs-test", None);
    let chrome = browser::discover(Some(path), &Host::current()).unwrap();
    engine
        .open(OpenRequest {
            chrome: Some(chrome),
            url: Some(url),
            timeout_ms: TIMEOUT_MS,
            ..OpenRequest::default()
        })
        .await
        .unwrap();
    engine
}

async fn eval(engine: &Engine, tab: Option<&str>, expression: &str) -> Value {
    let params = json!({ "expression": expression, "returnByValue": true });
    let output = engine
        .cdp(CdpRequest {
            method: "Runtime.evaluate".into(),
            params: params.as_object().unwrap().clone(),
            target: Target::Page,
            tab: tab.map(str::to_owned),
            frame: None,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap();
    output.data["result"]["value"].clone()
}

async fn tabs(engine: &Engine) -> Vec<Value> {
    let output = engine
        .tabs(TabsRequest {
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap();
    output.data["tabs"].as_array().unwrap().clone()
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

#[tokio::test]
async fn init_scripts_run_before_a_popups_first_script() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = open(&path, site.url("/tabs/popup.html")).await;
    engine
        .add_init_script("window.__chromeCdpInit = true", Duration::from_secs(5))
        .await
        .unwrap();

    for (opener, popup) in [("#open", "t2"), ("#blank", "t3")] {
        eval(
            &engine,
            None,
            &format!("document.querySelector('{opener}').click()"),
        )
        .await;
        let tab = eventually(&format!("popup {popup}"), async || {
            tabs(&engine)
                .await
                .into_iter()
                .find(|t| t["id"] == json!(popup) && t["title"] != json!(""))
                .filter(|t| t["title"].as_str().unwrap().starts_with("Popup: "))
        })
        .await;
        assert_eq!(tab["title"], json!("Popup: init script ran first"), "{tab}");
        assert_eq!(tab["opener"], json!("t1"), "{tab}");
        assert_eq!(tab["active"], json!(false), "{tab}");
    }
    engine.close().await;
}

#[tokio::test]
async fn cross_site_frames_are_attached_set_up_and_resumed() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = open(&path, "about:blank".into()).await;
    engine
        .add_init_script("window.__chromeCdpInit = true", Duration::from_secs(5))
        .await
        .unwrap();
    let output = engine
        .tab_new(TabNewRequest {
            url: Some(site.url("/frames/cross-origin.html")),
            base_url: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
    assert_eq!(output.data["tab"]["id"], json!("t2"));
    // The frame's script ran (so it was resumed) after the init script.
    let title = eventually("the child frame's message", async || {
        let title = eval(&engine, None, "document.title").await;
        (title != json!("Cross-origin frame: waiting")).then_some(title)
    })
    .await;
    assert_eq!(
        title,
        json!("Cross-origin frame: child ran after the init script")
    );
    // It is an out-of-process iframe, which is why it needed its own
    // auto-attach.
    let targets = engine
        .cdp(CdpRequest {
            method: "Target.getTargets".into(),
            params: serde_json::Map::new(),
            target: Target::Browser,
            tab: None,
            frame: None,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap();
    let frames: Vec<&Value> = targets.data["targetInfos"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["type"] == json!("iframe"))
        .collect();
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(
        frames[0]["url"],
        json!(site.url_b("/frames/child.html")),
        "{frames:?}"
    );
    engine.close().await;
}

#[tokio::test]
async fn workers_are_resumed() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = open(&path, site.url("/workers/worker.html")).await;
    let title = eventually("the worker's answer", async || {
        let title = eval(&engine, None, "document.title").await;
        (title != json!("Worker: waiting")).then_some(title)
    })
    .await;
    assert_eq!(title, json!("Worker: pong"));
    engine.close().await;
}
