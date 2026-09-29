//! `scroll [loc] [--by dx,dy | --to where]` (plan 05, milestone 5.2).
//!
//! Scrolling is done with real wheel events (`Input.dispatchMouseEvent`
//! `mouseWheel`) over the element, or without one over the page (the
//! middle of the viewport, unless a scroll container sits there), so the page's scroll and wheel listeners run (infinite scroll
//! loads more). The report is on the scroll container a wheel there
//! scrolls first: the element or its nearest scrollable ancestor, or the
//! page. `scroll <loc>` alone scrolls the element into view (the
//! actionability pipeline does that when it aims).

use std::fmt::Write as _;

use cdp::domains::runtime;
use protocol::schema::{ActedElement, ScrollData};
use protocol::{ErrorInfo, ErrorKind, Output};
use serde::Deserialize;
use serde_json::json;
use tokio::time::Instant;

use super::{Actor, Checks, Ready, describe, returned};
use crate::errors::from_cdp;
use crate::frames::World;
use crate::injected::{self, GUARD};
use crate::locator::read_listed;
use crate::request::{Action, ScrollTo};
use crate::snapshot::NodeKey;
use crate::tabs::FrameContext;

/// How many wheel events `--to` sends at most (content that keeps growing
/// under an infinite scroll would otherwise never end it).
const MAX_WHEELS: usize = 10;

/// Frames without movement after which a wheel counts as having done
/// nothing.
const IDLE_FRAMES: u32 = 10;

/// `scrollState()`'s answer.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Scroll {
    x: f64,
    y: f64,
    max_x: f64,
    max_y: f64,
    #[serde(default)]
    page: bool,
}

impl Scroll {
    fn at(self, x: f64, y: f64) -> bool {
        (self.x - x).abs() < 1.0 && (self.y - y).abs() < 1.0
    }
}

/// `--to`'s value as typed.
pub(super) fn to_arg(to: ScrollTo) -> String {
    match to {
        ScrollTo::Top => "top".to_owned(),
        ScrollTo::Bottom => "bottom".to_owned(),
        ScrollTo::Left => "left".to_owned(),
        ScrollTo::Right => "right".to_owned(),
        ScrollTo::Point { x, y } => format!("{x},{y}"),
    }
}

/// Where the element (or the page) is scrolled from: its handle, for the
/// page script, in its frame's context.
struct Scroller<'a> {
    context: FrameContext,
    /// The element; `None` for the page.
    object_id: Option<String>,
    frame: String,
    actor: &'a Actor<'a>,
}

impl Scroller<'_> {
    /// Calls a `__ccdp` function with `this` as the element (or `null`).
    async fn call(
        &self,
        function: &str,
        args: Vec<serde_json::Value>,
        by_value: bool,
    ) -> Result<runtime::CallFunctionOnReturns, ErrorInfo> {
        let mut params = runtime::CallFunctionOnParams::new(format!(
            "function (...args) {{ {GUARD} return globalThis.__ccdp.{function}(this === globalThis ? null : this, ...args); }}"
        ));
        params.object_id.clone_from(&self.object_id);
        params.arguments = Some(args.into_iter().map(super::argument).collect());
        params.return_by_value = Some(by_value);
        params.await_promise = Some(true);
        let session = self.context.session.with_timeout(self.actor.timeout);
        injected::call(&session, self.context.context_id, params)
            .await
            .map_err(|e| from_cdp(&e, self.actor.session()))
    }

    /// Where its container is now, and the container's key (`None` for
    /// the page).
    async fn state(&self) -> Result<(Scroll, Option<NodeKey>), ErrorInfo> {
        let result = self.call("scrollState", Vec::new(), false).await?;
        let session = self.context.session.with_timeout(self.actor.timeout);
        let object_id = match &result.result.object_id {
            Some(id) if result.exception_details.is_none() => id.clone(),
            _ => {
                returned(result)?;
                return Err(ErrorInfo::new(
                    ErrorKind::ProtocolError,
                    "reading the scroll position gave nothing",
                ));
            }
        };
        let (json, keys) = read_listed(&session, &object_id, &self.frame, 1)
            .await
            .map_err(|e| from_cdp(&e, self.actor.session()))?;
        let scroll: Scroll = serde_json::from_str(&json).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::ProtocolError,
                format!("reading the scroll position: {e}"),
            )
        })?;
        let key = if scroll.page {
            None
        } else {
            keys.into_iter().next().flatten()
        };
        Ok((scroll, key))
    }

    /// Waits until a wheel's scroll is over, and where it ended.
    async fn settled(&self, from: Scroll) -> Result<Scroll, ErrorInfo> {
        let result = self
            .call(
                "scrollSettled",
                vec![
                    json!({ "x": from.x, "y": from.y }),
                    json!(IDLE_FRAMES),
                    json!(self.actor.left().as_millis().min(3000)),
                ],
                true,
            )
            .await?;
        let value = returned(result)?;
        let scroll: Scroll = serde_json::from_value(value).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::ProtocolError,
                format!("reading the scroll position: {e}"),
            )
        })?;
        Ok(Scroll {
            page: from.page,
            ..scroll
        })
    }
}

