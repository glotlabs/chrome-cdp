//! `why <loc>` against a real Chrome (`CHROME_PATH`; skipped without it
//! unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture site's
//! `why/` pages (plan 16, milestone 16.1).
//!
//! Each case's text output is compared with `testdata/site/why/<case>.expected`
//! (`UPDATE_GOLDEN=1` rewrites them), with the numbers that depend on
//! fonts and timing masked. A table test runs `why` and then the action
//! itself on every case, and checks they agree: `why` says "would act now"
//! exactly when the action passes at once.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::Host;
use engine::{
    Action, ActionRequest, Button, Engine, NavigateRequest, Navigation, OpenRequest, WaitUntil,
    WhyFor, WhyRequest,
};
use fixtures::Fixtures;
use protocol::{ErrorInfo, Output};
use regex::Regex;

const TIMEOUT_MS: u64 = 30_000;

/// The timeout of an action that `why` says would wait.
const SHORT: Duration = Duration::from_millis(1500);

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

async fn why(engine: &Engine, locator: &str, action: WhyFor, no_scroll: bool) -> Output {
    engine
        .why(WhyRequest {
            tab: None,
            locator: locator.to_owned(),
            action,
            no_scroll,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("why {locator}: {}", e.to_text()))
}

/// The output with what depends on fonts and timing masked: box sizes
/// and places, scroll distances, and how far an animation moved.
fn masked(text: &str) -> String {
    let boxes = Regex::new(r"\d+×\d+ at -?\d+,-?\d+").unwrap();
    let points = Regex::new(r"reaches it at -?\d+,-?\d+").unwrap();
    let scrolls = Regex::new(r"by -?\d+px").unwrap();
    let moved = Regex::new(r"moved (less than 1|\d+)px").unwrap();
    let at = Regex::new(r"x=-?\d+ y=-?\d+").unwrap();
    let viewport = Regex::new(r"viewport is \d+x\d+").unwrap();
    let text = boxes.replace_all(text, "W×H at X,Y");
    let text = points.replace_all(&text, "reaches it at X,Y");
    let text = scrolls.replace_all(&text, "by Npx");
    let text = moved.replace_all(&text, "moved Npx");
    let text = at.replace_all(&text, "x=X y=Y");
    viewport.replace_all(&text, "viewport is WxH").into_owned()
}

/// Compares the masked text output with golden file `case`, or rewrites
/// it with `UPDATE_GOLDEN=1`.
fn golden(case: &str, output: &Output) {
    let path = fixtures::site_dir()
        .join("why")
        .join(format!("{case}.expected"));
    let actual = format!("{}\n", masked(&output.text));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
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

/// The golden cases: `(case, page, locator, --for, --no-scroll)`.
const CASES: &[(&str, &str, &str, WhyFor, bool)] = &[
    ("fine", "cases", "#fine", WhyFor::Click, false),
    ("cookie", "cases", "#cookie", WhyFor::Click, false),
    ("fieldset", "cases", "#fieldset", WhyFor::Click, false),
    // Hover does not need it enabled.
    ("fieldset-hover", "cases", "#fieldset", WhyFor::Hover, false),
    ("aria", "cases", "#aria", WhyFor::Click, false),
    ("inert", "cases", "#inert", WhyFor::Click, false),
    ("modal", "modal", "#behind", WhyFor::Click, false),
    ("no-pointer", "cases", "#no-pointer", WhyFor::Click, false),
    ("pulse", "cases", "#pulse", WhyFor::Click, false),
    ("grandchild", "cases", "#grandchild", WhyFor::Click, false),
    ("clear", "cases", "#clear", WhyFor::Click, false),
    ("clipped", "cases", "#clipped", WhyFor::Click, false),
    ("readonly", "cases", "#readonly", WhyFor::Fill, false),
    // A button can never take text.
    ("fine-fill", "cases", "#fine", WhyFor::Fill, false),
    ("far", "cases", "#far", WhyFor::Click, false),
    ("far-no-scroll", "cases", "#far", WhyFor::Click, true),
    ("frame", "cases", "frame=pay >> #pay", WhyFor::Click, false),
    ("file-off", "cases", "#file-off", WhyFor::Upload, false),
    (
        "twin",
        "cases",
        "role=button[name=\"Twin\"]",
        WhyFor::Click,
        false,
    ),
    ("nope", "cases", "#nope", WhyFor::Click, false),
];

#[tokio::test]
async fn explains_each_case() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "why-cases").await;
    for &(case, page, locator, action, no_scroll) in CASES {
        goto(&engine, &site.url(&format!("/why/{page}.html"))).await;
        let output = why(&engine, locator, action, no_scroll).await;
        golden(case, &output);
    }
    engine.close().await;
}

