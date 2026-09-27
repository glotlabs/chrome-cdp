//! Input and actionability (plan 05).
//!
//! Milestone 5.1 is the actionability pipeline ([`actionability`]): before
//! an action touches an element, it waits until the element is there,
//! visible, stable, enabled (and editable) and not covered, scrolls it into
//! view, and finds the point to act at in the tab's viewport. Milestone 5.2
//! adds the mouse and pointer actions on top ([`Engine::act`]): `click`,
//! `dblclick`, `hover`, `tap`, `drag`, `scroll`, and the low-level `mouse`
//! commands. Every event is real input (`Input.dispatchMouseEvent`, …) sent
//! to the tab, so the page sees trusted events in the order a user makes
//! them. Later milestones add keyboard and text (5.3), files (5.4) and the
//! post-action report (5.5) as more [`Action`]s.

mod actionability;
mod drag;
mod mouse;
mod scroll;

use std::fmt::Write as _;
use std::time::Duration;

use cdp::domains::runtime;
use protocol::schema::{ActedElement, Point};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use serde::Deserialize;
use serde_json::Value;
use tokio::time::Instant;

pub use actionability::{ActionPoint, Checks};
pub(crate) use mouse::Pointer;
pub use mouse::parse_modifiers;

use crate::errors::from_cdp;
use crate::injected::{self, GUARD};
use crate::locator::{LocateOptions, Locator};
use crate::request::{Action, ActionRequest, Modifier, SnapshotEngine, millis};
use crate::snapshot::Target;
use crate::tabs::{Page, tab_name};
use crate::{Engine, Running};

use actionability::Actionable;

/// Checks the arguments of an action without a browser.
pub fn check_action(req: &ActionRequest) -> Result<(), ErrorInfo> {
    let options = locate_options(req);
    let bad = |message: String, example: Cmd| {
        Err(ErrorInfo::new(ErrorKind::BadArgs, message).hint(Hint::run(example)))
    };
    match &req.action {
        Action::Click { locator, count, .. } => {
            options.parse(locator)?;
            if *count == 0 {
                return bad(
                    "--count must be at least 1".to_owned(),
                    Cmd::new("click").arg(locator).flag("count", "2"),
                );
            }
        }
        Action::Hover { locator, .. } | Action::Tap { locator, .. } => {
            options.parse(locator)?;
        }
        Action::Drag {
            from, to, steps, ..
        } => {
            options.parse(from)?;
            options.parse(to)?;
            if *steps == 0 {
                return bad(
                    "--steps must be at least 1".to_owned(),
                    Cmd::new("drag").arg(from).arg(to).flag("steps", "10"),
                );
            }
        }
        Action::Scroll {
            locator, by, to, ..
        } => {
            if let Some(locator) = locator {
                options.parse(locator)?;
            } else if by.is_none() && to.is_none() {
                return bad(
                    "scroll needs a locator, --by or --to".to_owned(),
                    Cmd::new("scroll").flag("by", "0,600"),
                );
            }
        }
        Action::Mouse(op) => mouse::check(op)?,
    }
    Ok(())
}

fn locate_options(req: &ActionRequest) -> LocateOptions {
    LocateOptions {
        strict_refs: req.strict_refs,
        testid_attribute: req.testid_attribute.clone(),
    }
}

impl Engine {
    /// Runs an input action (plan 05) in the tab `req` names, holding the
    /// tab's lock: waits until its element is actionable, sends the input
    /// events, and says what it acted on.
    pub async fn act(&self, req: ActionRequest) -> Result<Output, ErrorInfo> {
        check_action(&req)?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let _tab = self.tabs.lock(&page.target_id).await;
        let actor = Actor {
            engine: self,
            running: &running,
            page: &page,
            req: &req,
            options: locate_options(&req),
            timeout,
            deadline: Instant::now() + timeout,
        };
        let output = Box::pin(self.unless_blocked(&running, &page, mark, actor.run())).await?;
        Ok(self.report_events(&running, mark, output, None).await)
    }
}

/// One action in progress.
struct Actor<'a> {
    engine: &'a Engine,
    running: &'a Running,
    page: &'a Page,
    req: &'a ActionRequest,
    options: LocateOptions,
    /// Bounds each CDP call.
    timeout: Duration,
    /// When the whole action gives up (every wait shares it).
    deadline: Instant,
}

/// An element that passed the pipeline, and how the output names it.
struct Ready {
    ready: Actionable,
    element: ActedElement,
}

impl Ready {
    fn point(&self) -> Point {
        Point {
            x: round(self.ready.x),
            y: round(self.ready.y),
        }
    }

    fn release(self) {
        self.ready.target.release();
    }
}

