//! `expect` against a real Chrome (`CHROME_PATH`; skipped without it
//! unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture site
//! (plan 09, milestone 9.1): every matcher passing, failing, and passing
//! once the page gets there; the page checks; `mark`; and the accessibility
//! rules.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use browser::Host;
use engine::{
    A11yLevel, ConsoleRequest, Engine, ExpectRequest, Expectation, MarkRequest, Matcher,
    NavigateRequest, Navigation, NetworkRequest, OpenRequest, Since, SnapshotRequest, WaitUntil,
    World,
};
use fixtures::Fixtures;
use protocol::schema::{ConsoleData, ExpectData, Level, NetworkData};
use protocol::{ErrorInfo, ErrorKind, Output};

const TIMEOUT_MS: u64 = 30_000;
const TIMEOUT: Duration = Duration::from_secs(10);

/// How long an assertion that should fail gets.
const SHORT_MS: u64 = 300;

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
    let engine = Engine::new("engine-expect-test", None, None);
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

async fn eval(engine: &Engine, js: &str) {
    engine
        .evaluate(None, None, World::Main, js, TIMEOUT)
        .await
        .unwrap_or_else(|e| panic!("{js}: {}", e.to_text()));
}

async fn expect(
    engine: &Engine,
    expectation: &Expectation,
    not: bool,
    timeout_ms: u64,
) -> Result<Output, ErrorInfo> {
    Box::pin(engine.expect(ExpectRequest {
        expectation: expectation.clone(),
        not,
        tab: None,
        strict_refs: false,
        testid_attribute: None,
        timeout_ms,
        max_chars: 20_000,
    }))
    .await
}

async fn passes(engine: &Engine, expectation: &Expectation, not: bool) -> ExpectData {
    let output = expect(engine, expectation, not, 5_000)
        .await
        .unwrap_or_else(|e| panic!("{expectation:?} (not: {not}): {}", e.to_text()));
    assert!(output.text.starts_with("passed: "), "{}", output.text);
    serde_json::from_value(output.data).unwrap()
}

async fn fails(engine: &Engine, expectation: &Expectation, not: bool) -> ErrorInfo {
    let Err(err) = expect(engine, expectation, not, SHORT_MS).await else {
        panic!("{expectation:?} (not: {not}) passed");
    };
    let text = err.to_text();
    eprintln!("{text}");
    assert_eq!(err.kind, ErrorKind::ExpectFailed, "{text}");
    assert_eq!(err.exit_code(), 1);
    assert!(err.message.starts_with("expect failed: "), "{text}");
    let labels: Vec<&str> = err.notes.iter().map(|n| n.label.as_str()).collect();
    assert_eq!(labels[..2], ["expected", "actual"], "{text}");
    assert!(err.hint.is_some(), "{text}");
    err
}

fn el(locator: &str, matcher: Matcher) -> Expectation {
    Expectation::Element {
        locator: locator.to_owned(),
        matcher,
    }
}

fn text_is(text: &str, exact: bool) -> Matcher {
    Matcher::Text {
        text: text.to_owned(),
        exact,
    }
}

/// Every matcher on the fixture: whether it holds when the page loads,
/// and after `later()` changed the page (400ms later).
fn matrix() -> Vec<(Expectation, bool, bool)> {
    let count = |count, min, max| Matcher::Count { count, min, max };
    vec![
        (el("#ghost", Matcher::Visible), false, true),
        (el("#ghost", Matcher::Hidden), true, false),
        (el("#heading", Matcher::Attached), true, false),
        (el("#heading", Matcher::Detached), false, true),
        (el("#save", Matcher::Enabled), false, true),
        (el("#save", Matcher::Disabled), true, false),
        (el("#locked", Matcher::Editable), false, true),
        (el("#news", Matcher::Checked), false, true),
        (el("#agree", Matcher::Unchecked), false, true),
        (el("#email", Matcher::Focused), false, true),
        (el("#far", Matcher::InViewport), false, true),
        (el("#status", text_is("Saved", false)), false, true),
        (el("#status", text_is("/^load/i", false)), true, false),
        (el("#status", text_is("Loading…", true)), true, false),
        (
            el(
                "#email",
                Matcher::Value {
                    value: "bob@example.com".into(),
                },
            ),
            false,
            true,
        ),
        (
            el(
                "#colours",
                Matcher::Values {
                    values: vec!["green".into(), "red".into()],
                },
            ),
            true,
            false,
        ),
        (
            el(
                "#home",
                Matcher::Attr {
                    name: "aria-expanded".into(),
                    value: Some("true".into()),
                },
            ),
            false,
            true,
        ),
        (
            el(
                "#home",
                Matcher::Attr {
                    name: "data-kind".into(),
                    value: None,
                },
            ),
            true,
            true,
        ),
        (
            el(
                "#home",
                Matcher::Class {
                    name: "active".into(),
                },
            ),
            false,
            true,
        ),
        (
            el(
                "#paint",
                Matcher::Css {
                    property: "color".into(),
                    value: "green".into(),
                },
            ),
            false,
            true,
        ),
        (
            el(
                "#paint",
                Matcher::Css {
                    property: "color".into(),
                    value: "rgb(255, 0, 0)".into(),
                },
            ),
            true,
            false,
        ),
        (el(".row", count(Some(4), None, None)), false, true),
        (el(".row", count(None, Some(2), Some(3))), true, false),
        (
            Expectation::Title {
                text: "Done".into(),
                exact: true,
            },
            false,
            true,
        ),
        (
            Expectation::Url {
                pattern: "#done".into(),
            },
            false,
            true,
        ),
    ]
}

