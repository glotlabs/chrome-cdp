//! The actionability pipeline against a real Chrome (`CHROME_PATH`;
//! skipped without it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the
//! fixture site's `actions/` pages (plan 05, milestone 5.1).
//!
//! Each passing case aims with [`Engine::action_point`], then presses the
//! mouse there with raw `Input.dispatchMouseEvent` calls and checks the
//! page logged a click on the element: the point is right, in the tab's
//! viewport, through scrolling, shadow roots and iframes. Each failed
//! check has a golden error message in `tests/actionability/`
//! (`UPDATE_GOLDEN=1` rewrites them).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::Host;
use engine::{
    ActionPoint, CdpRequest, Checks, Engine, LocateOptions, NavigateRequest, Navigation,
    OpenRequest, Target, WaitUntil,
};
use fixtures::Fixtures;
use protocol::{ErrorInfo, ErrorKind};
use serde_json::{Value, json};

const TIMEOUT_MS: u64 = 30_000;
const TIMEOUT: Duration = Duration::from_millis(TIMEOUT_MS);

/// How long a check that never passes is given.
const SHORT: Duration = Duration::from_millis(1200);

const CLICK: Checks = Checks {
    enabled: true,
    editable: false,
    force: false,
    position: None,
    no_hit_test: false,
};
const HOVER: Checks = Checks {
    enabled: false,
    editable: false,
    force: false,
    position: None,
    no_hit_test: false,
};
const FILL: Checks = Checks {
    enabled: true,
    editable: true,
    force: false,
    position: None,
    no_hit_test: false,
};
const FORCE: Checks = Checks {
    enabled: true,
    editable: false,
    force: true,
    position: None,
    no_hit_test: false,
};

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

async fn aim(
    engine: &Engine,
    locator: &str,
    checks: Checks,
    timeout: Duration,
) -> Result<ActionPoint, ErrorInfo> {
    engine
        .action_point(None, locator, &LocateOptions::default(), checks, timeout)
        .await
}

async fn cdp(engine: &Engine, method: &str, params: Value) -> Value {
    let Value::Object(params) = params else {
        panic!("params must be an object")
    };
    engine
        .cdp(CdpRequest {
            all_sessions: false,
            method: Some(method.to_owned()),
            params,
            target: Target::Page,
            tab: None,
            frame: None,
            listen: Vec::new(),
            count: 0,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap_or_else(|e| panic!("{method}: {}", e.to_text()))
        .data
}

/// Presses and releases the left button at `point`, as a click does.
async fn click_at(engine: &Engine, point: &ActionPoint) {
    for kind in ["mouseMoved", "mousePressed", "mouseReleased"] {
        cdp(
            engine,
            "Input.dispatchMouseEvent",
            json!({ "type": kind, "x": point.x, "y": point.y, "button": "left", "clickCount": 1 }),
        )
        .await;
    }
}

/// The ids the page (or the frame `frame=` names) logged clicks on.
async fn log(engine: &Engine, frame: Option<&str>) -> String {
    let locator = match frame {
        Some(frame) => format!("frame={frame} >> #log"),
        None => "#log".to_owned(),
    };
    engine
        .call_on_locator(
            None,
            &locator,
            &LocateOptions::default(),
            "function () { return this.textContent; }",
            TIMEOUT,
        )
        .await
        .unwrap()
        .value
        .as_str()
        .unwrap()
        .to_owned()
}

/// Aims at `locator`, clicks there, and checks the click reached `id`.
async fn clicks(engine: &Engine, locator: &str, frame: Option<&str>, id: &str) {
    let point = aim(engine, locator, CLICK, TIMEOUT)
        .await
        .unwrap_or_else(|e| panic!("{locator}: {}", e.to_text()));
    let before = log(engine, frame).await;
    click_at(engine, &point).await;
    let after = log(engine, frame).await;
    assert_eq!(
        after.strip_prefix(&before).map(str::trim),
        Some(id),
        "{locator}: a click at x={} y={} logged {after:?} (before: {before:?})",
        point.x,
        point.y
    );
}

fn golden_path(case: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/actionability")
        .join(format!("{case}.txt"))
}

/// Compares the error's text form with golden file `case`, or rewrites
/// it with `UPDATE_GOLDEN=1`.
fn golden(case: &str, err: &ErrorInfo) {
    let path = golden_path(case);
    let actual = format!("{}\n", err.to_text());
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (UPDATE_GOLDEN=1 writes it)", path.display()));
    assert_eq!(
        expected,
        actual,
        "{} differs (UPDATE_GOLDEN=1 accepts it)",
        path.display()
    );
}

/// Aims at `locator`, which must fail with `kind`; checks the golden text.
async fn fails(
    engine: &Engine,
    locator: &str,
    checks: Checks,
    kind: ErrorKind,
    case: &str,
) -> ErrorInfo {
    let Err(err) = aim(engine, locator, checks, SHORT).await else {
        panic!("{locator} should not be actionable");
    };
    assert_eq!(err.kind, kind, "{locator}: {}", err.to_text());
    golden(case, &err);
    err
}

#[tokio::test]
async fn aims_where_a_click_reaches_the_element() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "actionable-aim").await;

    goto(&engine, &site.url("/actions/scroll.html")).await;
    clicks(&engine, "#near", None, "near").await;
    // Far below the fold, inside a scroll container, and far right.
    clicks(&engine, "#far", None, "far").await;
    clicks(&engine, "#deep", None, "deep").await;
    clicks(&engine, "#right", None, "right").await;
    clicks(&engine, "role=button[name=\"Near\"]", None, "near").await;

    goto(&engine, &site.url("/actions/shadow.html")).await;
    let before = log(&engine, None).await;
    let point = aim(&engine, "#inside", CLICK, TIMEOUT).await.unwrap();
    click_at(&engine, &point).await;
    // The page's own listener sees the host; the shadow root's, the button.
    assert!(
        log(&engine, None).await[before.len()..].contains("inside"),
        "a click in the shadow root did not reach the button"
    );

    engine.close().await;
}