impl Actor<'_> {
    async fn run(&self) -> Result<Output, ErrorInfo> {
        match &self.req.action {
            Action::Click { .. } | Action::Hover { .. } | Action::Tap { .. } => {
                self.pointer().await
            }
            Action::Mouse(op) => self.mouse(*op).await,
            Action::Drag { .. } => self.drag().await,
            Action::Scroll { .. } => self.scroll().await,
        }
    }

    fn tab(&self) -> String {
        tab_name(self.page.id)
    }

    fn session(&self) -> &str {
        &self.engine.session
    }

    /// The time left before the deadline (at least 1 ms).
    fn left(&self) -> Duration {
        self.deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1))
    }

    /// This command again with `locator` in place of the one typed (the
    /// `from` one for `drag`), for hints.
    fn retry(&self, locator: &str) -> Cmd {
        let action = &self.req.action;
        let cmd = Cmd::new(action.command());
        let with_modifiers = |mut cmd: Cmd, modifiers: &[Modifier]| {
            if !modifiers.is_empty() {
                let names: Vec<&str> = modifiers.iter().map(|m| m.as_str()).collect();
                cmd = cmd.flag("modifiers", names.join(","));
            }
            cmd
        };
        let with_position = |cmd: Cmd, position: &Option<(f64, f64)>| match position {
            Some((x, y)) => cmd.flag("position", format!("{x},{y}")),
            None => cmd,
        };
        match action {
            Action::Click {
                button,
                count,
                modifiers,
                position,
                dblclick,
                ..
            } => {
                let mut cmd = cmd.arg(locator);
                if *button != crate::request::Button::Left {
                    cmd = cmd.flag("button", button.as_str());
                }
                if !dblclick && *count != 1 {
                    cmd = cmd.flag("count", count.to_string());
                }
                with_position(with_modifiers(cmd, modifiers), position)
            }
            Action::Hover {
                modifiers,
                position,
                ..
            }
            | Action::Tap {
                modifiers,
                position,
                ..
            } => with_position(with_modifiers(cmd.arg(locator), modifiers), position),
            Action::Drag { to, steps, .. } => {
                let cmd = cmd.arg(locator).arg(to);
                if *steps == drag::DEFAULT_STEPS {
                    cmd
                } else {
                    cmd.flag("steps", steps.to_string())
                }
            }
            Action::Scroll { by, to, .. } => {
                let cmd = cmd.arg(locator);
                match (by, to) {
                    (Some((x, y)), _) => cmd.flag("by", format!("{x},{y}")),
                    (_, Some(to)) => cmd.flag("to", scroll::to_arg(*to)),
                    _ => cmd,
                }
            }
            Action::Mouse(_) => cmd,
        }
        .session(self.session())
    }

    fn parse(&self, locator: &str) -> Result<Locator, ErrorInfo> {
        self.options.parse(locator)
    }

    /// Runs the pipeline on `locator` and names the element it found.
    /// `retry` builds this command with another locator, for hints.
    async fn ready(
        &self,
        locator: &Locator,
        checks: Checks,
        retry: &(dyn Fn(&str) -> Cmd + Sync),
    ) -> Result<Ready, ErrorInfo> {
        let ready = self.aim(locator, checks, retry).await?;
        match self.name(&ready.target).await {
            Ok(element) => Ok(Ready { ready, element }),
            Err(err) => {
                ready.target.release();
                Err(err)
            }
        }
    }

    /// Runs the pipeline on `locator`.
    async fn aim(
        &self,
        locator: &Locator,
        checks: Checks,
        retry: &(dyn Fn(&str) -> Cmd + Sync),
    ) -> Result<Actionable, ErrorInfo> {
        self.engine
            .actionable(
                self.running,
                self.page,
                locator,
                &self.options,
                checks,
                Some(retry),
                self.timeout,
                self.deadline,
            )
            .await
    }

    /// How the output names `target`: its role, name and tag, and its ref
    /// (given now if it has none).
    async fn name(&self, target: &Target) -> Result<ActedElement, ErrorInfo> {
        #[derive(Deserialize)]
        struct Described {
            role: String,
            name: String,
            tag: String,
        }
        let generation = self
            .running
            .tabs
            .snapshot_memory(self.page.id, |m| m.refs.generation());
        let value = self
            .call(
                target,
                "function () { return globalThis.__ccdp.describe(this); }",
                Vec::new(),
            )
            .await?;
        let described: Described = serde_json::from_value(value).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::ProtocolError,
                format!("describing the element: {e}"),
            )
        })?;
        let reference = self
            .running
            .tabs
            .snapshot_memory(self.page.id, |m| {
                if let Some(number) = m.refs.get(&target.key) {
                    return Some(number);
                }
                // A node of a document that is gone must not get a ref.
                (Some(m.refs.generation()) == generation).then(|| {
                    m.refs.assign_from(
                        &target.key,
                        &described.role,
                        &described.name,
                        SnapshotEngine::Dom,
                    )
                })
            })
            .flatten()
            .map(|n| format!("e{n}"));
        Ok(ActedElement {
            reference,
            role: described.role,
            name: described.name,
            tag: described.tag,
        })
    }

    /// Calls `function` (which may use `__ccdp`) on `target` with
    /// `arguments` and returns its value, awaited.
    async fn call(
        &self,
        target: &Target,
        function: &str,
        arguments: Vec<Value>,
    ) -> Result<Value, ErrorInfo> {
        let mut params = runtime::CallFunctionOnParams::new(format!(
            "function (...args) {{ {GUARD} return ({function}).apply(this, args); }}"
        ));
        params.object_id = Some(target.object_id.clone());
        params.arguments = Some(arguments.into_iter().map(argument).collect());
        params.return_by_value = Some(true);
        params.await_promise = Some(true);
        let session = target.context.session.with_timeout(self.timeout);
        let result = injected::call(&session, target.context.context_id, params)
            .await
            .map_err(|e| from_cdp(&e, self.session()))?;
        returned(result)
    }
}

