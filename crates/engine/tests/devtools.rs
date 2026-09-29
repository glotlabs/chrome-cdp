//! `highlight` and `inspect` against a real Chrome (`CHROME_PATH`; skipped
//! without it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture
//! site (plan 06, milestone 6.6). The machine has no display, so the
//! browser is headless: `highlight` shows in screenshots, and `inspect`
//! refuses.

#![allow(
    clippy::unwrap_used,
    clippy::print_stderr,
    clippy::cast_possible_truncation
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use browser::Host;
use engine::{
    Engine, HighlightRequest, ImageFormat, InspectRequest, NavigateRequest, Navigation,
    OpenRequest, ScreenshotRequest, ShotScale, WaitUntil,
};
use fixtures::Fixtures;
use protocol::ErrorKind;
use protocol::schema::{HighlightData, ScreenshotData};

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

fn highlight(locator: &str, duration_ms: u64) -> HighlightRequest {
    HighlightRequest {
        tab: None,
        locator: locator.to_owned(),
        duration_ms,
        strict_refs: false,
        testid_attribute: None,
        timeout_ms: TIMEOUT_MS,
    }
}

/// The colour at the middle of `#red` in a viewport screenshot.
async fn middle_of_red(engine: &Engine, dir: &Path, name: &str) -> [u8; 3] {
    let path = dir.join(name);
    let out = engine
        .screenshot(ScreenshotRequest {
            tab: None,
            locator: None,
            full: false,
            container: None,
            format: ImageFormat::Png,
            quality: None,
            path: Some(path.clone()),
            scale: ShotScale::Css,
            max_size: 0,
            omit_background: false,
            mask: Vec::new(),
            annotate: false,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("screenshot: {}", e.to_text()));
    let _: ScreenshotData = serde_json::from_value(out.data).unwrap();
    let rect = engine
        .eval(engine::EvalRequest {
            script: "(() => { const r = document.querySelector('#red').getBoundingClientRect(); \
                     return [Math.round(r.x + r.width / 2), Math.round(r.y + r.height / 2)]; })()"
                .to_owned(),
            locator: None,
            frame: None,
            world: engine::EvalWorld::Main,
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            sourcemaps: false,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap();
    let (x, y) = (
        rect.data["value"][0].as_u64().unwrap() as usize,
        rect.data["value"][1].as_u64().unwrap() as usize,
    );
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    let channels = info.color_type.samples();
    let at = (y * info.width as usize + x) * channels;
    [buf[at], buf[at + 1], buf[at + 2]]
}

#[tokio::test(flavor = "multi_thread")]
async fn highlight_paints_over_an_element_for_a_while() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let dir = tempfile::tempdir().unwrap();
    let engine = start(&path, "devtools-highlight").await;
    goto(&engine, &site.url("/shots/index.html")).await;
    let before = middle_of_red(&engine, dir.path(), "before.png").await;
    assert!(before[0] > 200 && before[1] < 40, "{before:?}");

    let out = engine
        .highlight(highlight("#red", 700))
        .await
        .unwrap_or_else(|e| panic!("highlight: {}", e.to_text()));
    let data: HighlightData = serde_json::from_value(out.data.clone()).unwrap();
    assert_eq!(data.tab, "t1");
    assert_eq!(data.duration_ms, 700);
    // It gets a ref, as an acted-on element does.
    assert_eq!(data.element.reference.as_deref(), Some("e1"));
    assert_eq!(out.quiet.as_deref(), Some("e1"));
    assert!(out.text.starts_with("highlighted "), "{}", out.text);
    assert!(out.text.ends_with(" for 700ms"), "{}", out.text);
    assert!(
        out.warnings.iter().any(|w| w.contains("headless")),
        "{:?}",
        out.warnings
    );
    // DevTools' blue content box over the red.
    let lit = middle_of_red(&engine, dir.path(), "lit.png").await;
    assert!(lit[1] > 80 && lit[2] > 100, "{lit:?}");

    // It goes away on its own.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let after = middle_of_red(&engine, dir.path(), "after.png").await;
    assert!(after[0] > 200 && after[1] < 40, "{after:?}");

    let err = engine.highlight(highlight("#nope", 0)).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    engine.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn inspect_needs_a_window() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "devtools-inspect").await;
    let err = engine
        .inspect(InspectRequest {
            tab: None,
            panel: Some("console".to_owned()),
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::ProtocolError);
    let text = err.to_text();
    assert!(text.contains("headless"), "{text}");
    assert!(text.contains("open --headed --restart"), "{text}");
    engine.close().await;
}
