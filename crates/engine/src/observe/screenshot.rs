//! `screenshot` (plan 06, milestone 6.3).
//!
//! One `Page.captureScreenshot` with a `clip` in page coordinates: the
//! viewport, the whole page (`--full`, with `captureBeyondViewport`), or
//! one element's box after it is scrolled into view. The clip's `scale`
//! sets the image's size, so `--scale css` (image pixels are CSS pixels)
//! and `--max-size` (the longest side, 1568 by default to suit vision
//! models) need no image processing here: Chrome renders the smaller
//! image itself.
//!
//! `--mask` and `--annotate` draw into the page just before the capture
//! and remove it right after: a zero-size element (`<chrome-cdp-overlay>`,
//! its content in a closed shadow root) placed at the top of each frame's
//! document, with boxes positioned in document coordinates. Masks paint
//! over every match; labels name the refs `snapshot -i` would give, so a
//! vision agent can map what it sees to refs (Set-of-Marks).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use base64::Engine as _;
use cdp::domains::{dom, emulation, page as cdp_page, runtime};
use protocol::schema::ScreenshotData;
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde_json::json;
use tokio::time::Instant;

use super::save::save;
use crate::errors::from_cdp;
use crate::frames::World;
use crate::input::waits_for;
use crate::locator::{FrameOnly, LocateOptions, Locator, Want, release};
use crate::request::{ImageFormat, ScreenshotRequest, ShotScale, millis};
use crate::snapshot::NodeKey;
use crate::tabs::{FrameContext, Page, tab_name};
use crate::{Engine, Running};

/// How long to wait between looks for an element to capture.
const RETRY_DELAY: Duration = Duration::from_millis(100);

/// How many labelled refs the text form lists.
const LISTED_REFS: usize = 30;

/// Draws boxes over the elements `els` in their document: filled for a
/// mask, outlined with a label for `labels`. Returns each element's box
/// in its frame's viewport (`null` when it has none).
const OVERLAY_JS: &str = r"function (els, labels) {
  const host = document.createElement('chrome-cdp-overlay');
  host.style.cssText = 'all: initial !important; position: absolute !important; left: 0 !important;'
    + ' top: 0 !important; width: 0 !important; height: 0 !important; overflow: visible !important;'
    + ' z-index: 2147483647 !important; pointer-events: none !important; display: block !important;';
  const root = host.attachShadow({ mode: 'closed' });
  document.documentElement.append(host);
  // Where the host's containing block starts, whatever styles the page
  // gives <html>.
  const origin = host.getBoundingClientRect();
  const boxes = els.map((el, i) => {
    const r = el.getBoundingClientRect();
    if (!r.width || !r.height) return null;
    const x = r.left - origin.left, y = r.top - origin.top;
    const box = document.createElement('div');
    box.style.cssText = `position: absolute; left: ${x}px; top: ${y}px; width: ${r.width}px;`
      + ` height: ${r.height}px; box-sizing: border-box;`;
    if (labels) {
      box.style.border = '2px solid #e0115f';
      const tag = document.createElement('div');
      tag.textContent = labels[i];
      tag.style.cssText = 'position: absolute; left: -2px; padding: 0 3px; background: #e0115f;'
        + ' color: #fff; font: bold 12px/16px monospace; white-space: nowrap;'
        + (r.top >= 16 ? ' top: -18px;' : ' top: -2px;');
      box.append(tag);
    } else {
      box.style.background = '#ff00ff';
    }
    root.append(box);
    return [r.left, r.top, r.width, r.height];
  });
  return boxes;
}";

/// Removes what [`OVERLAY_JS`] drew.
const CLEAR_JS: &str = "function () { for (const el of document.querySelectorAll('chrome-cdp-overlay')) el.remove(); }";

/// Checks a screenshot request's arguments without a browser, so the CLI
/// can fail before it starts one.
pub fn check_screenshot(req: &ScreenshotRequest) -> Result<(), ErrorInfo> {
    locators(req).map(drop)
}

fn locate_options(req: &ScreenshotRequest) -> LocateOptions {
    LocateOptions {
        strict_refs: req.strict_refs,
        testid_attribute: req.testid_attribute.clone(),
    }
}

