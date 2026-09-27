//! The mouse (plan 05, milestone 5.2): `click`, `dblclick`, `hover`,
//! `tap`, and the low-level `mouse move|down|up|wheel`.
//!
//! Each tab keeps where chrome-cdp last put the mouse and which buttons it
//! holds ([`Pointer`]), so every event carries the right `buttons` bit
//! field, moves start where the last one ended, and a `mouse down` in one
//! command and a `mouse up` in the next make one drag.

use std::fmt::Write as _;

use cdp::domains::input::{
    DispatchMouseEventParams, DispatchMouseEventType, DispatchTouchEventParams,
    DispatchTouchEventType, MouseButton, TouchPoint,
};
use protocol::schema::{MouseData, Point, PointerData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};

use super::{Actor, Checks, describe, modifier_bits, point_text, round};
use crate::errors::from_cdp;
use crate::request::{Action, Button, Modifier, MouseOp};

/// Where the mouse of a tab is and which buttons it holds.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Pointer {
    pub(crate) x: f64,
    pub(crate) y: f64,
    /// Left 1, right 2, middle 4.
    pub(crate) buttons: u32,
}

impl Pointer {
    /// The button an event names while buttons are held (the first of
    /// left, right, middle), for moves.
    fn held(self) -> Option<Button> {
        [Button::Left, Button::Right, Button::Middle]
            .into_iter()
            .find(|b| self.buttons & bit(*b) != 0)
    }
}

impl Pointer {
    /// The one button held, if exactly one is.
    fn only_held(self) -> Option<Button> {
        [Button::Left, Button::Right, Button::Middle]
            .into_iter()
            .find(|b| self.buttons == bit(*b))
    }
}

/// A button's bit in `buttons`.
fn bit(button: Button) -> u32 {
    match button {
        Button::Left => 1,
        Button::Right => 2,
        Button::Middle => 4,
    }
}

fn cdp_button(button: Option<Button>) -> MouseButton {
    match button {
        None => MouseButton::None,
        Some(Button::Left) => MouseButton::Left,
        Some(Button::Right) => MouseButton::Right,
        Some(Button::Middle) => MouseButton::Middle,
    }
}

/// Checks a `mouse` command's arguments.
pub(super) fn check(op: &MouseOp) -> Result<(), ErrorInfo> {
    let bad = |message: &str, example: Cmd| {
        Err(ErrorInfo::new(ErrorKind::BadArgs, message).hint(Hint::run(example)))
    };
    match op {
        MouseOp::Move { steps: 0, x, y } => bad(
            "--steps must be at least 1",
            Cmd::new("mouse")
                .arg("move")
                .arg(x.to_string())
                .arg(y.to_string())
                .flag("steps", "5"),
        ),
        MouseOp::Down { count: 0, .. } | MouseOp::Up { count: 0, .. } => bad(
            "--count must be at least 1",
            Cmd::new("mouse").arg(op.command()).flag("count", "2"),
        ),
        _ => Ok(()),
    }
}

