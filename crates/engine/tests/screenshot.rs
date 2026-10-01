//! `screenshot` and `pdf` against a real Chrome (`CHROME_PATH`; skipped
//! without it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture
//! site's `shots/` pages (plan 06, milestone 6.3).
//!
//! The `--annotate` image is compared with `tests/golden/annotate.png`
//! with a tolerance for font rendering; `UPDATE_GOLDEN=1` rewrites it.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use browser::Host;
use engine::{
    CdpRequest, EmulateRequest, EmulationChange, Engine, ImageFormat, Margins, NavigateRequest,
    Navigation, OpenRequest, PaperFormat, PdfRequest, ScreenshotRequest, ShotScale, Target,
    WaitUntil,
};
use fixtures::Fixtures;
use protocol::schema::{PdfData, ScreenshotData, Viewport};
use protocol::{ErrorInfo, ErrorKind, Output};
use serde_json::json;

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

async fn cdp(engine: &Engine, method: &str, params: serde_json::Value) -> serde_json::Value {
    engine
        .cdp(CdpRequest {
            all_sessions: false,
            method: Some(method.to_owned()),
            params: params.as_object().unwrap().clone(),
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

/// A viewport of `width` x `height` CSS pixels at `dpr`.
async fn viewport(engine: &Engine, width: u32, height: u32, dpr: f64) {
    cdp(
        engine,
        "Emulation.setDeviceMetricsOverride",
        json!({ "width": width, "height": height, "deviceScaleFactor": dpr, "mobile": false }),
    )
    .await;
}

fn request() -> ScreenshotRequest {
    ScreenshotRequest {
        tab: None,
        locator: None,
        full: false,
        container: None,
        format: ImageFormat::Png,
        quality: None,
        path: None,
        scale: ShotScale::Css,
        max_size: 1568,
        omit_background: false,
        mask: Vec::new(),
        annotate: false,
        strict_refs: false,
        testid_attribute: None,
        timeout_ms: TIMEOUT_MS,
    }
}

async fn shoot(engine: &Engine, req: ScreenshotRequest) -> (ScreenshotData, Output) {
    let output = engine
        .screenshot(req)
        .await
        .unwrap_or_else(|e| panic!("screenshot: {}", e.to_text()));
    (serde_json::from_value(output.data.clone()).unwrap(), output)
}

async fn shoot_err(engine: &Engine, req: ScreenshotRequest) -> ErrorInfo {
    match engine.screenshot(req).await {
        Ok(output) => panic!("screenshot worked: {}", output.text),
        Err(e) => e,
    }
}

/// A decoded PNG: RGBA pixels.
struct Image {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Image {
    fn load(path: &str) -> Self {
        let decoder =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        let channels = info.color_type.samples();
        let rgba = buf[..info.buffer_size()]
            .chunks(channels)
            .flat_map(|p| match channels {
                4 => [p[0], p[1], p[2], p[3]],
                3 => [p[0], p[1], p[2], 255],
                _ => panic!("{channels} channels"),
            })
            .collect();
        Self {
            width: info.width,
            height: info.height,
            rgba,
        }
    }

    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * self.width + x) * 4) as usize;
        self.rgba[at..at + 4].try_into().unwrap()
    }

    /// The share of pixels that differ from `other`'s by more than 64 in
    /// a channel.
    #[allow(clippy::cast_precision_loss)]
    fn differs(&self, other: &Self) -> f64 {
        assert_eq!((self.width, self.height), (other.width, other.height));
        let differing = self
            .rgba
            .chunks(4)
            .zip(other.rgba.chunks(4))
            .filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 64))
            .count();
        differing as f64 / f64::from(self.width * self.height)
    }
}

fn near(pixel: [u8; 4], rgb: [u8; 3]) -> bool {
    pixel[..3].iter().zip(rgb).all(|(a, b)| a.abs_diff(b) <= 8)
}

