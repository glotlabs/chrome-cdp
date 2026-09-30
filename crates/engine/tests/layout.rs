//! `layout` and `expect --layout` against a real Chrome (`CHROME_PATH`;
//! skipped without it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the
//! fixture site's `layout/` pages (plan 16, milestone 16.2).
//!
//! Each page's text output at `--viewports mobile,tablet,desktop` is
//! compared with `testdata/site/layout/<case>.expected` (`UPDATE_GOLDEN=1`
//! rewrites them), with the numbers that depend on fonts masked. Other
//! tests check that the page, its emulation, scroll and text are as they
//! were after a run, that contrast is measured on pixels where `expect
//! --a11y` reads declared colours, and `expect --layout`'s baselines.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use browser::Host;
use engine::{
    A11yLevel, Engine, EvalRequest, EvalWorld, ExpectRequest, Expectation, LayoutBaseline,
    LayoutRequest, NavigateRequest, Navigation, OpenRequest, WaitUntil,
};
use fixtures::Fixtures;
use protocol::{ErrorKind, Output};
use regex::Regex;
use serde_json::Value;

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

async fn eval(engine: &Engine, script: &str) -> Value {
    let out = engine
        .eval(EvalRequest {
            script: script.to_owned(),
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
        .unwrap_or_else(|e| panic!("{script}: {}", e.to_text()));
    out.data["value"].clone()
}

fn request() -> LayoutRequest {
    LayoutRequest {
        tab: None,
        locator: None,
        viewports: Vec::new(),
        only: Vec::new(),
        all: false,
        full: false,
        min_target: None,
        stretch_text: None,
        strict_refs: false,
        testid_attribute: None,
        timeout_ms: TIMEOUT_MS,
        max_chars: 0,
        baseline: None,
    }
}

async fn layout(engine: &Engine, req: LayoutRequest) -> Output {
    engine
        .layout(req)
        .await
        .unwrap_or_else(|e| panic!("layout: {}", e.to_text()))
}

/// The output with what depends on fonts masked: sizes of text, points
/// that depend on a box's size, and contrast measured over text boxes.
fn masked(text: &str) -> String {
    let shown = Regex::new(r"\d+px of \d+px").unwrap();
    let at = Regex::new(r" at -?\d+,-?\d+").unwrap();
    let ratio = Regex::new(r"\d+\.\d:1 on #[0-9a-f]{6}(–#[0-9a-f]{6})?").unwrap();
    let text = shown.replace_all(text, "Npx of Mpx");
    let text = at.replace_all(&text, " at X,Y");
    ratio.replace_all(&text, "R:1 on COLOURS").into_owned()
}

/// Compares the masked text output with golden file `case`, or rewrites
/// it with `UPDATE_GOLDEN=1`. The page's origin is `<site>`.
fn golden(case: &str, output: &Output, site: &Fixtures) {
    let path = fixtures::site_dir()
        .join("layout")
        .join(format!("{case}.expected"));
    let actual = format!("{}\n", masked(&output.text)).replace(site.a(), "<site>");
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

/// The golden cases: `(case, page, --all, --full, --stretch-text)`, each
/// at the three preset viewports.
const CASES: &[(&str, &str, bool, bool, Option<u32>)] = &[
    ("clean", "clean", true, true, None),
    ("covered", "covered", false, false, None),
    ("overlap", "overlap", false, false, None),
    ("clipped", "clipped", true, false, None),
    ("overflow", "overflow", false, false, None),
    ("offscreen", "offscreen", false, false, None),
    ("small", "small", false, false, None),
    ("contrast", "contrast", false, false, None),
    ("frames", "frames", false, false, None),
    ("cards", "cards", false, false, None),
    ("cards-stretched", "cards", false, false, Some(40)),
    ("long", "long", false, false, None),
    ("long-full", "long", false, true, None),
];

#[tokio::test]
async fn finds_each_pages_findings_at_every_viewport() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "layout-cases").await;
    for &(case, page, all, full, stretch_text) in CASES {
        goto(&engine, &site.url(&format!("/layout/{page}.html"))).await;
        let output = layout(
            &engine,
            LayoutRequest {
                viewports: vec!["mobile,tablet,desktop".to_owned()],
                all,
                full,
                stretch_text,
                ..request()
            },
        )
        .await;
        golden(case, &output, site);
    }
    engine.close().await;
}

#[tokio::test]
async fn leaves_the_page_as_it_was() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "layout-restores").await;
    goto(&engine, &site.url("/layout/long.html")).await;
    let state = "JSON.stringify([innerWidth, innerHeight, devicePixelRatio, \
                 navigator.maxTouchPoints, matchMedia('(pointer: coarse)').matches, scrollX, \
                 scrollY, document.body.innerText, document.adoptedStyleSheets.length])";
    eval(&engine, "scrollTo(0, 300)").await;
    let before = eval(&engine, state).await;
    layout(
        &engine,
        LayoutRequest {
            viewports: vec![
                "mobile".to_owned(),
                "tablet".to_owned(),
                "390x600".to_owned(),
            ],
            full: true,
            stretch_text: Some(40),
            ..request()
        },
    )
    .await;
    assert_eq!(eval(&engine, state).await, before);

    // An emulated device's viewport and touch come back too.
    engine
        .emulate(engine::EmulateRequest {
            change: engine::EmulationChange::Viewport {
                viewport: Some(protocol::schema::Viewport {
                    width: 700,
                    height: 500,
                    dpr: 2.0,
                    mobile: false,
                    touch: true,
                    screen_width: None,
                    screen_height: None,
                }),
            },
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
    let before = eval(&engine, state).await;
    let output = layout(
        &engine,
        LayoutRequest {
            viewports: vec!["desktop".to_owned()],
            ..request()
        },
    )
    .await;
    assert_eq!(output.data["viewports"][0]["width"], 1280);
    assert_eq!(eval(&engine, state).await, before);
    assert!(before.as_str().unwrap().starts_with("[700,500,2,5,true"));
    engine.close().await;
}

#[tokio::test]
async fn contrast_is_measured_on_pixels() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "layout-contrast").await;
    goto(&engine, &site.url("/layout/contrast.html")).await;
    let output = layout(
        &engine,
        LayoutRequest {
            only: vec!["contrast".to_owned()],
            ..request()
        },
    )
    .await;
    let findings = output.data["viewports"][0]["findings"].as_array().unwrap();
    let names: Vec<&str> = findings
        .iter()
        .map(|f| f["element"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "Free shipping on every order",
            "Summer sale starts today",
            "Terms and conditions apply",
            "Big but pale"
        ],
        "{}",
        output.text
    );
    let big = &findings[3]["details"];
    assert_eq!(big["needs"], 3.0);
    assert_eq!(big["large"], true);

    // expect --a11y reads declared colours: it passes the text on the
    // gradient and the photo (dark declared backgrounds), and fails the
    // text on the dark photo behind it (a white declared background).
    let a11y = |scope: &str| ExpectRequest {
        expectation: Expectation::A11y {
            scope: Some(scope.to_owned()),
            level: A11yLevel::AA,
        },
        not: false,
        tab: None,
        strict_refs: false,
        testid_attribute: None,
        timeout_ms: TIMEOUT_MS,
        max_chars: 0,
    };
    for passes in ["#gradient", "#photo"] {
        engine
            .expect(a11y(passes))
            .await
            .unwrap_or_else(|e| panic!("expect --a11y {passes}: {}", e.to_text()));
    }
    let err = engine.expect(a11y("#on-dark")).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::ExpectFailed, "{}", err.to_text());
    assert!(
        err.to_text().contains("color-contrast"),
        "{}",
        err.to_text()
    );
    engine.close().await;
}

