//! `drag <from> <to>` (plan 05, milestone 5.2).
//!
//! The mouse presses on `from`, moves to `to` in `steps` moves and
//! releases there: that is all a pointer-based drag (a sortable list built
//! on mouse or pointer events) needs. An HTML5 drag (`draggable`) cannot
//! be done with mouse events under CDP: once the page starts one, the
//! browser runs it on its own. So drags are intercepted
//! (`Input.setInterceptDrags`) while the mouse moves; if the page starts
//! one, the browser hands over its data (`Input.dragIntercepted`) and the
//! drag is finished with `Input.dispatchDragEvent` (enter, over, drop),
//! and the mouse is released. The tab's session is enough for drags in
//! out-of-process iframes too: Chrome 154 runs a drag for the whole tab
//! and reports it there (checked in `tests/pointer.rs`).

use std::time::Duration;

use cdp::domains::input::{
    DispatchDragEventParams, DispatchDragEventType, DragData, DragIntercepted,
    SetInterceptDragsParams,
};
use protocol::schema::DragResult;
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use serde_json::Value;

use super::{Actor, Checks, Ready, describe, point_text};
use crate::context::Scoped as _;
use crate::errors::from_cdp;
use crate::request::{Action, Button};

/// `--steps` when not given.
pub(crate) const DEFAULT_STEPS: u32 = 10;

/// How long to wait for the browser's drag data once the mouse moved on a
/// draggable element with the button down.
const DRAG_START: Duration = Duration::from_secs(1);

/// How far (in CSS pixels) an element may move while the other is aimed
/// at before the two do not fit in the viewport together.
const MOVED: f64 = 1.0;

impl Actor<'_> {
    pub(super) async fn drag(&self) -> Result<Output, ErrorInfo> {
        let Action::Drag {
            from,
            to,
            steps,
            force,
        } = &self.req.action
        else {
            unreachable!("not a drag")
        };
        let (from_loc, to_loc) = (self.parse(from)?, self.parse(to)?);
        let checks = Checks {
            force: *force,
            ..Checks::default()
        };
        let retry_from = |l: &str| self.retry(l);
        let retry_to = |l: &str| {
            let mut cmd = Cmd::new("drag").arg(from).arg(l);
            if *steps != DEFAULT_STEPS {
                cmd = cmd.flag("steps", steps.to_string());
            }
            cmd.scoped(self.session())
        };
        // Aiming at one may scroll the other away: aim at both twice, and
        // they must stay put the second time.
        let source = self.ready(&from_loc, checks, &retry_from).await?;
        let target = match self.ready(&to_loc, checks, &retry_to).await {
            Ok(target) => target,
            Err(err) => {
                source.release();
                return Err(err);
            }
        };
        let again = async {
            let source2 = self.aim(&from_loc, checks, &retry_from).await?;
            let moved = far(&source, source2.x, source2.y);
            source2.target.release();
            let target2 = self.aim(&to_loc, checks, &retry_to).await?;
            let moved = moved || far(&target, target2.x, target2.y);
            target2.target.release();
            Ok::<_, ErrorInfo>(moved)
        }
        .await;
        let (from_xy, to_xy) = (
            (source.ready.x, source.ready.y),
            (target.ready.x, target.ready.y),
        );
        let data_of = |method: &str| DragResult {
            tab: self.tab(),
            from: source.element.clone(),
            to: target.element.clone(),
            from_point: source.point(),
            to_point: target.point(),
            method: method.to_owned(),
        };
        let draggable = self
            .call(
                &source.ready.target,
                "function () { return globalThis.__ccdp.draggable(this); }",
                Vec::new(),
            )
            .await
            .map(|v| v == Value::Bool(true));
        let (from_name, to_name) = (source.element.clone(), target.element.clone());
        let result = match (again, draggable) {
            (Err(err), _) | (_, Err(err)) => Err(err),
            (Ok(true), _) => Err(ErrorInfo::new(
                ErrorKind::Obscured,
                format!("{from} and {to} do not fit in the viewport together"),
            )
            .note("from", describe(&from_name))
            .note("to", describe(&to_name))
            .hint(Hint::text(
                "drag in shorter hops (mouse down, then scroll, then mouse up), or make the viewport larger",
            ))),
            (Ok(false), Ok(draggable)) => Ok(draggable),
        };
        let data = (data_of("pointer"), data_of("html5"));
        source.release();
        target.release();
        let draggable = result?;
        let html5 = self
            .drag_with_mouse(from_xy, to_xy, *steps, draggable)
            .await?;
        let data = if html5 { data.1 } else { data.0 };
        let how = if html5 {
            "HTML5 drag and drop"
        } else {
            "mouse events"
        };
        let text = format!(
            "dragged {} to {} ({how})",
            describe(&data.from),
            describe(&data.to)
        );
        let quiet = point_text(data.to_point);
        Ok(Output::typed(&data, text).quiet(quiet))
    }

    /// Presses at `from`, moves to `to` in `steps` and releases, finishing
    /// an HTML5 drag the page starts with drag events. Whether it did.
    async fn drag_with_mouse(
        &self,
        from: (f64, f64),
        to: (f64, f64),
        steps: u32,
        draggable: bool,
    ) -> Result<bool, ErrorInfo> {
        let session = self.page.session.with_timeout(self.timeout);
        let mut drags = session.listen::<DragIntercepted>();
        session
            .call(SetInterceptDragsParams::new(true))
            .await
            .map_err(|e| from_cdp(&e, self.session()))?;
        let result = async {
            self.move_to(from.0, from.1, 1, 0).await?;
            self.press(Button::Left, true, 1, 0).await?;
            let mut data: Option<DragData> = None;
            let start = self.pointer_state();
            for step in 1..=steps {
                let t = f64::from(step) / f64::from(steps);
                let (x, y) = (
                    start.x + (to.0 - start.x) * t,
                    start.y + (to.1 - start.y) * t,
                );
                self.move_to(x, y, 1, 0).await?;
                // Polls once: the event comes with the move that starts it.
                if let Ok(Some(event)) = tokio::time::timeout(Duration::ZERO, drags.next()).await {
                    data = Some(event.data);
                    break;
                }
            }
            if data.is_none()
                && draggable
                && let Ok(Some(event)) = tokio::time::timeout(DRAG_START, drags.next()).await
            {
                data = Some(event.data);
            }
            let Some(data) = data else {
                self.press(Button::Left, false, 1, 0).await?;
                return Ok(false);
            };
            for kind in [
                DispatchDragEventType::DragEnter,
                DispatchDragEventType::DragOver,
                DispatchDragEventType::Drop,
            ] {
                session
                    .call(DispatchDragEventParams {
                        r#type: kind,
                        x: to.0,
                        y: to.1,
                        data: data.clone(),
                        modifiers: Some(self.held_bits()),
                    })
                    .await
                    .map_err(|e| from_cdp(&e, self.session()))?;
            }
            self.set_pointer(|p| {
                p.x = to.0;
                p.y = to.1;
            });
            self.press(Button::Left, false, 1, 0).await?;
            Ok(true)
        }
        .await;
        let off = session.call(SetInterceptDragsParams::new(false)).await;
        let html5 = result?;
        off.map_err(|e| from_cdp(&e, self.session()))?;
        Ok(html5)
    }
}

/// Whether `(x, y)` is not where `ready` was aimed.
fn far(ready: &Ready, x: f64, y: f64) -> bool {
    (ready.ready.x - x).abs() > MOVED || (ready.ready.y - y).abs() > MOVED
}