#[tokio::test(flavor = "multi_thread")]
async fn viewport_element_and_full_page() {
    let Some(chrome) = chrome_path() else { return };
    let out = tempfile::tempdir().unwrap();
    let engine = start(&chrome, "engine-shot-test", out.path()).await;
    let fixtures = Fixtures::shared();
    goto(&engine, &fixtures.url("/shots/index.html")).await;
    viewport(&engine, 800, 600, 1.0).await;

    // The viewport, numbered in the session's shots directory.
    let (data, output) = shoot(&engine, request()).await;
    let first = out.path().join("shots/1.png");
    assert_eq!(data.path, first.display().to_string());
    assert_eq!((data.width, data.height, data.scale), (800, 600, 1.0));
    assert!(!data.downscaled);
    assert_eq!(output.quiet.as_deref(), Some(data.path.as_str()));
    assert!(
        output
            .text
            .starts_with(&format!("{}: 800x600 png, ", data.path)),
        "{}",
        output.text
    );
    let image = Image::load(&data.path);
    assert!(near(image.pixel(30, 30), [255, 0, 0]));
    // The same-origin iframe's content is in the image.
    assert!(near(image.pixel(20 + 40 + 5, 140 + 30 + 5), [0, 200, 0]));
    let (data, _) = shoot(&engine, request()).await;
    assert_eq!(
        data.path,
        out.path().join("shots/2.png").display().to_string()
    );

    // An element: its box exactly.
    let (data, output) = shoot(
        &engine,
        ScreenshotRequest {
            locator: Some("#red".into()),
            ..request()
        },
    )
    .await;
    assert_eq!((data.width, data.height), (200, 100), "{}", output.text);
    assert_eq!(data.locator.as_deref(), Some("#red"));
    let image = Image::load(&data.path);
    assert!(near(image.pixel(0, 0), [255, 0, 0]));
    assert!(near(image.pixel(199, 99), [255, 0, 0]));

    // One far below the fold is scrolled into view first.
    let (data, _) = shoot(
        &engine,
        ScreenshotRequest {
            locator: Some("#below".into()),
            ..request()
        },
    )
    .await;
    assert_eq!((data.width, data.height), (150, 50));
    assert!(near(Image::load(&data.path).pixel(75, 25), [0, 128, 0]));

    // One inside an iframe.
    let (data, _) = shoot(
        &engine,
        ScreenshotRequest {
            locator: Some("frame=inner >> #inner".into()),
            ..request()
        },
    )
    .await;
    assert_eq!((data.width, data.height), (100, 50));
    let image = Image::load(&data.path);
    assert!(near(image.pixel(1, 1), [0, 200, 0]));
    assert!(near(image.pixel(98, 48), [0, 200, 0]));

    // One inside another site's iframe, below the fold.
    goto(&engine, &fixtures.url("/shots/remote.html")).await;
    let (data, _) = shoot(
        &engine,
        ScreenshotRequest {
            locator: Some("frame=remote >> #inner".into()),
            ..request()
        },
    )
    .await;
    assert_eq!((data.width, data.height), (100, 50));
    let image = Image::load(&data.path);
    assert!(near(image.pixel(1, 1), [0, 200, 0]));
    assert!(near(image.pixel(98, 48), [0, 200, 0]));

    // An element that appears later is waited for.
    goto(&engine, &fixtures.url("/shots/index.html")).await;
    let (data, _) = shoot(
        &engine,
        ScreenshotRequest {
            locator: Some("#late".into()),
            ..request()
        },
    )
    .await;
    assert_eq!((data.width, data.height), (60, 30));

    // One that is never rendered fails once the time is up.
    let err = shoot_err(
        &engine,
        ScreenshotRequest {
            locator: Some("#hidden".into()),
            timeout_ms: 500,
            ..request()
        },
    )
    .await;
    assert_eq!(err.kind, ErrorKind::NotFound, "{}", err.to_text());
    assert!(
        err.message.contains("#hidden is not rendered"),
        "{}",
        err.to_text()
    );
    let err = shoot_err(
        &engine,
        ScreenshotRequest {
            locator: Some("#nothing".into()),
            timeout_ms: 1000,
            ..request()
        },
    )
    .await;
    assert_eq!(err.kind, ErrorKind::NotFound, "{}", err.to_text());

    // A 20,000px page, whole and full size.
    goto(&engine, &fixtures.url("/shots/tall.html")).await;
    let (data, _) = shoot(
        &engine,
        ScreenshotRequest {
            full: true,
            max_size: 0,
            ..request()
        },
    )
    .await;
    assert!(data.full);
    assert_eq!((data.width, data.height), (800, 20_000));
    let image = Image::load(&data.path);
    assert!(near(image.pixel(400, 19_990), [255, 0, 0]));
    assert!(!near(image.pixel(400, 18_990), [255, 0, 0]));

    // By default it is scaled down to 1568px on its longest side.
    let (data, output) = shoot(
        &engine,
        ScreenshotRequest {
            full: true,
            ..request()
        },
    )
    .await;
    assert!(data.downscaled);
    assert_eq!(data.height, 1568);
    assert_eq!(data.width, 63);
    assert!(
        output.text.contains("scaled down to fit --max-size 1568"),
        "{}",
        output.text
    );
    assert_eq!(
        output.warnings,
        [
            "the 800x20000 area was shrunk to 63x1568, too small to read; chrome-cdp screenshot \
             --full --max-size 0 --session engine-shot-test keeps its size, or a locator captures \
             one part"
        ]
    );
    let image = Image::load(&data.path);
    assert!(near(image.pixel(30, 1560), [255, 0, 0]));

    // --scale device on a 2x screen doubles the pixels; css does not.
    viewport(&engine, 400, 300, 2.0).await;
    let (data, _) = shoot(&engine, request()).await;
    assert_eq!((data.width, data.height, data.scale), (400, 300, 1.0));
    let (data, output) = shoot(
        &engine,
        ScreenshotRequest {
            scale: ShotScale::Device,
            ..request()
        },
    )
    .await;
    assert_eq!((data.width, data.height, data.scale), (800, 600, 2.0));
    assert!(output.text.contains("(1 CSS px = 2 px)"), "{}", output.text);

    engine.close().await;
}

