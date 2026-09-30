//! `layout` (plan 16, milestone 16.2): a lint of what the page looks
//! like. The checks run in the page (`injected/src/layout.ts`), once per
//! frame, on geometry and computed styles; this module moves their boxes
//! into the page's coordinates, measures contrast on screenshots
//! ([`contrast`]), gives the elements refs and keys, and runs it all per
//! viewport (`--viewports`), over the whole page (`--full`) and with
//! stretched text (`--stretch-text`), putting the page back as it was
//! each time. `expect --layout` ([`expect`]) compares the findings with a
//! baseline.

mod contrast;
mod expect;

use std::collections::HashSet;
use std::fmt::Write as _;
use std::time::Duration;

use base64::Engine as _;
use cdp::domains::{page as cdp_page, runtime};
use protocol::schema::{
    LayoutBox, LayoutData, LayoutFinding, LayoutSeverity, LayoutViewport, Viewport, WhyElement,
};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::time::Instant;

use crate::context::Scoped as _;
use crate::errors::from_cdp;
use crate::expect::pixels::Image;
use crate::frames::World;
use crate::injected::{self, GUARD};
use crate::input::{Candidate, element_text, refer_all};
use crate::locator::{LocateOptions, frame_step, read_listed, release};
use crate::request::{LayoutRequest, millis};
use crate::snapshot::{NodeKey, Target};
use crate::tabs::{FrameContext, Page, tab_name};
use crate::{Engine, Running};

/// The kinds, in the order the output sorts them.
pub(crate) const KINDS: [&str; 8] = [
    "covered",
    "overlap",
    "clipped",
    "truncated",
    "overflow-x",
    "offscreen",
    "small-target",
    "contrast",
];

/// The kinds that look only at what is in view: `--full` runs them at
/// every scroll position.
const IN_VIEW: [&str; 2] = ["covered", "contrast"];

/// Kinds shown only with `--all`.
const INFO: [&str; 1] = ["truncated"];

/// At most this many text elements are measured for contrast.
const TEXTS: usize = 2000;

/// At most this many elements of one answer get refs.
const NAMED: usize = 1000;

/// `--full` scrolls at most this many viewports down.
const STEPS: usize = 40;

/// How long a viewport change may take to settle.
const SETTLE: Duration = Duration::from_secs(10);

/// How long to wait for an iframe's context.
const FRAME_WAIT: Duration = Duration::from_secs(2);

/// The object group of the answers' handles.
const GROUP: &str = "chrome-cdp-layout";

/// `--viewports`' presets: the name, and the viewport it emulates.
fn presets() -> [(&'static str, Viewport); 3] {
    let viewport = |width, height, dpr, mobile, touch| Viewport {
        width,
        height,
        dpr,
        mobile,
        touch,
        screen_width: None,
        screen_height: None,
    };
    [
        ("mobile", viewport(390, 844, 3.0, true, true)),
        ("tablet", viewport(820, 1180, 2.0, false, true)),
        ("desktop", viewport(1280, 800, 1.0, false, false)),
    ]
}

/// A viewport to check at: its name, and what to emulate (`None`: the
/// tab's own).
type Wanted = (String, Option<Viewport>);

/// `--viewports` as typed: presets and `WxH` sizes.
fn parse_viewports(list: &[String]) -> Result<Vec<Wanted>, ErrorInfo> {
    let mut out: Vec<Wanted> = Vec::new();
    for item in list.iter().flat_map(|l| l.split(',')) {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        if let Some((name, viewport)) = presets().into_iter().find(|(n, _)| *n == item) {
            out.push((name.to_owned(), Some(viewport)));
            continue;
        }
        let size = item
            .split_once(['x', 'X'])
            .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)))
            .filter(|&(w, h)| (1..=10_000).contains(&w) && (1..=10_000).contains(&h));
        let Some((width, height)) = size else {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("--viewports takes mobile, tablet, desktop or WxH sizes, got {item:?}"),
            )
            .hint(Hint::run(
                Cmd::new("layout").flag("viewports", "mobile,tablet,desktop"),
            )));
        };
        out.push((
            format!("{width}x{height}"),
            Some(Viewport {
                width,
                height,
                dpr: 1.0,
                mobile: false,
                touch: false,
                screen_width: None,
                screen_height: None,
            }),
        ));
    }
    if out.is_empty() {
        out.push((String::new(), None));
    }
    Ok(out)
}

/// Checks `layout`'s arguments without a browser.
pub fn check_layout(req: &LayoutRequest) -> Result<(), ErrorInfo> {
    parse_viewports(&req.viewports)?;
    kinds(req)?;
    if req.min_target == Some(0) {
        return Err(
            ErrorInfo::new(ErrorKind::BadArgs, "--min-target must be at least 1 px")
                .hint(Hint::run(Cmd::new("layout").flag("min-target", "24"))),
        );
    }
    if let Some(pct) = req.stretch_text
        && !(1..=500).contains(&pct)
    {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("--stretch-text takes a percentage from 1 to 500, got {pct}"),
        )
        .hint(Hint::run(Cmd::new("layout").flag("stretch-text", "40"))));
    }
    if let Some(locator) = &req.locator {
        LocateOptions {
            strict_refs: req.strict_refs,
            testid_attribute: req.testid_attribute.clone(),
        }
        .parse(locator)?;
    }
    if let Some(baseline) = &req.baseline {
        expect::check_name(&baseline.name)?;
    }
    Ok(())
}

/// The kinds to check: `--only`'s, else all.
fn kinds(req: &LayoutRequest) -> Result<Vec<&'static str>, ErrorInfo> {
    if req.only.is_empty() {
        return Ok(KINDS.to_vec());
    }
    let mut out = Vec::new();
    for kind in req.only.iter().flat_map(|k| k.split(',')) {
        let kind = kind.trim();
        if kind.is_empty() {
            continue;
        }
        let Some(known) = KINDS.iter().find(|k| **k == kind) else {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("--only: there is no finding kind {kind:?}"),
            )
            .note("kinds", KINDS.join(", "))
            .hint(Hint::run(
                Cmd::new("layout").flag("only", "covered,clipped"),
            )));
        };
        if !out.contains(known) {
            out.push(*known);
        }
    }
    Ok(out)
}