impl Actor<'_> {
    pub(super) async fn scroll(&self) -> Result<Output, ErrorInfo> {
        let Action::Scroll {
            locator,
            by,
            to,
            force,
        } = &self.req.action
        else {
            unreachable!("not a scroll")
        };
        let ready = match locator {
            Some(locator) => {
                let parsed = self.parse(locator)?;
                let checks = Checks {
                    force: *force,
                    ..Checks::default()
                };
                let retry = |l: &str| self.retry(l);
                Some(self.ready(&parsed, checks, &retry).await?)
            }
            None => None,
        };
        let result = self.scroll_with(ready.as_ref(), *by, *to).await;
        let note = ready.as_ref().and_then(|r| r.ready.target.note.clone());
        if let Some(ready) = ready {
            ready.release();
        }
        let mut output = result?;
        if let Some(note) = note {
            output = output.warning(note);
        }
        Ok(output)
    }

    async fn scroll_with(
        &self,
        ready: Option<&Ready>,
        by: Option<(f64, f64)>,
        to: Option<ScrollTo>,
    ) -> Result<Output, ErrorInfo> {
        let scroller = if let Some(ready) = ready {
            Scroller {
                context: ready.ready.target.context.clone(),
                object_id: Some(ready.ready.target.object_id.clone()),
                frame: ready.ready.target.key.frame.clone(),
                actor: self,
            }
        } else {
            let context = self
                .engine
                .frame_context(self.running, self.page, None, World::Isolated, self.timeout)
                .await?;
            Scroller {
                context,
                object_id: None,
                // The main frame's id is the tab's target id.
                frame: self.page.target_id.clone(),
                actor: self,
            }
        };
        let (mut now, key) = scroller.state().await?;
        let mut wheeled = false;
        let mut moved = false;
        if by.is_some() || to.is_some() {
            let (x, y) = match ready {
                Some(ready) => (ready.ready.x, ready.ready.y),
                None => self.viewport_centre().await?,
            };
            self.move_to(x, y, 1, 0).await?;
            let mut wheels = 0;
            loop {
                let (dx, dy) = match (by, to) {
                    // `--by`: one wheel.
                    (Some(by), _) if wheels == 0 => by,
                    // `--to`: wheels until there, or stuck.
                    (None, Some(to)) => match remaining(now, to) {
                        Some(delta) if wheels < MAX_WHEELS && Instant::now() < self.deadline => {
                            delta
                        }
                        _ => break,
                    },
                    _ => break,
                };
                self.wheel(dx, dy).await?;
                wheeled = true;
                wheels += 1;
                let after = scroller.settled(now).await?;
                let progressed = !after.at(now.x, now.y);
                moved |= progressed;
                now = after;
                if !progressed {
                    break;
                }
            }
        }
        let container = match key {
            Some(key) => Some(self.container(&key, ready).await),
            None => None,
        };
        let data = ScrollData {
            tab: self.tab(),
            element: ready.map(|r| r.element.clone()),
            container,
            x: now.x,
            y: now.y,
            max_x: now.max_x,
            max_y: now.max_y,
            at_top: now.y <= 0.0,
            at_bottom: now.y >= now.max_y,
            at_left: now.x <= 0.0,
            at_right: now.x >= now.max_x,
        };
        let text = scroll_text(&data, wheeled, moved);
        let quiet = format!("{},{}", data.x, data.y);
        let mut output = Output::typed(&data, text).quiet(quiet);
        if wheeled && !moved {
            output = output.warning(
                "the wheel did not scroll anything: it is already at that end, or the page ignores wheel events there",
            );
        }
        Ok(output)
    }

    /// How the output names the scroll container `key`: the element itself
    /// when it is its own container.
    async fn container(&self, key: &NodeKey, ready: Option<&Ready>) -> ActedElement {
        if let Some(ready) = ready
            && &ready.ready.target.key == key
        {
            return ready.element.clone();
        }
        let fallback = ActedElement {
            reference: None,
            role: "generic".to_owned(),
            name: String::new(),
            tag: "element".to_owned(),
        };
        let Ok(Some((context, object_id))) = self
            .engine
            .live_node(self.running, self.page, key, self.timeout)
            .await
        else {
            return fallback;
        };
        let target = crate::snapshot::Target {
            key: key.clone(),
            context,
            object_id,
            note: None,
        };
        let element = self.name(&target).await.unwrap_or(fallback);
        target.release();
        element
    }

    /// Where a wheel scrolls the page: the middle of the tab's viewport,
    /// unless a scroll container (a fixed sidebar, a map) or an iframe
    /// sits there and would take the wheel first; then the first of a few
    /// other points that has none.
    async fn viewport_centre(&self) -> Result<(f64, f64), ErrorInfo> {
        #[derive(Deserialize)]
        struct Point {
            x: f64,
            y: f64,
        }
        let context = self
            .engine
            .frame_context(self.running, self.page, None, World::Isolated, self.timeout)
            .await?;
        let mut params = runtime::CallFunctionOnParams::new(PAGE_POINT.to_owned());
        params.execution_context_id = Some(context.context_id);
        params.return_by_value = Some(true);
        let result = context
            .session
            .with_timeout(self.timeout)
            .call(params)
            .await
            .map_err(|e| from_cdp(&e, self.session()))?;
        let point: Point = serde_json::from_value(returned(result)?).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::ProtocolError,
                format!("reading the viewport: {e}"),
            )
        })?;
        Ok((point.x, point.y))
    }
}