#[tokio::test]
async fn every_matcher_passes_fails_and_waits() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    let page = site.url("/expect/index.html");
    goto(&engine, &page).await;
    for (expectation, before, _) in matrix() {
        if before {
            passes(&engine, &expectation, false).await;
            fails(&engine, &expectation, true).await;
        } else {
            fails(&engine, &expectation, false).await;
            passes(&engine, &expectation, true).await;
        }
    }
    // What holds only after the page changed: retried until it does.
    for (expectation, before, after) in matrix() {
        if before == after {
            continue;
        }
        goto(&engine, &page).await;
        eval(&engine, "later()").await;
        let data = passes(&engine, &expectation, before).await;
        assert!(
            data.elapsed_ms >= 200,
            "{expectation:?} passed after {}ms",
            data.elapsed_ms
        );
        if !matches!(expectation, Expectation::Url { .. }) {
            assert!(data.retries > 0, "{expectation:?}: {data:?}");
        }
    }
}

#[tokio::test]
async fn retries_count_across_a_navigation() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/expect/index.html")).await;

    // The page goes to another document 900ms into a 1s assertion: the
    // probe starts again there, and its count with it.
    eval(
        &engine,
        "setTimeout(() => { location.href = '?again'; }, 900)",
    )
    .await;
    let Err(err) = expect(
        &engine,
        &el("#status", text_is("Never", false)),
        false,
        1_000,
    )
    .await
    else {
        panic!("text Never passed");
    };
    let text = err.to_text();
    assert_eq!(err.kind, ErrorKind::ExpectFailed, "{text}");
    passes(
        &engine,
        &Expectation::Url {
            pattern: "again".into(),
        },
        false,
    )
    .await;
    // At least one try per 100ms in the first document, which the second
    // (under 100ms) cannot reach alone.
    let retries = err.details.as_ref().unwrap()["retries"].as_u64().unwrap();
    assert!(retries >= 9, "{retries} retries: {text}");
}