#[tokio::test]
async fn names_causes_with_refs_that_work() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "why-refs").await;
    goto(&engine, &site.url("/why/cases.html")).await;

    // The covering banner and its dismiss button get refs; clicking the
    // dismiss button frees the element.
    let output = why(&engine, "#cookie", WhyFor::Click, false).await;
    let hit = &output.data["checks"][5];
    assert_eq!(hit["name"], "hit test", "{}", output.data);
    assert_eq!(hit["status"], "failed");
    assert_eq!(hit["element"]["class"], "cookie-banner");
    let dismiss = hit["dismiss"]["ref"].as_str().unwrap().to_owned();
    act(&engine, click(&dismiss), SHORT).await.unwrap();
    let output = why(&engine, "#cookie", WhyFor::Click, false).await;
    assert_eq!(output.data["verdict"]["acts_now"], true, "{}", output.text);

    // The disabled fieldset is named with a ref that locates it.
    let output = why(&engine, "#fieldset", WhyFor::Click, false).await;
    let enabled = &output.data["checks"][3];
    assert_eq!(enabled["name"], "enabled");
    assert_eq!(enabled["element"]["role"], "group");
    assert!(enabled["element"]["ref"].is_string(), "{}", output.data);

    // The element itself gets a ref, and the output uses it.
    let reference = output.data["element"]["ref"].as_str().unwrap().to_owned();
    let again = why(&engine, &reference, WhyFor::Click, false).await;
    assert!(
        again
            .text
            .starts_with(&format!("why {reference} button \"Ship\" (for click)")),
        "{}",
        again.text
    );
    assert_eq!(
        again.quiet.as_deref(),
        Some("click would wait until the timeout: disabled")
    );
    engine.close().await;
}

fn click(locator: &str) -> Action {
    Action::Click {
        locator: locator.to_owned(),
        button: Button::default(),
        count: 1,
        modifiers: Vec::new(),
        position: None,
        force: false,
        dblclick: false,
    }
}

async fn act(engine: &Engine, action: Action, timeout: Duration) -> Result<Output, ErrorInfo> {
    engine
        .act(ActionRequest {
            action,
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap(),
            no_wait_after: true,
            snap: false,
            max_chars: 0,
        })
        .await
}

/// The action `why --for action` explains, on `locator`.
fn action_of(action: WhyFor, locator: &str, file: &Path) -> Action {
    let locator = locator.to_owned();
    match action {
        WhyFor::Click => click(&locator),
        WhyFor::Hover => Action::Hover {
            locator,
            modifiers: Vec::new(),
            position: None,
            force: false,
        },
        WhyFor::Fill => Action::Fill {
            locator,
            text: "x".to_owned(),
            force: false,
        },
        WhyFor::Type => Action::Type {
            text: "x".to_owned(),
            locator: Some(locator),
            delay_ms: 0,
            force: false,
        },
        WhyFor::Check => Action::Check {
            locator,
            checked: true,
            force: false,
        },
        WhyFor::Select => Action::Select {
            locator,
            values: vec!["M".to_owned()],
            force: false,
        },
        WhyFor::Upload => Action::Upload {
            locator,
            files: vec![file.display().to_string()],
            clear: false,
            force: false,
            drop: false,
        },
        WhyFor::Drag => Action::Drag {
            from: locator,
            to: "#fine".to_owned(),
            steps: 5,
            force: false,
        },
    }
}

#[tokio::test]
async fn agrees_with_the_actions() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "why-agrees").await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("receipt.pdf");
    std::fs::write(&file, "%PDF-1.4\n").unwrap();

    let mut table: Vec<(&str, &str, WhyFor)> = CASES
        .iter()
        .filter(|c| !c.4)
        .map(|&(_, page, locator, action, _)| (page, locator, action))
        .collect();
    table.extend([
        ("cases", "#name", WhyFor::Fill),
        ("cases", "#name", WhyFor::Type),
        ("cases", "#readonly", WhyFor::Type),
        ("cases", "#size", WhyFor::Select),
        ("cases", "#agree", WhyFor::Check),
        ("cases", "#file", WhyFor::Upload),
        ("cases", "#fine", WhyFor::Hover),
        ("cases", "#cookie", WhyFor::Hover),
        ("cases", "#fieldset", WhyFor::Drag),
        ("cases", "#inert", WhyFor::Fill),
        ("modal", "#behind", WhyFor::Hover),
    ]);
    let mut disagree = Vec::new();
    for (page, locator, action) in table {
        goto(&engine, &site.url(&format!("/why/{page}.html"))).await;
        let explained = why(&engine, locator, action, false).await;
        let verdict = &explained.data["verdict"];
        goto(&engine, &site.url(&format!("/why/{page}.html"))).await;
        let started = Instant::now();
        let acted = act(&engine, action_of(action, locator, &file), SHORT).await;
        let took = started.elapsed();
        let what = format!("{} {locator} ({page})", action.name());
        match (&acted, verdict["acts_now"].as_bool().unwrap()) {
            (Ok(_), true) => {}
            (Err(err), false) => {
                // Failing at once and waiting until the timeout are told
                // apart (the pipeline gives up a retry before its deadline).
                let at_once = verdict["fails"].is_string();
                if at_once && took >= SHORT / 2 {
                    disagree.push(format!(
                        "{what}: why fails at once, the action waited: {}",
                        err.to_text()
                    ));
                }
                if !at_once && took < SHORT / 2 {
                    disagree.push(format!(
                        "{what}: why waits, the action failed at once: {}",
                        err.to_text()
                    ));
                }
            }
            (Ok(_), false) => disagree.push(format!(
                "{what}: why says it waits, but it acted:\n{}",
                explained.text
            )),
            (Err(err), true) => disagree.push(format!(
                "{what}: why says it acts now, but it failed: {}\n{}",
                err.to_text(),
                explained.text
            )),
        }
    }
    assert!(disagree.is_empty(), "{}", disagree.join("\n\n"));
    engine.close().await;
}
