//! The actionability pipeline (plan 05, milestone 5.1).
//!
//! Before an action touches the one element its locator matches, it runs
//! these steps, and retries all of them (after 0, 20, 100, 100, then every
//! 500 ms) until they pass or the timeout expires:
//!
//! 1. **Resolve** the locator to exactly one element. A ref resolves at
//!    once (a gone ref fails at once); other locators wait until they
//!    match. `Ambiguous` fails at once.
//! 2. **Visible**: a non-empty box, not `visibility: hidden`, not
//!    `opacity: 0` (checked in the page, `injected/src/actionable.ts`).
//! 3. **Stable**: the same box on two consecutive animation frames.
//! 4. **Enabled** (when the action needs it): not `disabled`, not inside a
//!    disabled `<fieldset>`, not `aria-disabled="true"`.
//! 5. **Editable** (`fill`, `type`): an input, textarea or contenteditable
//!    element that is not read-only.
//! 6. **Scroll into view** (`DOM.scrollIntoViewIfNeeded`), then measure
//!    (`DOM.getContentQuads`) and aim at the centre of the first quad that
//!    is in the viewport (clipped to it).
//! 7. **Hit test** (`DOM.getNodeForLocation`): what is at that point must
//!    be the element or inside it (across shadow roots). Otherwise the
//!    error names what covers it, with a ref, and a button in it that
//!    likely dismisses it.
//!
//! Quads and hit tests are in the coordinates of the element's *local
//! root* (the nearest ancestor frame in another process, or the main
//! frame): Chrome hit-tests through same-process iframes by itself. For an
//! element in an out-of-process iframe, the point is moved into each
//! parent's coordinates by the `<iframe>`'s content box, and the hit test
//! there must find that `<iframe>`, up to the tab's viewport, where input
//! events go.
//!
//! When the timeout expires, the error names the check that failed last
//! and what it saw. `--force` ([`Checks::force`]) skips checks 2-5 and 7,
//! but still scrolls the element into view and aims at it. Actions that
//! do not use the mouse skip 7 ([`Checks::no_hit_test`]).

use std::time::Duration;

use cdp::CdpError;
use cdp::domains::{dom, runtime};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::Instant;

use crate::errors::from_cdp;
use crate::frames::World;
use crate::injected::{self, GUARD};
use crate::locator::{LocateOptions, Locator, read_listed, release};
use crate::snapshot::{FrameSource, NodeKey, Target};
use crate::tabs::{FrameContext, Page};
use crate::{Engine, Running};

use geometry::{bounds, centre, is_below, local_root, offset, origin, out_of_view, pick};

mod geometry;
mod report;

/// How long to wait before each retry; the last one repeats.
const RETRY_DELAYS: [Duration; 5] = [
    Duration::ZERO,
    Duration::from_millis(20),
    Duration::from_millis(100),
    Duration::from_millis(100),
    Duration::from_millis(500),
];

/// How long the look after the timeout, for refs to what covers the
/// element, may take.
const LAST_LOOK: Duration = Duration::from_secs(2);

/// The object group of the hit test's handles.
const GROUP: &str = "chrome-cdp-aim";

/// What an action needs of its element besides being there, visible,
/// stable, in view and not covered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Checks {
    /// Not disabled (click, check, select; not hover).
    #[serde(default)]
    pub enabled: bool,
    /// Takes text (fill, type); implies `enabled`.
    #[serde(default)]
    pub editable: bool,
    /// `--force`: skip the checks and the hit test; still scroll the
    /// element into view and aim at it.
    #[serde(default)]
    pub force: bool,
    /// `--position dx,dy`: aim this far from the top-left corner of the
    /// element's box instead of at the centre of its visible part.
    #[serde(default)]
    pub position: Option<(f64, f64)>,
    /// Skip the hit test (7): the action reaches the element through the
    /// keyboard or the page, not a mouse at a point (fill, type, select).
    #[serde(default)]
    pub no_hit_test: bool,
}

/// Where an action acts on its element: a point in the tab's viewport, in
/// CSS pixels (what `Input.dispatchMouseEvent` takes on the tab's
/// session).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActionPoint {
    pub x: f64,
    pub y: f64,
    /// Set when a ref was re-resolved by role and name: the note to show.
    pub note: Option<String>,
}