/// The element to capture and the `--mask` locators, parsed.
fn locators(req: &ScreenshotRequest) -> Result<(Option<Locator>, Vec<Locator>), ErrorInfo> {
    let bad = |message: &str, example: Cmd| {
        Err(ErrorInfo::new(ErrorKind::BadArgs, message).hint(Hint::run(example)))
    };
    if req.full && req.locator.is_some() {
        return bad(
            "--full captures the whole page, not an element",
            Cmd::new("screenshot").switch("full"),
        );
    }
    if req.quality.is_some() && req.format == ImageFormat::Png {
        return bad(
            "--quality is for jpeg and webp; png is lossless",
            Cmd::new("screenshot")
                .flag("format", "jpeg")
                .flag("quality", "80"),
        );
    }
    if req.quality.is_some_and(|q| q > 100) {
        return bad(
            "--quality goes from 0 to 100",
            Cmd::new("screenshot")
                .flag("format", "jpeg")
                .flag("quality", "80"),
        );
    }
    if req.omit_background && req.format == ImageFormat::Jpeg {
        return bad(
            "jpeg has no transparency; --omit-background needs png or webp",
            Cmd::new("screenshot").switch("omit-background"),
        );
    }
    let options = locate_options(req);
    let locator = req
        .locator
        .as_deref()
        .map(|l| options.parse(l))
        .transpose()?;
    let masks = req
        .mask
        .iter()
        .map(|m| {
            options.parse(m).map_err(|e| {
                let message = format!("--mask: {}", e.message);
                ErrorInfo { message, ..e }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((locator, masks))
}

/// A rectangle in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Area {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Area {
    fn intersects(&self, other: &Self) -> bool {
        self.x < other.x + other.width
            && other.x < self.x + self.width
            && self.y < other.y + other.height
            && other.y < self.y + self.height
    }

    /// The smallest rectangle of whole pixels around this one.
    fn outward(self) -> Self {
        let (x, y) = (self.x.floor(), self.y.floor());
        Self {
            x,
            y,
            width: (self.x + self.width).ceil() - x,
            height: (self.y + self.height).ceil() - y,
        }
    }
}

/// What is captured: `clip` in page coordinates, `shown` the same area in
/// the tab's viewport (for which labels are in the image).
#[derive(Debug)]
struct Capture {
    clip: Area,
    shown: Area,
    beyond_viewport: bool,
    /// Device pixels per CSS pixel.
    dpr: f64,
    /// Set when a ref was re-resolved by role and name.
    note: Option<String>,
}

/// One screenshot in progress.
struct Shot<'a> {
    engine: &'a Engine,
    running: &'a Running,
    page: &'a Page,
    req: &'a ScreenshotRequest,
    options: LocateOptions,
    timeout: Duration,
    deadline: Instant,
}

/// What `--mask` and `--annotate` drew.
#[derive(Debug, Default)]
struct Drawn {
    /// The frames drawn in, to clear afterwards.
    frames: Vec<String>,
    /// The refs labelled inside the captured area.
    labelled: Vec<String>,
    masked: usize,
    warnings: Vec<String>,
}

impl Engine {
    /// `screenshot`: the active tab (or `--tab`) as an image file.
    pub async fn screenshot(&self, req: ScreenshotRequest) -> Result<Output, ErrorInfo> {
        let (locator, masks) = locators(&req)?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let deadline = Instant::now() + timeout;
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let _tab = self.tabs.lock(&page.target_id).await;
        let shot = Shot {
            engine: self,
            running: &running,
            page: &page,
            req: &req,
            options: locate_options(&req),
            timeout,
            deadline,
        };
        let work = shot.run(locator.as_ref(), &masks, mark);
        let output = Box::pin(self.unless_blocked(&running, &page, mark, work)).await?;
        Ok(self.report_events(&running, mark, output, None).await)
    }
}

impl Engine {
    /// A screenshot as `req` asks, as bytes instead of a file (`req.path`
    /// is not looked at), and the warnings `screenshot` would print. The
    /// caller holds the tab's lock and handles a dialog that blocks it.
    pub(crate) async fn screenshot_bytes(
        &self,
        running: &Running,
        page: &Page,
        req: &ScreenshotRequest,
        mark: u64,
        deadline: Instant,
    ) -> Result<(Vec<u8>, Vec<String>), ErrorInfo> {
        let (locator, masks) = locators(req)?;
        let shot = Shot {
            engine: self,
            running,
            page,
            req,
            options: locate_options(req),
            timeout: deadline.saturating_duration_since(Instant::now()),
            deadline,
        };
        let (capture, drawn, bytes) = shot.take(locator.as_ref(), &masks, mark).await?;
        let mut warnings: Vec<String> = capture.note.into_iter().collect();
        warnings.extend(drawn.warnings);
        Ok((bytes, warnings))
    }
}

impl Shot<'_> {
    fn left(&self) -> Duration {
        self.deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1))
    }

    fn session(&self) -> &str {
        &self.engine.session
    }

    async fn run(
        &self,
        locator: Option<&Locator>,
        masks: &[Locator],
        mark: u64,
    ) -> Result<Output, ErrorInfo> {
        let (capture, drawn, bytes) = self.take(locator, masks, mark).await?;
        self.output(locator, &capture, drawn, &bytes)
    }

    /// Finds the area, draws the masks and labels, takes the image and
    /// clears what was drawn.
    async fn take(
        &self,
        locator: Option<&Locator>,
        masks: &[Locator],
        mark: u64,
    ) -> Result<(Capture, Drawn, Vec<u8>), ErrorInfo> {
        let capture = match locator {
            Some(locator) => self.element(locator).await?,
            None => self.page_area().await?,
        };
        let mut drawn = Drawn::default();
        let result = async {
            self.draw(masks, mark, &capture, &mut drawn).await?;
            self.capture(&capture).await
        }
        .await;
        self.clear(&drawn.frames).await;
        let bytes = result?;
        Ok((capture, drawn, bytes))
    }

    /// The viewport, or the whole page with `--full`.
    async fn page_area(&self) -> Result<Capture, ErrorInfo> {
        let metrics = self.metrics().await?;
        let viewport = &metrics.css_layout_viewport;
        #[allow(clippy::cast_precision_loss)]
        let (page_x, page_y, width, height) = (
            viewport.page_x as f64,
            viewport.page_y as f64,
            viewport.client_width as f64,
            viewport.client_height as f64,
        );
        let dpr = self.dpr().await?;
        if self.req.full {
            let size = &metrics.css_content_size;
            let clip = Area {
                x: 0.0,
                y: 0.0,
                width: size.width.max(width),
                height: size.height.max(height),
            }
            .outward();
            return Ok(Capture {
                shown: Area {
                    x: -page_x,
                    y: -page_y,
                    ..clip
                },
                clip,
                beyond_viewport: true,
                dpr,
                note: None,
            });
        }
        Ok(Capture {
            clip: Area {
                x: page_x,
                y: page_y,
                width,
                height,
            },
            shown: Area {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
            beyond_viewport: false,
            dpr,
            note: None,
        })
    }

    /// The box of the one element `locator` matches, scrolled into view;
    /// waits for it to be there and rendered.
    async fn element(&self, locator: &Locator) -> Result<Capture, ErrorInfo> {
        let retry = |r: &str| Cmd::new("screenshot").arg(r);
        loop {
            let found = self
                .engine
                .locate_one(
                    self.running,
                    self.page,
                    locator,
                    &self.options,
                    Some(&retry),
                    self.left(),
                )
                .await;
            let err = match found {
                Ok(target) => {
                    let measured = self.measure(&target.context, &target.object_id).await;
                    let (frame, note) = (target.key.frame.clone(), target.note.clone());
                    target.release();
                    match measured? {
                        Some(rect) => {
                            self.scroll_owners(rect, &frame).await?;
                            return self.element_area(rect, &frame, note).await;
                        }
                        None => ErrorInfo::new(
                            ErrorKind::NotFound,
                            format!("{locator} is not rendered, so it has no box to capture"),
                        )
                        .hint(
                            Hint::run(
                                Cmd::new("wait")
                                    .arg(locator.to_string())
                                    .session(self.session()),
                            )
                            .why("waits until it is visible"),
                        )
                        .details(json!({ "locator": locator.to_string(), "rendered": false })),
                    }
                }
                Err(err) if waits_for(&err) => err,
                Err(err) => return Err(err),
            };
            if Instant::now() + RETRY_DELAY >= self.deadline {
                return Err(err.note("waited", format::duration(self.timeout)));
            }
            tokio::time::sleep(RETRY_DELAY).await;
        }
    }

    /// Scrolls the element into view and reads its box in its frame's
    /// viewport; `None` if it has none.
    async fn measure(
        &self,
        context: &FrameContext,
        object_id: &str,
    ) -> Result<Option<Area>, ErrorInfo> {
        let session = context.session.with_timeout(self.left());
        // An element that is not rendered cannot be scrolled to; measuring
        // says so.
        let _ = session
            .call(dom::ScrollIntoViewIfNeededParams {
                object_id: Some(object_id.to_owned()),
                ..dom::ScrollIntoViewIfNeededParams::default()
            })
            .await;
        let mut params = runtime::CallFunctionOnParams::new(
            "function () { const r = this.getBoundingClientRect();\n\
             return r.width && r.height ? [r.x, r.y, r.width, r.height] : null; }"
                .to_owned(),
        );
        params.object_id = Some(object_id.to_owned());
        params.return_by_value = Some(true);
        let result = session
            .call(params)
            .await
            .map_err(|e| from_cdp(&e, self.session()))?;
        Ok(result
            .result
            .value
            .and_then(|v| serde_json::from_value::<[f64; 4]>(v).ok())
            .map(|[x, y, width, height]| Area {
                x,
                y,
                width,
                height,
            }))
    }

    /// Scrolls each `<iframe>` that frame `frame` is in so that `rect`, a
    /// box in the frame's viewport, is in view there too: an
    /// out-of-process frame's own scroll reaches its parent only later,
    /// after the image would be taken.
    async fn scroll_owners(&self, rect: Area, frame: &str) -> Result<(), ErrorInfo> {
        let frames = self.running.tabs.frames(self.page.id);
        let mut frame = frame.to_owned();
        let mut rect = rect;
        while frames.iter().any(|f| f.id == frame && f.parent.is_some()) {
            let (parent, context, owner) = self
                .engine
                .frame_owner(self.running, self.page, &frame, self.left())
                .await?;
            let session = context.session.with_timeout(self.left());
            let scrolled = async {
                // Where the frame's content starts in the owner's border
                // box.
                let [dx, dy, ..] = owner_box(&session, &owner.object_id).await?;
                let _ = session
                    .call(dom::ScrollIntoViewIfNeededParams {
                        object_id: Some(owner.object_id.clone()),
                        rect: Some(dom::Rect {
                            x: rect.x + dx,
                            y: rect.y + dy,
                            width: rect.width,
                            height: rect.height,
                        }),
                        ..dom::ScrollIntoViewIfNeededParams::default()
                    })
                    .await;
                let [dx, dy, left, top] = owner_box(&session, &owner.object_id).await?;
                Ok::<_, cdp::CdpError>(Area {
                    x: left + dx + rect.x,
                    y: top + dy + rect.y,
                    ..rect
                })
            }
            .await;
            release(&context, owner.object_id);
            rect = scrolled.map_err(|e| from_cdp(&e, self.session()))?;
            frame = parent;
        }
        Ok(())
    }

    /// The capture of `rect`, a box in frame `frame`'s viewport.
    async fn element_area(
        &self,
        rect: Area,
        frame: &str,
        note: Option<String>,
    ) -> Result<Capture, ErrorInfo> {
        let (dx, dy) = self
            .engine
            .frame_offset(self.running, self.page, frame, self.left())
            .await?;
        let shown = Area {
            x: rect.x + dx,
            y: rect.y + dy,
            ..rect
        };
        let metrics = self.metrics().await?;
        let viewport = &metrics.css_layout_viewport;
        #[allow(clippy::cast_precision_loss)]
        let (page_x, page_y, width, height) = (
            viewport.page_x as f64,
            viewport.page_y as f64,
            viewport.client_width as f64,
            viewport.client_height as f64,
        );
        let clip = Area {
            x: shown.x + page_x,
            y: shown.y + page_y,
            ..shown
        }
        .outward();
        let beyond_viewport = shown.x < 0.0
            || shown.y < 0.0
            || shown.x + shown.width > width
            || shown.y + shown.height > height;
        Ok(Capture {
            clip,
            shown,
            beyond_viewport,
            dpr: self.dpr().await?,
            note,
        })
    }

    async fn metrics(&self) -> Result<cdp_page::GetLayoutMetricsReturns, ErrorInfo> {
        self.page
            .session
            .with_timeout(self.left())
            .call(cdp_page::GetLayoutMetricsParams {})
            .await
            .map_err(|e| from_cdp(&e, self.session()))
    }

    /// Device pixels per CSS pixel (`devicePixelRatio`, which device
    /// emulation changes too).
    async fn dpr(&self) -> Result<f64, ErrorInfo> {
        let context = self
            .engine
            .frame_context(self.running, self.page, None, World::Isolated, self.left())
            .await?;
        let mut params = runtime::EvaluateParams::new("devicePixelRatio".to_owned());
        params.context_id = Some(context.context_id);
        params.return_by_value = Some(true);
        let result = context
            .session
            .with_timeout(self.left())
            .call(params)
            .await
            .map_err(|e| from_cdp(&e, self.session()))?;
        Ok(result
            .result
            .value
            .and_then(|v| v.as_f64())
            .filter(|dpr| dpr.is_finite() && *dpr > 0.0)
            .unwrap_or(1.0))
    }

    /// Draws the masks and the labels.
    async fn draw(
        &self,
        masks: &[Locator],
        mark: u64,
        capture: &Capture,
        drawn: &mut Drawn,
    ) -> Result<(), ErrorInfo> {
        let mask_js = format!("function () {{ return ({OVERLAY_JS})(this, null); }}");
        for mask in masks {
            let called = self
                .engine
                .call_on(
                    self.running,
                    self.page,
                    Some(mask),
                    &self.options,
                    Want::All,
                    FrameOnly::Owner,
                    &mask_js,
                    self.left(),
                )
                .await?;
            if !drawn.frames.contains(&called.frame) {
                drawn.frames.push(called.frame.clone());
            }
            let boxes = called
                .value
                .as_array()
                .map_or(0, |b| b.iter().filter(|b| !b.is_null()).count());
            if boxes == 0 {
                drawn.warnings.push(format!(
                    "--mask {mask} matches nothing shown; nothing was masked"
                ));
            }
            drawn.masked += boxes;
        }
        if self.req.annotate {
            let refs = self
                .engine
                .interactive_refs(self.running, self.page, mark, self.left())
                .await?;
            self.label(&refs, capture, drawn).await?;
            if drawn.labelled.is_empty() {
                drawn.warnings.push(
                    "no element with a ref is in the image, so nothing was labelled".to_owned(),
                );
            }
        }
        Ok(())
    }

    /// Labels each ref's element with the ref, frame by frame.
    async fn label(
        &self,
        refs: &[(u32, NodeKey)],
        capture: &Capture,
        drawn: &mut Drawn,
    ) -> Result<(), ErrorInfo> {
        let mut by_frame: BTreeMap<&str, Vec<(u32, &NodeKey)>> = BTreeMap::new();
        for (reference, key) in refs {
            by_frame
                .entry(key.frame.as_str())
                .or_default()
                .push((*reference, key));
        }
        let mut labelled = Vec::new();
        for (frame, refs) in by_frame {
            let mut context = None;
            let mut handles = Vec::new();
            let mut numbers = Vec::new();
            for (reference, key) in refs {
                if let Some((ctx, object_id)) = self
                    .engine
                    .live_node(self.running, self.page, key, self.left())
                    .await?
                {
                    context.get_or_insert(ctx);
                    handles.push(object_id);
                    numbers.push(reference);
                }
            }
            let Some(context) = context else { continue };
            drawn.frames.push(frame.to_owned());
            let boxes = self.overlay(&context, &handles, &numbers).await;
            for object_id in handles {
                release(&context, object_id);
            }
            let (dx, dy) = self
                .engine
                .frame_offset(self.running, self.page, frame, self.left())
                .await?;
            for (number, rect) in numbers.iter().zip(boxes?) {
                let Some([x, y, width, height]) = rect else {
                    continue;
                };
                let rect = Area {
                    x: x + dx,
                    y: y + dy,
                    width,
                    height,
                };
                if rect.intersects(&capture.shown) {
                    labelled.push(*number);
                }
            }
        }
        labelled.sort_unstable();
        drawn.labelled = labelled.into_iter().map(|n| format!("e{n}")).collect();
        Ok(())
    }

    /// Runs [`OVERLAY_JS`] with labels on the elements `handles` of one
    /// frame.
    async fn overlay(
        &self,
        context: &FrameContext,
        handles: &[String],
        numbers: &[u32],
    ) -> Result<Vec<Option<[f64; 4]>>, ErrorInfo> {
        let mut params = runtime::CallFunctionOnParams::new(format!(
            "function (labels, ...els) {{ return ({OVERLAY_JS})(els, labels); }}"
        ));
        let labels: Vec<String> = numbers.iter().map(|n| format!("e{n}")).collect();
        let mut arguments = vec![runtime::CallArgument {
            value: Some(json!(labels)),
            unserializable_value: None,
            object_id: None,
        }];
        arguments.extend(handles.iter().map(|id| runtime::CallArgument {
            value: None,
            unserializable_value: None,
            object_id: Some(id.clone()),
        }));
        params.arguments = Some(arguments);
        params.execution_context_id = Some(context.context_id);
        params.return_by_value = Some(true);
        let result = context
            .session
            .with_timeout(self.left())
            .call(params)
            .await
            .map_err(|e| from_cdp(&e, self.session()))?;
        if let Some(exception) = result.exception_details {
            return Err(ErrorInfo::new(
                ErrorKind::ProtocolError,
                format!("drawing the labels failed: {}", exception.text),
            ));
        }
        Ok(result
            .result
            .value
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default())
    }

    /// Removes the masks and labels from `frames` (best effort: a frame
    /// that went away took them along).
    async fn clear(&self, frames: &[String]) {
        for frame in frames {
            let Ok(context) = self
                .engine
                .frame_context(
                    self.running,
                    self.page,
                    Some(frame),
                    World::Isolated,
                    self.timeout,
                )
                .await
            else {
                continue;
            };
            let mut params = runtime::CallFunctionOnParams::new(CLEAR_JS.to_owned());
            params.execution_context_id = Some(context.context_id);
            let _ = context
                .session
                .with_timeout(self.timeout)
                .call(params)
                .await;
        }
    }

    /// Takes the image.
    async fn capture(&self, capture: &Capture) -> Result<Vec<u8>, ErrorInfo> {
        let clip = capture.clip;
        if clip.width < 1.0 || clip.height < 1.0 {
            return Err(ErrorInfo::new(
                ErrorKind::NotFound,
                "there is nothing to capture: the area is empty",
            ));
        }
        let (scale, _) = image_scale(self.req.scale, self.req.max_size, capture);
        let session = self.page.session.with_timeout(self.left());
        if self.req.omit_background {
            session
                .call(emulation::SetDefaultBackgroundColorOverrideParams {
                    color: Some(dom::RGBA {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: Some(0.0),
                    }),
                })
                .await
                .map_err(|e| from_cdp(&e, self.session()))?;
        }
        let params = cdp_page::CaptureScreenshotParams {
            format: Some(match self.req.format {
                ImageFormat::Png => cdp_page::CaptureScreenshotFormat::Png,
                ImageFormat::Jpeg => cdp_page::CaptureScreenshotFormat::Jpeg,
                ImageFormat::Webp => cdp_page::CaptureScreenshotFormat::Webp,
            }),
            quality: self.req.quality.map(i64::from),
            clip: Some(cdp_page::Viewport {
                x: clip.x,
                y: clip.y,
                width: clip.width,
                height: clip.height,
                scale: scale / capture.dpr,
            }),
            capture_beyond_viewport: Some(capture.beyond_viewport),
            ..cdp_page::CaptureScreenshotParams::default()
        };
        let result = session.call(params).await;
        if self.req.omit_background {
            let _ = session
                .call(emulation::SetDefaultBackgroundColorOverrideParams { color: None })
                .await;
        }
        let data = result.map_err(|e| from_cdp(&e, self.session()))?.data;
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| {
                ErrorInfo::new(
                    ErrorKind::ProtocolError,
                    format!("Chrome sent an image that is not base64: {e}"),
                )
            })
    }

    fn output(
        &self,
        locator: Option<&Locator>,
        capture: &Capture,
        drawn: Drawn,
        bytes: &[u8],
    ) -> Result<Output, ErrorInfo> {
        let format = self.req.format;
        let path = save(
            bytes,
            self.req.path.as_deref(),
            &self.engine.output_dir().join("shots"),
            format.extension(),
        )?;
        let (wanted, downscaled) = image_scale(self.req.scale, self.req.max_size, capture);
        let (width, height) = dimensions(format, bytes).unwrap_or_else(|| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let px = |css: f64| (css * wanted).round() as u32;
            (px(capture.clip.width), px(capture.clip.height))
        });
        let scale = (f64::from(width) / capture.clip.width * 1000.0).round() / 1000.0;
        let path_text = path.display().to_string();
        let mut text = format!(
            "{path_text}: {width}x{height} {}, {}",
            format.as_str(),
            format::size(bytes.len() as u64)
        );
        if downscaled {
            let _ = write!(
                text,
                " (scaled down to fit --max-size {}: 1 CSS px = {} px)",
                self.req.max_size,
                round2(scale)
            );
        } else if (scale - 1.0).abs() > 0.005 {
            let _ = write!(text, " (1 CSS px = {} px)", round2(scale));
        }
        if self.req.annotate && !drawn.labelled.is_empty() {
            let mut listed = drawn.labelled[..drawn.labelled.len().min(LISTED_REFS)].join(" ");
            if drawn.labelled.len() > LISTED_REFS {
                let _ = write!(listed, " … and {} more", drawn.labelled.len() - LISTED_REFS);
            }
            let _ = write!(
                text,
                "\nlabelled {} {}: {listed}",
                drawn.labelled.len(),
                if drawn.labelled.len() == 1 {
                    "ref"
                } else {
                    "refs"
                }
            );
        }
        if drawn.masked > 0 {
            let _ = write!(
                text,
                "\nmasked {} {}",
                drawn.masked,
                if drawn.masked == 1 {
                    "element"
                } else {
                    "elements"
                }
            );
        }
        let data = ScreenshotData {
            tab: tab_name(self.page.id),
            path: path_text.clone(),
            format: format.as_str().to_owned(),
            width,
            height,
            bytes: bytes.len() as u64,
            locator: locator.map(ToString::to_string),
            full: self.req.full,
            scale,
            downscaled,
            annotated: drawn.labelled,
            masked: drawn.masked,
        };
        let mut output = Output::typed(&data, text).quiet(path_text);
        if let Some(note) = &capture.note {
            output = output.warning(note.clone());
        }
        if let Some(ext) = path_mismatch(self.req.path.as_deref(), format) {
            output = output.warning(format!(
                "the file is {} but its name ends in .{ext}",
                format.as_str()
            ));
        }
        for warning in drawn.warnings {
            output = output.warning(warning);
        }
        if downscaled && scale < SMALL {
            let mut cmd = Cmd::new("screenshot");
            if self.req.full {
                cmd = cmd.switch("full");
            }
            if let Some(locator) = locator {
                cmd = cmd.arg(locator.to_string());
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let (w, h) = (
                capture.clip.width.round() as u64,
                capture.clip.height.round() as u64,
            );
            output = output.warning(format!(
                "the {w}x{h} area was shrunk to {width}x{height}, too small to read; {} keeps \
                 its size, or a locator captures one part",
                cmd.flag("max-size", "0").session(self.session())
            ));
        }
        Ok(output)
    }
}