/// A frame the check runs in.
struct Place {
    id: String,
    context: FrameContext,
    /// `layout LOCATOR`: the element, in this frame.
    root: Option<String>,
    /// Where its viewport's top left is in the tab's viewport.
    offset: (f64, f64),
    /// The part of its viewport the tab shows, in its own coordinates;
    /// `None` when none of it shows.
    view: Option<[f64; 4]>,
    main: bool,
}

/// `layoutCheck()`'s answer.
#[derive(Debug, Default, Deserialize)]
struct Checked {
    #[serde(default)]
    findings: Vec<PageFinding>,
    #[serde(default)]
    texts: Vec<PageText>,
    #[serde(default)]
    skipped: usize,
    #[serde(default)]
    scroll: [f64; 2],
    #[serde(default)]
    names: Vec<Candidate>,
    #[serde(default)]
    keys: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PageFinding {
    kind: String,
    el: usize,
    #[serde(rename = "box")]
    area: [f64; 4],
    #[serde(default)]
    others: Vec<usize>,
    #[serde(default)]
    details: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
struct PageText {
    #[serde(rename = "box")]
    area: [f64; 4],
    colour: [f64; 4],
    large: bool,
}

/// `textNames()`'s answer.
#[derive(Debug, Default, Deserialize)]
struct Texts {
    #[serde(default)]
    at: Vec<i64>,
    #[serde(default)]
    names: Vec<Candidate>,
    #[serde(default)]
    keys: Vec<String>,
}

/// What one viewport's run found, before it goes in the data.
struct Run {
    name: String,
    width: u32,
    height: u32,
    findings: Vec<LayoutFinding>,
}

/// What `layout` found, with the warnings to show.
pub(crate) struct Linted {
    url: String,
    runs: Vec<Run>,
    warnings: Vec<String>,
}

/// How a run names its viewport, for keys and output.
struct At<'a> {
    viewport: &'a str,
    /// The main frame's scroll: boxes move by it into the page's
    /// coordinates.
    scroll: (f64, f64),
}

impl Engine {
    /// `layout [LOCATOR]`: what looks broken in the active tab (or
    /// `--tab`), at its viewport or each of `--viewports`.
    pub async fn layout(&self, req: LayoutRequest) -> Result<Output, ErrorInfo> {
        check_layout(&req)?;
        if req.baseline.is_some() {
            return self.expect_layout(req).await;
        }
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let linted = {
            let _tab = self.tabs.lock(&page.target_id).await;
            let work = self.lint(&running, &page, &req, timeout);
            Box::pin(self.unless_blocked(&running, &page, mark, work)).await?
        };
        let data = layout_data(&req, tab_name(page.id), &linted);
        let errors: u64 = data.viewports.iter().map(|v| v.errors).sum();
        let text = layout_text(&data);
        let more = "--only KINDS, a LOCATOR or --max-chars 0 for all of it";
        Ok(linted.warnings.into_iter().fold(
            Output::typed(&data, text)
                .quiet(errors.to_string())
                .limit(req.max_chars, more),
            Output::warning,
        ))
    }

    /// Runs the check at each viewport asked for, and gives the tab its
    /// own viewport back after.
    pub(crate) async fn lint(
        &self,
        running: &Running,
        page: &Page,
        req: &LayoutRequest,
        timeout: Duration,
    ) -> Result<Linted, ErrorInfo> {
        let wanted = parse_viewports(&req.viewports)?;
        let overrides = wanted.iter().any(|(_, v)| v.is_some());
        let mut warnings = Vec::new();
        let result = async {
            let mut runs = Vec::new();
            for (name, viewport) in &wanted {
                if let Some(viewport) = viewport {
                    running
                        .tabs
                        .override_viewport(page, viewport, timeout)
                        .await
                        .map_err(|e| from_cdp(&e, &self.session))?;
                    if !self.settled_after_resize(running, page, timeout).await? {
                        warnings.push(format!(
                            "the page was still loading at {name} after {}; checked it as it was",
                            format::duration(SETTLE.min(timeout))
                        ));
                    }
                }
                runs.push(
                    self.lint_viewport(
                        running,
                        page,
                        req,
                        name,
                        viewport.as_ref(),
                        timeout,
                        &mut warnings,
                    )
                    .await?,
                );
            }
            Ok::<_, ErrorInfo>(runs)
        }
        .await;
        if overrides {
            let restored = running.tabs.end_viewport_override(page, timeout).await;
            if let Err(e) = restored
                && result.is_ok()
            {
                return Err(from_cdp(&e, &self.session));
            }
        }
        let runs = result?;
        let url = running
            .tabs
            .frames(page.id)
            .first()
            .map(|f| f.url.clone())
            .unwrap_or_default();
        warnings.dedup();
        Ok(Linted {
            url,
            runs,
            warnings,
        })
    }

    /// Waits until the page is quiet after a viewport change, as `wait
    /// --settle` does; `false` if it did not in time.
    async fn settled_after_resize(
        &self,
        running: &Running,
        page: &Page,
        timeout: Duration,
    ) -> Result<bool, ErrorInfo> {
        let now = Instant::now();
        let deadline = now + SETTLE.min(timeout);
        Ok(self.settle(running, page, now, deadline).await?.is_ok())
    }