/// An element that passed the pipeline: a handle on it, and where to act.
#[derive(Debug)]
pub(crate) struct Actionable {
    pub(crate) target: Target,
    pub(crate) x: f64,
    pub(crate) y: f64,
}

/// Why one run of the pipeline did not pass (it is retried).
#[derive(Debug)]
enum Failure {
    /// The locator matches nothing (yet).
    Missing(ErrorInfo),
    /// A check in the page failed: `check` is its name.
    Check { check: String, why: String },
    /// No part of the element is in the viewport after scrolling.
    OutOfView { why: String },
    /// Something else is at the point the action aims at.
    Obscured(Box<Obscured>),
}

/// What the hit test found instead of the element.
#[derive(Debug)]
struct Obscured {
    /// The point, in the local root's viewport.
    x: f64,
    y: f64,
    /// The tab's ref generation when it was seen: its elements get refs
    /// only if no frame loaded a new document since.
    generation: Option<u64>,
    covering: Option<Listed>,
    dismiss: Option<Listed>,
    /// The element does not take pointer events: the hit is its ancestor.
    inert: bool,
    pointer_events: Option<String>,
}

/// An element an error names, as the bundle describes it.
#[derive(Debug, Clone)]
struct Listed {
    key: Option<NodeKey>,
    role: String,
    name: String,
    tag: String,
}

/// The outcome of one run that did not pass.
enum Attempt {
    Retry(Failure),
    Final(ErrorInfo),
}

impl From<ErrorInfo> for Attempt {
    fn from(err: ErrorInfo) -> Self {
        Self::Final(err)
    }
}

/// A check the page reported as failed.
#[derive(Debug, Deserialize)]
struct Failed {
    check: String,
    why: String,
    #[serde(default)]
    r#final: bool,
}

#[derive(Debug, Default, Deserialize)]
struct Candidate {
    #[serde(default)]
    role: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    tag: String,
}

/// `covering()`'s answer.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Covered {
    covering: Option<Candidate>,
    dismiss: Option<Candidate>,
    #[serde(default)]
    inert: bool,
    pointer_events: Option<String>,
}

/// A local root's viewport: its size and scroll offset, in CSS pixels.
#[derive(Debug, Clone, Copy, Deserialize)]
struct Viewport {
    width: f64,
    height: f64,
    #[serde(default)]
    scroll_x: f64,
    #[serde(default)]
    scroll_y: f64,
}

/// The element the hit test must find at a point: the target, or an
/// `<iframe>` it is in.
struct Aim<'a> {
    key: &'a NodeKey,
    context: &'a FrameContext,
    object_id: &'a str,
}

