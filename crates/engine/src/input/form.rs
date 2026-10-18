//! Forms (plan 05, milestone 5.3): `fill`, `select`, `check`, `uncheck`,
//! `focus` and `blur`.
//!
//! `fill` is the fast path agents should use: it focuses the field,
//! selects what it holds, and inserts the text in one trusted `input`
//! (`Input.insertText`), so frameworks see a user's edit; the browser
//! fires `change` when the field loses focus, as after typing. Types that
//! take no typing (date, time, colour, range) get their value set, with
//! `input` and `change`. `select` sets a native `<select>`'s options the
//! same way. `check`/`uncheck` click (the label of a visually hidden
//! input), only when the state differs, and check that it changed.

use std::fmt::Write as _;
use std::time::Duration;

use cdp::domains::runtime;
use protocol::schema::{ActedElement, CheckData, FillData, FocusData, SelectData, SelectOption};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::Instant;

use super::keys;
use super::{Actor, Checks, Described, Ready, describe, returned};
use crate::context::Scoped as _;
use crate::errors::from_cdp;
use crate::injected::{self, GUARD};
use crate::locator::{Locator, read_listed, release};
use crate::request::{Action, Button};
use crate::snapshot::{NodeKey, Target};

use super::actionability::waits_for;

/// How long `check` waits for the state to change after its click.
const SETTLE: Duration = Duration::from_secs(1);

/// How often `select` and `check` look again.
const POLL: Duration = Duration::from_millis(50);

/// The least time left for `select` to look for its options again (the
/// pipeline runs again first).
const LOOK_AGAIN: Duration = Duration::from_millis(250);

/// What `fillPrepare` answers.
#[derive(Debug, Default, Deserialize)]
struct Prepared {
    #[serde(default)]
    set: bool,
    error: Option<String>,
    example: Option<String>,
    /// It is a `<select>`.
    #[serde(default)]
    select: bool,
}

/// What `fieldValue` answers.
#[derive(Debug, Default, Deserialize)]
struct Field {
    #[serde(default)]
    value: String,
    #[serde(default)]
    secret: bool,
}

/// What `selectOptions` answers.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Selected {
    selected: Option<Vec<SelectOption>>,
    #[serde(default)]
    not_select: bool,
    #[serde(default)]
    role: String,
    missing: Option<String>,
    #[serde(default)]
    options: Vec<Listed>,
    #[serde(default)]
    more: usize,
    disabled: Option<String>,
    single: Option<usize>,
}

/// An option `select` lists in an error.
#[derive(Debug, Default, Deserialize)]
struct Listed {
    value: String,
    label: String,
    #[serde(default)]
    disabled: bool,
}

/// What `checkState` answers.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Default, Deserialize)]
struct State {
    kind: Option<String>,
    state: Value,
    #[serde(default)]
    radio: bool,
    #[serde(default)]
    candidate: Described,
    #[serde(default)]
    click: Option<Described>,
    #[serde(default)]
    disabled: bool,
}

impl State {
    fn checked(&self) -> bool {
        self.state == Value::Bool(true)
    }
}