    /// One viewport: the check, and with `--stretch-text` the check again
    /// with the text stretched, keeping what only that finds.
    #[allow(clippy::too_many_arguments)]
    async fn lint_viewport(
        &self,
        running: &Running,
        page: &Page,
        req: &LayoutRequest,
        name: &str,
        viewport: Option<&Viewport>,
        timeout: Duration,
        warnings: &mut Vec<String>,
    ) -> Result<Run, ErrorInfo> {
        let options = LocateOptions {
            strict_refs: req.strict_refs,
            testid_attribute: req.testid_attribute.clone(),
        };
        let scope = match &req.locator {
            Some(locator) => {
                let locator = options.parse(locator)?;
                let retry = |r: &str| Cmd::new("layout").arg(r).scoped(&self.session);
                Some(
                    self.locate_one(running, page, &locator, &options, Some(&retry), timeout)
                        .await?,
                )
            }
            None => None,
        };
        let result = async {
            let size = self.viewport_size(running, page, timeout).await?;
            let name = match (name, viewport) {
                (name, Some(_)) => name.to_owned(),
                (_, None) => presets()
                    .into_iter()
                    .find(|(_, v)| (v.width, v.height) == size)
                    .map_or_else(|| format!("{}x{}", size.0, size.1), |(n, _)| n.to_owned()),
            };
            let (width, height) = viewport.map_or(size, |v| (v.width, v.height));
            let mut findings = self
                .lint_page(running, page, req, scope.as_ref(), &name, timeout, warnings)
                .await?;
            if let Some(pct) = req.stretch_text {
                let places = self
                    .frame_places(running, page, scope.as_ref(), timeout)
                    .await?;
                let stretched = async {
                    for place in &places {
                        self.layout_call(place, "stretchText", vec![json!(pct)], timeout)
                            .await?;
                    }
                    self.lint_page(running, page, req, scope.as_ref(), &name, timeout, warnings)
                        .await
                }
                .await;
                for place in &places {
                    let _ = self
                        .layout_call(place, "unstretchText", Vec::new(), timeout)
                        .await;
                }
                let seen: HashSet<(String, String)> = findings
                    .iter()
                    .map(|f| (f.kind.clone(), f.key.clone()))
                    .collect();
                for mut finding in stretched? {
                    if !seen.contains(&(finding.kind.clone(), finding.key.clone())) {
                        finding.stretched = true;
                        findings.push(finding);
                    }
                }
                sort(&mut findings);
            }
            Ok::<_, ErrorInfo>(Run {
                name,
                width,
                height,
                findings,
            })
        }
        .await;
        if let Some(scope) = scope {
            scope.release();
        }
        result
    }

    /// The main frame's viewport size, in CSS pixels.
    async fn viewport_size(
        &self,
        running: &Running,
        page: &Page,
        timeout: Duration,
    ) -> Result<(u32, u32), ErrorInfo> {
        let context = self
            .frame_context(running, page, None, World::Isolated, timeout)
            .await?;
        let mut params = runtime::CallFunctionOnParams::new(
            "function () { return [innerWidth, innerHeight]; }".to_owned(),
        );
        params.execution_context_id = Some(context.context_id);
        params.return_by_value = Some(true);
        let result = context
            .session
            .with_timeout(timeout)
            .call(params)
            .await
            .map_err(|e| from_cdp(&e, &self.session))?;
        let size: (u32, u32) = result
            .result
            .value
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or((0, 0));
        Ok(size)
    }

    /// The check at the page's current viewport: every kind where the
    /// page is scrolled to; with `--full`, the kinds that look only at
    /// what is in view again at each viewport down the page.
    #[allow(clippy::too_many_arguments)]
    async fn lint_page(
        &self,
        running: &Running,
        page: &Page,
        req: &LayoutRequest,
        scope: Option<&Target>,
        viewport: &str,
        timeout: Duration,
        warnings: &mut Vec<String>,
    ) -> Result<Vec<LayoutFinding>, ErrorInfo> {
        let kinds = kinds(req)?;
        let mut findings = Vec::new();
        let places = self.frame_places(running, page, scope, timeout).await?;
        let first: Vec<&str> = kinds
            .iter()
            .copied()
            .filter(|k| !req.full || !IN_VIEW.contains(k))
            .collect();
        self.lint_here(
            running,
            page,
            req,
            &places,
            &first,
            viewport,
            timeout,
            warnings,
            &mut findings,
        )
        .await?;
        let again: Vec<&str> = kinds
            .iter()
            .copied()
            .filter(|k| IN_VIEW.contains(k))
            .collect();
        if req.full && !again.is_empty() {
            let main = places.iter().find(|p| p.main).unwrap_or(&places[0]);
            let extent = self
                .layout_call(main, "pageExtent", Vec::new(), timeout)
                .await?;
            let [height, inner, x0, y0]: [f64; 4] =
                serde_json::from_value(extent).unwrap_or([0.0; 4]);
            let scrolled = async {
                let mut y = 0.0;
                for _ in 0..STEPS {
                    self.layout_call(main, "scrollPage", vec![json!(x0), json!(y)], timeout)
                        .await?;
                    let places = self.frame_places(running, page, scope, timeout).await?;
                    self.lint_here(
                        running,
                        page,
                        req,
                        &places,
                        &again,
                        viewport,
                        timeout,
                        warnings,
                        &mut findings,
                    )
                    .await?;
                    y += inner.max(1.0);
                    if y >= height - 1.0 {
                        break;
                    }
                }
                Ok::<_, ErrorInfo>(())
            }
            .await;
            let back = self
                .layout_call(main, "scrollPage", vec![json!(x0), json!(y0)], timeout)
                .await;
            scrolled?;
            back?;
        }
        // The same finding seen from two scroll positions counts once.
        let mut seen = HashSet::new();
        findings.retain(|f| seen.insert((f.kind.clone(), f.key.clone())));
        sort(&mut findings);
        Ok(findings)
    }

