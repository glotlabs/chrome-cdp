//! Input and actionability (plan 05).
//!
//! Milestone 5.1 is the actionability pipeline ([`actionability`]): before
//! an action touches an element, it waits until the element is there,
//! visible, stable, enabled (and editable) and not covered, scrolls it into
//! view, and finds the point to act at in the tab's viewport. Milestone 5.2
//! adds the mouse and pointer actions on top ([`Engine::act`]): `click`,
//! `dblclick`, `hover`, `tap`, `drag`, `scroll`, and the low-level `mouse`
//! commands. Milestone 5.3 adds the keyboard ([`keys`]: `press`, `type`)
//! and forms (`fill`, `select`, `check`, `uncheck`, `focus`, `blur`).
//! Every event is real input (`Input.dispatchMouseEvent`,
//! `Input.dispatchKeyEvent`, `Input.insertText`) sent to the tab, so the
//! page sees trusted events in the order a user makes them; only values
//! that take no typing (a date, a `<select>`) are set in the page.
//! Milestone 5.4 adds `upload` (files into a file input, or the file
//! chooser a click opens); `clipboard` lives in [`crate::clipboard`].
//! Milestone 5.5 adds the post-action report ([`report`]): after the
//! input, each action waits for what it caused and says what happened.

mod actionability;
mod drag;
mod form;
mod keyboard;
mod keys;
mod mouse;
mod report;

pub(crate) use report::origin;
mod scroll;
mod upload;

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
use crate::snapshot::{NodeKey, Target};
use crate::tabs::{Page, tab_name};
use crate::{Engine, Running};

use actionability::Actionable;
pub(crate) use actionability::waits_for;

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
        Action::Hover { locator, .. }
        | Action::Tap { locator, .. }
        | Action::Fill { locator, .. }
        | Action::Check { locator, .. }
        | Action::Focus { locator }
        | Action::Blur { locator } => {
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
        Action::Press { keys, locator } => {
            if let Some(locator) = locator {
                options.parse(locator)?;
            }
            if let Err(message) = keys::parse(keys) {
                let info = ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!(
                        "{message}; name keys like Enter, Tab, ArrowDown, F1, a or A, \
                         and hold them together with + (Control+A, Mod+Shift+Z)"
                    ),
                );
                return Err(match keys::suggest(keys) {
                    Some(fixed) => info
                        .note("did you mean", &fixed)
                        .hint(Hint::run(Cmd::new("press").arg(fixed))),
                    None => info.hint(Hint::run(Cmd::new("press").arg("Control+A Backspace"))),
                });
            }
        }
        Action::Type { text, locator, .. } => {
            if let Some(locator) = locator {
                options.parse(locator)?;
            }
            if text.is_empty() {
                let mut example = Cmd::new("fill");
                example = match locator {
                    Some(locator) => example.arg(locator),
                    None => example.raw("<locator>"),
                };
                return Err(ErrorInfo::new(ErrorKind::BadArgs, "nothing to type")
                    .hint(Hint::run(example.arg("")).why("clears a field")));
            }
        }
        Action::Upload {
            locator,
            files,
            clear,
            ..
        } => {
            options.parse(locator)?;
            match (clear, files.is_empty()) {
                (true, false) => {
                    return bad(
                        "--clear takes no files".to_owned(),
                        Cmd::new("upload").arg(locator).switch("clear"),
                    );
                }
                (false, true) => {
                    return bad(
                        "upload needs a file, or --clear".to_owned(),
                        Cmd::new("upload").arg(locator).raw("<file>"),
                    );
                }
                _ => {
                    upload::check_files(files)?;
                }
            }
        }
        Action::Select {
            locator, values, ..
        } => {
            options.parse(locator)?;
            if values.is_empty() {
                return bad(
                    "select needs an option's value or label".to_owned(),
                    Cmd::new("select").arg(locator).raw("<value>"),
                );
            }
        }
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
        let acted = async {
            let before = actor.before().await;
            let output = actor.run().await?;
            actor.report(output, before, mark).await
        };
        Box::pin(self.unless_blocked(&running, &page, mark, acted)).await
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
            Action::Press { .. } => self.press_keys().await,
            Action::Type { .. } => self.type_text().await,
            Action::Fill { .. } => self.fill().await,
            Action::Select { .. } => self.select().await,
            Action::Check { .. } => self.check().await,
            Action::Focus { .. } | Action::Blur { .. } => self.focus().await,
            Action::Upload { .. } => self.upload().await,
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
            Action::Press { keys, .. } => cmd.arg(keys).flag("in", locator),
            Action::Type { text, delay_ms, .. } => {
                let cmd = cmd.arg(text).flag("in", locator);
                if *delay_ms == 0 {
                    cmd
                } else {
                    cmd.flag(
                        "delay",
                        protocol::format::duration(Duration::from_millis(*delay_ms)),
                    )
                }
            }
            Action::Fill { text, .. } => cmd.arg(locator).arg(text),
            Action::Select { values, .. } => values.iter().fold(cmd.arg(locator), Cmd::arg),
            Action::Check { .. } | Action::Focus { .. } | Action::Blur { .. } => cmd.arg(locator),
            Action::Upload { files, clear, .. } => {
                let cmd = files.iter().fold(cmd.arg(locator), Cmd::arg);
                if *clear { cmd.switch("clear") } else { cmd }
            }
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
        let generation = self.generation();
        let value = self
            .call(
                target,
                "function () { return globalThis.__ccdp.acted(this); }",
                Vec::new(),
            )
            .await?;
        let described: Described = serde_json::from_value(value).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::ProtocolError,
                format!("describing the element: {e}"),
            )
        })?;
        Ok(self.acted(Some(&target.key), described, generation))
    }

    /// The tab's ref generation now: elements seen from now on get refs
    /// only while it holds.
    fn generation(&self) -> Option<u64> {
        self.running
            .tabs
            .snapshot_memory(self.page.id, |m| m.refs.generation())
    }

    /// How the output names the element `key` names, as the page
    /// `described` it, with its ref (given now if it has none, unless a
    /// frame loaded a new document since `generation`).
    fn acted(
        &self,
        key: Option<&NodeKey>,
        described: Described,
        generation: Option<u64>,
    ) -> ActedElement {
        let reference = key
            .and_then(|key| {
                self.running
                    .tabs
                    .snapshot_memory(self.page.id, |m| {
                        if let Some(number) = m.refs.get(key) {
                            return Some(number);
                        }
                        // A node of a document that is gone must not get a ref.
                        (Some(m.refs.generation()) == generation).then(|| {
                            m.refs.assign_from(
                                key,
                                &described.role,
                                &described.name,
                                SnapshotEngine::Dom,
                            )
                        })
                    })
                    .flatten()
            })
            .map(|n| format!("e{n}"));
        ActedElement {
            reference,
            role: described.role,
            name: described.name,
            tag: described.tag,
        }
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

/// An element as `__ccdp.describe` describes it.
#[derive(Debug, Default, Deserialize)]
struct Described {
    #[serde(default)]
    role: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    tag: String,
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
            // `Mod` is resolved by the browser's platform before; off
            // macOS it is Control.
            Modifier::Control | Modifier::Mod => 2,
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
            no_wait_after: false,
            snap: false,
            max_chars: 0,
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
