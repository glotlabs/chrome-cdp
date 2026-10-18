//! The wait engine against a real Chrome (`CHROME_PATH`; skipped without
//! it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture site:
//! each `wait` condition met, timing out, and what its timeout error
//! observed; network idle on pages with beacons and WebSockets; and the
//! settle that navigations run by default.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::Host;
use engine::{
    ElementState, Engine, NavigateRequest, Navigation, OpenRequest, SnapshotRequest, WaitFor,
    WaitRequest, WaitUntil, World,
};
use fixtures::Fixtures;
use protocol::rpc::Settle;
use protocol::schema::{NavigateData, WaitData};
use protocol::{ErrorInfo, ErrorKind, Output};
use serde_json::json;

const TIMEOUT_MS: u64 = 30_000;
const TIMEOUT: Duration = Duration::from_secs(10);

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

async fn start(path: &std::path::Path) -> Arc<Engine> {
    let engine = Engine::new("engine-wait-test", None, None);
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

async fn goto_with(engine: &Engine, url: &str, wait: WaitUntil) -> Output {
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
        .unwrap_or_else(|e| panic!("goto {url}: {}", e.to_text()))
}

async fn goto(engine: &Engine, url: &str) {
    goto_with(engine, url, WaitUntil::Load).await;
}

async fn wait(engine: &Engine, condition: WaitFor, timeout_ms: u64) -> Result<Output, ErrorInfo> {
    engine
        .wait(WaitRequest {
            condition,
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms,
            max_chars: 20_000,
        })
        .await
}

async fn met(engine: &Engine, condition: WaitFor) -> (WaitData, Output) {
    let output = wait(engine, condition.clone(), TIMEOUT_MS)
        .await
        .unwrap_or_else(|e| panic!("{condition:?}: {}", e.to_text()));
    (serde_json::from_value(output.data.clone()).unwrap(), output)
}

/// Waits `condition` with a short timeout and returns the error.
async fn times_out(engine: &Engine, condition: WaitFor) -> ErrorInfo {
    let Err(err) = wait(engine, condition.clone(), 700).await else {
        panic!("{condition:?} held");
    };
    eprintln!("{}", err.to_text());
    assert_eq!(err.kind, ErrorKind::Timeout, "{}", err.to_text());
    assert_eq!(err.exit_code(), 3);
    err
}

fn element(locator: &str, state: ElementState) -> WaitFor {
    WaitFor::Element {
        locator: locator.to_owned(),
        state,
    }
}

fn text(text: &str) -> WaitFor {
    WaitFor::Text {
        text: text.to_owned(),
        within: None,
        gone: false,
    }
}

async fn eval(engine: &Engine, js: &str) -> serde_json::Value {
    engine
        .evaluate(None, None, World::Main, js, TIMEOUT)
        .await
        .unwrap_or_else(|e| panic!("{js}: {}", e.to_text()))
}

#[tokio::test]
async fn elements_reach_each_state() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/wait/lazy.html?after=500")).await;
    let (data, output) = met(&engine, element("#welcome", ElementState::Visible)).await;
    assert_eq!(data.condition, "element");
    assert_eq!(data.count, Some(1));
    assert!(data.elapsed_ms >= 100, "{}", data.elapsed_ms);
    assert!(
        output.text.starts_with("#welcome is visible ("),
        "{}",
        output.text
    );
    assert_eq!(output.quiet.as_deref(), Some("1"));
    for (locator, state) in [
        ("#loading", ElementState::Detached),
        ("#go", ElementState::Enabled),
        ("#banner", ElementState::Hidden),
        ("#banner", ElementState::Attached),
        ("xpath=//h2[text()='Later']", ElementState::Visible),
        ("css=button", ElementState::Visible),
    ] {
        let (data, output) = met(&engine, element(locator, state)).await;
        eprintln!("{}", output.text);
        assert!(data.elapsed_ms < 2_000, "{locator}: {}", data.elapsed_ms);
    }
    let (data, output) = met(&engine, element("css=button", ElementState::Visible)).await;
    assert_eq!(data.count, Some(2));
    assert!(output.text.contains("(2 matches)"), "{}", output.text);

    // Timeouts say what they saw.
    let err = times_out(&engine, element("#nope", ElementState::Visible)).await;
    assert_eq!(err.message, "waited 700ms for #nope to be visible");
    let text = err.to_text();
    assert!(text.contains("observed: no element matches\n"), "{text}");
    assert!(
        text.contains(&format!(
            "page: {} \"Lazy\"",
            site.url("/wait/lazy.html?view=done")
        )),
        "{text}"
    );
    assert!(
        text.contains(
            "hint: chrome-cdp wait '#nope' --timeout 1.4s --session engine-wait-test   (no element matches"
        ),
        "{text}"
    );
    let details = err.details.unwrap();
    assert_eq!(details["condition"], "element");
    assert_eq!(details["observed"]["count"], 0);
    assert_eq!(details["timeout_ms"], 700);
    let err = times_out(&engine, element("#banner", ElementState::Visible)).await;
    assert!(
        err.to_text()
            .contains("observed: 1 match, not visible, enabled\n"),
        "{}",
        err.to_text()
    );
    let err = times_out(&engine, element("css=button", ElementState::Hidden)).await;
    assert_eq!(err.message, "waited 700ms for css=button to be hidden");
    assert!(
        err.to_text()
            .contains("observed: 2 matches, 2 visible, 2 enabled\n")
    );

    // A selector the page rejects fails at once.
    let start = Instant::now();
    let err = wait(&engine, element("#[", ElementState::Visible), TIMEOUT_MS)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadLocator, "{}", err.to_text());
    assert_eq!(err.message, "#[: #[ is not a valid CSS selector");
    assert!(start.elapsed() < TIMEOUT);
    engine.close().await;
}