    /// Runs `kinds` in every place where the page is now, adding the
    /// findings to `out`.
    #[allow(clippy::too_many_arguments)]
    async fn lint_here(
        &self,
        running: &Running,
        page: &Page,
        req: &LayoutRequest,
        places: &[Place],
        kinds: &[&str],
        viewport: &str,
        timeout: Duration,
        warnings: &mut Vec<String>,
        out: &mut Vec<LayoutFinding>,
    ) -> Result<(), ErrorInfo> {
        let generation = running
            .tabs
            .snapshot_memory(page.id, |m| m.refs.generation());
        let frames = running.tabs.frames(page.id);
        let testid = req
            .testid_attribute
            .clone()
            .unwrap_or_else(|| crate::locator::DEFAULT_TESTID_ATTRIBUTE.to_owned());
        let mut scroll = (0.0, 0.0);
        let mut texts: Vec<(usize, Vec<PageText>)> = Vec::new();
        let mut skipped = 0;
        for (index, place) in places.iter().enumerate() {
            let in_view: Vec<&str> = kinds
                .iter()
                .copied()
                .filter(|k| place.view.is_some() || !IN_VIEW.contains(k))
                .collect();
            if in_view.is_empty() {
                continue;
            }
            let options = json!({
                "kinds": in_view,
                "minTarget": req.min_target.unwrap_or(24),
                "main": place.main,
                "view": place.view,
                "testid": testid,
                "texts": TEXTS,
            });
            let root = place.root.as_ref().map(|id| runtime::CallArgument {
                value: None,
                unserializable_value: None,
                object_id: Some(id.clone()),
            });
            let root = root.unwrap_or_else(|| argument(Value::Null));
            let answer = self
                .layout_listed(
                    place,
                    "function (root, options) { return globalThis.__ccdp.layoutCheck(root, options); }",
                    vec![root, argument(options)],
                    timeout,
                )
                .await;
            let (json, keys) = match answer {
                Ok(answer) => answer,
                // An iframe that went away meanwhile has nothing to check.
                Err(_) if !place.main => continue,
                Err(e) => return Err(e),
            };
            let checked: Checked = serde_json::from_str(&json).unwrap_or_default();
            if place.main {
                scroll = (checked.scroll[0], checked.scroll[1]);
            }
            let names = refer_all(running, page, &checked.names, &keys, generation);
            let at = At { viewport, scroll };
            for found in &checked.findings {
                out.push(finding(found, &names, &checked.keys, place, &frames, &at));
            }
            skipped += checked.skipped;
            if !checked.texts.is_empty() {
                texts.push((index, checked.texts));
            }
        }
        if skipped > 0 {
            warnings.push(format!(
                "contrast: measured the first {TEXTS} text elements, skipped {skipped}"
            ));
        }
        if texts.is_empty() {
            return Ok(());
        }
        let measured = self
            .measure_contrast(running, page, places, &texts, timeout)
            .await?;
        for (index, failing) in measured {
            let place = &places[index];
            let indexes: Vec<usize> = failing.iter().map(|(i, _)| *i).collect();
            let answer = self
                .layout_listed(
                    place,
                    "function (indexes, testid) { return globalThis.__ccdp.textNames(indexes, testid); }",
                    vec![argument(json!(indexes)), argument(json!(testid))],
                    timeout,
                )
                .await;
            let Ok((json, keys)) = answer else {
                continue;
            };
            let named: Texts = serde_json::from_str(&json).unwrap_or_default();
            let names = refer_all(running, page, &named.names, &keys, generation);
            let at = At { viewport, scroll };
            for ((_, (text, measured)), el) in failing.iter().zip(&named.at) {
                let Ok(el) = usize::try_from(*el) else {
                    continue;
                };
                let needs = if text.large {
                    contrast::LARGE
                } else {
                    contrast::NORMAL
                };
                let mut details = Map::new();
                details.insert(
                    "ratio".into(),
                    json!((measured.ratio * 100.0).round() / 100.0),
                );
                details.insert("needs".into(), json!(needs));
                details.insert("large".into(), json!(text.large));
                let [r, g, b, _] = text.colour.map(|c| {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let c = c.round().clamp(0.0, 255.0) as u8;
                    c
                });
                details.insert("colour".into(), json!(contrast::hex([r, g, b])));
                details.insert(
                    "background".into(),
                    json!([contrast::hex(measured.low), contrast::hex(measured.high)]),
                );
                let found = PageFinding {
                    kind: "contrast".to_owned(),
                    el,
                    area: text.area,
                    others: Vec::new(),
                    details,
                };
                out.push(finding(&found, &names, &named.keys, place, &frames, &at));
            }
        }
        Ok(())
    }

    /// Takes the two screenshots and measures every text: the ones below
    /// the contrast they need, by place, with their index in the place's
    /// texts.
    #[allow(clippy::type_complexity)]
    async fn measure_contrast(
        &self,
        running: &Running,
        page: &Page,
        places: &[Place],
        texts: &[(usize, Vec<PageText>)],
        timeout: Duration,
    ) -> Result<Vec<(usize, Vec<(usize, (PageText, contrast::Measured))>)>, ErrorInfo> {
        let before = self.layout_shot(running, page, timeout).await?;
        let hidden = async {
            for place in places {
                let hid = self
                    .layout_call(place, "hideText", Vec::new(), timeout)
                    .await;
                if let Err(e) = hid
                    && place.main
                {
                    return Err(e);
                }
            }
            self.layout_shot(running, page, timeout).await
        }
        .await;
        for place in places {
            let _ = self
                .layout_call(place, "showText", Vec::new(), timeout)
                .await;
        }
        let after = hidden?;
        let mut out = Vec::new();
        for (index, list) in texts {
            let place = &places[*index];
            let mut failing = Vec::new();
            for (i, text) in list.iter().enumerate() {
                let [x, y, w, h] = text.area;
                let area = [x + place.offset.0, y + place.offset.1, w, h];
                let Some(measured) = contrast::measure(&before, &after, area, text.colour) else {
                    continue;
                };
                let needs = if text.large {
                    contrast::LARGE
                } else {
                    contrast::NORMAL
                };
                if measured.ratio < needs {
                    failing.push((
                        i,
                        (
                            PageText {
                                area: text.area,
                                colour: text.colour,
                                large: text.large,
                            },
                            measured,
                        ),
                    ));
                }
            }
            if !failing.is_empty() {
                out.push((*index, failing));
            }
        }
        Ok(out)
    }

