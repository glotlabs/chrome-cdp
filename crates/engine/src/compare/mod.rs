//! `compare` (plan 16, milestone 16.5): what differs between two pages,
//! local against production or before against after. Each side is a
//! URL (opened in a new tab of the current context, then closed), a tab
//! of this session, or a page read elsewhere (another session's active
//! tab, which the CLI asks that session for with `compare_capture`, or a
//! `snapshot --json` file).
//!
//! The snapshots are diffed by structure ([`crate::snapshot::compare`]),
//! after links to B's origin are moved to A's. With `--visual`, both
//! viewports are shot at one size and the changed pixels grouped into
//! regions ([`regions`]), each named by the smallest element of B at its
//! centre (`injected/src/compare.ts`).

mod regions;

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use cdp::domains::runtime;
use protocol::schema::{
    CompareData, CompareRegion, CompareSide as SideData, CompareSnapshot, CompareVisual, Viewport,
    WhyElement,
};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde::Deserialize;
use serde_json::json;

use crate::errors::from_cdp;
use crate::expect::pixels::{self, Image};
use crate::frames::World;
use crate::injected::{self, GUARD};
use crate::input::{Candidate, element_text, origin, refer_all};
use crate::locator::{LocateOptions, Locator, read_listed};
use crate::observe::save;
use crate::request::{
    CompareCapture, CompareCaptureRequest, CompareNameRequest, CompareRequest, CompareSide,
    Navigation, WaitUntil, millis,
};
use crate::snapshot::{Node, compare as diff, scope_locator};
use crate::tabs::{BLANK, Page, tab_name};
use crate::{Engine, Running};

/// How many regions are listed and named.
const LISTED: usize = 10;

/// The object group of `elementsAt`'s answer.
const GROUP: &str = "chrome-cdp-compare";

/// The outline of a region on the diff image.
const OUTLINE: [u8; 4] = [0, 102, 255, 255];

/// Checks a `compare` request's arguments without a browser.
pub fn check_compare(req: &CompareRequest) -> Result<(), ErrorInfo> {
    if !(0.0..=1.0).contains(&req.threshold) {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("--threshold must be from 0 to 1, got {}", req.threshold),
        )
        .hint(Hint::text(
            "0.1 is the default; lower counts fainter changes",
        )));
    }
    if let Some((w, h)) = req.viewport
        && !((1..=10_000).contains(&w) && (1..=10_000).contains(&h))
    {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("--viewport {w}x{h}: each side must be from 1 to 10000"),
        ));
    }
    if req.viewport.is_some() && !req.visual {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            "--viewport is the size --visual shoots both pages at",
        )
        .hint(Hint::text("add --visual, or leave --viewport out")));
    }
    for side in [&req.a, &req.b] {
        if req.visual
            && let CompareSide::Captured(capture) = side
            && capture.shot.is_none()
        {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!(
                    "--visual needs a live page: {} is a snapshot file",
                    capture.side
                ),
            )
            .hint(Hint::text(
                "compare a URL, tab:<id> or session:<name> with --visual, or leave --visual out",
            )));
        }
    }
    scope_locator(req.scope.as_deref(), &options(req)).map(drop)
}

fn options(req: &CompareRequest) -> LocateOptions {
    LocateOptions {
        strict_refs: req.strict_refs,
        testid_attribute: req.testid_attribute.clone(),
    }
}

/// One side, read.
struct Read {
    data: SideData,
    nodes: Vec<Node>,
    shot: Option<Image>,
    /// Its tab, while it is open.
    page: Option<Page>,
    /// Refs of its tab's elements stay usable (a tab of this session;
    /// not a URL's tab, which is closed).
    refs: bool,
}

/// `elementsAt`'s answer.
#[derive(Debug, Default, Deserialize)]
struct At {
    #[serde(default)]
    names: Vec<Candidate>,
    #[serde(default)]
    at: Vec<i64>,
}

impl Engine {
    /// `compare <A> <B>`.
    pub async fn compare(&self, req: CompareRequest) -> Result<Output, ErrorInfo> {
        check_compare(&req)?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let mut opened = Vec::new();
        let result = Box::pin(self.compare_sides(&running, &req, &mut opened, timeout)).await;
        for page in &opened {
            let _ = running.tabs.close(page.id, timeout).await;
        }
        // A side's tab was brought to the front for its screenshot.
        if req.visual
            && let Some(active) = running.tabs.active()
        {
            let _ = running.tabs.bring_to_front(active, timeout).await;
        }
        result
    }