#[tokio::test]
async fn text_appears_and_goes() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/wait/lazy.html?after=500")).await;
    // Spaces are collapsed: the page says "Welcome,   Ada".
    let (data, output) = met(&engine, text("Welcome, Ada")).await;
    assert_eq!(data.condition, "text");
    assert!(
        output
            .text
            .starts_with("text \"Welcome, Ada\" is visible (")
    );
    let (_, output) = met(
        &engine,
        WaitFor::Text {
            text: "Loading".into(),
            within: None,
            gone: true,
        },
    )
    .await;
    assert!(output.text.starts_with("text \"Loading\" is gone ("));
    met(
        &engine,
        WaitFor::Text {
            text: "Ada".into(),
            within: Some("#main".into()),
            gone: false,
        },
    )
    .await;

    let err = times_out(&engine, text("Goodbye")).await;
    assert_eq!(err.message, "waited 700ms for text \"Goodbye\"");
    assert!(
        err.to_text()
            .contains("observed: not in the page's visible text\n")
    );
    // Hidden text does not count.
    let err = times_out(&engine, text("We use cookies")).await;
    assert_eq!(err.details.unwrap()["observed"]["found"], false);
    let err = times_out(
        &engine,
        WaitFor::Text {
            text: "Welcome".into(),
            within: Some("#nowhere".into()),
            gone: false,
        },
    )
    .await;
    assert!(
        err.to_text()
            .contains("observed: no element matches #nowhere\n")
    );
    let err = times_out(
        &engine,
        WaitFor::Text {
            text: "Welcome".into(),
            within: None,
            gone: true,
        },
    )
    .await;
    assert_eq!(err.message, "waited 700ms for text \"Welcome\" to go away");
    assert!(
        err.to_text()
            .contains("observed: the text is still visible\n")
    );
    engine.close().await;
}

/// Without `--in`, `--text` and `--text-gone` read every frame, a
/// cross-site one too, and the timeout says which were read.
#[tokio::test]
async fn text_in_every_frame() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/wait/framed.html")).await;
    let (_, output) = met(&engine, text("Other ready")).await;
    assert!(
        output
            .text
            .starts_with("text \"Other ready\" is visible in iframe \"other\" ("),
        "{}",
        output.text
    );
    let gone = |text: &str| WaitFor::Text {
        text: text.to_owned(),
        within: None,
        gone: true,
    };
    let (_, output) = met(&engine, gone("Loading frame")).await;
    assert!(output.text.starts_with("text \"Loading frame\" is gone ("));

    let err = times_out(&engine, text("Nowhere")).await;
    let shown = err.to_text();
    assert!(
        shown.contains("read: main frame, iframe \"same\", iframe \"other\"\n"),
        "{shown}"
    );
    let err = times_out(&engine, gone("Same ready")).await;
    let shown = err.to_text();
    assert!(shown.contains("still in: iframe \"same\"\n"), "{shown}");
    let frames = &err.details.unwrap()["observed"]["frames"];
    assert_eq!(frames[1]["frame"], "iframe \"same\"");
    assert_eq!(frames[1]["found"], true);
    assert_eq!(frames[2]["found"], false);
    engine.close().await;
}