    /// A screenshot of the tab's viewport in CSS pixels.
    async fn layout_shot(
        &self,
        running: &Running,
        page: &Page,
        timeout: Duration,
    ) -> Result<Image, ErrorInfo> {
        let session = page.session.with_timeout(timeout);
        let metrics = session
            .call(cdp_page::GetLayoutMetricsParams {})
            .await
            .map_err(|e| from_cdp(&e, &self.session))?;
        let context = self
            .frame_context(running, page, None, World::Isolated, timeout)
            .await?;
        let mut params = runtime::EvaluateParams::new("devicePixelRatio".to_owned());
        params.context_id = Some(context.context_id);
        params.return_by_value = Some(true);
        let dpr = context
            .session
            .with_timeout(timeout)
            .call(params)
            .await
            .ok()
            .and_then(|r| r.result.value)
            .and_then(|v| v.as_f64())
            .filter(|d| d.is_finite() && *d > 0.0)
            .unwrap_or(1.0);
        let viewport = &metrics.css_layout_viewport;
        #[allow(clippy::cast_precision_loss)]
        let clip = cdp_page::Viewport {
            x: viewport.page_x as f64,
            y: viewport.page_y as f64,
            width: viewport.client_width as f64,
            height: viewport.client_height as f64,
            scale: 1.0 / dpr,
        };
        let params = cdp_page::CaptureScreenshotParams {
            format: Some(cdp_page::CaptureScreenshotFormat::Png),
            clip: Some(clip),
            ..cdp_page::CaptureScreenshotParams::default()
        };
        let data = session
            .call(params)
            .await
            .map_err(|e| from_cdp(&e, &self.session))?
            .data;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| ErrorInfo::new(ErrorKind::ProtocolError, e.to_string()))?;
        Image::decode(&bytes).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::ProtocolError,
                format!("Chrome sent a screenshot that is not a PNG: {e}"),
            )
        })
    }

    /// The frames to check, with where each is in the tab's viewport:
    /// the scope's frame alone, or the main frame and every iframe that
    /// has a box.
    async fn frame_places(
        &self,
        running: &Running,
        page: &Page,
        scope: Option<&Target>,
        timeout: Duration,
    ) -> Result<Vec<Place>, ErrorInfo> {
        let (main_id, context) = self
            .frame_and_context(running, page, None, World::Isolated, timeout)
            .await?;
        let mut places = vec![Place {
            id: main_id.clone(),
            context,
            root: None,
            offset: (0.0, 0.0),
            view: None,
            main: true,
        }];
        let size = self.viewport_size(running, page, timeout).await?;
        places[0].view = Some([0.0, 0.0, f64::from(size.0), f64::from(size.1)]);
        let frames = running.tabs.frames(page.id);
        let wanted = |id: &str| match scope {
            None => true,
            // The scope's frame and the frames above it (to place it).
            Some(scope) => {
                let mut current = Some(scope.key.frame.clone());
                while let Some(frame) = current {
                    if frame == id {
                        return true;
                    }
                    current = frames
                        .iter()
                        .find(|f| f.id == frame)
                        .and_then(|f| f.parent.clone());
                }
                false
            }
        };
        let mut pending: Vec<_> = frames
            .iter()
            .filter(|f| f.parent.is_some() && wanted(&f.id))
            .collect();
        // Parents before their iframes.
        loop {
            let before = pending.len();
            let mut left = Vec::new();
            for frame in pending {
                let parent = frame.parent.as_deref().unwrap_or_default();
                let Some(outer) = places.iter().position(|p| p.id == parent) else {
                    left.push(frame);
                    continue;
                };
                if let Some(place) = self
                    .frame_place(running, page, &frame.id, &places[outer], timeout)
                    .await
                {
                    places.push(place);
                }
            }
            if left.is_empty() || left.len() == before {
                break;
            }
            pending = left;
        }
        let Some(scope) = scope else {
            return Ok(places);
        };
        let mut place = places
            .into_iter()
            .find(|p| p.id == scope.key.frame)
            .ok_or_else(|| {
                ErrorInfo::new(
                    ErrorKind::NotFound,
                    "the element's frame is not shown on the page",
                )
            })?;
        place.context = scope.context.clone();
        place.root = Some(scope.object_id.clone());
        Ok(vec![place])
    }

    /// Iframe `id` as a place, from its `<iframe>`'s content box in
    /// `outer`; `None` when it has none or went away.
    async fn frame_place(
        &self,
        running: &Running,
        page: &Page,
        id: &str,
        outer: &Place,
        timeout: Duration,
    ) -> Option<Place> {
        let context = running
            .tabs
            .context(page.id, id, World::Isolated, FRAME_WAIT)
            .await
            .ok()?;
        let (_, owner_context, owner) = self.frame_owner(running, page, id, timeout).await.ok()?;
        let mut params = runtime::CallFunctionOnParams::new(format!(
            "function () {{ {GUARD} return globalThis.__ccdp.contentBox(this); }}"
        ));
        params.object_id = Some(owner.object_id.clone());
        params.return_by_value = Some(true);
        let session = owner_context.session.with_timeout(timeout);
        let result = injected::call(&session, owner_context.context_id, params).await;
        release(&owner_context, owner.object_id);
        let content: [f64; 4] = serde_json::from_value(result.ok()?.result.value?).ok()?;
        let [x, y, w, h] = content;
        if w < 1.0 || h < 1.0 {
            return None;
        }
        let view = outer.view.and_then(|v| {
            let left = v[0].max(x);
            let top = v[1].max(y);
            let right = (v[0] + v[2]).min(x + w);
            let bottom = (v[1] + v[3]).min(y + h);
            (right > left && bottom > top).then_some([
                left - x,
                top - y,
                right - left,
                bottom - top,
            ])
        });
        Some(Place {
            id: id.to_owned(),
            context,
            root: None,
            offset: (outer.offset.0 + x, outer.offset.1 + y),
            view,
            main: false,
        })
    }

    /// Calls the bundle's `function` with `args` in `place` and returns
    /// its value.
    async fn layout_call(
        &self,
        place: &Place,
        function: &str,
        args: Vec<Value>,
        timeout: Duration,
    ) -> Result<Value, ErrorInfo> {
        let names: Vec<String> = (0..args.len()).map(|i| format!("a{i}")).collect();
        let declaration = format!(
            "function ({params}) {{ {GUARD} return globalThis.__ccdp.{function}({params}); }}",
            params = names.join(", ")
        );
        let mut params = runtime::CallFunctionOnParams::new(declaration);
        params.arguments = Some(args.into_iter().map(argument).collect());
        params.return_by_value = Some(true);
        params.await_promise = Some(true);
        let session = place.context.session.with_timeout(timeout);
        let result = injected::call(&session, place.context.context_id, params)
            .await
            .map_err(|e| from_cdp(&e, &self.session))?;
        if let Some(exception) = &result.exception_details {
            return Err(ErrorInfo::new(
                ErrorKind::EvalError,
                format!("checking the layout failed: {}", exception.text),
            ));
        }
        Ok(result.result.value.unwrap_or(Value::Null))
    }

    /// Calls `declaration`, which returns `{json, els}`, in `place`: the
    /// JSON text and the keys of the elements it names.
    async fn layout_listed(
        &self,
        place: &Place,
        declaration: &str,
        arguments: Vec<runtime::CallArgument>,
        timeout: Duration,
    ) -> Result<(String, Vec<Option<NodeKey>>), ErrorInfo> {
        let declaration = declaration.replacen("{ ", &format!("{{ {GUARD} "), 1);
        let mut params = runtime::CallFunctionOnParams::new(declaration);
        params.arguments = Some(arguments);
        params.object_group = Some(GROUP.to_owned());
        let session = place.context.session.with_timeout(timeout);
        let result = async {
            let result = injected::call(&session, place.context.context_id, params)
                .await
                .map_err(|e| from_cdp(&e, &self.session))?;
            if let Some(exception) = &result.exception_details {
                return Err(ErrorInfo::new(
                    ErrorKind::EvalError,
                    format!("checking the layout failed: {}", exception.text),
                ));
            }
            let Some(object_id) = result.result.object_id else {
                return Ok((String::new(), Vec::new()));
            };
            read_listed(&session, &object_id, &place.id, NAMED)
                .await
                .map_err(|e| from_cdp(&e, &self.session))
        }
        .await;
        let _ = session
            .call(runtime::ReleaseObjectGroupParams::new(GROUP.to_owned()))
            .await;
        result
    }
}