fn argument(value: Value) -> runtime::CallArgument {
    runtime::CallArgument {
        value: Some(value),
        unserializable_value: None,
        object_id: None,
    }
}

/// A function's value, or its exception as an error.
fn returned(result: runtime::CallFunctionOnReturns) -> Result<Value, ErrorInfo> {
    if let Some(exception) = result.exception_details {
        let message = exception
            .exception
            .and_then(|e| e.description)
            .unwrap_or(exception.text);
        let first = message.lines().next().unwrap_or_default().to_owned();
        return Err(ErrorInfo::new(
            ErrorKind::EvalError,
            format!("asking the page failed: {first}"),
        ));
    }
    Ok(result.result.value.unwrap_or(Value::Null))
}

/// `e6 button "Sign in"`: how the text output names an element.
pub(crate) fn describe(element: &ActedElement) -> String {
    let mut text = String::new();
    if let Some(reference) = &element.reference {
        let _ = write!(text, "{reference} ");
    }
    if element.role == "generic" || element.role.is_empty() {
        text.push_str(&element.tag);
    } else {
        text.push_str(&element.role);
    }
    if !element.name.is_empty() {
        let _ = write!(text, " \"{}\"", element.name.replace('"', "\\\""));
    }
    text
}

/// A coordinate as the output shows it: to hundredths.
fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// `12.5,300`: a point as the text output shows it.
fn point_text(point: Point) -> String {
    format!("{},{}", point.x, point.y)
}

/// The `modifiers` bit field of `Input` events: Alt 1, Control 2, Meta 4,
/// Shift 8.
fn modifier_bits(modifiers: &[Modifier]) -> i64 {
    modifiers
        .iter()
        .map(|m| match m {
            Modifier::Alt => 1,
            Modifier::Control => 2,
            Modifier::Meta => 4,
            Modifier::Shift => 8,
        })
        .fold(0, |bits, bit| bits | bit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(reference: Option<&str>, role: &str, name: &str, tag: &str) -> ActedElement {
        ActedElement {
            reference: reference.map(str::to_owned),
            role: role.to_owned(),
            name: name.to_owned(),
            tag: tag.to_owned(),
        }
    }

    #[test]
    fn elements_are_named_like_snapshot_lines() {
        assert_eq!(
            describe(&element(Some("e6"), "button", "Sign in", "button")),
            "e6 button \"Sign in\""
        );
        assert_eq!(describe(&element(None, "generic", "", "div")), "div");
        assert_eq!(
            describe(&element(Some("e2"), "link", "say \"hi\"", "a")),
            "e2 link \"say \\\"hi\\\"\""
        );
    }

    #[test]
    fn modifiers_are_bits() {
        assert_eq!(modifier_bits(&[]), 0);
        assert_eq!(modifier_bits(&[Modifier::Shift, Modifier::Meta]), 12);
        assert_eq!(modifier_bits(&[Modifier::Alt, Modifier::Control]), 3);
    }

    fn request(action: Action) -> ActionRequest {
        ActionRequest {
            action,
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: 1000,
        }
    }

    #[test]
    fn bad_arguments_fail_before_the_browser() {
        let click = |count| Action::Click {
            locator: "#go".to_owned(),
            button: crate::request::Button::Left,
            count,
            modifiers: Vec::new(),
            position: None,
            force: false,
            dblclick: false,
        };
        check_action(&request(click(1))).unwrap();
        let err = check_action(&request(click(0))).unwrap_err();
        assert_eq!(err.message, "--count must be at least 1");
        assert_eq!(err.exit_code(), 2);
        let scroll = Action::Scroll {
            locator: None,
            by: None,
            to: None,
            force: false,
        };
        let err = check_action(&request(scroll)).unwrap_err();
        assert_eq!(err.message, "scroll needs a locator, --by or --to");
        let drag = Action::Drag {
            from: "#a".to_owned(),
            to: "role=nope[".to_owned(),
            steps: 10,
            force: false,
        };
        assert_eq!(check_action(&request(drag)).unwrap_err().exit_code(), 2);
    }
}