/// Below this many image pixels per CSS pixel, text is too small to read.
const SMALL: f64 = 0.5;

/// An `<iframe>`'s content-box offset in its border box, and its border
/// box's top-left corner in its frame's viewport: `[dx, dy, left, top]`.
async fn owner_box(session: &cdp::Session, object_id: &str) -> cdp::Result<[f64; 4]> {
    let mut params = runtime::CallFunctionOnParams::new(
        "function () { const r = this.getBoundingClientRect(); const s = getComputedStyle(this);\n\
         return [this.clientLeft + parseFloat(s.paddingLeft),\n\
                 this.clientTop + parseFloat(s.paddingTop), r.left, r.top]; }"
            .to_owned(),
    );
    params.object_id = Some(object_id.to_owned());
    params.return_by_value = Some(true);
    let result = session.call(params).await?;
    Ok(result
        .result
        .value
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
}

/// Image pixels per CSS pixel for `capture`, and whether `max_size` made
/// it smaller than `scale` asks.
fn image_scale(scale: ShotScale, max_size: u32, capture: &Capture) -> (f64, bool) {
    let wanted = match scale {
        ShotScale::Css => 1.0,
        ShotScale::Device => capture.dpr,
    };
    let longest = capture.clip.width.max(capture.clip.height);
    if max_size == 0 || longest * wanted <= f64::from(max_size) {
        return (wanted, false);
    }
    (f64::from(max_size) / longest, true)
}

fn round2(n: f64) -> f64 {
    (n * 100.0).round() / 100.0
}

/// The extension of `path` when it names another format than `format`.
fn path_mismatch(path: Option<&Path>, format: ImageFormat) -> Option<String> {
    let path = path?;
    let named = ImageFormat::from_path(path)?;
    (named != format).then(|| {
        path.extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_default()
    })
}

/// An image's width and height, read from its header.
fn dimensions(format: ImageFormat, bytes: &[u8]) -> Option<(u32, u32)> {
    let be32 = |at: usize| -> Option<u32> {
        Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
    };
    let be16 = |at: usize| -> Option<u32> {
        Some(u32::from(u16::from_be_bytes(
            bytes.get(at..at + 2)?.try_into().ok()?,
        )))
    };
    let le16 = |at: usize| -> Option<u32> {
        Some(u32::from(u16::from_le_bytes(
            bytes.get(at..at + 2)?.try_into().ok()?,
        )))
    };
    let le24 = |at: usize| -> Option<u32> {
        let b = bytes.get(at..at + 3)?;
        Some(u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16)
    };
    match format {
        // The IHDR chunk comes first.
        ImageFormat::Png => {
            (bytes.get(12..16)? == b"IHDR").then(|| Some((be32(16)?, be32(20)?)))?
        }
        // The first start-of-frame segment.
        ImageFormat::Jpeg => {
            let mut at = 2;
            while at + 9 < bytes.len() {
                if bytes[at] != 0xFF {
                    return None;
                }
                let marker = bytes[at + 1];
                let length = usize::try_from(be16(at + 2)?).ok()?;
                if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
                    return Some((be16(at + 7)?, be16(at + 5)?));
                }
                at += 2 + length;
            }
            None
        }
        ImageFormat::Webp => match bytes.get(12..16)? {
            b"VP8 " => Some((le16(26)? & 0x3FFF, le16(28)? & 0x3FFF)),
            b"VP8L" => {
                let b = bytes.get(21..25)?;
                let bits = u32::from_le_bytes(b.try_into().ok()?);
                Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
            }
            b"VP8X" => Some((le24(24)? + 1, le24(27)? + 1)),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{Area, Capture, dimensions, image_scale, path_mismatch};
    use crate::request::{ImageFormat, ShotScale};

    fn capture(width: f64, height: f64, dpr: f64) -> Capture {
        let clip = Area {
            x: 0.0,
            y: 0.0,
            width,
            height,
        };
        Capture {
            clip,
            shown: clip,
            beyond_viewport: false,
            dpr,
            note: None,
        }
    }

    #[test]
    fn images_are_css_pixels_unless_too_large() {
        assert_eq!(
            image_scale(ShotScale::Css, 1568, &capture(1280.0, 720.0, 2.0)),
            (1.0, false)
        );
        assert_eq!(
            image_scale(ShotScale::Device, 0, &capture(1280.0, 720.0, 2.0)),
            (2.0, false)
        );
        let (scale, downscaled) =
            image_scale(ShotScale::Device, 1568, &capture(1280.0, 720.0, 2.0));
        assert!(downscaled);
        assert!((scale - 1568.0 / 1280.0).abs() < 1e-9);
        let (scale, downscaled) = image_scale(ShotScale::Css, 1568, &capture(800.0, 20_000.0, 1.0));
        assert!(downscaled);
        assert!((scale * 20_000.0 - 1568.0).abs() < 1e-6);
    }

    #[test]
    fn areas_round_outward_to_whole_pixels() {
        let area = Area {
            x: 10.4,
            y: 0.5,
            width: 20.2,
            height: 10.0,
        }
        .outward();
        assert_eq!(
            area,
            Area {
                x: 10.0,
                y: 0.0,
                width: 21.0,
                height: 11.0
            }
        );
    }

    #[test]
    fn dimensions_come_from_the_header() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend(640u32.to_be_bytes());
        png.extend(480u32.to_be_bytes());
        assert_eq!(dimensions(ImageFormat::Png, &png), Some((640, 480)));
        // SOI, an APP0 of 4 bytes, then SOF0 with height 300 and width 400.
        let jpeg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x01,
            0x2C, 0x01, 0x90, 0x03, 0x00, 0x00,
        ];
        assert_eq!(dimensions(ImageFormat::Jpeg, &jpeg), Some((400, 300)));
        assert_eq!(dimensions(ImageFormat::Png, b"nope"), None);
    }

    #[test]
    fn a_file_name_for_another_format_is_noticed() {
        let path = std::path::Path::new("/tmp/shot.jpg");
        assert_eq!(
            path_mismatch(Some(path), ImageFormat::Png),
            Some("jpg".to_owned())
        );
        assert_eq!(path_mismatch(Some(path), ImageFormat::Jpeg), None);
        let other = std::path::Path::new("/tmp/shot.bin");
        assert_eq!(path_mismatch(Some(other), ImageFormat::Png), None);
    }
}