fn argument(value: Value) -> runtime::CallArgument {
    runtime::CallArgument {
        value: Some(value),
        unserializable_value: None,
        object_id: None,
    }
}

/// A finding of the page as the data has it: named elements, a key, a
/// box in the page's coordinates, and its line.
fn finding(
    found: &PageFinding,
    names: &[WhyElement],
    keys: &[String],
    place: &Place,
    frames: &[crate::frames::FrameInfo],
    at: &At<'_>,
) -> LayoutFinding {
    let unknown = || WhyElement {
        reference: None,
        role: "generic".to_owned(),
        name: String::new(),
        tag: "element".to_owned(),
        class: String::new(),
    };
    let element = names.get(found.el).cloned().unwrap_or_else(unknown);
    let others: Vec<WhyElement> = found
        .others
        .iter()
        .map(|&i| names.get(i).cloned().unwrap_or_else(unknown))
        .collect();
    let frame_step = (!place.main).then(|| {
        frame_step(frames, &place.id).unwrap_or_else(|| {
            let url = frames
                .iter()
                .find(|f| f.id == place.id)
                .map(|f| f.url.clone())
                .unwrap_or_default();
            format!("frame=\"{}\"", url.replace('"', "\\\""))
        })
    });
    let locator = |i: usize| {
        let key = keys.get(i).cloned().unwrap_or_default();
        match &frame_step {
            Some(step) => format!("{step} >> {key}"),
            None => key,
        }
    };
    let key = match found.kind.as_str() {
        "overflow-x" => format!("overflow-x {}", at.viewport),
        "covered" | "overlap" => format!(
            "{} {} {} {}",
            found.kind,
            at.viewport,
            locator(found.el),
            found
                .others
                .first()
                .map(|&i| locator(i))
                .unwrap_or_default()
        )
        .trim_end()
        .to_owned(),
        kind => format!("{kind} {} {}", at.viewport, locator(found.el)),
    };
    // Into the page's coordinates: the frame's place in the tab's
    // viewport, and the main frame's scroll.
    let (dx, dy) = (place.offset.0 + at.scroll.0, place.offset.1 + at.scroll.1);
    let [x, y, w, h] = found.area;
    #[allow(clippy::cast_possible_truncation)]
    let area = LayoutBox {
        x: (x + dx).round() as i64,
        y: (y + dy).round() as i64,
        width: w.round() as i64,
        height: h.round() as i64,
    };
    let mut details = found.details.clone();
    if let Some(Value::Array(point)) = details.get("at").cloned()
        && let [Some(px), Some(py)] =
            [point.first(), point.get(1)].map(|v| v.and_then(Value::as_f64))
    {
        #[allow(clippy::cast_possible_truncation)]
        let point = [(px + dx).round() as i64, (py + dy).round() as i64];
        details.insert("at".into(), json!(point));
    }
    match found.kind.as_str() {
        "covered" => {
            details.insert("covering".into(), json!(others.first()));
            if let Some(dismiss) = others.get(1) {
                details.insert("dismiss".into(), json!(dismiss));
            } else {
                details.remove("dismiss");
            }
        }
        "overlap" => {
            details.insert("other".into(), json!(others.first()));
        }
        "clipped" | "truncated" => {
            if let Some(by) = others.first() {
                details.insert("by".into(), json!(by));
            }
        }
        "overflow-x" => {
            let ends = details
                .get("ends")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let listed: Vec<Value> = others
                .iter()
                .zip(ends)
                .map(|(e, end)| json!({ "element": e, "right": end }))
                .collect();
            details.remove("ends");
            details.insert("elements".into(), json!(listed));
        }
        _ => {}
    }
    let message = message(&found.kind, &element, &others, &details);
    let severity = if INFO.contains(&found.kind.as_str()) {
        LayoutSeverity::Info
    } else {
        LayoutSeverity::Error
    };
    LayoutFinding {
        kind: found.kind.clone(),
        severity,
        key,
        message,
        element,
        area,
        details,
        stretched: false,
        frame: (!place.main).then(|| place.id.clone()),
    }
}

/// How a finding names its element: as `why` does, but an element with
/// no role, found for its text, by the text.
fn named(kind: &str, element: &WhyElement) -> String {
    let text_kind = matches!(kind, "clipped" | "truncated" | "contrast");
    if text_kind
        && (element.role == "generic" || element.role.is_empty())
        && !element.name.is_empty()
    {
        let mut out = format!("text \"{}\"", element.name.replace('"', "\\\""));
        if let Some(reference) = &element.reference {
            let _ = write!(out, " [{reference}]");
        }
        return out;
    }
    element_text(element)
}

fn number(details: &Map<String, Value>, key: &str) -> i64 {
    details.get(key).and_then(Value::as_f64).map_or(0, |v| {
        #[allow(clippy::cast_possible_truncation)]
        let v = v.round() as i64;
        v
    })
}