impl Actor<'_> {
    /// The tab's mouse state.
    pub(super) fn pointer_state(&self) -> Pointer {
        self.running
            .tabs
            .pointer(self.page.id, |p| *p)
            .unwrap_or_default()
    }

    pub(super) fn set_pointer(&self, f: impl FnOnce(&mut Pointer)) {
        self.running.tabs.pointer(self.page.id, f);
    }

    /// Sends one mouse event to the tab.
    async fn mouse_event(&self, params: DispatchMouseEventParams) -> Result<(), ErrorInfo> {
        self.page
            .session
            .with_timeout(self.timeout)
            .call(params)
            .await
            .map_err(|e| from_cdp(&e, self.session()))?;
        Ok(())
    }

    /// Moves the mouse to `(x, y)` in `steps` even moves from where it is.
    pub(super) async fn move_to(
        &self,
        x: f64,
        y: f64,
        steps: u32,
        modifiers: i64,
    ) -> Result<(), ErrorInfo> {
        let from = self.pointer_state();
        for step in 1..=steps.max(1) {
            let t = f64::from(step) / f64::from(steps.max(1));
            let (px, py) = (from.x + (x - from.x) * t, from.y + (y - from.y) * t);
            let mut params =
                DispatchMouseEventParams::new(DispatchMouseEventType::MouseMoved, px, py);
            params.button = Some(cdp_button(from.held()));
            params.buttons = Some(i64::from(from.buttons));
            params.modifiers = Some(modifiers);
            self.set_pointer(|p| {
                p.x = px;
                p.y = py;
            });
            self.mouse_event(params).await?;
        }
        Ok(())
    }

    /// Presses (`down`) or releases `button` where the mouse is, as click
    /// number `count` of a series.
    pub(super) async fn press(
        &self,
        button: Button,
        down: bool,
        count: u32,
        modifiers: i64,
    ) -> Result<(), ErrorInfo> {
        let mut now = self.pointer_state();
        if down {
            now.buttons |= bit(button);
        } else {
            now.buttons &= !bit(button);
        }
        self.set_pointer(|p| p.buttons = now.buttons);
        let kind = if down {
            DispatchMouseEventType::MousePressed
        } else {
            DispatchMouseEventType::MouseReleased
        };
        let mut params = DispatchMouseEventParams::new(kind, now.x, now.y);
        params.button = Some(cdp_button(Some(button)));
        params.buttons = Some(i64::from(now.buttons));
        params.click_count = Some(i64::from(count));
        params.modifiers = Some(modifiers);
        self.mouse_event(params).await
    }

    /// A wheel event where the mouse is.
    pub(super) async fn wheel(&self, dx: f64, dy: f64) -> Result<(), ErrorInfo> {
        let now = self.pointer_state();
        let mut params =
            DispatchMouseEventParams::new(DispatchMouseEventType::MouseWheel, now.x, now.y);
        params.buttons = Some(i64::from(now.buttons));
        params.delta_x = Some(dx);
        params.delta_y = Some(dy);
        self.mouse_event(params).await
    }

    /// `click`, `dblclick`, `hover` and `tap`.
    pub(super) async fn pointer(&self) -> Result<Output, ErrorInfo> {
        let (locator, modifiers, position, force) = match &self.req.action {
            Action::Click {
                locator,
                modifiers,
                position,
                force,
                ..
            }
            | Action::Hover {
                locator,
                modifiers,
                position,
                force,
            }
            | Action::Tap {
                locator,
                modifiers,
                position,
                force,
            } => (locator, modifiers, *position, *force),
            _ => unreachable!("not a pointer action"),
        };
        let parsed = self.parse(locator)?;
        let checks = Checks {
            // Hovering a disabled control works (it shows its tooltip).
            enabled: !matches!(self.req.action, Action::Hover { .. }),
            editable: false,
            force,
            position,
            no_hit_test: false,
        };
        let retry = |l: &str| self.retry(l);
        let ready = self.ready(&parsed, checks, &retry).await?;
        if matches!(self.req.action, Action::Tap { .. }) {
            let touch = self
                .call(
                    &ready.ready.target,
                    "function () { return navigator.maxTouchPoints > 0; }",
                    Vec::new(),
                )
                .await;
            if !matches!(touch, Ok(serde_json::Value::Bool(true))) {
                ready.release();
                return Err(self.no_touch(locator));
            }
        }
        let point = ready.point();
        let (x, y) = (ready.ready.x, ready.ready.y);
        let element = ready.element.clone();
        let note = ready.ready.target.note.clone();
        ready.release();
        let modifiers = &self.resolve_modifiers(modifiers).await?;
        let bits = modifier_bits(modifiers);
        let mut data = PointerData {
            tab: self.tab(),
            action: self.req.action.command().to_owned(),
            locator: locator.clone(),
            element,
            point,
            button: None,
            count: None,
            modifiers: modifiers.iter().map(|m| m.as_str().to_owned()).collect(),
        };
        let verb = match &self.req.action {
            Action::Click { button, count, .. } => {
                self.move_to(x, y, 1, bits).await?;
                for n in 1..=*count {
                    self.press(*button, true, n, bits).await?;
                    self.press(*button, false, n, bits).await?;
                }
                data.button = Some(button.as_str().to_owned());
                data.count = Some(*count);
                clicked(*button, *count)
            }
            Action::Hover { .. } => {
                self.move_to(x, y, 1, bits).await?;
                "hovered".to_owned()
            }
            _ => {
                self.tap(x, y, bits).await?;
                "tapped".to_owned()
            }
        };
        let mut text = format!("{verb} {}", describe(&data.element));
        if let Action::Click { count, .. } = &self.req.action
            && *count > 2
        {
            let _ = write!(text, " {count} times");
        }
        if !modifiers.is_empty() {
            let _ = write!(text, " with {}", data.modifiers.join("+"));
        }
        let quiet = data
            .element
            .reference
            .clone()
            .unwrap_or_else(|| verb.clone());
        let mut output = Output::typed(&data, text).quiet(quiet);
        if let Some(note) = note {
            output = output.warning(note);
        }
        Ok(output)
    }

    /// A touch at `(x, y)`: start, then end. The browser makes the
    /// click (and the mouse events before it) of the tap.
    async fn tap(&self, x: f64, y: f64, modifiers: i64) -> Result<(), ErrorInfo> {
        let session = self.page.session.with_timeout(self.timeout);
        let point = TouchPoint {
            x,
            y,
            radius_x: None,
            radius_y: None,
            rotation_angle: None,
            force: None,
            tangential_pressure: None,
            tilt_x: None,
            tilt_y: None,
            twist: None,
            id: None,
        };
        let mut start =
            DispatchTouchEventParams::new(DispatchTouchEventType::TouchStart, vec![point]);
        start.modifiers = Some(modifiers);
        let mut end = DispatchTouchEventParams::new(DispatchTouchEventType::TouchEnd, Vec::new());
        end.modifiers = Some(modifiers);
        session
            .call(start)
            .await
            .map_err(|e| from_cdp(&e, self.session()))?;
        session
            .call(end)
            .await
            .map_err(|e| from_cdp(&e, self.session()))?;
        Ok(())
    }

    /// `tap` without touch emulation: the page would get no touch events.
    fn no_touch(&self, locator: &str) -> ErrorInfo {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            "tap needs touch emulation, which is off in this tab",
        )
        .hint(
            Hint::run(Cmd::new("click").arg(locator).session(self.session()))
                .why("a mouse click")
                .or(Cmd::new("cdp")
                    .flag("target", "page")
                    .arg("Emulation.setTouchEmulationEnabled")
                    .arg(r#"{"enabled": true}"#)
                    .session(self.session())
                    .to_string()),
        )
    }

    /// `mouse move|down|up|wheel`.
    pub(super) async fn mouse(&self, op: MouseOp) -> Result<Output, ErrorInfo> {
        let op = match op {
            MouseOp::Up {
                at,
                button: None,
                count,
            } => MouseOp::Up {
                at,
                button: Some(self.pointer_state().only_held().unwrap_or(Button::Left)),
                count,
            },
            op => op,
        };
        let mut button = None;
        let mut delta = None;
        let text = match op {
            MouseOp::Move { x, y, steps } => {
                self.move_to(x, y, steps, 0).await?;
                format!("moved the mouse to {}", point_text(rounded(x, y)))
            }
            MouseOp::Down {
                at,
                button: b,
                count,
            }
            | MouseOp::Up {
                at,
                button: Some(b),
                count,
            } => {
                let down = matches!(op, MouseOp::Down { .. });
                if let Some((x, y)) = at {
                    self.move_to(x, y, 1, 0).await?;
                }
                self.press(b, down, count, 0).await?;
                button = Some(b.as_str().to_owned());
                let now = self.pointer_state();
                format!(
                    "{} the {} button at {}",
                    if down { "pressed" } else { "released" },
                    b.as_str(),
                    point_text(rounded(now.x, now.y))
                )
            }
            MouseOp::Up { button: None, .. } => unreachable!("resolved above"),
            MouseOp::Wheel { dx, dy } => {
                self.wheel(dx, dy).await?;
                delta = Some(Point { x: dx, y: dy });
                let now = self.pointer_state();
                format!(
                    "turned the wheel by {dx},{dy} at {}",
                    point_text(rounded(now.x, now.y))
                )
            }
        };
        let now = self.pointer_state();
        let point = rounded(now.x, now.y);
        let data = MouseData {
            tab: self.tab(),
            action: op.command().to_owned(),
            point,
            button,
            buttons: now.buttons,
            delta,
        };
        Ok(Output::typed(&data, text).quiet(point_text(point)))
    }
}