#[tokio::test]
async fn a_wait_carries_on_across_navigations() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/nav/a.html")).await;
    let next = site.url("/wait/lazy.html?after=200");
    eval(
        &engine,
        &format!("setTimeout(() => location.href = {next:?}, 300); 1"),
    )
    .await;
    let (data, _) = met(&engine, text("Welcome, Ada")).await;
    assert_eq!(data.url, site.url("/wait/lazy.html?view=done"));
    engine.close().await;
}

#[tokio::test]
async fn url_and_fn_conditions() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/wait/lazy.html?after=500")).await;
    let (data, output) = met(
        &engine,
        WaitFor::Url {
            pattern: "?view=done".into(),
        },
    )
    .await;
    assert_eq!(data.url, site.url("/wait/lazy.html?view=done"));
    assert_eq!(output.quiet.as_deref(), Some(data.url.as_str()));
    met(
        &engine,
        WaitFor::Url {
            pattern: "**/wait/*.html?view=*".into(),
        },
    )
    .await;
    met(
        &engine,
        WaitFor::Url {
            pattern: r"/lazy\.html\?view=(done|later)$/".into(),
        },
    )
    .await;
    let err = times_out(
        &engine,
        WaitFor::Url {
            pattern: "/dashboard".into(),
        },
    )
    .await;
    assert_eq!(
        err.message,
        "waited 700ms for a URL matching \"/dashboard\""
    );
    assert!(
        err.to_text()
            .contains(&format!("url: {}\n", site.url("/wait/lazy.html?view=done")))
    );

    // --fn runs in the page's world and returns the value.
    let (data, output) = met(
        &engine,
        WaitFor::Function {
            expression: "window.appReady && window.appReady.user".into(),
        },
    )
    .await;
    assert_eq!(data.value, Some(json!("Ada")));
    assert!(output.text.starts_with("--fn returned \"Ada\" ("));
    assert_eq!(output.quiet.as_deref(), Some("\"Ada\""));
    // Promises are awaited.
    let (data, _) = met(
        &engine,
        WaitFor::Function {
            expression: "new Promise(r => setTimeout(() => r({ n: 2 }), 50))".into(),
        },
    )
    .await;
    assert_eq!(data.value, Some(json!({ "n": 2 })));
    let err = times_out(
        &engine,
        WaitFor::Function {
            expression: "document.title === 'Never'".into(),
        },
    )
    .await;
    assert!(err.to_text().contains("last value: false\n"));
    let err = times_out(
        &engine,
        WaitFor::Function {
            expression: "window.missing.field".into(),
        },
    )
    .await;
    assert!(
        err.to_text()
            .contains("last error: TypeError: Cannot read properties of undefined")
    );
    // A syntax error fails at once.
    let err = wait(
        &engine,
        WaitFor::Function {
            expression: "1 +".into(),
        },
        TIMEOUT_MS,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::EvalError, "{}", err.to_text());
    assert!(
        err.message
            .starts_with("the --fn expression does not compile: SyntaxError"),
        "{}",
        err.message
    );
    engine.close().await;
}