#[tokio::test]
async fn names_elements_with_refs_and_keys() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "layout-refs").await;
    goto(&engine, &site.url("/layout/covered.html")).await;
    let output = layout(&engine, request()).await;
    let finding = &output.data["viewports"][0]["findings"][0];
    assert_eq!(finding["kind"], "covered", "{}", output.text);
    assert_eq!(
        finding["key"],
        "covered desktop role=button[name=\"Checkout\"] text=\"We use cookies. Accept\""
    );
    assert_eq!(finding["details"]["covering"]["class"], "cookie-banner");
    assert_eq!(finding["details"]["dismiss"]["name"], "Accept");
    // The refs are the tab's: the element's works as a locator.
    let reference = finding["element"]["ref"].as_str().unwrap();
    let scoped = layout(
        &engine,
        LayoutRequest {
            locator: Some(reference.to_owned()),
            ..request()
        },
    )
    .await;
    assert_eq!(scoped.data["viewports"][0]["errors"], 1, "{}", scoped.text);
    assert_eq!(scoped.data["locator"], reference);

    // An iframe's finding is placed in the page, and keyed with its frame.
    goto(&engine, &site.url("/layout/frames.html")).await;
    let output = layout(&engine, request()).await;
    let finding = &output.data["viewports"][0]["findings"][0];
    assert_eq!(finding["kind"], "clipped", "{}", output.text);
    assert_eq!(
        finding["key"],
        "clipped desktop frame=shop >> text=\"Delivered by tomorrow afternoon\""
    );
    assert!(finding["frame"].is_string());
    // The iframe is 40px in, with a 1px border; its body's margin is 8px.
    assert_eq!(finding["box"]["x"], 49);
    engine.close().await;
}

/// Where a test's baselines go.
fn baseline(dir: &Path, name: &str, update: bool) -> Option<LayoutBaseline> {
    Some(LayoutBaseline {
        name: name.to_owned(),
        path: dir.join("layout").join(format!("{name}.txt")),
        update,
        dir: Some(dir.display().to_string()),
    })
}