/// Finds a point in the viewport over which a wheel scrolls the page (see
/// `viewport_centre`): the middle first, then around it.
const PAGE_POINT: &str = r"function () {
  const w = document.documentElement.clientWidth || innerWidth;
  const h = document.documentElement.clientHeight || innerHeight;
  const root = document.scrollingElement || document.documentElement;
  const takesWheel = (el) => {
    for (; el && el !== root && el !== document.body; el = el.parentElement || el.getRootNode().host) {
      if (el.localName === 'iframe' || el.localName === 'frame') return true;
      const s = getComputedStyle(el);
      const y = /auto|scroll/.test(s.overflowY) && el.scrollHeight > el.clientHeight;
      const x = /auto|scroll/.test(s.overflowX) && el.scrollWidth > el.clientWidth;
      if (x || y) return true;
    }
    return false;
  };
  const at = [[0.5, 0.5], [0.5, 0.25], [0.5, 0.75], [0.25, 0.5], [0.75, 0.5],
    [0.25, 0.25], [0.75, 0.25], [0.25, 0.75], [0.75, 0.75], [0.1, 0.5], [0.9, 0.5]];
  for (const [fx, fy] of at) {
    const x = Math.round(w * fx), y = Math.round(h * fy);
    if (!takesWheel(document.elementFromPoint(x, y))) return { x, y };
  }
  return { x: Math.round(w / 2), y: Math.round(h / 2) };
}";

/// The wheel deltas still needed to reach `to` from `now`, or `None` when
/// it is there.
fn remaining(now: Scroll, to: ScrollTo) -> Option<(f64, f64)> {
    let (x, y) = match to {
        ScrollTo::Top => (now.x, 0.0),
        ScrollTo::Bottom => (now.x, now.max_y),
        ScrollTo::Left => (0.0, now.y),
        ScrollTo::Right => (now.max_x, now.y),
        ScrollTo::Point { x, y } => (x.clamp(0.0, now.max_x), y.clamp(0.0, now.max_y)),
    };
    (!now.at(x, y)).then_some((x - now.x, y - now.y))
}