    async fn compare_sides(
        &self,
        running: &Running,
        req: &CompareRequest,
        opened: &mut Vec<Page>,
        timeout: Duration,
    ) -> Result<Output, ErrorInfo> {
        let options = options(req);
        let scope = scope_locator(req.scope.as_deref(), &options)?;
        let mut size = req.viewport;
        let mut warnings = Vec::new();
        let mut sides = Vec::with_capacity(2);
        for side in [&req.a, &req.b] {
            let read = self
                .compare_read(
                    running,
                    req,
                    side,
                    scope.as_ref(),
                    &options,
                    &mut size,
                    opened,
                    &mut warnings,
                    timeout,
                )
                .await?;
            sides.push(read);
        }
        let (mut a, mut b) = {
            let b = sides.pop();
            (sides.pop(), b)
        };
        let (Some(a), Some(b)) = (a.as_mut(), b.as_mut()) else {
            return Err(ErrorInfo::new(
                ErrorKind::ProtocolError,
                "a side was not read",
            ));
        };
        if let (Some(from), Some(to)) = (origin(&b.data.url), origin(&a.data.url))
            && from != to
        {
            let (from, to) = (from.to_owned(), to.to_owned());
            diff::rewrite_hrefs(&mut b.nodes, &from, &to);
        }
        let changes = diff::diff(&a.nodes, &b.nodes);
        let snapshot = CompareSnapshot {
            changes: diff::count(&changes) as u64,
            diff: changes.iter().map(diff::diff_json).collect(),
        };
        let visual = if req.visual {
            Some(
                self.compare_pixels(running, req, a, b, &mut warnings, timeout)
                    .await?,
            )
        } else {
            None
        };
        let differs = snapshot.changes > 0 || visual.as_ref().is_some_and(|v| v.region_count > 0);
        let data = CompareData {
            a: a.data.clone(),
            b: b.data.clone(),
            differs,
            snapshot,
            visual,
        };
        let text = compare_text(&data);
        let quiet = compare_quiet(&data);
        let output = Output::typed(&data, text)
            .quiet(quiet)
            .limit(req.max_chars, "--scope, -i or --max-chars 0 for all of it");
        Ok(warnings.into_iter().fold(output, Output::warning))
    }

    /// Reads one side: its snapshot and, with `--visual`, its
    /// screenshot. A URL's tab goes in `opened`; `size` is the viewport
    /// both are shot at, once known.
    #[allow(clippy::too_many_arguments)]
    async fn compare_read(
        &self,
        running: &Running,
        req: &CompareRequest,
        side: &CompareSide,
        scope: Option<&Locator>,
        options: &LocateOptions,
        size: &mut Option<(u32, u32)>,
        opened: &mut Vec<Page>,
        warnings: &mut Vec<String>,
        timeout: Duration,
    ) -> Result<Read, ErrorInfo> {
        let (label, page, refs) = match side {
            CompareSide::Captured(capture) => return captured(req, capture, size, warnings),
            CompareSide::Url(url) => {
                let browser_id = self.current_context()?.map(|s| s.browser_id.clone());
                let page = running
                    .tabs
                    .create_in(BLANK, browser_id.as_deref(), timeout)
                    .await
                    .map_err(|e| self.tab_error(running, e))?;
                opened.push(page.clone());
                if req.visual
                    && let Some(wanted) = *size
                {
                    self.compare_viewport(running, &page, wanted, timeout)
                        .await?;
                }
                let _tab = self.tabs.lock(&page.target_id).await;
                let navigation = Navigation::Goto {
                    url: url.clone(),
                    base_url: req.base_url.clone(),
                };
                let navigated = self
                    .navigate_page(running, &page, &navigation, WaitUntil::Settle, timeout)
                    .await
                    .map_err(|e| on_side(url, e))?;
                warnings.extend(navigated.warnings);
                (url.clone(), page, false)
            }
            CompareSide::Tab(tab) => {
                let page = self.page(running, Some(tab), timeout).await?;
                (format!("tab:{}", tab_name(page.id)), page, true)
            }
        };
        let _tab = self.tabs.lock(&page.target_id).await;
        self.check_not_blocked(running, &page)?;
        let mark = running.tabs.mark();
        let retry = |r: &str| {
            let side = |s: &CompareSide| match s {
                CompareSide::Url(url) => url.clone(),
                CompareSide::Tab(tab) => format!("tab:{tab}"),
                CompareSide::Captured(c) => c.side.clone(),
            };
            Cmd::new("compare")
                .arg(side(&req.a))
                .arg(side(&req.b))
                .flag("scope", r)
        };
        let (url, title, nodes) = self
            .compare_tree(
                running,
                &page,
                scope,
                options,
                req.interactive,
                &retry,
                mark,
                timeout,
            )
            .await
            .map_err(|e| on_side(&label, e))?;
        let shot = if req.visual {
            running
                .tabs
                .bring_to_front(page.id, timeout)
                .await
                .map_err(|e| self.tab_error(running, e))?;
            let shot = self.layout_shot(running, &page, timeout).await?;
            size.get_or_insert((shot.width, shot.height));
            Some(shot)
        } else {
            None
        };
        Ok(Read {
            data: SideData {
                side: label,
                url,
                title,
                tab: refs.then(|| tab_name(page.id)),
            },
            nodes,
            shot,
            page: Some(page),
            refs,
        })
    }

