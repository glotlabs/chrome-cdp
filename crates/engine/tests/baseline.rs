//! `expect-snapshot` and `expect-screenshot` against a real Chrome
//! (`CHROME_PATH`; skipped without it unless `CHROME_CDP_REQUIRE_CHROME` is
//! set) and the fixture site (plan 09, milestone 9.2): each passing,
//! failing and passing once the page gets there, and the baseline created,
//! updated, found up to date, or missing in CI.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use browser::Host;
use engine::{
    Engine, ExpectScreenshotRequest, ExpectSnapshotRequest, NavigateRequest, Navigation,
    OpenRequest, WaitUntil, World,
};
use fixtures::Fixtures;
use protocol::schema::{BaselineResult, ExpectScreenshotData, ExpectSnapshotData};
use protocol::{ErrorInfo, ErrorKind, Output};

const TIMEOUT_MS: u64 = 30_000;
const TIMEOUT: Duration = Duration::from_secs(10);

/// How long a comparison that should fail gets.
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

async fn start(path: &Path, name: &str, out: &Path) -> Arc<Engine> {
    let engine = Engine::new(name, None, None);
    engine.set_output_dir(out.to_owned());
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

async fn goto(engine: &Engine, path: &str) {
    let url = format!("{}{path}", Fixtures::shared().a());
    engine
        .navigate(NavigateRequest {
            navigation: Navigation::Goto {
                url: url.clone(),
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

fn snapshot_request(path: &Path) -> ExpectSnapshotRequest {
    ExpectSnapshotRequest {
        name: "login".into(),
        path: path.to_owned(),
        scope: None,
        full: false,
        update: false,
        create: true,
        dir: None,
        tab: None,
        strict_refs: false,
        testid_attribute: None,
        timeout_ms: 5_000,
        max_chars: 20_000,
    }
}

async fn snapshot(engine: &Engine, req: ExpectSnapshotRequest) -> Result<Output, ErrorInfo> {
    Box::pin(engine.expect_snapshot(req)).await
}

async fn snapshot_ok(engine: &Engine, req: ExpectSnapshotRequest) -> (Output, ExpectSnapshotData) {
    let output = snapshot(engine, req)
        .await
        .unwrap_or_else(|e| panic!("{}", e.to_text()));
    let data = serde_json::from_value(output.data.clone()).unwrap();
    (output, data)
}

/// The labels of `err`'s notes.
fn labels(err: &ErrorInfo) -> Vec<&str> {
    err.notes.iter().map(|n| n.label.as_str()).collect()
}

fn note<'a>(err: &'a ErrorInfo, label: &str) -> &'a str {
    &err.notes.iter().find(|n| n.label == label).unwrap().text
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expect_snapshot_against_a_text_baseline() {
    let Some(chrome) = chrome_path() else { return };
    let out = tempfile::tempdir().unwrap();
    let engine = start(&chrome, "engine-baseline-snapshot", out.path()).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("forms/login.txt");
    goto(&engine, "/expect/baseline.html").await;

    // In CI, a missing baseline fails and nothing is written.
    let err = snapshot(
        &engine,
        ExpectSnapshotRequest {
            create: false,
            ..snapshot_request(&path)
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::ExpectFailed, "{}", err.to_text());
    assert_eq!(
        err.message,
        "expect-snapshot login failed: there is no baseline yet"
    );
    assert_eq!(
        err.hint.as_deref(),
        Some(
            "chrome-cdp expect-snapshot login --update --session engine-baseline-snapshot   \
             (stores the page as the baseline; commit the file)"
        )
    );
    assert!(!path.exists());

    // The first run stores it: `snapshot -i` without refs.
    let (output, data) = snapshot_ok(&engine, snapshot_request(&path)).await;
    assert_eq!(data.result, BaselineResult::Created);
    let stored = std::fs::read_to_string(&path).unwrap();
    assert_eq!(stored, format!("{}\n", data.snapshot));
    assert!(stored.contains("    - button \"Sign in\"\n"), "{stored}");
    assert!(!stored.contains("[e"), "refs in the baseline: {stored}");
    assert!(!stored.contains("heading"), "not only -i: {stored}");
    assert_eq!(data.lines, 7);
    assert!(
        output.text.starts_with(&format!(
            "created baseline {} (7 lines):\n- main",
            path.display()
        )),
        "{}",
        output.text
    );

    // Pass, and `--update` of a matching page changes nothing.
    let (output, data) = snapshot_ok(&engine, snapshot_request(&path)).await;
    assert_eq!(data.result, BaselineResult::Passed);
    assert_eq!(data.retries, 0);
    assert!(
        output
            .text
            .starts_with("passed: expect-snapshot login: 7 lines as in the baseline ("),
        "{}",
        output.text
    );
    let (_, data) = snapshot_ok(
        &engine,
        ExpectSnapshotRequest {
            update: true,
            ..snapshot_request(&path)
        },
    )
    .await;
    assert_eq!(data.result, BaselineResult::Unchanged);

    // Line ends and trailing spaces in the file do not count.
    std::fs::write(&path, stored.replace('\n', "  \r\n") + "\r\n").unwrap();
    snapshot_ok(&engine, snapshot_request(&path)).await;

    // Fail, with the diff.
    eval(
        &engine,
        "document.getElementById('go').textContent = 'Continue'",
    )
    .await;
    let err = snapshot(
        &engine,
        ExpectSnapshotRequest {
            timeout_ms: SHORT_MS,
            ..snapshot_request(&path)
        },
    )
    .await
    .unwrap_err();
    let text = err.to_text();
    eprintln!("{text}");
    assert_eq!(err.kind, ErrorKind::ExpectFailed, "{text}");
    assert_eq!(
        err.message,
        "expect-snapshot login failed: the page differs from the baseline"
    );
    assert_eq!(labels(&err), ["baseline", "actual", "diff"], "{text}");
    assert!(
        note(&err, "actual").starts_with("7 lines, 1 line added, 1 removed (after 300ms, "),
        "{text}"
    );
    assert!(
        text.contains("\n    -    - button \"Sign in\"\n    +    - button \"Continue\"\n"),
        "{text}"
    );
    assert_eq!(
        err.hint.as_deref(),
        Some(
            "chrome-cdp expect-snapshot login --update --session engine-baseline-snapshot   \
             (if the page is right and the baseline is out of date)"
        )
    );

    // Pass once the page gets there.
    eval(
        &engine,
        "setTimeout(() => { document.getElementById('go').textContent = 'Sign in' }, 400)",
    )
    .await;
    let (_, data) = snapshot_ok(&engine, snapshot_request(&path)).await;
    assert_eq!(data.result, BaselineResult::Passed);
    assert!(data.retries > 0, "{data:?}");
    assert!(data.elapsed_ms >= 300, "{data:?}");

    // `--update` stores the change and shows it.
    eval(
        &engine,
        "document.getElementById('go').textContent = 'Continue'",
    )
    .await;
    let (output, data) = snapshot_ok(
        &engine,
        ExpectSnapshotRequest {
            update: true,
            ..snapshot_request(&path)
        },
    )
    .await;
    assert_eq!(data.result, BaselineResult::Updated);
    assert!(
        output.text.starts_with(&format!(
            "updated baseline {} (1 line added, 1 removed):\n--- baseline\n+++ page\n",
            path.display()
        )),
        "{}",
        output.text
    );
    assert!(data.diff.unwrap().contains("+    - button \"Continue\"\n"));
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("button \"Continue\"")
    );

    // --scope and --full.
    let form = dir.path().join("form.txt");
    let (_, data) = snapshot_ok(
        &engine,
        ExpectSnapshotRequest {
            name: "form".into(),
            scope: Some("#login".into()),
            ..snapshot_request(&form)
        },
    )
    .await;
    assert!(
        data.snapshot.starts_with("- form\n  - textbox \"Email\""),
        "{}",
        data.snapshot
    );
    let full = dir.path().join("full.txt");
    let (_, data) = snapshot_ok(
        &engine,
        ExpectSnapshotRequest {
            full: true,
            ..snapshot_request(&full)
        },
    )
    .await;
    assert!(
        data.snapshot.contains("- heading \"Sign in\" (h1)"),
        "{}",
        data.snapshot
    );

    // A scope that is not there fails like `snapshot --scope` does.
    let err = snapshot(
        &engine,
        ExpectSnapshotRequest {
            scope: Some("#nope".into()),
            timeout_ms: SHORT_MS,
            ..snapshot_request(&form)
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound, "{}", err.to_text());

    // A page that never settles is stored anyway, with a warning.
    goto(&engine, "/expect/baseline.html?tick").await;
    let (output, data) = snapshot_ok(
        &engine,
        ExpectSnapshotRequest {
            full: true,
            update: true,
            timeout_ms: 500,
            ..snapshot_request(&full)
        },
    )
    .await;
    assert_eq!(data.result, BaselineResult::Updated);
    assert!(
        output.warnings[0].starts_with("the page kept changing for "),
        "{:?}",
        output.warnings
    );
}

fn screenshot_request(path: &Path) -> ExpectScreenshotRequest {
    ExpectScreenshotRequest {
        name: "box".into(),
        path: path.to_owned(),
        locator: Some("#box".into()),
        full: false,
        mask: Vec::new(),
        threshold: 0.1,
        max_diff_pixels: 0,
        update: false,
        create: true,
        dir: None,
        tab: None,
        strict_refs: false,
        testid_attribute: None,
        timeout_ms: 5_000,
    }
}

async fn screenshot(engine: &Engine, req: ExpectScreenshotRequest) -> Result<Output, ErrorInfo> {
    Box::pin(engine.expect_screenshot(req)).await
}

async fn screenshot_ok(
    engine: &Engine,
    req: ExpectScreenshotRequest,
) -> (Output, ExpectScreenshotData) {
    let output = screenshot(engine, req)
        .await
        .unwrap_or_else(|e| panic!("{}", e.to_text()));
    let data = serde_json::from_value(output.data.clone()).unwrap();
    (output, data)
}

async fn screenshot_fails(engine: &Engine, req: ExpectScreenshotRequest) -> ErrorInfo {
    let err = screenshot(
        engine,
        ExpectScreenshotRequest {
            timeout_ms: SHORT_MS,
            ..req
        },
    )
    .await
    .unwrap_err();
    eprintln!("{}", err.to_text());
    assert_eq!(err.kind, ErrorKind::ExpectFailed, "{}", err.to_text());
    err
}

/// The size of the PNG at `path`.
fn png_size(path: &Path) -> (u32, u32) {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let info = decoder.read_info().unwrap();
    (info.info().width, info.info().height)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expect_screenshot_against_a_png_baseline() {
    let Some(chrome) = chrome_path() else { return };
    let out = tempfile::tempdir().unwrap();
    let engine = start(&chrome, "engine-baseline-screenshot", out.path()).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("box-test.png");
    goto(&engine, "/expect/baseline.html").await;

    let err = screenshot_fails(
        &engine,
        ExpectScreenshotRequest {
            create: false,
            ..screenshot_request(&path)
        },
    )
    .await;
    assert_eq!(
        err.message,
        "expect-screenshot box failed: there is no baseline yet"
    );
    assert!(!path.exists());

    // Created, then passed.
    let (output, data) = screenshot_ok(&engine, screenshot_request(&path)).await;
    assert_eq!(data.result, BaselineResult::Created);
    assert_eq!((data.width, data.height), (120, 60));
    assert_eq!(png_size(&path), (120, 60));
    assert_eq!(
        output.text,
        format!("created baseline {} (120x60)", path.display())
    );
    let (output, data) = screenshot_ok(&engine, screenshot_request(&path)).await;
    assert_eq!(data.result, BaselineResult::Passed);
    assert_eq!(data.diff_pixels, 0);
    assert!(
        output
            .text
            .starts_with("passed: expect-screenshot box: 120x60 as in the baseline ("),
        "{}",
        output.text
    );

    // A colour a little off passes the threshold, but not a threshold of 0.
    eval(
        &engine,
        "document.getElementById('box').style.background = '#3467cc'",
    )
    .await;
    screenshot_ok(&engine, screenshot_request(&path)).await;
    let err = screenshot_fails(
        &engine,
        ExpectScreenshotRequest {
            threshold: 0.0,
            ..screenshot_request(&path)
        },
    )
    .await;
    assert_eq!(
        err.message,
        "expect-screenshot box failed: 7,200 pixels (100.00%) differ from the baseline"
    );

    // Another colour fails, saving the screenshot and a diff image.
    eval(
        &engine,
        "document.getElementById('box').style.background = '#cc3333'",
    )
    .await;
    let err = screenshot_fails(&engine, screenshot_request(&path)).await;
    assert_eq!(labels(&err), ["baseline", "actual", "diff"]);
    let details = err.details.clone().unwrap();
    assert_eq!(details["diff_pixels"], 7200);
    for file in ["actual", "diff"] {
        let saved = PathBuf::from(details[file].as_str().unwrap());
        assert!(saved.starts_with(out.path().join("shots")), "{saved:?}");
        assert_eq!(png_size(&saved), (120, 60), "{file}");
    }
    assert!(
        note(&err, "actual").contains("(120x60, after 300ms, "),
        "{}",
        err.to_text()
    );
    assert_eq!(
        err.hint.as_deref(),
        Some(
            "chrome-cdp expect-screenshot box '#box' --update --session \
             engine-baseline-screenshot   (if the page is right and the baseline is out of date)"
        )
    );

    // --max-diff-pixels lets that many through.
    let (output, data) = screenshot_ok(
        &engine,
        ExpectScreenshotRequest {
            max_diff_pixels: 7200,
            ..screenshot_request(&path)
        },
    )
    .await;
    assert_eq!(data.diff_pixels, 7200);
    assert!(
        output.text.starts_with(
            "passed: expect-screenshot box: 120x60 as in the baseline but for 7,200 pixels ("
        ),
        "{}",
        output.text
    );

    // Pass once the page gets there.
    eval(
        &engine,
        "setTimeout(() => { document.getElementById('box').style.background = '#3366cc' }, 400)",
    )
    .await;
    let (_, data) = screenshot_ok(&engine, screenshot_request(&path)).await;
    assert!(data.retries > 0, "{data:?}");

    // Another size fails without a diff image.
    eval(
        &engine,
        "document.getElementById('box').style.width = '100px'",
    )
    .await;
    let err = screenshot_fails(&engine, screenshot_request(&path)).await;
    assert_eq!(
        err.message,
        "expect-screenshot box failed: the page is 100x60, the baseline 120x60"
    );
    assert_eq!(labels(&err), ["baseline", "actual"]);

    // --update stores it.
    let (output, data) = screenshot_ok(
        &engine,
        ExpectScreenshotRequest {
            update: true,
            ..screenshot_request(&path)
        },
    )
    .await;
    assert_eq!(data.result, BaselineResult::Updated);
    assert_eq!(
        output.text,
        format!("updated baseline {} (100x60; was 120x60)", path.display())
    );
    assert_eq!(png_size(&path), (100, 60));
    let (_, data) = screenshot_ok(
        &engine,
        ExpectScreenshotRequest {
            update: true,
            ..screenshot_request(&path)
        },
    )
    .await;
    assert_eq!(data.result, BaselineResult::Unchanged);

    // --mask hides what changes all the time.
    goto(&engine, "/expect/baseline.html?tick").await;
    let page = dir.path().join("page.png");
    let masked = || ExpectScreenshotRequest {
        name: "page".into(),
        locator: None,
        mask: vec!["#clock".into()],
        ..screenshot_request(&page)
    };
    let (_, data) = screenshot_ok(&engine, masked()).await;
    assert_eq!(data.result, BaselineResult::Created);
    assert_eq!((data.width, data.height), (1280, 800));
    for _ in 0..3 {
        let (_, data) = screenshot_ok(&engine, masked()).await;
        assert_eq!(data.result, BaselineResult::Passed);
    }

    // A baseline that is not a PNG.
    std::fs::write(&page, "not a png").unwrap();
    let err = screenshot(&engine, masked()).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadArgs, "{}", err.to_text());
    assert!(err.message.contains("is not a PNG"), "{}", err.message);
}