#[tokio::test]
async fn expect_layout_against_a_baseline() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "layout-expect").await;
    let dir = tempfile::tempdir().unwrap();
    goto(&engine, &site.url("/layout/clipped.html")).await;
    let expect = |update: bool, timeout_ms: u64| LayoutRequest {
        baseline: baseline(dir.path(), "clipped", update),
        timeout_ms,
        ..request()
    };

    // Missing: fails, and says how to make it.
    let err = engine.layout(expect(false, 500)).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::ExpectFailed);
    assert!(err.hint.as_deref().unwrap().contains("--update"), "{err:?}");

    // --update writes the two error findings (not the info ones).
    let out = engine.layout(expect(true, TIMEOUT_MS)).await.unwrap();
    assert_eq!(out.data["layout"]["result"], "created");
    let file = dir.path().join("layout/clipped.txt");
    let written = std::fs::read_to_string(&file).unwrap();
    let keys: Vec<&str> = written.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(keys.len(), 2, "{written}");
    assert!(keys.iter().all(|k| k.starts_with("clipped desktop ")));

    // Up to date: passes, after a reload and a re-render too.
    engine.layout(expect(false, TIMEOUT_MS)).await.unwrap();
    goto(&engine, &site.url("/layout/clipped.html")).await;
    eval(&engine, "document.body.innerHTML = document.body.innerHTML").await;
    let out = engine.layout(expect(false, TIMEOUT_MS)).await.unwrap();
    assert_eq!(out.data["layout"]["result"], "passed");
    let again = engine.layout(expect(true, TIMEOUT_MS)).await.unwrap();
    assert_eq!(again.data["layout"]["result"], "unchanged");

    // A new finding fails; a fixed one is named as no longer found.
    eval(
        &engine,
        "document.querySelector('.card h2').style.whiteSpace = 'normal'; \
         document.body.insertAdjacentHTML('beforeend', \
           '<button style=\"position:absolute;left:0;top:0;width:10px;height:10px;padding:0;border:0;box-sizing:border-box\" aria-label=\"A\"></button>\
            <button style=\"position:absolute;left:12px;top:0;width:10px;height:10px;padding:0;border:0;box-sizing:border-box\" aria-label=\"B\"></button>')",
    )
    .await;
    let err = engine.layout(expect(false, 500)).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::ExpectFailed);
    let details = err.details.clone().unwrap();
    assert_eq!(
        details["new"],
        serde_json::json!([
            "small-target desktop role=button[name=\"A\"]",
            "small-target desktop role=button[name=\"B\"]"
        ])
    );
    eval(
        &engine,
        "document.querySelectorAll('button').forEach((b) => b.remove())",
    )
    .await;
    let out = engine.layout(expect(false, TIMEOUT_MS)).await.unwrap();
    assert_eq!(
        out.data["layout"]["stale"],
        serde_json::json!([
            "clipped desktop role=heading[name=\"Wireless noise-cancelling headphones\"]"
        ])
    );
    assert!(
        out.warnings.iter().any(|w| w.contains("no longer found")),
        "{:?}",
        out.warnings
    );
    // --update drops it, and keeps the file's comments.
    std::fs::write(&file, format!("# accepted on purpose\n{written}")).unwrap();
    let out = engine.layout(expect(true, TIMEOUT_MS)).await.unwrap();
    assert_eq!(out.data["layout"]["result"], "updated");
    let rewritten = std::fs::read_to_string(&file).unwrap();
    assert!(
        rewritten.starts_with("# accepted on purpose\n"),
        "{rewritten}"
    );
    assert_eq!(rewritten.lines().filter(|l| !l.starts_with('#')).count(), 1);
    engine.close().await;
}

#[tokio::test]
async fn expect_layout_retries_until_the_page_settles() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "layout-retry").await;
    let dir = tempfile::tempdir().unwrap();
    goto(&engine, &site.url("/layout/clean.html")).await;
    let req = |update| LayoutRequest {
        baseline: baseline(dir.path(), "clean", update),
        ..request()
    };
    engine.layout(req(true)).await.unwrap();
    // A small target that grows after 500 ms: it fails at first, then
    // passes once it has grown.
    eval(
        &engine,
        "const b = document.createElement('button'); b.textContent = 'Go'; \
         b.style.cssText = 'position:absolute;left:600px;top:10px;width:12px;height:12px;padding:0'; \
         document.body.append(b); \
         const c = b.cloneNode(); c.textContent = 'Stop'; c.style.left = '614px'; document.body.append(c); \
         setTimeout(() => { b.style.width = b.style.height = c.style.width = c.style.height = '30px'; \
           c.style.left = '640px'; }, 500)",
    )
    .await;
    let out = engine.layout(req(false)).await.unwrap();
    assert!(out.data["retries"].as_u64().unwrap() > 0, "{}", out.data);
    engine.close().await;
}