    /// Gives a URL's new tab the viewport `wanted`, unless it has it.
    async fn compare_viewport(
        &self,
        running: &Running,
        page: &Page,
        wanted: (u32, u32),
        timeout: Duration,
    ) -> Result<(), ErrorInfo> {
        if self.viewport_size(running, page, timeout).await? == wanted {
            return Ok(());
        }
        let viewport = running.tabs.default_viewport().map_or(
            Viewport {
                width: wanted.0,
                height: wanted.1,
                dpr: 1.0,
                mobile: false,
                touch: false,
                screen_width: None,
                screen_height: None,
            },
            |v| Viewport {
                width: wanted.0,
                height: wanted.1,
                screen_width: None,
                screen_height: None,
                ..v
            },
        );
        running
            .tabs
            .override_viewport(page, &viewport, timeout)
            .await
            .map_err(|e| from_cdp(&e, &self.session))
    }

    /// `--visual`: the regions of changed pixels, named on B, and the
    /// diff image.
    async fn compare_pixels(
        &self,
        running: &Running,
        req: &CompareRequest,
        a: &Read,
        b: &Read,
        warnings: &mut Vec<String>,
        timeout: Duration,
    ) -> Result<CompareVisual, ErrorInfo> {
        let (Some(shot_a), Some(shot_b)) = (&a.shot, &b.shot) else {
            return Err(ErrorInfo::new(
                ErrorKind::ProtocolError,
                "a side has no screenshot",
            ));
        };
        let (width, height) = (
            shot_a.width.min(shot_b.width),
            shot_a.height.min(shot_b.height),
        );
        if (shot_a.width, shot_a.height) != (shot_b.width, shot_b.height) {
            warnings.push(format!(
                "the viewports differ ({} is {}x{}, {} is {}x{}); compared the {width}x{height} \
                 they share",
                a.data.side, shot_a.width, shot_a.height, b.data.side, shot_b.width, shot_b.height
            ));
        }
        let (shot_a, shot_b) = (shot_a.crop(width, height), shot_b.crop(width, height));
        let changed = pixels::changed(&shot_a, &shot_b, req.threshold);
        let changed_pixels = changed.iter().filter(|&&c| c).count() as u64;
        let found = regions::regions(&changed, width);
        let listed = &found[..found.len().min(LISTED)];
        let elements = match &b.page {
            Some(page) if !listed.is_empty() => {
                let points: Vec<(f64, f64)> = listed.iter().map(regions::Region::centre).collect();
                self.elements_at(running, page, &points, b.refs, timeout)
                    .await
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        };
        let image = if found.is_empty() {
            None
        } else {
            let mut image = pixels::diff_image(&shot_a, &shot_b, req.threshold);
            for region in listed {
                image.outline(
                    [region.x, region.y, region.width, region.height],
                    2,
                    OUTLINE,
                );
            }
            let png = image
                .encode()
                .map_err(|e| ErrorInfo::new(ErrorKind::ProtocolError, e))?;
            let path = save::save(&png, None, &self.output_dir().join("shots"), "png")?;
            Some(path.display().to_string())
        };
        Ok(CompareVisual {
            width,
            height,
            threshold: req.threshold,
            changed_pixels,
            region_count: found.len() as u64,
            regions: listed
                .iter()
                .enumerate()
                .map(|(i, r)| CompareRegion {
                    x: r.x,
                    y: r.y,
                    width: r.width,
                    height: r.height,
                    changed: r.changed,
                    element: elements.get(i).cloned().flatten(),
                })
                .collect(),
            image,
        })
    }

    /// The smallest element at each point of `page`'s viewport, named
    /// (with a ref when `refs`).
    async fn elements_at(
        &self,
        running: &Running,
        page: &Page,
        points: &[(f64, f64)],
        refs: bool,
        timeout: Duration,
    ) -> Result<Vec<Option<WhyElement>>, ErrorInfo> {
        let generation = running
            .tabs
            .snapshot_memory(page.id, |m| m.refs.generation());
        let (frame, context) = self
            .frame_and_context(running, page, None, World::Isolated, timeout)
            .await?;
        let mut params = runtime::CallFunctionOnParams::new(format!(
            "function (points) {{ {GUARD} return globalThis.__ccdp.elementsAt(points); }}"
        ));
        params.arguments = Some(vec![runtime::CallArgument {
            value: Some(json!(points)),
            unserializable_value: None,
            object_id: None,
        }]);
        params.object_group = Some(GROUP.to_owned());
        let session = context.session.with_timeout(timeout);
        let answer = async {
            let result = injected::call(&session, context.context_id, params)
                .await
                .map_err(|e| from_cdp(&e, &self.session))?;
            if let Some(exception) = &result.exception_details {
                return Err(ErrorInfo::new(
                    ErrorKind::EvalError,
                    format!("naming the regions failed: {}", exception.text),
                ));
            }
            let Some(object_id) = result.result.object_id else {
                return Ok((String::new(), Vec::new()));
            };
            read_listed(&session, &object_id, &frame, LISTED)
                .await
                .map_err(|e| from_cdp(&e, &self.session))
        }
        .await;
        let _ = session
            .call(runtime::ReleaseObjectGroupParams::new(GROUP.to_owned()))
            .await;
        let (json, keys) = answer?;
        let at: At = serde_json::from_str(&json).unwrap_or_default();
        let keys = if refs { keys } else { Vec::new() };
        let names = refer_all(running, page, &at.names, &keys, generation);
        Ok(at
            .at
            .iter()
            .map(|&i| usize::try_from(i).ok().and_then(|i| names.get(i).cloned()))
            .collect())
    }

    /// `compare`'s `session:NAME` side, asked of this session: its active
    /// tab's snapshot and, with `visual`, a screenshot of its viewport
    /// (saved in its `shots/`).
    pub async fn compare_capture(&self, req: CompareCaptureRequest) -> Result<Output, ErrorInfo> {
        let options = LocateOptions {
            strict_refs: req.strict_refs,
            testid_attribute: req.testid_attribute.clone(),
        };
        let scope = scope_locator(req.scope.as_deref(), &options)?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, None, timeout).await?;
        let _tab = self.tabs.lock(&page.target_id).await;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let retry = |r: &str| Cmd::new("snapshot").flag("scope", r);
        let (url, title, nodes) = self
            .compare_tree(
                &running,
                &page,
                scope.as_ref(),
                &options,
                req.interactive,
                &retry,
                mark,
                timeout,
            )
            .await
            .map_err(|e| on_side(&req.side, e))?;
        let shot = if req.visual {
            let shot = self.layout_shot(&running, &page, timeout).await?;
            let png = shot
                .encode()
                .map_err(|e| ErrorInfo::new(ErrorKind::ProtocolError, e))?;
            Some(save::save(
                &png,
                None,
                &self.output_dir().join("shots"),
                "png",
            )?)
        } else {
            None
        };
        let capture = CompareCapture {
            side: req.side,
            url,
            title,
            nodes: nodes.iter().map(diff::json).collect(),
            shot,
            warnings: Vec::new(),
        };
        Ok(Output::typed(&capture, String::new()))
    }