/// A page script's value.
async fn eval(engine: &Engine, expression: &str) -> serde_json::Value {
    let data = cdp(
        engine,
        "Runtime.evaluate",
        json!({ "expression": expression, "returnByValue": true }),
    )
    .await;
    data["result"]["value"].clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn full_page_of_an_app_that_scrolls_inside() {
    let Some(chrome) = chrome_path() else { return };
    let out = tempfile::tempdir().unwrap();
    let engine = start(&chrome, "engine-shot-scroller", out.path()).await;
    goto(&engine, &Fixtures::shared().url("/shots/scroller.html")).await;
    viewport(&engine, 800, 600, 1.0).await;
    eval(&engine, "document.getElementById('feed').scrollTop = 500").await;
    let full = |container: Option<&str>| ScreenshotRequest {
        full: true,
        container: container.map(str::to_owned),
        max_size: 0,
        ..request()
    };

    // The document does not scroll, <main> does: the whole feed (3000px
    // under a 60px header) is in the image, from its start.
    let (data, output) = shoot(&engine, full(None)).await;
    assert_eq!((data.width, data.height), (800, 3060), "{}", output.text);
    assert_eq!(data.container.as_deref(), Some("main#feed"));
    assert!(
        output
            .text
            .ends_with("\nthe page scrolls in main#feed; the viewport grew to show all of it"),
        "{}",
        output.text
    );
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
    let image = Image::load(&data.path);
    assert!(near(image.pixel(400, 30), [0, 0, 255]));
    assert!(near(image.pixel(400, 70), [255, 0, 0]));
    assert!(near(image.pixel(400, 3050), [0, 200, 0]));
    // The viewport and the feed's scroll position are as they were.
    assert_eq!(
        eval(&engine, "[innerWidth, innerHeight]").await,
        json!([800, 600])
    );
    assert_eq!(
        eval(&engine, "document.getElementById('feed').scrollTop").await,
        json!(500)
    );

    // --full <loc> names another container: the 2000px sidebar.
    let (data, output) = shoot(&engine, full(Some("#side"))).await;
    assert_eq!((data.width, data.height), (800, 2060), "{}", output.text);
    assert_eq!(data.container.as_deref(), Some("nav#side"));
    assert!(near(
        Image::load(&data.path).pixel(100, 2050),
        [255, 255, 0]
    ));
    assert_eq!(eval(&engine, "innerHeight").await, json!(600));

    // One that does not scroll leaves the page as it is, and says so.
    let (data, output) = shoot(&engine, full(Some("css=header"))).await;
    assert_eq!((data.width, data.height), (800, 600));
    assert_eq!(data.container, None);
    assert_eq!(
        output.warnings,
        ["css=header does not scroll, so --full captured the page as it is"]
    );
    let err = shoot_err(
        &engine,
        ScreenshotRequest {
            timeout_ms: 1000,
            ..full(Some("#nothing"))
        },
    )
    .await;
    assert_eq!(err.kind, ErrorKind::NotFound, "{}", err.to_text());

    // The viewport the session emulates is the one it gets back, screen
    // and pixel ratio included.
    engine
        .emulate(EmulateRequest {
            change: EmulationChange::Viewport {
                viewport: Some(Viewport {
                    width: 500,
                    height: 400,
                    dpr: 2.0,
                    mobile: false,
                    touch: false,
                    screen_width: Some(1000),
                    screen_height: Some(900),
                }),
            },
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
    let (data, output) = shoot(&engine, full(None)).await;
    assert_eq!((data.width, data.height), (500, 3060), "{}", output.text);
    assert_eq!(
        eval(
            &engine,
            "[innerWidth, innerHeight, devicePixelRatio, screen.width, screen.height]"
        )
        .await,
        json!([500, 400, 2, 1000, 900])
    );

    // A page that scrolls itself is captured as before.
    goto(&engine, &Fixtures::shared().url("/shots/tall.html")).await;
    let (data, _) = shoot(&engine, full(None)).await;
    assert_eq!((data.width, data.height), (500, 20_000));
    assert_eq!(data.container, None);

    engine.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn formats_masks_and_backgrounds() {
    let Some(chrome) = chrome_path() else { return };
    let out = tempfile::tempdir().unwrap();
    let engine = start(&chrome, "engine-shot-formats", out.path()).await;
    goto(&engine, &Fixtures::shared().url("/shots/index.html")).await;
    viewport(&engine, 800, 600, 1.0).await;

    // jpeg and webp, to a given path.
    let path = out.path().join("a/b/shot.jpg");
    let (data, output) = shoot(
        &engine,
        ScreenshotRequest {
            format: ImageFormat::Jpeg,
            quality: Some(50),
            path: Some(path.clone()),
            ..request()
        },
    )
    .await;
    assert_eq!(data.path, path.display().to_string());
    assert_eq!(
        (data.format.as_str(), data.width, data.height),
        ("jpeg", 800, 600)
    );
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
    assert_eq!(&std::fs::read(&path).unwrap()[..2], &[0xFF, 0xD8]);
    let (data, output) = shoot(
        &engine,
        ScreenshotRequest {
            format: ImageFormat::Webp,
            path: Some(out.path().join("shot.png")),
            ..request()
        },
    )
    .await;
    assert_eq!((data.width, data.height), (800, 600));
    assert_eq!(
        output.warnings,
        ["the file is webp but its name ends in .png"]
    );

    // A mask paints over the element, and is gone afterwards.
    let (data, output) = shoot(
        &engine,
        ScreenshotRequest {
            mask: vec!["#secret".into(), ".nothing".into()],
            ..request()
        },
    )
    .await;
    assert_eq!(data.masked, 1);
    assert!(
        output.text.ends_with("\nmasked 1 element"),
        "{}",
        output.text
    );
    assert_eq!(
        output.warnings,
        ["--mask .nothing matches nothing shown; nothing was masked"]
    );
    let image = Image::load(&data.path);
    assert!(near(image.pixel(250, 30), [255, 0, 255]));
    assert!(near(image.pixel(30, 30), [255, 0, 0]));
    let (data, _) = shoot(&engine, request()).await;
    assert!(near(Image::load(&data.path).pixel(250, 30), [0, 0, 255]));

    // A page without a background can be transparent.
    let (data, _) = shoot(&engine, request()).await;
    assert_eq!(Image::load(&data.path).pixel(700, 500)[3], 255);
    let (data, _) = shoot(
        &engine,
        ScreenshotRequest {
            omit_background: true,
            ..request()
        },
    )
    .await;
    let image = Image::load(&data.path);
    assert_eq!(image.pixel(700, 500)[3], 0);
    assert!(near(image.pixel(30, 30), [255, 0, 0]));
    // ... and only for that screenshot.
    let (data, _) = shoot(&engine, request()).await;
    assert_eq!(Image::load(&data.path).pixel(700, 500)[3], 255);

    // Arguments that cannot work are refused before anything is sent.
    for (req, message) in [
        (
            ScreenshotRequest {
                full: true,
                locator: Some("#red".into()),
                ..request()
            },
            "--full captures the whole page, not an element",
        ),
        (
            ScreenshotRequest {
                quality: Some(80),
                ..request()
            },
            "--quality is for jpeg and webp; png is lossless",
        ),
        (
            ScreenshotRequest {
                format: ImageFormat::Jpeg,
                omit_background: true,
                ..request()
            },
            "jpeg has no transparency; --omit-background needs png or webp",
        ),
    ] {
        let err = shoot_err(&engine, req).await;
        assert_eq!(err.kind, ErrorKind::BadArgs);
        assert_eq!(err.message, message);
    }

    engine.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn annotate_labels_refs() {
    let Some(chrome) = chrome_path() else { return };
    let out = tempfile::tempdir().unwrap();
    let engine = start(&chrome, "engine-shot-annotate", out.path()).await;
    goto(&engine, &Fixtures::shared().url("/shots/annotate.html")).await;
    viewport(&engine, 400, 300, 1.0).await;

    let (data, output) = shoot(
        &engine,
        ScreenshotRequest {
            annotate: true,
            ..request()
        },
    )
    .await;
    // The button far below the viewport has a ref but is not in the image.
    assert_eq!(data.annotated, ["e1", "e2", "e3"], "{}", output.text);
    assert!(
        output.text.ends_with("\nlabelled 3 refs: e1 e2 e3"),
        "{}",
        output.text
    );
    let image = Image::load(&data.path);
    // A label sits above the button's top-left corner.
    assert!(near(image.pixel(40, 60 - 10), [0xe0, 0x11, 0x5f]));
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/annotate.png");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::copy(&data.path, &golden).unwrap();
    }
    let expected = Image::load(golden.to_str().unwrap());
    let differs = image.differs(&expected);
    assert!(
        differs < 0.01,
        "{:.2}% of the pixels differ",
        differs * 100.0
    );

    // The labels are gone afterwards, and the refs work.
    let (data, _) = shoot(&engine, request()).await;
    assert!(near(Image::load(&data.path).pixel(40, 50), [255, 255, 255]));
    let (data, _) = shoot(
        &engine,
        ScreenshotRequest {
            locator: Some("e1".into()),
            ..request()
        },
    )
    .await;
    assert_eq!((data.width, data.height), (120, 40));

    engine.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn annotate_draws_above_the_top_layer() {
    let Some(chrome) = chrome_path() else { return };
    let out = tempfile::tempdir().unwrap();
    let engine = start(&chrome, "engine-shot-annotate-modal", out.path()).await;
    goto(
        &engine,
        &Fixtures::shared().url("/shots/annotate-modal.html"),
    )
    .await;
    viewport(&engine, 400, 300, 1.0).await;

    let (data, output) = shoot(
        &engine,
        ScreenshotRequest {
            annotate: true,
            ..request()
        },
    )
    .await;
    // The dialog's buttons; the page under it is inert.
    assert_eq!(data.annotated, ["e1", "e2", "e3"], "{}", output.text);
    let image = Image::load(&data.path);
    let label = [0xe0, 0x11, 0x5f];
    // Above the modal: OK's label (its padding, left of the text), and
    // its outline.
    assert!(near(image.pixel(41, 50), label));
    assert!(near(image.pixel(100, 61), label));
    // #one's label (at 142-158) is in place; #two's (at 152-168) would
    // cover it, and moved down a label's height (168-184).
    assert!(near(image.pixel(201, 145), label));
    assert!(near(image.pixel(201, 176), label));

    // The overlay is gone afterwards, and the dialog still open.
    let (data, _) = shoot(&engine, request()).await;
    let image = Image::load(&data.path);
    assert!(near(image.pixel(41, 50), [255, 255, 255]));
    assert!(near(image.pixel(201, 176), [0xcc, 0xcc, 0xcc]));
    assert!(near(image.pixel(100, 80), [0xcc, 0xcc, 0xcc]));
    engine.close().await;
}

/// The text of each page of a PDF Chrome printed: its content streams'
/// `Tj` strings (glyph ids in hex), through the fonts' `ToUnicode` maps.
/// The maps are merged, which holds while the text is in one font.
fn pdf_text(pdf: &[u8]) -> Vec<String> {
    use std::io::Read as _;
    let hex = regex::Regex::new(r"<([0-9A-Fa-f]+)>\s*Tj").unwrap();
    let pair =
        regex::Regex::new(r"<([0-9A-Fa-f]+)>[ \t]*<([0-9A-Fa-f]+)>(?:[ \t]*<([0-9A-Fa-f]+)>)?")
            .unwrap();
    let mut streams = Vec::new();
    let mut rest = pdf;
    while let Some(at) = rest.windows(7).position(|w| w == b"stream\n") {
        let body = &rest[at + 7..];
        let end = body.windows(9).position(|w| w == b"endstream").unwrap();
        let mut out = String::new();
        if flate2::read::ZlibDecoder::new(&body[..end])
            .read_to_string(&mut out)
            .is_ok()
        {
            streams.push(out);
        }
        rest = &body[end + 9..];
    }
    let code = |h: &str| u32::from_str_radix(h, 16).unwrap();
    let mut glyphs = std::collections::HashMap::new();
    for cmap in streams.iter().filter(|s| s.contains("begincmap")) {
        for section in cmap.split("beginbf").skip(1) {
            let body = section.split("endbf").next().unwrap();
            for c in pair.captures_iter(body) {
                let (lo, second) = (code(&c[1]), code(&c[2]));
                match c.get(3) {
                    // bfrange: lo hi first-char.
                    Some(first) => {
                        for (i, glyph) in (lo..=second).enumerate() {
                            let ch = code(first.as_str()) + u32::try_from(i).unwrap();
                            glyphs.insert(glyph, char::from_u32(ch).unwrap());
                        }
                    }
                    None => {
                        glyphs.insert(lo, char::from_u32(second).unwrap());
                    }
                }
            }
        }
    }
    streams
        .iter()
        .filter(|s| !s.contains("begincmap") && s.contains("Tj"))
        .map(|s| {
            hex.captures_iter(s)
                .flat_map(|c| {
                    c[1].as_bytes()
                        .chunks(4)
                        .map(|g| {
                            glyphs
                                .get(&code(std::str::from_utf8(g).unwrap()))
                                .copied()
                                .unwrap_or('?')
                        })
                        .collect::<Vec<char>>()
                })
                .collect()
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn pdf_prints_the_page() {
    let Some(chrome) = chrome_path() else { return };
    let out = tempfile::tempdir().unwrap();
    let engine = start(&chrome, "engine-pdf-test", out.path()).await;
    goto(&engine, &Fixtures::shared().url("/shots/tall.html")).await;

    let pdf = |format: PaperFormat, landscape: bool, path: Option<PathBuf>| PdfRequest {
        tab: None,
        path,
        format,
        landscape,
        background: true,
        margin: Some(Margins::parse("0").unwrap()),
        pages: None,
        css_page_size: false,
        header: None,
        footer: None,
        scale: None,
        timeout_ms: TIMEOUT_MS,
    };
    let output = engine.pdf(pdf(PaperFormat::A4, false, None)).await.unwrap();
    let data: PdfData = serde_json::from_value(output.data.clone()).unwrap();
    assert_eq!(
        data.path,
        out.path().join("pdfs/1.pdf").display().to_string()
    );
    assert!(std::fs::read(&data.path).unwrap().starts_with(b"%PDF-"));
    // 20,000px at 96 per inch on 11.69in pages.
    assert_eq!(data.pages, Some(18));
    assert_eq!(data.paper, "A4");
    assert!(
        output
            .text
            .starts_with(&format!("{}: 18 pages, A4, ", data.path)),
        "{}",
        output.text
    );

    let path = out.path().join("letter.pdf");
    let output = engine
        .pdf(pdf(PaperFormat::Letter, true, Some(path.clone())))
        .await
        .unwrap();
    let data: PdfData = serde_json::from_value(output.data.clone()).unwrap();
    assert_eq!(data.path, path.display().to_string());
    assert!(data.landscape);
    // 20,000px on 8.5in tall pages.
    assert_eq!(data.pages, Some(25), "{}", output.text);

    // Some pages only.
    let output = engine
        .pdf(PdfRequest {
            pages: Some("2-3,5".to_owned()),
            ..pdf(PaperFormat::A4, false, None)
        })
        .await
        .unwrap();
    let data: PdfData = serde_json::from_value(output.data.clone()).unwrap();
    assert_eq!(data.pages, Some(3), "{}", output.text);
    for pages in ["40", "x"] {
        let err = engine
            .pdf(PdfRequest {
                pages: Some(pages.to_owned()),
                ..pdf(PaperFormat::A4, false, None)
            })
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadArgs, "{}", err.to_text());
        assert!(
            err.message.starts_with(&format!("--pages {pages} ")),
            "{}",
            err.message
        );
    }

    // A footer with the page number, no default header, half the scale:
    // half as many pages (the page numbers count every page, not only
    // those printed).
    let output = engine
        .pdf(PdfRequest {
            pages: Some("1-3".to_owned()),
            margin: Some(Margins::parse("2cm").unwrap()),
            scale: Some(0.5),
            footer: Some(
                r#"<div style="font-size:10px;width:100%;text-align:center">Page <span class="pageNumber"></span> of <span class="totalPages"></span></div>"#
                    .to_owned(),
            ),
            ..pdf(PaperFormat::A4, false, None)
        })
        .await
        .unwrap();
    let data: PdfData = serde_json::from_value(output.data.clone()).unwrap();
    assert_eq!(data.pages, Some(3), "{}", output.text);
    assert!(output.text.contains(", scale 0.5, "), "{}", output.text);
    let pages = pdf_text(&std::fs::read(&data.path).unwrap());
    assert_eq!(pages.len(), 3, "{pages:?}");
    for (n, text) in pages.iter().enumerate() {
        assert!(text.contains(&format!("Page {} of 11", n + 1)), "{pages:?}");
        assert!(!text.contains("Tall page"), "a default header: {pages:?}");
    }

    // The page's `@page` size (4in, 288pt), else A4's paper (596pt wide),
    // the 4in pages scaled up to fit.
    goto(&engine, &Fixtures::shared().url("/shots/page-size.html")).await;
    let width = |path: &str| {
        let pdf = String::from_utf8_lossy(&std::fs::read(path).unwrap()).into_owned();
        let at = pdf.find("/MediaBox [").unwrap() + "/MediaBox [".len();
        let numbers: Vec<f64> = pdf[at..]
            .split(']')
            .next()
            .unwrap()
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect();
        format!("{:.0}", numbers[2] - numbers[0])
    };
    let output = engine.pdf(pdf(PaperFormat::A4, false, None)).await.unwrap();
    let data: PdfData = serde_json::from_value(output.data.clone()).unwrap();
    assert_eq!(width(&data.path), "596", "{}", output.text);
    let output = engine
        .pdf(PdfRequest {
            css_page_size: true,
            ..pdf(PaperFormat::A4, false, None)
        })
        .await
        .unwrap();
    let data: PdfData = serde_json::from_value(output.data.clone()).unwrap();
    assert_eq!(width(&data.path), "288", "{}", output.text);
    assert_eq!(data.pages, Some(3), "{}", output.text);

    engine.close().await;
}