impl Engine {
    /// Runs the actionability pipeline on the one element `locator`
    /// matches in tab `tab`, and returns where an action on it would act.
    /// Holds the tab's lock. Plan 05's actions run the same pipeline
    /// before they act; on its own, it serves tests.
    pub async fn action_point(
        &self,
        tab: Option<&str>,
        locator: &str,
        options: &LocateOptions,
        checks: Checks,
        timeout: Duration,
    ) -> Result<ActionPoint, ErrorInfo> {
        let locator = options.parse(locator)?;
        let running = self.running()?;
        let page = self.page(&running, tab, timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let _tab = self.tabs.lock(&page.target_id).await;
        let deadline = Instant::now() + timeout;
        let work = self.actionable(
            &running, &page, &locator, options, checks, None, timeout, deadline,
        );
        let ready = Box::pin(self.unless_blocked(&running, &page, mark, work)).await?;
        let point = ActionPoint {
            x: ready.x,
            y: ready.y,
            note: ready.target.note.clone(),
        };
        ready.target.release();
        Ok(point)
    }

    /// The actionability pipeline (see the module docs): the element
    /// `locator` matches in `page`, once it passes `checks`, and the point
    /// to act at. `retry` gives the action's command with another locator,
    /// for hints. It gives up at `deadline`; errors say it waited
    /// `timeout` (the action's, which may have begun earlier). Call it
    /// holding the tab's lock.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn actionable(
        &self,
        running: &Running,
        page: &Page,
        locator: &Locator,
        options: &LocateOptions,
        checks: Checks,
        retry: Option<&(dyn Fn(&str) -> Cmd + Sync)>,
        timeout: Duration,
        deadline: Instant,
    ) -> Result<Actionable, ErrorInfo> {
        let mut delays = RETRY_DELAYS.iter().copied();
        let mut last = None;
        let failure = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let attempt = self
                .attempt(running, page, locator, options, checks, retry, left)
                .await;
            let failure = match attempt {
                Ok(ready) => return Ok(ready),
                Err(Attempt::Retry(failure)) => failure,
                Err(Attempt::Final(err)) => match last {
                    // A step that ran out of time: say what failed before.
                    Some(failure) if err.kind == ErrorKind::Timeout => break failure,
                    _ => return Err(err),
                },
            };
            let delay = delays
                .next()
                .unwrap_or(RETRY_DELAYS[RETRY_DELAYS.len() - 1]);
            if Instant::now() + delay >= deadline {
                break failure;
            }
            last = Some(failure);
            tokio::time::sleep(delay).await;
        };
        // What covers the element gets refs only if no frame loaded a new
        // document since the hit test: look once more, so the error can
        // name it with a ref.
        let failure = match failure {
            Failure::Obscured(obscured)
                if running
                    .tabs
                    .snapshot_memory(page.id, |m| m.refs.generation())
                    != obscured.generation =>
            {
                match self
                    .attempt(running, page, locator, options, checks, retry, LAST_LOOK)
                    .await
                {
                    Ok(ready) => return Ok(ready),
                    Err(Attempt::Retry(again @ Failure::Obscured(_))) => again,
                    Err(_) => Failure::Obscured(obscured),
                }
            }
            failure => failure,
        };
        let failure = match failure {
            Failure::Obscured(mut obscured) => {
                self.still_there(running, page, &mut obscured).await;
                Failure::Obscured(obscured)
            }
            failure => failure,
        };
        Err(self.not_actionable(running, page, locator, failure, timeout, retry))
    }

    /// When a frame loaded a new document since the hit test (another
    /// frame, if the page keeps loading ads), what covers the element may
    /// still be there: if it is, its elements get refs after all. One that
    /// is gone gets none.
    async fn still_there(&self, running: &Running, page: &Page, obscured: &mut Obscured) {
        let generation = || {
            running
                .tabs
                .snapshot_memory(page.id, |m| m.refs.generation())
        };
        for _ in 0..3 {
            let now = generation();
            if now == obscured.generation {
                return;
            }
            for listed in [&mut obscured.covering, &mut obscured.dismiss]
                .into_iter()
                .flatten()
            {
                let Some(key) = &listed.key else { continue };
                match self.live_node(running, page, key, LAST_LOOK).await {
                    Ok(Some((context, object_id))) => release(&context, object_id),
                    _ => listed.key = None,
                }
            }
            if generation() == now {
                obscured.generation = now;
                return;
            }
        }
    }

    /// One run of the pipeline.
    #[allow(clippy::too_many_arguments)]
    async fn attempt(
        &self,
        running: &Running,
        page: &Page,
        locator: &Locator,
        options: &LocateOptions,
        checks: Checks,
        retry: Option<&(dyn Fn(&str) -> Cmd + Sync)>,
        timeout: Duration,
    ) -> Result<Actionable, Attempt> {
        let target = match self
            .locate_one(running, page, locator, options, retry, timeout)
            .await
        {
            Ok(target) => target,
            Err(err) if waits_for(&err) => return Err(Attempt::Retry(Failure::Missing(err))),
            Err(err) => return Err(Attempt::Final(err)),
        };
        match self
            .aim(running, page, locator, &target, checks, timeout)
            .await
        {
            Ok((x, y)) => Ok(Actionable { target, x, y }),
            Err(err) => {
                target.release();
                Err(err)
            }
        }
    }

    /// Steps 2-7 on a resolved element: the point to act at, in the tab's
    /// viewport.
    async fn aim(
        &self,
        running: &Running,
        page: &Page,
        locator: &Locator,
        target: &Target,
        checks: Checks,
        timeout: Duration,
    ) -> Result<(f64, f64), Attempt> {
        let generation = running
            .tabs
            .snapshot_memory(page.id, |m| m.refs.generation());
        if !checks.force
            && let Some(failed) = self.check_in_page(target, checks, timeout).await?
        {
            if failed.r#final {
                return Err(Attempt::Final(
                    ErrorInfo::new(
                        ErrorKind::NotEnabled,
                        format!("{locator} cannot take text: {}", failed.why),
                    )
                    .hint(
                        Hint::run(Cmd::new("snapshot").raw("-i").session(&self.session))
                            .why("lists the page's text fields"),
                    )
                    .details(json!({
                        "locator": locator.to_string(),
                        "check": failed.check,
                        "why": failed.why,
                    })),
                ));
            }
            return Err(Attempt::Retry(Failure::Check {
                check: failed.check,
                why: failed.why,
            }));
        }
        let session = target.context.session.with_timeout(timeout);
        let object_id = Some(target.object_id.clone());
        let scrolled = session
            .call(dom::ScrollIntoViewIfNeededParams {
                object_id: object_id.clone(),
                ..dom::ScrollIntoViewIfNeededParams::default()
            })
            .await;
        if let Err(err) = scrolled
            && !checks.force
        {
            return Err(self.no_box(&err));
        }
        let quads = session
            .call(dom::GetContentQuadsParams {
                object_id,
                ..dom::GetContentQuadsParams::default()
            })
            .await
            .map_err(|e| self.no_box(&e))?
            .quads;
        let frames = running.tabs.frame_sources(page.id);
        let mut root = local_root(&frames, &target.key.frame);
        let viewport = self.viewport(running, page, &root, timeout).await?;
        // The element's box, to scroll into view in each parent frame.
        let mut bounds = quads.first().map(|q| bounds(q)).unwrap_or_default();
        let picked = match checks.position {
            Some(position) => offset(&quads, position, viewport),
            None => pick(&quads, viewport).ok_or_else(|| out_of_view(&quads, viewport)),
        };
        let (mut x, mut y) = match picked {
            Ok(point) => point,
            Err(_) if checks.force && !quads.is_empty() => match checks.position {
                Some((dx, dy)) => {
                    let [left, top, ..] = geometry::bounds(&quads[0]);
                    (left + dx, top + dy)
                }
                None => centre(&quads[0]),
            },
            Err(why) => return Err(Attempt::Retry(Failure::OutOfView { why })),
        };
        let hit_test = !checks.force && !checks.no_hit_test;
        if hit_test {
            let aim = Aim {
                key: &target.key,
                context: &target.context,
                object_id: &target.object_id,
            };
            self.hit(
                running,
                page,
                &frames,
                &aim,
                (x, y),
                viewport,
                generation,
                timeout,
            )
            .await?;
        }
        // Out of each out-of-process iframe, up to the tab's viewport.
        let mut crossed = false;
        while frames
            .iter()
            .any(|f| f.info.id == root && f.info.parent.is_some())
        {
            let (parent, context, owner) = self.frame_owner(running, page, &root, timeout).await?;
            let outer = local_root(&frames, &parent);
            let result = async {
                let session = context.session.with_timeout(timeout);
                let box_model = || {
                    session.call(dom::GetBoxModelParams {
                        object_id: Some(owner.object_id.clone()),
                        ..dom::GetBoxModelParams::default()
                    })
                };
                // Scrolling in an out-of-process iframe scrolls its parent
                // later, on its own: scroll the element's box into view
                // here now, so the parent does not move after we measure.
                let model = box_model().await.map_err(|e| self.no_box(&e))?.model;
                let (dx, dy) = origin(&model.content);
                let (bx, by) = origin(&model.border);
                let [left, top, right, bottom] = bounds;
                let scrolled = session
                    .call(dom::ScrollIntoViewIfNeededParams {
                        object_id: Some(owner.object_id.clone()),
                        rect: Some(dom::Rect {
                            x: left + dx - bx,
                            y: top + dy - by,
                            width: (right - left).max(1.0),
                            height: (bottom - top).max(1.0),
                        }),
                        ..dom::ScrollIntoViewIfNeededParams::default()
                    })
                    .await;
                if let Err(err) = scrolled
                    && !checks.force
                {
                    return Err(self.no_box(&err));
                }
                let model = box_model().await.map_err(|e| self.no_box(&e))?.model;
                let (dx, dy) = origin(&model.content);
                x += dx;
                y += dy;
                bounds = [left + dx, top + dy, right + dx, bottom + dy];
                if checks.force {
                    return Ok(());
                }
                let viewport = self.viewport(running, page, &outer, timeout).await?;
                if x < 0.0 || y < 0.0 || x >= viewport.width || y >= viewport.height {
                    return Err(Attempt::Retry(Failure::OutOfView {
                        why: format!(
                            "its iframe shows it at x={} y={}, outside the {}x{} viewport",
                            x.round(),
                            y.round(),
                            viewport.width.round(),
                            viewport.height.round()
                        ),
                    }));
                }
                if !hit_test {
                    return Ok(());
                }
                let aim = Aim {
                    key: &owner.key,
                    context: &context,
                    object_id: &owner.object_id,
                };
                self.hit(
                    running,
                    page,
                    &frames,
                    &aim,
                    (x, y),
                    viewport,
                    generation,
                    timeout,
                )
                .await
            }
            .await;
            release(&context, owner.object_id);
            result?;
            root = outer;
            crossed = true;
        }
        if crossed {
            self.presented(running, page, timeout).await?;
        }
        Ok((x, y))
    }

    /// Waits until the tab has drawn two frames. The browser sends input
    /// into an out-of-process iframe by where it was last drawn, so after
    /// a scroll, a click at once can miss it.
    async fn presented(
        &self,
        running: &Running,
        page: &Page,
        timeout: Duration,
    ) -> Result<(), Attempt> {
        let context = self
            .frame_context(running, page, None, World::Isolated, timeout)
            .await?;
        let mut params = runtime::CallFunctionOnParams::new(
            "function () { return new Promise((resolve) => {\n\
               requestAnimationFrame(() => requestAnimationFrame(resolve));\n\
               setTimeout(resolve, 200); }); }"
                .to_owned(),
        );
        params.execution_context_id = Some(context.context_id);
        params.await_promise = Some(true);
        context
            .session
            .with_timeout(timeout)
            .call(params)
            .await
            .map_err(|e| self.gone_or(&e))?;
        Ok(())
    }

    /// Checks 2-5 in the page: the first that failed, or `None`.
    async fn check_in_page(
        &self,
        target: &Target,
        checks: Checks,
        timeout: Duration,
    ) -> Result<Option<Failed>, Attempt> {
        let mut params = runtime::CallFunctionOnParams::new(format!(
            "function (needs) {{ {GUARD} return globalThis.__ccdp.checkActionable(this, needs); }}"
        ));
        params.object_id = Some(target.object_id.clone());
        params.arguments = Some(vec![argument(json!({
            "enabled": checks.enabled,
            "editable": checks.editable,
        }))]);
        params.return_by_value = Some(true);
        params.await_promise = Some(true);
        let session = target.context.session.with_timeout(timeout);
        let result = injected::call(&session, target.context.context_id, params)
            .await
            .map_err(|e| self.gone_or(&e))?;
        let value = returned(result)?;
        Ok(serde_json::from_value(value).ok())
    }

    /// The size and scroll offset of frame `frame`'s viewport.
    async fn viewport(
        &self,
        running: &Running,
        page: &Page,
        frame: &str,
        timeout: Duration,
    ) -> Result<Viewport, Attempt> {
        let context = running
            .tabs
            .context(page.id, frame, World::Isolated, timeout)
            .await
            .map_err(|_| {
                Attempt::Retry(Failure::Missing(ErrorInfo::new(
                    ErrorKind::NotFound,
                    "its frame went away",
                )))
            })?;
        let mut params = runtime::CallFunctionOnParams::new(
            "function () { const v = visualViewport;\n\
             return { width: v ? v.width : innerWidth, height: v ? v.height : innerHeight,\n\
                      scroll_x: scrollX, scroll_y: scrollY }; }"
                .to_owned(),
        );
        params.execution_context_id = Some(context.context_id);
        params.return_by_value = Some(true);
        let result = context
            .session
            .with_timeout(timeout)
            .call(params)
            .await
            .map_err(|e| self.gone_or(&e))?;
        serde_json::from_value(returned(result)?)
            .map_err(|e| Attempt::Final(ErrorInfo::new(ErrorKind::ProtocolError, e.to_string())))
    }

    /// Step 7 for one frame level: what is at `(x, y)` in `viewport`
    /// (the viewport of `aim`'s local root) must be `aim` or inside it.
    #[allow(clippy::too_many_arguments)]
    async fn hit(
        &self,
        running: &Running,
        page: &Page,
        frames: &[FrameSource],
        aim: &Aim<'_>,
        (x, y): (f64, f64),
        viewport: Viewport,
        generation: Option<u64>,
        timeout: Duration,
    ) -> Result<(), Attempt> {
        let session = aim.context.session.with_timeout(timeout);
        // The hit test takes whole pixels of the document, not the
        // viewport: the point is at most half a pixel off.
        #[allow(clippy::cast_possible_truncation)]
        let params = dom::GetNodeForLocationParams::new(
            (x + viewport.scroll_x).round() as i64,
            (y + viewport.scroll_y).round() as i64,
        );
        let found = match session.call(params).await {
            Ok(found) => found,
            Err(CdpError::Protocol { message, .. }) if message.contains("No node found") => {
                return Err(Attempt::Retry(Failure::OutOfView {
                    why: format!("nothing is at its centre (x={} y={})", x.round(), y.round()),
                }));
            }
            Err(err) => return Err(self.gone_or(&err)),
        };
        let mut hit = NodeKey {
            frame: found.frame_id,
            backend: found.backend_node_id,
        };
        if &hit == aim.key {
            return Ok(());
        }
        // A hit in an iframe below the aim's frame stands for its
        // `<iframe>` there.
        while hit.frame != aim.key.frame && is_below(frames, &hit.frame, &aim.key.frame) {
            let (parent, context, owner) =
                self.frame_owner(running, page, &hit.frame, timeout).await?;
            hit = owner.key;
            release(&context, owner.object_id);
            debug_assert_eq!(hit.frame, parent);
        }
        let covered = if hit.frame == aim.key.frame {
            let resolved = session
                .call(dom::ResolveNodeParams {
                    backend_node_id: Some(hit.backend),
                    execution_context_id: Some(aim.context.context_id),
                    object_group: Some(GROUP.to_owned()),
                    ..dom::ResolveNodeParams::default()
                })
                .await
                .map_err(|e| self.gone_or(&e))?;
            let hit_id = resolved.object.object_id.unwrap_or_default();
            let mut params = runtime::CallFunctionOnParams::new(format!(
                "function (hit) {{ {GUARD} const ccdp = globalThis.__ccdp;\n\
                 return ccdp.reaches(this, hit) ? null : ccdp.covering(this, hit); }}"
            ));
            params.object_id = Some(aim.object_id.to_owned());
            params.arguments = Some(vec![runtime::CallArgument {
                value: None,
                unserializable_value: None,
                object_id: Some(hit_id),
            }]);
            params.object_group = Some(GROUP.to_owned());
            let result = self.covered(aim.context, &hit.frame, params, timeout).await;
            release_group(aim.context);
            result?
        } else {
            // Something in another frame (a parent's overlay) is on top.
            let Some((context, object_id)) = self.live_node(running, page, &hit, timeout).await?
            else {
                return Err(Attempt::Retry(Failure::Missing(ErrorInfo::new(
                    ErrorKind::NotFound,
                    "what covered it went away",
                ))));
            };
            let mut params = runtime::CallFunctionOnParams::new(format!(
                "function () {{ {GUARD} return globalThis.__ccdp.covering(null, this); }}"
            ));
            params.object_id = Some(object_id);
            params.object_group = Some(GROUP.to_owned());
            let result = self.covered(&context, &hit.frame, params, timeout).await;
            release_group(&context);
            result?
        };
        let Some((covered, keys)) = covered else {
            return Ok(());
        };
        let listed = |candidate: Option<Candidate>, key: Option<&Option<NodeKey>>| {
            candidate.map(|c| Listed {
                key: key.cloned().flatten(),
                role: c.role,
                name: c.name,
                tag: c.tag,
            })
        };
        Err(Attempt::Retry(Failure::Obscured(Box::new(Obscured {
            x,
            y,
            generation,
            covering: listed(covered.covering, keys.first()),
            dismiss: listed(covered.dismiss, keys.get(1)),
            inert: covered.inert,
            pointer_events: covered.pointer_events,
        }))))
    }

    /// Calls a function that returns `null` (nothing covers the element)
    /// or `covering()`'s `{json, els}`, and reads the latter.
    async fn covered(
        &self,
        context: &FrameContext,
        frame: &str,
        params: runtime::CallFunctionOnParams,
        timeout: Duration,
    ) -> Result<Option<(Covered, Vec<Option<NodeKey>>)>, Attempt> {
        let session = context.session.with_timeout(timeout);
        let result = injected::call(&session, context.context_id, params)
            .await
            .map_err(|e| self.gone_or(&e))?;
        if let Some(exception) = &result.exception_details {
            return Err(Attempt::Final(ErrorInfo::new(
                ErrorKind::EvalError,
                format!("the hit test failed: {}", exception.text),
            )));
        }
        let Some(object_id) = result.result.object_id else {
            return Ok(None);
        };
        let (json, keys) = read_listed(&session, &object_id, frame, 2)
            .await
            .map_err(|e| self.gone_or(&e))?;
        Ok(Some((
            serde_json::from_str(&json).unwrap_or_default(),
            keys,
        )))
    }

    /// A failed scroll or measure: the element has no box (retry), or the
    /// call failed.
    fn no_box(&self, err: &CdpError) -> Attempt {
        match err {
            CdpError::Protocol { message, .. }
                if message.contains("layout object") || message.contains("content quads") =>
            {
                Attempt::Retry(Failure::Check {
                    check: "visible".to_owned(),
                    why: "it is not rendered".to_owned(),
                })
            }
            _ => self.gone_or(err),
        }
    }

    /// A failed call: the element or its document went away (retry), or
    /// an error.
    fn gone_or(&self, err: &CdpError) -> Attempt {
        if is_gone(err) {
            Attempt::Retry(Failure::Missing(ErrorInfo::new(
                ErrorKind::NotFound,
                "it was removed from the page",
            )))
        } else {
            Attempt::Final(from_cdp(err, &self.session))
        }
    }
}