    /// Names the smallest element at each point of the active tab, with
    /// refs (`compare --visual`'s regions on a `session:` side).
    pub async fn compare_name(&self, req: CompareNameRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, None, timeout).await?;
        let _tab = self.tabs.lock(&page.target_id).await;
        let elements = self
            .elements_at(&running, &page, &req.points, true, timeout)
            .await?;
        Ok(Output::new(json!({ "elements": elements }), String::new()))
    }
}

/// A side read elsewhere.
fn captured(
    req: &CompareRequest,
    capture: &CompareCapture,
    size: &mut Option<(u32, u32)>,
    warnings: &mut Vec<String>,
) -> Result<Read, ErrorInfo> {
    let mut nodes: Vec<Node> = capture.nodes.iter().map(diff::from_json).collect();
    if req.interactive {
        nodes = diff::interactive(nodes);
    }
    if diff::neutral(&mut nodes) {
        warnings.push(format!(
            "{} folded parts of the page (snapshot --fold or --budget); what they hid \
             compares as missing",
            capture.side
        ));
    }
    warnings.extend(capture.warnings.iter().cloned());
    let shot = match (&capture.shot, req.visual) {
        (Some(path), true) => {
            let shot = read_shot(path)?;
            size.get_or_insert((shot.width, shot.height));
            Some(shot)
        }
        _ => None,
    };
    Ok(Read {
        data: SideData {
            side: capture.side.clone(),
            url: capture.url.clone(),
            title: capture.title.clone(),
            tab: None,
        },
        nodes,
        shot,
        page: None,
        refs: false,
    })
}