#[tokio::test]
async fn aims_into_same_and_cross_origin_iframes() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "actionable-frames").await;
    goto(&engine, &site.url("/actions/frames.html")).await;

    clicks(&engine, "frame=same >> #inner", Some("same"), "inner").await;
    clicks(
        &engine,
        "frame=same >> #inner-far",
        Some("same"),
        "inner-far",
    )
    .await;
    clicks(&engine, "frame=other >> #inner", Some("other"), "inner").await;
    clicks(
        &engine,
        "frame=other >> #inner-far",
        Some("other"),
        "inner-far",
    )
    .await;

    // A parent's element over an out-of-process iframe covers what is in it.
    let err = fails(
        &engine,
        "frame=covered >> #inner",
        CLICK,
        ErrorKind::Obscured,
        "iframe-covered",
    )
    .await;
    assert!(err.details.unwrap()["covering"]["ref"].is_string());

    engine.close().await;
}

#[tokio::test]
async fn names_what_covers_the_element_with_refs() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "actionable-obscured").await;
    goto(&engine, &site.url("/actions/obscured.html")).await;

    let err = fails(&engine, "#under", CLICK, ErrorKind::Obscured, "obscured").await;
    let details = err.details.unwrap();
    let covering = details["covering"]["ref"].as_str().unwrap().to_owned();
    let dismiss = details["dismiss"]["ref"].as_str().unwrap().to_owned();
    assert_eq!(details["dismiss"]["name"], "Accept");
    // The obstruction's ref names the banner; the dismiss ref works as a
    // locator, and clicking it frees the button.
    let banner = engine
        .call_on_locator(
            None,
            &covering,
            &LocateOptions::default(),
            "function () { return this.className; }",
            TIMEOUT,
        )
        .await
        .unwrap();
    assert_eq!(banner.value, "cookie-banner");
    clicks(&engine, &dismiss, None, "accept").await;
    clicks(&engine, "#under", None, "under").await;

    // pointer-events: none on the element itself.
    fails(&engine, "#inert", CLICK, ErrorKind::Obscured, "inert").await;
    // --force skips the hit test.
    aim(&engine, "#inert", FORCE, SHORT).await.unwrap();

    engine.close().await;
}