#[tokio::test]
async fn requests_and_responses_since_the_previous_command() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/wait/lazy.html?after=0")).await;
    // The click's response may arrive before `wait` starts: it counts.
    eval(&engine, "document.getElementById('save').click(); 1").await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let (data, output) = met(
        &engine,
        WaitFor::Response {
            pattern: "/api/save".into(),
            status: Some(201),
        },
    )
    .await;
    let request = data.request.unwrap();
    let url = site.url("/wait/api/save?status=201&delay=200");
    assert_eq!(request.method, "POST");
    assert_eq!(request.url, url);
    assert_eq!(request.status, Some(201));
    assert!(
        output
            .text
            .starts_with(&format!("response 201 POST {url} (")),
        "{}",
        output.text
    );
    assert_eq!(output.quiet.as_deref(), Some(url.as_str()));

    // A request still to come.
    eval(
        &engine,
        "setTimeout(() => document.getElementById('save').click(), 300); 1",
    )
    .await;
    let (data, _) = met(
        &engine,
        WaitFor::Request {
            pattern: "**/api/save?*".into(),
        },
    )
    .await;
    assert!(data.elapsed_ms >= 100, "{}", data.elapsed_ms);

    // Only what came after the previous command started counts, and a
    // response with another status is named.
    tokio::time::sleep(Duration::from_millis(500)).await;
    eval(&engine, "1").await;
    let err = times_out(
        &engine,
        WaitFor::Response {
            pattern: "/api/save".into(),
            status: Some(200),
        },
    )
    .await;
    assert_eq!(
        err.message,
        "waited 700ms for a response from \"/api/save\" with status 200"
    );
    assert!(
        err.to_text().contains("observed: no matching response\n"),
        "{}",
        err.to_text()
    );
    eval(&engine, "document.getElementById('save').click(); 1").await;
    let err = times_out(
        &engine,
        WaitFor::Response {
            pattern: "/api/save".into(),
            status: Some(200),
        },
    )
    .await;
    assert!(
        err.to_text()
            .contains(&format!("got: response 201 POST {url}\n")),
        "{}",
        err.to_text()
    );
    engine.close().await;
}

#[tokio::test]
async fn load_states() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    let slow = site.url("/nav/slow-image.html?img_delay=1000");
    goto_with(&engine, &slow, WaitUntil::DomContentLoaded).await;
    let (_, output) = met(&engine, WaitFor::Load).await;
    assert!(output.text.starts_with("load event reached ("));
    met(&engine, WaitFor::DomContentLoaded).await;

    let slower = site.url("/nav/slow-image.html?img_delay=20000");
    goto_with(&engine, &slower, WaitUntil::DomContentLoaded).await;
    let err = times_out(&engine, WaitFor::Load).await;
    assert_eq!(err.message, "waited 700ms for the load event");
    assert!(
        err.to_text()
            .contains("observed: document.readyState is \"interactive\"\n")
    );
    engine.close().await;
}

#[tokio::test]
async fn the_other_conditions_time_out_and_say_what_they_saw() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    // A parser-blocking script holds up DOMContentLoaded.
    goto_with(
        &engine,
        &site.url("/wait/slow-script.html?ms=20000"),
        WaitUntil::Commit,
    )
    .await;
    let err = times_out(&engine, WaitFor::DomContentLoaded).await;
    assert_eq!(err.message, "waited 700ms for DOMContentLoaded");
    assert!(
        err.to_text()
            .contains("observed: document.readyState is \"loading\"\n"),
        "{}",
        err.to_text()
    );

    // Element states the lazy page never reaches (it never runs its timer).
    goto(&engine, &site.url("/wait/lazy.html?after=600000")).await;
    let err = times_out(&engine, element("#go", ElementState::Enabled)).await;
    assert_eq!(err.message, "waited 700ms for #go to be enabled");
    assert!(
        err.to_text()
            .contains("observed: 1 match, visible, disabled\n"),
        "{}",
        err.to_text()
    );
    let err = times_out(&engine, element("#loading", ElementState::Detached)).await;
    assert_eq!(err.details.unwrap()["observed"]["count"], 1);
    let err = times_out(&engine, element("#welcome", ElementState::Attached)).await;
    assert!(
        err.to_text().contains("observed: no element matches\n"),
        "{}",
        err.to_text()
    );

    // A request nobody sends.
    let err = times_out(
        &engine,
        WaitFor::Request {
            pattern: "/api/save".into(),
        },
    )
    .await;
    assert_eq!(err.message, "waited 700ms for a request to \"/api/save\"");
    assert!(
        err.to_text().contains("observed: no matching request\n"),
        "{}",
        err.to_text()
    );
    assert!(
        err.hint
            .as_deref()
            .is_some_and(|h| h.contains("run the action that sends it first")),
        "{:?}",
        err.hint
    );

    // A settle that times out names the requests still open.
    goto(&engine, &site.url("/wait/slow-request.html?ms=20000")).await;
    let err = times_out(&engine, WaitFor::Settle).await;
    assert!(
        err.to_text().contains(&format!(
            "in flight: GET {} (",
            site.url("/wait/slow.json?delay=20000")
        )),
        "{}",
        err.to_text()
    );
    engine.close().await;
}