impl Actor<'_> {
    /// `fill <loc> <text>`.
    pub(super) async fn fill(&self) -> Result<Output, ErrorInfo> {
        let Action::Fill {
            locator,
            text,
            force,
        } = &self.req.action
        else {
            unreachable!("not fill")
        };
        let parsed = self.parse(locator)?;
        let checks = Checks {
            enabled: true,
            editable: true,
            force: *force,
            no_hit_test: true,
            ..Checks::default()
        };
        let retry = |l: &str| self.retry(l);
        let ready = self.ready(&parsed, checks, &retry).await?;
        let filled = self.fill_ready(&ready, locator, text).await;
        let element = ready.element.clone();
        let note = ready.ready.target.note.clone();
        ready.release();
        let field = filled?;
        let mut output = Output::typed(
            &FillData {
                tab: self.tab(),
                locator: locator.clone(),
                element: element.clone(),
                value: (!field.secret).then(|| field.value.clone()),
            },
            format!("filled {}", describe(&element)),
        )
        .quiet(
            element
                .reference
                .clone()
                .unwrap_or_else(|| "filled".to_owned()),
        );
        if let Some(note) = note {
            output = output.warning(note);
        }
        if let Some(warning) = differs(text, &field) {
            output = output.warning(warning);
        }
        Ok(output)
    }

    /// Fills the field `ready` found with `text`, and reads it back.
    async fn fill_ready(
        &self,
        ready: &Ready,
        locator: &str,
        text: &str,
    ) -> Result<Field, ErrorInfo> {
        let target = &ready.ready.target;
        self.focus_on(target, &ready.element).await?;
        let value = self
            .call(
                target,
                "function (text) { return globalThis.__ccdp.fillPrepare(this, text); }",
                vec![json!(text)],
            )
            .await?;
        let prepared: Prepared = serde_json::from_value(value).unwrap_or_default();
        if let Some(why) = prepared.error {
            let hint = match prepared.example {
                _ if prepared.select => Hint::run(
                    Cmd::new("select")
                        .arg(locator)
                        .arg(text)
                        .scoped(self.session()),
                ),
                Some(example) => Hint::run(
                    Cmd::new("fill")
                        .arg(locator)
                        .arg(example)
                        .scoped(self.session()),
                ),
                None => Hint::run(Cmd::new("snapshot").raw("-i").scoped(self.session()))
                    .why("lists the page's text fields"),
            };
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("{} cannot hold that text: {why}", describe(&ready.element)),
            )
            .hint(hint)
            .details(json!({ "locator": locator, "why": why })));
        }
        if !prepared.set {
            if text.is_empty() {
                // Inserting nothing leaves the selection; Delete clears it.
                let mac = self.mac().await?;
                for event in keys::chord_events(&["Delete".to_owned()], mac, &[])
                    .map_err(|m| ErrorInfo::new(ErrorKind::BadArgs, m))?
                {
                    self.key_event(&event).await?;
                }
            } else {
                self.insert_text(text).await?;
            }
        }
        let value = self
            .call(
                target,
                "function () { return globalThis.__ccdp.fieldValue(this); }",
                Vec::new(),
            )
            .await?;
        Ok(serde_json::from_value(value).unwrap_or_default())
    }

    /// `select <loc> <value|label>...`: waits until the options are there.
    pub(super) async fn select(&self) -> Result<Output, ErrorInfo> {
        let Action::Select {
            locator,
            values,
            force,
        } = &self.req.action
        else {
            unreachable!("not select")
        };
        let parsed = self.parse(locator)?;
        let checks = Checks {
            enabled: true,
            force: *force,
            no_hit_test: true,
            ..Checks::default()
        };
        let retry = |l: &str| self.retry(l);
        // The last answer, for the error when the time runs out while
        // looking again.
        let mut last: Option<(Selected, ActedElement)> = None;
        loop {
            let ready = match self.ready(&parsed, checks, &retry).await {
                Ok(ready) => ready,
                Err(err) => match last {
                    Some((answer, element)) if err.kind == ErrorKind::Timeout => {
                        return Err(self.no_option(locator, &element, &answer));
                    }
                    _ => return Err(err),
                },
            };
            let answer = self
                .call(
                    &ready.ready.target,
                    "function (values) { return globalThis.__ccdp.selectOptions(this, values); }",
                    vec![json!(values)],
                )
                .await;
            let element = ready.element.clone();
            let note = ready.ready.target.note.clone();
            ready.release();
            let answer: Selected = answer?
                .as_str()
                .and_then(|json| serde_json::from_str(json).ok())
                .unwrap_or_default();
            if let Some(selected) = answer.selected {
                let labels: Vec<String> = selected
                    .iter()
                    .map(|o| format!("\"{}\"", o.label.replace('"', "\\\"")))
                    .collect();
                let text = format!("selected {} in {}", labels.join(", "), describe(&element));
                let quiet = selected
                    .iter()
                    .map(|o| o.value.clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                let data = SelectData {
                    tab: self.tab(),
                    locator: locator.clone(),
                    element,
                    selected,
                };
                let mut output = Output::typed(&data, text).quiet(quiet);
                if let Some(note) = note {
                    output = output.warning(note);
                }
                return Ok(output);
            }
            if answer.not_select {
                return Err(self.not_a_select(locator, &element, &answer.role, &values[0]));
            }
            if let Some(count) = answer.single {
                return Err(ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!("{} takes one option, not {count}", describe(&element)),
                )
                .hint(Hint::run(
                    Cmd::new("select")
                        .arg(locator)
                        .arg(&values[0])
                        .scoped(self.session()),
                )));
            }
            // A missing or disabled option may come: the page may still be
            // loading them.
            if Instant::now() + LOOK_AGAIN >= self.deadline {
                return Err(self.no_option(locator, &element, &answer));
            }
            last = Some((answer, element));
            tokio::time::sleep(POLL).await;
        }
    }

    /// `select` on something that is not a `<select>`.
    fn not_a_select(
        &self,
        locator: &str,
        element: &ActedElement,
        role: &str,
        value: &str,
    ) -> ErrorInfo {
        let described = describe(element);
        let target = element.reference.as_deref().unwrap_or(locator);
        let error = ErrorInfo::new(
            ErrorKind::NotEnabled,
            format!("{described} is not a <select>"),
        )
        .details(json!({ "locator": locator, "role": role, "check": "select" }));
        if role == "combobox" || role == "listbox" {
            // A custom list: open it, then click the option.
            let option = Cmd::new("click")
                .arg(format!(
                    "role=option[name=\"{}\"]",
                    value.replace('"', "\\\"")
                ))
                .scoped(self.session());
            error.hint(
                Hint::run(Cmd::new("click").arg(target).scoped(self.session()))
                    .why(format!("opens the list; then {option}")),
            )
        } else {
            error.hint(
                Hint::run(Cmd::new("snapshot").raw("-i").scoped(self.session()))
                    .why("lists the page's controls"),
            )
        }
    }

    /// No option matches a value (or it stayed disabled) until the timeout.
    fn no_option(&self, locator: &str, element: &ActedElement, answer: &Selected) -> ErrorInfo {
        let described = describe(element);
        if let Some(value) = &answer.disabled {
            return ErrorInfo::new(
                ErrorKind::NotEnabled,
                format!("the option \"{value}\" of {described} is disabled"),
            )
            .note("waited", protocol::format::duration(self.timeout))
            .hint(
                Hint::run(Cmd::new("snapshot").scoped(self.session()))
                    .why("an option is often disabled until the form allows it"),
            )
            .details(json!({ "locator": locator, "option": value, "check": "enabled" }));
        }
        let missing = answer.missing.clone().unwrap_or_default();
        let mut listed = answer
            .options
            .iter()
            .map(|o| {
                let mut text = format!("\"{}\"", o.label);
                if !o.value.is_empty() && o.value != o.label {
                    let _ = write!(text, " ({})", o.value);
                }
                if o.disabled {
                    text.push_str(" disabled");
                }
                text
            })
            .collect::<Vec<_>>()
            .join(", ");
        if answer.more > 0 {
            let _ = write!(listed, ", … {} more", answer.more);
        }
        if listed.is_empty() {
            listed.push_str("none");
        }
        let mut error = ErrorInfo::new(
            ErrorKind::NotFound,
            format!("no option of {described} has the value or label \"{missing}\""),
        )
        .note("options", listed)
        .note("waited", protocol::format::duration(self.timeout))
        .details(json!({
            "locator": locator,
            "missing": missing,
            "options": answer.options.iter().map(|o| json!({"value": o.value, "label": o.label})).collect::<Vec<_>>(),
        }));
        let usable = |o: &&Listed| !o.disabled && !o.value.is_empty();
        if let Some(first) = answer.options.iter().find(usable) {
            error = error.hint(Hint::run(
                Cmd::new("select")
                    .arg(locator)
                    .arg(&first.label)
                    .scoped(self.session()),
            ));
        }
        error
    }

    /// `check <loc>` and `uncheck <loc>`.
    pub(super) async fn check(&self) -> Result<Output, ErrorInfo> {
        let Action::Check {
            locator,
            checked: want,
            force,
        } = &self.req.action
        else {
            unreachable!("not check")
        };
        let parsed = self.parse(locator)?;
        let target = self.locate_waiting(&parsed).await?;
        let generation = self.generation();
        let read = self.state(&target).await;
        target.release();
        let (state, keys) = read?;
        let control_key = keys.first().cloned().flatten();
        let element = self.acted(
            control_key.as_ref(),
            Described {
                role: state.candidate.role.clone(),
                name: state.candidate.name.clone(),
                tag: state.candidate.tag.clone(),
            },
            generation,
        );
        let verb = self.req.action.command();
        if state.kind.is_none() {
            return Err(ErrorInfo::new(
                ErrorKind::NotEnabled,
                format!(
                    "{} is not a checkbox, radio button or switch",
                    describe(&element)
                ),
            )
            .hint(
                Hint::run(
                    Cmd::new("click")
                        .arg(element.reference.as_deref().unwrap_or(locator))
                        .scoped(self.session()),
                )
                .why("to click it anyway"),
            )
            .details(json!({ "locator": locator, "check": "checkable" })));
        }
        if state.checked() == *want {
            let text = format!(
                "{} was already {}",
                describe(&element),
                if *want { "checked" } else { "unchecked" }
            );
            let data = CheckData {
                tab: self.tab(),
                locator: locator.clone(),
                element: element.clone(),
                checked: *want,
                changed: false,
                label: None,
            };
            return Ok(Output::typed(&data, text)
                .quiet(element.reference.unwrap_or_else(|| verb.to_owned())));
        }
        if !*want && state.radio {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!(
                    "{} cannot be unchecked: check another radio button of its group",
                    describe(&element)
                ),
            )
            .hint(
                Hint::run(Cmd::new("snapshot").raw("-i").scoped(self.session()))
                    .why("lists its group"),
            ));
        }
        // Click the label instead of a control styled away behind it.
        let click_key = keys.get(1).cloned().flatten();
        let label = match (&click_key, &state.click) {
            (Some(key), Some(click)) if Some(key) != control_key.as_ref() => Some(self.acted(
                Some(key),
                Described {
                    role: click.role.clone(),
                    name: click.name.clone(),
                    tag: click.tag.clone(),
                },
                generation,
            )),
            _ => None,
        };
        let click_locator = match label.as_ref().and_then(|l| l.reference.as_deref()) {
            Some(reference) if reference_differs(locator, reference) => self.parse(reference)?,
            _ => parsed.clone(),
        };
        let checks = Checks {
            enabled: true,
            force: *force,
            ..Checks::default()
        };
        let retry = |l: &str| self.retry(l);
        let ready = self.ready(&click_locator, checks, &retry).await?;
        let (x, y) = (ready.ready.x, ready.ready.y);
        let note = ready.ready.target.note.clone();
        ready.release();
        self.move_to(x, y, 1, 0).await?;
        self.press(Button::Left, true, 1, 0).await?;
        self.press(Button::Left, false, 1, 0).await?;
        // The page may update the state a little later (a framework's
        // render).
        let settle = Instant::now() + SETTLE;
        // The state the last look saw: a look with almost no time left can
        // time out instead, which says less.
        let mut seen = None;
        let last = loop {
            let read = match self.locate_waiting(&parsed).await {
                Ok(target) => {
                    let read = self.state(&target).await;
                    target.release();
                    read
                }
                Err(err) => Err(err),
            };
            let now = match (read, seen.take()) {
                (Ok((now, _)), _) => now,
                (Err(err), Some(seen)) if err.kind == ErrorKind::Timeout => break seen,
                (Err(err), _) => return Err(err),
            };
            if now.checked() == *want || Instant::now() + POLL >= settle.min(self.deadline) {
                break now;
            }
            seen = Some(now);
            tokio::time::sleep(POLL).await;
        };
        if last.checked() != *want {
            let mut why = format!("clicking {} did not {verb} it", describe(&element));
            if last.disabled {
                why.push_str(": it is disabled");
            }
            return Err(ErrorInfo::new(ErrorKind::NotEnabled, why)
                .hint(
                    Hint::run(Cmd::new("snapshot").scoped(self.session()))
                        .why("the page may ignore or undo the click"),
                )
                .details(json!({ "locator": locator, "check": "checked" })));
        }
        let mut text = format!(
            "{} {}",
            if *want { "checked" } else { "unchecked" },
            describe(&element)
        );
        if let Some(label) = &label {
            let _ = write!(text, " by its label {}", describe(label));
        }
        let data = CheckData {
            tab: self.tab(),
            locator: locator.clone(),
            element: element.clone(),
            checked: *want,
            changed: true,
            label,
        };
        let mut output =
            Output::typed(&data, text).quiet(element.reference.unwrap_or_else(|| verb.to_owned()));
        if let Some(note) = note {
            output = output.warning(note);
        }
        Ok(output)
    }

    /// The one element `locator` matches, waiting (until the deadline)
    /// while it matches none.
    pub(super) async fn locate_waiting(&self, locator: &Locator) -> Result<Target, ErrorInfo> {
        let retry = |l: &str| self.retry(l);
        loop {
            match self
                .engine
                .locate_one(
                    self.running,
                    self.page,
                    locator,
                    &self.options,
                    Some(&retry),
                    self.left(),
                )
                .await
            {
                Err(err) if waits_for(&err) && Instant::now() + POLL < self.deadline => {
                    tokio::time::sleep(POLL).await;
                }
                Err(err) if waits_for(&err) => {
                    return Err(err.note("waited", protocol::format::duration(self.timeout)));
                }
                result => return result,
            }
        }
    }

    /// `checkState` of `target`, and the keys of its control and of what
    /// to click.
    async fn state(&self, target: &Target) -> Result<(State, Vec<Option<NodeKey>>), ErrorInfo> {
        let session = target.context.session.with_timeout(self.timeout);
        let mut params = runtime::CallFunctionOnParams::new(format!(
            "function () {{ {GUARD} return globalThis.__ccdp.checkState(this); }}"
        ));
        params.object_id = Some(target.object_id.clone());
        let result = injected::call(&session, target.context.context_id, params)
            .await
            .map_err(|e| from_cdp(&e, self.session()))?;
        if result.exception_details.is_some() {
            returned(result)?;
            unreachable!("an exception is an error");
        }
        let Some(object_id) = result.result.object_id else {
            return Err(ErrorInfo::new(
                ErrorKind::ProtocolError,
                "reading the checkbox's state gave nothing",
            ));
        };
        let listed = read_listed(&session, &object_id, &target.key.frame, 2).await;
        release(&target.context, object_id);
        let (json, keys) = listed.map_err(|e| from_cdp(&e, self.session()))?;
        let state = serde_json::from_str(&json).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::ProtocolError,
                format!("reading the checkbox's state: {e}"),
            )
        })?;
        Ok((state, keys))
    }

    /// `focus <loc>` and `blur <loc>`.
    pub(super) async fn focus(&self) -> Result<Output, ErrorInfo> {
        let (Action::Focus { locator } | Action::Blur { locator }) = &self.req.action else {
            unreachable!("not focus or blur")
        };
        let blur = matches!(self.req.action, Action::Blur { .. });
        let parsed = self.parse(locator)?;
        let checks = Checks {
            no_hit_test: true,
            ..Checks::default()
        };
        let retry = |l: &str| self.retry(l);
        let ready = self.ready(&parsed, checks, &retry).await?;
        let done = if blur {
            self.call(
                &ready.ready.target,
                "function () { const ccdp = globalThis.__ccdp;\n\
                 if (!ccdp.hasFocus(this)) return false;\n\
                 (ccdp.focused(this) || this).blur(); return true; }",
                Vec::new(),
            )
            .await
            .map(|v| v.as_bool() == Some(true))
        } else {
            self.focus_on(&ready.ready.target, &ready.element)
                .await
                .map(|_| true)
        };
        let element = ready.element.clone();
        let note = ready.ready.target.note.clone();
        ready.release();
        let done = done?;
        let described = describe(&element);
        let text = match (blur, done) {
            (false, _) => format!("focused {described}"),
            (true, true) => format!("blurred {described}"),
            (true, false) => format!("{described} did not have focus"),
        };
        let action = self.req.action.command();
        let data = FocusData {
            tab: self.tab(),
            action: action.to_owned(),
            locator: locator.clone(),
            element: element.clone(),
        };
        let mut output = Output::typed(&data, text)
            .quiet(element.reference.unwrap_or_else(|| action.to_owned()));
        if let Some(note) = note {
            output = output.warning(note);
        }
        Ok(output)
    }
}