/// `err`, saying which side it is about.
fn on_side(side: &str, err: ErrorInfo) -> ErrorInfo {
    let message = format!("{side}: {}", err.message);
    ErrorInfo { message, ..err }
}

/// A screenshot another session saved.
fn read_shot(path: &Path) -> Result<Image, ErrorInfo> {
    let bytes = std::fs::read(path).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::ProtocolError,
            format!("cannot read the screenshot {}: {e}", path.display()),
        )
    })?;
    Image::decode(&bytes).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::ProtocolError,
            format!("{} is not a PNG: {e}", path.display()),
        )
    })
}

/// How the output names a side: what was given, and the page's URL when
/// that is not it.
fn side_text(side: &SideData) -> String {
    if side.url.is_empty() || side.url == side.side {
        side.side.clone()
    } else {
        format!("{} ({})", side.side, side.url)
    }
}

/// `compare`'s text form.
pub fn compare_text(data: &CompareData) -> String {
    let mut text = format!("compare {} ↔ {}", side_text(&data.a), side_text(&data.b));
    match data.snapshot.changes {
        0 => text.push_str("\nsnapshot: no changes"),
        1 => text.push_str("\nsnapshot: 1 change"),
        n => {
            let _ = write!(
                text,
                "\nsnapshot: {} changes",
                format::thousands(usize::try_from(n).unwrap_or(usize::MAX))
            );
        }
    }
    diff::diff_text_json(&data.snapshot.diff, &mut text);
    if let Some(visual) = &data.visual {
        let at = format!(
            "({}x{}, threshold {})",
            visual.width, visual.height, visual.threshold
        );
        let pixels =
            format::thousands(usize::try_from(visual.changed_pixels).unwrap_or(usize::MAX));
        match visual.region_count {
            0 => {
                let _ = write!(text, "\nvisual: no pixels differ {at}");
            }
            n => {
                let regions = if n == 1 {
                    "region differs"
                } else {
                    "regions differ"
                };
                let _ = write!(text, "\nvisual: {n} {regions}, {pixels} pixels {at}");
                if usize::try_from(n).unwrap_or(usize::MAX) > visual.regions.len() {
                    let _ = write!(text, "; the {} largest:", visual.regions.len());
                }
            }
        }
        for (i, region) in visual.regions.iter().enumerate() {
            let _ = write!(
                text,
                "\n  {}. {},{} {}×{}",
                i + 1,
                region.x,
                region.y,
                region.width,
                region.height
            );
            if let Some(element) = &region.element {
                let _ = write!(text, " around {}", element_text(element));
            }
        }
        if let Some(image) = &visual.image {
            let _ = write!(text, "\n  image: {image}");
        }
    }
    text
}

/// `compare -q`: how many differences, snapshot changes and regions.
pub fn compare_quiet(data: &CompareData) -> String {
    let regions = data.visual.as_ref().map_or(0, |v| v.region_count);
    (data.snapshot.changes + regions).to_string()
}