#[tokio::test]
async fn network_idle_ignores_beacons_and_websockets() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    for page in ["/wait/beacon.html", "/wait/websocket.html"] {
        goto(&engine, &site.url(page)).await;
        // Well within 2 s, though the page never stops talking.
        let output = wait(&engine, WaitFor::Idle, 2_000)
            .await
            .unwrap_or_else(|e| panic!("{page}: {}", e.to_text()));
        let data: WaitData = serde_json::from_value(output.data.clone()).unwrap();
        assert!(output.text.starts_with("network idle ("), "{}", output.text);
        assert!(data.elapsed_ms < 2_000);
        // `--wait networkidle` uses the same tracker.
        let output = goto_with(&engine, &site.url(page), WaitUntil::NetworkIdle).await;
        let nav: NavigateData = serde_json::from_value(output.data).unwrap();
        assert_eq!(nav.wait, "networkidle");
    }
    // The WebSocket really is busy.
    let echoes = eval(&engine, "window.echoes || 0").await;
    assert!(echoes.as_u64().unwrap() > 0, "{echoes}");
    engine.close().await;
}

#[tokio::test]
async fn network_idle_names_the_requests_in_flight() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/wait/slow-request.html?ms=20000")).await;
    let err = times_out(&engine, WaitFor::Idle).await;
    let slow = site.url("/wait/slow.json?delay=20000");
    assert_eq!(err.message, "waited 700ms for network idle");
    let text = err.to_text();
    assert!(text.contains(&format!("in flight: GET {slow} (")), "{text}");
    assert!(
        text.contains(
            "hint: chrome-cdp wait --load --session engine-wait-test   (does not wait for the requests still open)"
        ),
        "{text}"
    );
    let observed = &err.details.unwrap()["observed"]["in_flight"][0];
    assert_eq!(observed["url"], json!(slow));
    assert_eq!(observed["type"], "Fetch");
    let err = times_out(&engine, WaitFor::Settle).await;
    assert_eq!(err.message, "waited 700ms for the page to settle");

    // `goto --wait networkidle` times out the same way.
    let page = site.url("/wait/slow-request.html?ms=20000");
    let err = engine
        .navigate(NavigateRequest {
            navigation: Navigation::Goto {
                url: page.clone(),
                base_url: None,
            },
            wait: WaitUntil::NetworkIdle,
            tab: None,
            timeout_ms: 1_500,
        })
        .await
        .unwrap_err();
    let text = err.to_text();
    assert_eq!(err.kind, ErrorKind::Timeout, "{text}");
    assert_eq!(
        err.message,
        format!("{page} did not reach network idle within 1.5s")
    );
    assert!(text.contains("reached: commit, domcontentloaded"), "{text}");
    assert!(text.contains(&format!("in flight: GET {slow} (")), "{text}");
    engine.close().await;
}