/// `scrolled the page to 0,1200 (max 0,3400); at the bottom`.
fn scroll_text(data: &ScrollData, wheeled: bool, moved: bool) -> String {
    let what = match &data.container {
        Some(container) => describe(container),
        None => "the page".to_owned(),
    };
    let mut text = if moved {
        format!("scrolled {what} to")
    } else if wheeled {
        format!("did not scroll {what}; it is at")
    } else {
        match &data.element {
            Some(element) if data.container.as_ref() != Some(element) => {
                format!("{} is in view; {what} is at", describe(element))
            }
            Some(_) => format!("{what} is in view at"),
            None => format!("{what} is at"),
        }
    };
    let _ = write!(
        text,
        " {},{} (max {},{})",
        data.x, data.y, data.max_x, data.max_y
    );
    let mut ends = Vec::new();
    if data.max_y > 0.0 && data.at_bottom {
        ends.push("at the bottom");
    } else if data.max_y > 0.0 && data.at_top {
        ends.push("at the top");
    }
    if data.max_x > 0.0 && data.at_right {
        ends.push("at the right end");
    }
    if data.max_y <= 0.0 && data.max_x <= 0.0 {
        ends.push("nothing to scroll");
    }
    if !ends.is_empty() {
        let _ = write!(text, "; {}", ends.join(", "));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scroll(x: f64, y: f64) -> Scroll {
        Scroll {
            x,
            y,
            max_x: 0.0,
            max_y: 3000.0,
            page: true,
        }
    }

    #[test]
    fn wheels_go_the_rest_of_the_way() {
        assert_eq!(
            remaining(scroll(0.0, 100.0), ScrollTo::Bottom),
            Some((0.0, 2900.0))
        );
        assert_eq!(remaining(scroll(0.0, 3000.0), ScrollTo::Bottom), None);
        assert_eq!(
            remaining(scroll(0.0, 100.0), ScrollTo::Top),
            Some((0.0, -100.0))
        );
        assert_eq!(
            remaining(scroll(0.0, 100.0), ScrollTo::Point { x: 50.0, y: 9000.0 }),
            Some((0.0, 2900.0))
        );
    }

    fn data(y: f64, max_y: f64, container: Option<ActedElement>) -> ScrollData {
        ScrollData {
            tab: "t1".to_owned(),
            element: None,
            container,
            x: 0.0,
            y,
            max_x: 0.0,
            max_y,
            at_top: y <= 0.0,
            at_bottom: y >= max_y,
            at_left: true,
            at_right: true,
        }
    }

    #[test]
    fn reports_say_where_it_is() {
        assert_eq!(
            scroll_text(&data(1200.0, 3400.0, None), true, true),
            "scrolled the page to 0,1200 (max 0,3400)"
        );
        assert_eq!(
            scroll_text(&data(3400.0, 3400.0, None), true, true),
            "scrolled the page to 0,3400 (max 0,3400); at the bottom"
        );
        assert_eq!(
            scroll_text(&data(0.0, 3400.0, None), true, false),
            "did not scroll the page; it is at 0,0 (max 0,3400); at the top"
        );
        let list = ActedElement {
            reference: Some("e4".to_owned()),
            role: "list".to_owned(),
            name: "Feed".to_owned(),
            tag: "ul".to_owned(),
        };
        let mut shown = data(0.0, 0.0, Some(list.clone()));
        shown.element = Some(list);
        assert_eq!(
            scroll_text(&shown, false, false),
            "e4 list \"Feed\" is in view at 0,0 (max 0,0); nothing to scroll"
        );
        let mut item = data(600.0, 3400.0, None);
        item.element = Some(ActedElement {
            reference: Some("e9".to_owned()),
            role: "button".to_owned(),
            name: "More".to_owned(),
            tag: "button".to_owned(),
        });
        assert_eq!(
            scroll_text(&item, false, false),
            "e9 button \"More\" is in view; the page is at 0,600 (max 0,3400)"
        );
    }
}