#[tokio::test]
async fn failures_say_what_they_saw() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/expect/index.html")).await;

    let err = fails(&engine, &el("#status", text_is("Saved", false)), false).await;
    let text = err.to_text();
    assert!(
        text.contains("  expected: text containing \"Saved\"\n"),
        "{text}"
    );
    assert!(
        text.contains("  actual: text \"Loading…\" (after 300ms, "),
        "{text}"
    );
    assert!(text.contains("  element: status \"Loading…\" [e"), "{text}");
    assert!(err.hint.unwrap().starts_with("chrome-cdp text e"));
    let details = err.details.unwrap();
    assert_eq!(details["actual"], "Loading…");
    assert!(details["retries"].as_u64().unwrap() > 0);
    assert_eq!(details["elements"][0]["role"], "status");

    // Matchers of one element fail when several match, naming them.
    let err = fails(&engine, &el(".row", Matcher::Visible), false).await;
    let text = err.to_text();
    assert!(text.contains("  actual: 3 elements match (after"), "{text}");
    assert!(text.contains("  elements: listitem \"One\" [e"), "{text}");
    assert!(
        err.hint
            .as_deref()
            .unwrap()
            .contains("(the first of 3 matches"),
        "{text}"
    );
    // `--not` does not pass them either.
    fails(&engine, &el(".row", Matcher::Visible), true).await;

    // Nothing matches.
    let err = fails(&engine, &el("#nope", Matcher::Visible), false).await;
    assert!(
        err.to_text()
            .contains("  actual: no element matches (after")
    );
    assert!(
        passes(&engine, &el("#nope", Matcher::Hidden), false)
            .await
            .actual
            .is_string()
    );

    // Not a field, not a checkbox.
    let err = fails(
        &engine,
        &el("#status", Matcher::Value { value: "x".into() }),
        false,
    )
    .await;
    assert!(
        err.to_text().contains("  actual: it is a <p>, not a field"),
        "{}",
        err.to_text()
    );
    fails(&engine, &el("#status", Matcher::Checked), true).await;

    // A ref whose element is gone: gone counts as hidden and detached.
    let snapshot = engine
        .snapshot(SnapshotRequest {
            tab: None,
            interactive: true,
            scope: None,
            depth: None,
            diff: false,
            all_refs: false,
            all: false,
            text_limit: 100,
            no_text: false,
            engine: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap();
    let line = snapshot
        .text
        .lines()
        .find(|l| l.contains("button \"Save\""))
        .unwrap()
        .to_owned();
    let reference = line
        .split('[')
        .nth(1)
        .unwrap()
        .split(']')
        .next()
        .unwrap()
        .to_owned();
    eval(&engine, "document.getElementById('save').remove()").await;
    passes(&engine, &el(&reference, Matcher::Hidden), false).await;
    passes(&engine, &el(&reference, Matcher::Detached), false).await;
    let err = fails(&engine, &el(&reference, Matcher::Visible), false).await;
    assert!(
        err.to_text()
            .contains("  actual: its element is gone from the page"),
        "{}",
        err.to_text()
    );

    // A bad regex fails at once, as a usage error.
    let err = expect(&engine, &el("#status", text_is("/(/", false)), false, 5_000)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadArgs, "{}", err.to_text());
    assert!(
        err.message.contains("is not a valid regex"),
        "{}",
        err.message
    );
}

