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
    CdpRequest, Engine, ImageFormat, Margins, NavigateRequest, Navigation, OpenRequest,
    PaperFormat, PdfRequest, ScreenshotRequest, ShotScale, Target, WaitUntil,
};
use fixtures::Fixtures;
use protocol::schema::{PdfData, ScreenshotData};
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
            timeout_ms: 300,
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

    engine.close().await;
}