#[tokio::test]
async fn goto_settles_by_default() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    // Content rendered from data fetched after the load event is there
    // when `goto` returns, every time, in the DOM and in the snapshot.
    let runs: usize = std::env::var("CHROME_CDP_SETTLE_RUNS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(5);
    for run in 0..runs {
        let output = goto_with(
            &engine,
            &site.url("/wait/spa-fetch.html"),
            WaitUntil::Settle,
        )
        .await;
        let nav: NavigateData = serde_json::from_value(output.data.clone()).unwrap();
        assert_eq!(nav.wait, "settle");
        assert_eq!(nav.title, "SPA: Inbox", "run {run}");
        assert!(output.warnings.is_empty(), "{:?}", output.warnings);
        // The snapshot right after `goto` shows what the app rendered
        // (plan 03's SPA criterion).
        let snapshot = engine
            .snapshot(SnapshotRequest {
                tab: None,
                interactive: false,
                scope: None,
                depth: None,
                diff: false,
                all_refs: false,
                all: false,
                text_limit: 200,
                no_text: false,
                engine: None,
                strict_refs: false,
                testid_attribute: None,
                source: false,
                fold: false,
                budget: None,
                expand: Vec::new(),
                timeout_ms: TIMEOUT_MS,
                max_chars: 0,
            })
            .await
            .unwrap_or_else(|e| panic!("snapshot: {}", e.to_text()));
        let expected = "\n- main\n  - heading \"Inbox\" (h1)\n  - list\n    - listitem \"one\"\n    - listitem \"two\"\n    - listitem \"three\"";
        assert!(
            snapshot.text.ends_with(expected),
            "run {run}: {}",
            snapshot.text
        );
    }
    // A page whose DOM never stops changing still settles, after 3 s.
    goto(&engine, &site.url("/wait/animated.html")).await;
    let (data, _) = met(&engine, WaitFor::Settle).await;
    assert_eq!(
        data.met,
        "page settled: network idle; the DOM kept changing"
    );
    assert!(data.elapsed_ms >= 2_500, "{}", data.elapsed_ms);
    goto(&engine, &site.url("/wait/beacon.html")).await;
    let (data, _) = met(&engine, WaitFor::Settle).await;
    assert_eq!(data.met, "page settled: network idle, DOM quiet");

    // A page that keeps loading: `goto` succeeds after a while, and says so.
    let output = goto_with(
        &engine,
        &site.url("/wait/slow-request.html?ms=20000"),
        WaitUntil::Settle,
    )
    .await;
    assert_eq!(output.warnings.len(), 1, "{:?}", output.warnings);
    assert!(
        output.warnings[0].starts_with(&format!(
            "the page was still loading 5s after the load event (in flight: GET {} (",
            site.url("/wait/slow.json?delay=20000")
        )),
        "{}",
        output.warnings[0]
    );
    engine.close().await;
}

/// A test run's settle (plan 22.3): after the load event, the page's
/// own requests and a brief quiet DOM, 1 s at most and without a warning;
/// the command line's settle is unchanged.
#[tokio::test]
async fn a_test_runs_goto_settles_for_less() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    let elapsed = |output: &Output| {
        let nav: NavigateData = serde_json::from_value(output.data.clone()).unwrap();
        nav.elapsed_ms
    };
    let page = site.url("/wait/spa-fetch.html");
    let full = goto_with(&engine, &page, WaitUntil::Settle).await;
    let test = engine::in_settle(Settle::Test, goto_with(&engine, &page, WaitUntil::Settle)).await;
    // The full settle waits 500 ms of network idle after the data came.
    assert!(elapsed(&full) >= 500, "{}", elapsed(&full));
    assert!(
        elapsed(&test) < elapsed(&full),
        "test {} ms, full {} ms",
        elapsed(&test),
        elapsed(&full)
    );
    assert!(test.warnings.is_empty(), "{:?}", test.warnings);

    // A page that keeps loading: 1 s after the load event, no warning.
    let start = Instant::now();
    let output = engine::in_settle(
        Settle::Test,
        goto_with(
            &engine,
            &site.url("/wait/slow-request.html?ms=20000"),
            WaitUntil::Settle,
        ),
    )
    .await;
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
    assert!(
        start.elapsed() < Duration::from_secs(4),
        "{:?}",
        start.elapsed()
    );

    // `wait --settle` asks for the full settle, in a test run too: the
    // DOM quiet for 300 ms.
    goto(&engine, &site.url("/wait/beacon.html")).await;
    let (data, _) = Box::pin(engine::in_settle(
        Settle::Test,
        met(&engine, WaitFor::Settle),
    ))
    .await;
    assert_eq!(data.met, "page settled: network idle, DOM quiet");
    assert!(data.elapsed_ms >= 300, "{}", data.elapsed_ms);
    engine.close().await;
}

#[tokio::test]
async fn a_fixed_delay_nudges_toward_conditions() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path).await;
    let start = Instant::now();
    let output = wait(&engine, WaitFor::Delay { ms: 300 }, TIMEOUT_MS)
        .await
        .unwrap();
    assert!(start.elapsed() >= Duration::from_millis(300));
    assert_eq!(output.text, "waited 300ms");
    assert!(output.warnings[0].starts_with("a fixed wait is slow and flaky"));
    engine.close().await;
}