#[tokio::test]
async fn errors_and_failed_requests_since_the_page_or_a_mark() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    let no_errors = |since: Option<&str>| Expectation::NoErrors {
        since: since.map(str::to_owned),
        all_tabs: false,
    };
    let no_failed = |origin: Option<&str>| Expectation::NoFailedRequests {
        since: None,
        origin: origin.map(str::to_owned),
    };

    goto(&engine, &site.url("/expect/index.html")).await;
    passes(&engine, &no_errors(None), false).await;
    // The browser's own /favicon.ico request (a 404 here) does not count.
    passes(&engine, &no_failed(None), false).await;
    engine
        .mark(MarkRequest {
            name: "before".into(),
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
    eval(&engine, "fail()").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let err = fails(&engine, &no_errors(Some("before")), false).await;
    let text = err.to_text();
    assert!(text.contains("  actual: 2 errors\n"), "{text}");
    assert!(
        text.contains("  error: boom: the save failed (index.html:"),
        "{text}"
    );
    assert!(text.contains("  error: TypeError: late trouble"), "{text}");
    let err = fails(&engine, &no_failed(Some("self")), false).await;
    assert!(err.to_text().contains("  failed: #"), "{}", err.to_text());
    assert!(err.to_text().contains("GET /expect/missing.json (404)"));
    passes(&engine, &no_failed(Some("https://example.com")), false).await;

    // `errors` and `network` read from a mark too.
    let since = Since::Mark {
        name: "before".into(),
    };
    let errors = engine
        .console(ConsoleRequest {
            errors: true,
            tab: None,
            level: Level::Error,
            since: since.clone(),
            grep: None,
            limit: 0,
            wait_ms: 0,
            sourcemaps: false,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap();
    let errors: ConsoleData = serde_json::from_value(errors.data).unwrap();
    assert!(
        errors.entries.iter().any(|e| e.text.contains("boom")),
        "{errors:?}"
    );
    let requests = engine
        .network(NetworkRequest {
            tab: None,
            failed: false,
            types: Vec::new(),
            url: None,
            method: None,
            status: Vec::new(),
            since,
            limit: 0,
            wait_ms: 0,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap();
    let requests: NetworkData = serde_json::from_value(requests.data).unwrap();
    // Chrome may ask for the favicon after the mark too.
    let urls: Vec<&str> = requests
        .requests
        .iter()
        .map(|r| r.url.as_str())
        .filter(|u| !u.ends_with("/favicon.ico"))
        .collect();
    assert_eq!(urls, [site.url("/expect/missing.json")], "{urls:?}");

    // A mark after them leaves them out.
    engine
        .mark(MarkRequest {
            name: "after".into(),
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
    passes(&engine, &no_errors(Some("after")), false).await;
    let err = expect(&engine, &no_errors(Some("nope")), false, 5_000)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadArgs);
    assert!(err.to_text().contains("  marks: after, before\n"));

    // A new page starts over.
    goto(&engine, &site.url("/expect/index.html")).await;
    passes(&engine, &no_errors(None), false).await;
    // --all-tabs does not: every error since each tab opened.
    let all_tabs = Expectation::NoErrors {
        since: None,
        all_tabs: true,
    };
    let err = fails(&engine, &all_tabs, false).await;
    let text = err.to_text();
    assert!(
        text.contains("  expected: no uncaught exceptions or console errors in any tab\n"),
        "{text}"
    );
    assert!(text.contains("  actual: 2 errors\n"), "{text}");
    assert!(
        text.contains("  error: t1: boom: the save failed (index.html:"),
        "{text}"
    );
    assert!(text.contains("hint: chrome-cdp errors"), "{text}");

    // Errors while loading count, since the page loaded.
    goto(&engine, &site.url("/expect/errors.html")).await;
    fails(&engine, &no_errors(None), false).await;
}

#[tokio::test]
async fn accessibility_rules() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    let a11y = |scope: Option<&str>, level| Expectation::A11y {
        scope: scope.map(str::to_owned),
        level,
    };

    goto(&engine, &site.url("/expect/a11y-ok.html")).await;
    passes(&engine, &a11y(None, A11yLevel::AA), false).await;

    goto(&engine, &site.url("/expect/a11y.html")).await;
    let err = fails(&engine, &a11y(None, A11yLevel::AA), false).await;
    let details = err.details.unwrap();
    let rules: Vec<&str> = details["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["rule"].as_str().unwrap())
        .collect();
    assert_eq!(
        rules,
        [
            "html-has-lang",
            "image-alt",
            "button-name",
            "label",
            "aria-hidden-focus",
            "meta-viewport",
            "color-contrast",
        ]
    );
    // Large text needs 3:1 only: the big grey text passes.
    let contrast = &details["violations"][6];
    assert_eq!(contrast["count"], 1);
    assert_eq!(
        contrast["elements"][0]["name"],
        "Fine print nobody can read"
    );
    assert!(contrast["elements"][0]["ref"].is_string());

    // Level A leaves out the AA rules.
    let err = fails(&engine, &a11y(None, A11yLevel::A), false).await;
    let details = err.details.unwrap();
    assert_eq!(details["rules"], 9);
    assert_eq!(details["violations"].as_array().unwrap().len(), 5);

    // A scope checks only its element, and no page rules.
    let err = fails(&engine, &a11y(Some("#signup"), A11yLevel::AA), false).await;
    let details = err.details.unwrap();
    assert_eq!(details["violations"].as_array().unwrap().len(), 1);
    assert_eq!(details["violations"][0]["rule"], "label");

    // Iframes are checked too, cross-site ones included, each finding
    // naming its frame; page rules are the page's only.
    goto(&engine, &site.url("/expect/a11y-frames.html")).await;
    let err = fails(&engine, &a11y(None, A11yLevel::AA), false).await;
    let text = err.to_text();
    let details = err.details.unwrap();
    let rules: Vec<(&str, u64)> = details["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (v["rule"].as_str().unwrap(), v["count"].as_u64().unwrap()))
        .collect();
    assert_eq!(rules, [("button-name", 2), ("label", 2)], "{text}");
    let label = &details["violations"][1]["elements"];
    assert!(label[0]["frame"].is_string(), "{label}");
    assert_ne!(label[0]["frame"], label[1]["frame"]);
    assert!(label[0]["ref"].is_string(), "{label}");
    assert!(
        text.contains(" in iframe \"pay\" is a form field with no label"),
        "{text}"
    );
    assert!(
        text.contains(" in iframe \"same\" is a form field with no label"),
        "{text}"
    );
}