/// Whether a failed resolve means "not there yet": the locator matched
/// nothing, or names a frame that is not there. A ref that is gone does
/// not come back.
pub(crate) fn waits_for(err: &ErrorInfo) -> bool {
    err.kind == ErrorKind::NotFound
        && err
            .details
            .as_ref()
            .is_some_and(|d| d.get("count").is_some() || d.get("frame").is_some())
}

/// Whether a call failed because its element, context or document is
/// gone.
fn is_gone(err: &CdpError) -> bool {
    matches!(err, CdpError::Protocol { message, .. }
        if message.starts_with("Cannot find context with specified id")
            || message.starts_with("Execution context was destroyed")
            || message.starts_with("Inspected target navigated or closed")
            || message.contains("Could not find object with given id")
            || message.contains("No node with given id")
            || message.contains("Node is detached")
            || message.contains("Node with given id does not belong"))
}

/// The function's value, or its exception as an error.
fn returned(result: runtime::CallFunctionOnReturns) -> Result<Value, Attempt> {
    if let Some(exception) = result.exception_details {
        let message = exception
            .exception
            .and_then(|e| e.description)
            .unwrap_or(exception.text);
        let first = message.lines().next().unwrap_or_default().to_owned();
        return Err(Attempt::Final(ErrorInfo::new(
            ErrorKind::EvalError,
            format!("checking the element failed: {first}"),
        )));
    }
    Ok(result.result.value.unwrap_or(Value::Null))
}

fn argument(value: Value) -> runtime::CallArgument {
    runtime::CallArgument {
        value: Some(value),
        unserializable_value: None,
        object_id: None,
    }
}

/// Lets go of the hit test's handles in `context` (without waiting).
fn release_group(context: &FrameContext) {
    let session = context.session.clone();
    tokio::spawn(async move {
        let _ = session
            .call(runtime::ReleaseObjectGroupParams::new(GROUP.to_owned()))
            .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_not_found_matches_wait() {
        let none = ErrorInfo::new(ErrorKind::NotFound, "no element matches css=#x")
            .details(json!({ "locator": "css=#x", "count": 0 }));
        let frame =
            ErrorInfo::new(ErrorKind::NotFound, "no frame").details(json!({ "frame": "x" }));
        let unknown = ErrorInfo::new(ErrorKind::NotFound, "no e9").details(json!({ "ref": "e9" }));
        let stale = ErrorInfo::new(ErrorKind::RefStale, "stale");
        assert!(waits_for(&none));
        assert!(waits_for(&frame));
        assert!(!waits_for(&unknown));
        assert!(!waits_for(&stale));
    }
}