/// The finding's line after its kind.
fn message(
    kind: &str,
    element: &WhyElement,
    others: &[WhyElement],
    details: &Map<String, Value>,
) -> String {
    let el = named(kind, element);
    let other = |i: usize| others.get(i).map(element_text).unwrap_or_default();
    match kind {
        "covered" => {
            let at = details
                .get("at")
                .and_then(Value::as_array)
                .map(|p| {
                    p.iter()
                        .filter_map(Value::as_f64)
                        .map(|v| format!("{v:.0}"))
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            let mut out = format!("{el} under {} at {at}", other(0));
            if let Some(dismiss) = others.get(1).filter(|d| d.reference.is_some()) {
                let _ = write!(out, "; its {} likely dismisses it", element_text(dismiss));
            }
            out
        }
        "overlap" => format!(
            "{el} overlaps {} by {}×{}px",
            other(0),
            number(details, "width"),
            number(details, "height")
        ),
        "clipped" | "truncated" => {
            let (shown, full) = (number(details, "shown"), number(details, "full"));
            let axis = details.get("axis").and_then(Value::as_str) == Some("y");
            let size = if axis {
                format!("{shown}px of {full}px shown in height")
            } else {
                format!("{shown}px of {full}px shown")
            };
            let overflow = details
                .get("overflow")
                .and_then(Value::as_str)
                .unwrap_or("hidden");
            let on = if others.is_empty() {
                "on it".to_owned()
            } else {
                format!("on {}", other(0))
            };
            if kind == "clipped" {
                format!("{el}: {size} (overflow: {overflow} {on})")
            } else {
                let how = match details.get("how").and_then(Value::as_str) {
                    Some("line-clamp") => "a line clamp",
                    _ => "an ellipsis",
                };
                let text = details
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let text = if text.chars().count() > 80 {
                    format!("{}…", text.chars().take(79).collect::<String>())
                } else {
                    text.to_owned()
                };
                format!("{el}: cut with {how}, {size}: \"{text}\"")
            }
        }
        "overflow-x" => {
            let listed: Vec<String> = details
                .get("elements")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .zip(others)
                        .map(|(e, o)| {
                            format!(
                                "{} ends at {}px",
                                element_text(o),
                                e["right"].as_i64().unwrap_or(0)
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            format!(
                "the page is {}px wide at {}px: {}",
                number(details, "width"),
                number(details, "viewport"),
                listed.join(", ")
            )
        }
        "offscreen" => format!(
            "{el} at {},{}: no scroll reaches it (the page is {}x{})",
            number(details, "x"),
            number(details, "y"),
            number(details, "width"),
            number(details, "height")
        ),
        "small-target" => {
            let min = number(details, "min");
            format!(
                "{el}: {}×{}px, needs {min}×{min}",
                number(details, "width"),
                number(details, "height")
            )
        }
        "contrast" => {
            let ratio = details.get("ratio").and_then(Value::as_f64).unwrap_or(0.0);
            let needs = details.get("needs").and_then(Value::as_f64).unwrap_or(0.0);
            let background: Vec<&str> = details
                .get("background")
                .and_then(Value::as_array)
                .map(|b| b.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let on = match background.as_slice() {
                [low, high] if low != high => format!("{low}–{high}"),
                [one, ..] => (*one).to_owned(),
                [] => "its background".to_owned(),
            };
            format!(
                "{el}: {}:1 on {on}, needs {needs}:1",
                contrast::shown(ratio)
            )
        }
        _ => el,
    }
}

/// Sorts findings by kind (as [`KINDS`] lists them), then top to bottom
/// and left to right, then by key.
fn sort(findings: &mut [LayoutFinding]) {
    let rank = |kind: &str| KINDS.iter().position(|k| *k == kind).unwrap_or(KINDS.len());
    findings.sort_by(|a, b| {
        rank(&a.kind)
            .cmp(&rank(&b.kind))
            .then(a.area.y.cmp(&b.area.y))
            .then(a.area.x.cmp(&b.area.x))
            .then(a.key.cmp(&b.key))
    });
}

/// The data of what `layout` found: info findings only with `--all`.
fn layout_data(req: &LayoutRequest, tab: String, linted: &Linted) -> LayoutData {
    let viewports = linted
        .runs
        .iter()
        .map(|run| {
            let count = |severity| {
                run.findings
                    .iter()
                    .filter(|f| f.severity == severity)
                    .count() as u64
            };
            LayoutViewport {
                name: run.name.clone(),
                width: run.width,
                height: run.height,
                errors: count(LayoutSeverity::Error),
                info: count(LayoutSeverity::Info),
                findings: run
                    .findings
                    .iter()
                    .filter(|f| req.all || f.severity == LayoutSeverity::Error)
                    .cloned()
                    .collect(),
            }
        })
        .collect();
    LayoutData {
        tab,
        url: linted.url.clone(),
        locator: req.locator.clone(),
        stretch_text: req.stretch_text,
        viewports,
    }
}

/// `3 errors, 2 info (--all)`: the counts of a viewport, and the flag
/// that lists the info when it is not listed.
fn counts(viewport: &LayoutViewport) -> String {
    let errors = match viewport.errors {
        0 => "no errors".to_owned(),
        1 => "1 error".to_owned(),
        n => format!("{n} errors"),
    };
    if viewport.info == 0 {
        return errors;
    }
    let listed = viewport
        .findings
        .iter()
        .any(|f| f.severity == LayoutSeverity::Info);
    let flag = if listed { "" } else { " (--all)" };
    format!("{errors}, {} info{flag}", viewport.info)
}

/// The text output (see the plan's example).
fn layout_text(data: &LayoutData) -> String {
    let mut out = format!("layout {} {}", data.tab, data.url);
    if let Some(locator) = &data.locator {
        let _ = write!(out, " inside {locator}");
    }
    let lines = |out: &mut String, viewport: &LayoutViewport| {
        for f in &viewport.findings {
            let mark = match f.severity {
                LayoutSeverity::Error => '✗',
                LayoutSeverity::Info => '-',
            };
            let mut line = format!("{mark} {:<13} {}", f.kind, f.message);
            if f.stretched
                && let Some(pct) = data.stretch_text
            {
                let _ = write!(line, " (stretched {pct}%)");
            }
            let _ = write!(out, "\n{line}");
        }
    };
    match data.viewports.as_slice() {
        [one] => {
            let _ = write!(out, " ({}x{}): {}", one.width, one.height, counts(one));
            lines(&mut out, one);
        }
        many => {
            let summary: Vec<String> = many
                .iter()
                .map(|v| {
                    let errors = if v.errors == 1 {
                        "1 error".to_owned()
                    } else {
                        format!("{} errors", v.errors)
                    };
                    format!("{} {errors}", v.name)
                })
                .collect();
            let _ = write!(out, ": {}", summary.join(", "));
            for viewport in many {
                let _ = write!(
                    out,
                    "\n{} ({}x{}): {}",
                    viewport.name,
                    viewport.width,
                    viewport.height,
                    counts(viewport)
                );
                lines(&mut out, viewport);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(role: &str, name: &str, tag: &str, class: &str, reference: &str) -> WhyElement {
        WhyElement {
            reference: Some(reference.to_owned()),
            role: role.to_owned(),
            name: name.to_owned(),
            tag: tag.to_owned(),
            class: class.to_owned(),
        }
    }

    fn found(kind: &str, message: &str, y: i64) -> LayoutFinding {
        LayoutFinding {
            kind: kind.to_owned(),
            severity: if kind == "truncated" {
                LayoutSeverity::Info
            } else {
                LayoutSeverity::Error
            },
            key: format!("{kind} desktop x"),
            message: message.to_owned(),
            element: element("button", "x", "button", "", "e1"),
            area: LayoutBox {
                x: 0,
                y,
                width: 10,
                height: 10,
            },
            details: Map::new(),
            stretched: false,
            frame: None,
        }
    }

    #[test]
    fn viewports_are_presets_or_sizes() {
        let parsed = parse_viewports(&["mobile,390x844".to_owned(), "desktop".to_owned()]).unwrap();
        let names: Vec<&str> = parsed.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["mobile", "390x844", "desktop"]);
        let mobile = parsed[0].1.as_ref().unwrap();
        assert_eq!((mobile.width, mobile.height, mobile.dpr), (390, 844, 3.0));
        assert!(mobile.mobile && mobile.touch);
        assert!(parsed[2].1.as_ref().is_some_and(|v| !v.touch));
        assert_eq!(parse_viewports(&[]).unwrap(), vec![(String::new(), None)]);
        let err = parse_viewports(&["phone".to_owned()]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadArgs);
    }

    #[test]
    fn only_takes_known_kinds() {
        let mut req = LayoutRequest {
            tab: None,
            locator: None,
            viewports: Vec::new(),
            only: vec!["covered,clipped".to_owned()],
            all: false,
            full: false,
            min_target: None,
            stretch_text: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: 1000,
            max_chars: 0,
            baseline: None,
        };
        assert_eq!(kinds(&req).unwrap(), ["covered", "clipped"]);
        req.only = vec!["overlaps".to_owned()];
        assert_eq!(kinds(&req).unwrap_err().kind, ErrorKind::BadArgs);
        req.only.clear();
        assert_eq!(kinds(&req).unwrap().len(), KINDS.len());
        req.stretch_text = Some(0);
        assert!(check_layout(&req).is_err());
    }

    #[test]
    fn findings_sort_by_kind_then_place() {
        let mut findings = vec![
            found("contrast", "c", 0),
            found("covered", "b", 50),
            found("covered", "a", 10),
        ];
        sort(&mut findings);
        let order: Vec<&str> = findings.iter().map(|f| f.message.as_str()).collect();
        assert_eq!(order, ["a", "b", "c"]);
    }

    #[test]
    fn messages_name_the_elements() {
        let heading = element(
            "heading",
            "Wireless noise-cancelling headphones",
            "h2",
            "",
            "e21",
        );
        let card = element("generic", "", "div", "card", "e20");
        let mut details = Map::new();
        details.insert("axis".into(), json!("x"));
        details.insert("shown".into(), json!(212));
        details.insert("full".into(), json!(318));
        details.insert("overflow".into(), json!("hidden"));
        assert_eq!(
            message("clipped", &heading, std::slice::from_ref(&card), &details),
            "heading \"Wireless noise-cancelling headphones\" [e21]: 212px of 318px shown \
             (overflow: hidden on div.card [e20])"
        );
        let checkout = element("button", "Checkout", "button", "", "e12");
        let banner = element("generic", "We use cookies", "div", "cookie-banner", "e88");
        let mut details = Map::new();
        details.insert("at".into(), json!([640.0, 760.0]));
        assert_eq!(
            message("covered", &checkout, &[banner], &details),
            "button \"Checkout\" [e12] under div.cookie-banner [e88] at 640,760"
        );
        let text = element("generic", "Free shipping", "span", "", "e41");
        let mut details = Map::new();
        details.insert("ratio".into(), json!(2.93));
        details.insert("needs".into(), json!(4.5));
        details.insert("background".into(), json!(["#7fb1ff", "#a8c9ff"]));
        assert_eq!(
            message("contrast", &text, &[], &details),
            "text \"Free shipping\" [e41]: 2.9:1 on #7fb1ff–#a8c9ff, needs 4.5:1"
        );
        let close = element("link", "×", "a", "", "e40");
        let mut details = Map::new();
        details.insert("width".into(), json!(16));
        details.insert("height".into(), json!(16));
        details.insert("min".into(), json!(24));
        assert_eq!(
            message("small-target", &close, &[], &details),
            "link \"×\" [e40]: 16×16px, needs 24×24"
        );
    }

    #[test]
    fn text_counts_findings_and_hides_info() {
        let data = LayoutData {
            tab: "t1".to_owned(),
            url: "https://shop.test/cart".to_owned(),
            locator: None,
            stretch_text: None,
            viewports: vec![LayoutViewport {
                name: "desktop".to_owned(),
                width: 1280,
                height: 800,
                errors: 1,
                info: 2,
                findings: vec![found(
                    "small-target",
                    "link \"×\" [e40]: 16×16px, needs 24×24",
                    0,
                )],
            }],
        };
        assert_eq!(
            layout_text(&data),
            "layout t1 https://shop.test/cart (1280x800): 1 error, 2 info (--all)\n\
             ✗ small-target  link \"×\" [e40]: 16×16px, needs 24×24"
        );
    }
}