fn rounded(x: f64, y: f64) -> Point {
    Point {
        x: round(x),
        y: round(y),
    }
}

/// `clicked`, `double-clicked`, `right-clicked`, …
fn clicked(button: Button, count: u32) -> String {
    let times = if count == 2 { "double-" } else { "" };
    match button {
        Button::Left => format!("{times}clicked"),
        other => format!("{}-{times}clicked", other.as_str()),
    }
}

/// `Modifier`s from their names as typed.
pub fn parse_modifiers(text: &str) -> Result<Vec<Modifier>, String> {
    let mut out = Vec::new();
    for name in text
        .split([',', '+'])
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        let modifier = match name.to_ascii_lowercase().as_str() {
            "alt" | "option" => Modifier::Alt,
            "control" | "ctrl" => Modifier::Control,
            "meta" | "cmd" | "command" => Modifier::Meta,
            "shift" => Modifier::Shift,
            "mod" => Modifier::Mod,
            _ => {
                return Err(format!(
                    "unknown modifier `{name}`; use Alt, Control, Meta, Shift or Mod"
                ));
            }
        };
        if !out.contains(&modifier) {
            out.push(modifier);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbs_name_the_button_and_count() {
        assert_eq!(clicked(Button::Left, 1), "clicked");
        assert_eq!(clicked(Button::Left, 2), "double-clicked");
        assert_eq!(clicked(Button::Right, 1), "right-clicked");
        assert_eq!(clicked(Button::Middle, 2), "middle-double-clicked");
        assert_eq!(clicked(Button::Left, 3), "clicked");
    }

    #[test]
    fn modifiers_parse_by_name() {
        assert_eq!(
            parse_modifiers("Shift,Meta").unwrap(),
            vec![Modifier::Shift, Modifier::Meta]
        );
        assert_eq!(
            parse_modifiers("ctrl+alt+ctrl").unwrap(),
            vec![Modifier::Control, Modifier::Alt]
        );
        assert_eq!(
            parse_modifiers("Hyper").unwrap_err(),
            "unknown modifier `Hyper`; use Alt, Control, Meta, Shift or Mod"
        );
    }

    #[test]
    fn held_buttons_name_moves() {
        let mut p = Pointer::default();
        assert_eq!(p.held(), None);
        p.buttons = bit(Button::Right) | bit(Button::Middle);
        assert_eq!(p.held(), Some(Button::Right));
        assert_eq!(p.only_held(), None);
        p.buttons = bit(Button::Middle);
        assert_eq!(p.only_held(), Some(Button::Middle));
    }
}