#[tokio::test]
async fn waits_for_moving_elements_to_stop() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "actionable-moving").await;
    goto(&engine, &site.url("/actions/moving.html")).await;

    // It slides for 600 ms: the click lands where it stopped.
    clicks(&engine, "#settles", None, "settles").await;
    let err = fails(&engine, "#forever", CLICK, ErrorKind::Timeout, "moving").await;
    assert_eq!(err.details.unwrap()["check"], "stable");
    aim(&engine, "#forever", FORCE, SHORT).await.unwrap();

    engine.close().await;
}

#[tokio::test]
async fn checks_visible_enabled_and_editable() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "actionable-states").await;
    goto(&engine, &site.url("/actions/states.html")).await;

    // Enabled, shown and added after 500 ms: the pipeline waits.
    clicks(&engine, "#later", None, "later").await;
    clicks(&engine, "#shows", None, "shows").await;
    clicks(&engine, "#appears", None, "appears").await;

    fails(&engine, "#never", CLICK, ErrorKind::NotEnabled, "disabled").await;
    fails(
        &engine,
        "#in-fieldset",
        CLICK,
        ErrorKind::NotEnabled,
        "fieldset",
    )
    .await;
    fails(
        &engine,
        "#aria",
        CLICK,
        ErrorKind::NotEnabled,
        "aria-disabled",
    )
    .await;
    // Hover does not need it enabled.
    aim(&engine, "#never", HOVER, SHORT).await.unwrap();

    for (locator, case) in [
        ("#hidden", "display-none"),
        ("#invisible", "visibility-hidden"),
        ("#clear", "opacity"),
        ("#empty", "empty-box"),
    ] {
        let err = fails(&engine, locator, CLICK, ErrorKind::Timeout, case).await;
        assert_eq!(err.details.unwrap()["check"], "visible", "{locator}");
    }
    assert!(
        aim(&engine, "#hidden", FORCE, SHORT).await.is_err(),
        "a display: none element has no point, even with --force"
    );

    // Editable, for fill and type.
    for locator in ["#text", "#notes", "#editor"] {
        aim(&engine, locator, FILL, SHORT)
            .await
            .unwrap_or_else(|e| panic!("{locator}: {}", e.to_text()));
    }
    fails(
        &engine,
        "#readonly",
        FILL,
        ErrorKind::NotEnabled,
        "readonly",
    )
    .await;
    // What can never take text fails at once.
    let started = Instant::now();
    fails(
        &engine,
        "#checkbox",
        FILL,
        ErrorKind::NotEnabled,
        "not-text-input",
    )
    .await;
    fails(&engine, "#div", FILL, ErrorKind::NotEnabled, "not-editable").await;
    assert!(started.elapsed() < SHORT, "a final check was retried");

    // A locator that never matches waits, then says so; a ref fails at once.
    fails(&engine, "#nope", CLICK, ErrorKind::NotFound, "not-found").await;
    let started = Instant::now();
    let err = aim(&engine, "e999", CLICK, SHORT).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert!(started.elapsed() < SHORT, "an unknown ref was retried");

    engine.close().await;
}

#[tokio::test]
async fn names_what_covers_the_element_while_a_frame_keeps_loading() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "actionable-obscured-busy").await;
    goto(&engine, &site.url("/actions/obscured-busy.html")).await;

    // Another frame loads a new document every 50 ms, during the hit test
    // and the look after it: the banner still gets a ref.
    let Err(err) = aim(&engine, "#under", CLICK, SHORT).await else {
        panic!("#under should be covered");
    };
    assert_eq!(err.kind, ErrorKind::Obscured, "{}", err.to_text());
    let details = err.details.unwrap();
    assert!(details["covering"]["ref"].is_string(), "{details}");
    let dismiss = details["dismiss"]["ref"].as_str().unwrap().to_owned();
    clicks(&engine, &dismiss, None, "accept").await;
    clicks(&engine, "#under", None, "under").await;

    engine.close().await;
}