/// Whether the ref `reference` is not what `locator` already says.
fn reference_differs(locator: &str, reference: &str) -> bool {
    locator.trim_start_matches('@') != reference
}

/// A warning when the field does not hold what was filled (a mask, a
/// length limit, or the page changed it).
fn differs(text: &str, field: &Field) -> Option<String> {
    let flat: String = text.chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
    if field.value == text || field.value == flat || field.value == text.trim() {
        return None;
    }
    if field.secret {
        return Some(format!(
            "the field holds {} characters, not the {} filled (a length limit, or the page changed it)",
            field.value.chars().count(),
            text.chars().count()
        ));
    }
    let mut shown: String = field.value.chars().take(60).collect();
    if field.value.chars().count() > 60 {
        shown.push('…');
    }
    Some(format!(
        "the field holds \"{shown}\", not the text filled (a mask, a length limit, or the page changed it)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(value: &str, secret: bool) -> Field {
        Field {
            value: value.to_owned(),
            secret,
        }
    }

    #[test]
    fn a_field_that_holds_other_text_warns() {
        assert_eq!(differs("abc", &field("abc", false)), None);
        assert_eq!(differs("a\nb", &field("ab", false)), None);
        assert_eq!(differs(" 5 ", &field("5", false)), None);
        assert_eq!(
            differs("123456", &field("123-456", false)).unwrap(),
            "the field holds \"123-456\", not the text filled (a mask, a length limit, or the page changed it)"
        );
        assert_eq!(
            differs("hunter22", &field("hunt", true)).unwrap(),
            "the field holds 4 characters, not the 8 filled (a length limit, or the page changed it)"
        );
    }

    #[test]
    fn a_ref_that_is_the_locator_is_not_new() {
        assert!(!reference_differs("e5", "e5"));
        assert!(!reference_differs("@e5", "e5"));
        assert!(reference_differs("#remember", "e5"));
    }
}
