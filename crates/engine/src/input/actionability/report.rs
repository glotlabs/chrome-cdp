//! The pipeline's errors: the check that failed last, and what it saw.

use std::fmt::Write as _;
use std::time::Duration;

use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, format};
use serde_json::{Value, json};

use super::{Failure, Listed, Obscured};
use crate::context::Scoped as _;
use crate::locator::Locator;
use crate::request::SnapshotEngine;
use crate::tabs::Page;
use crate::{Engine, Running};

impl Engine {
    /// The error when the pipeline did not pass within `timeout`: the
    /// check that failed last, and what it saw.
    pub(super) fn not_actionable(
        &self,
        running: &Running,
        page: &Page,
        locator: &Locator,
        failure: Failure,
        timeout: Duration,
        retry: Option<&(dyn Fn(&str) -> Cmd + Sync)>,
    ) -> ErrorInfo {
        let waited_text = format::duration(timeout);
        let force = retry.map(|r| r(&locator.to_string()).switch("force"));
        let snapshot = || Hint::run(Cmd::new("snapshot").scoped(&self.session));
        let mut details = json!({
            "locator": locator.to_string(),
            "timeout_ms": u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        });
        let info = match failure {
            Failure::Missing(err) => {
                return err.note("waited", waited_text);
            }
            Failure::Check { check, why } => {
                details["check"] = json!(check);
                details["why"] = json!(why);
                let (kind, message, hint) = match check.as_str() {
                    "enabled" => (
                        ErrorKind::NotEnabled,
                        format!("{locator} is disabled: {why}"),
                        snapshot().why("a control is often disabled until the form is complete"),
                    ),
                    "editable" => (
                        ErrorKind::NotEnabled,
                        format!("{locator} is read-only: {why}"),
                        snapshot().why("shows the page's fields"),
                    ),
                    "stable" => (
                        ErrorKind::Timeout,
                        format!("{locator} did not stop moving: {why}"),
                        match &force {
                            Some(force) => Hint::run(force.clone())
                                .why("an endless animation never stops; act on it anyway"),
                            None => Hint::text(
                                "an endless animation never stops; act on it with --force",
                            ),
                        },
                    ),
                    "attached" => (
                        ErrorKind::Timeout,
                        format!("{locator} kept being replaced: {why}"),
                        snapshot().why("shows the page as it is now"),
                    ),
                    _ => (
                        ErrorKind::Timeout,
                        format!("{locator} is not visible: {why}"),
                        snapshot().why("shows what is on the page; it may need a click to open"),
                    ),
                };
                let hint = match (&force, check.as_str()) {
                    (Some(force), "visible") => hint.or(force.to_string()),
                    _ => hint,
                };
                ErrorInfo::new(kind, message).hint(hint)
            }
            Failure::OutOfView { why } => {
                details["check"] = json!("viewport");
                details["why"] = json!(why);
                ErrorInfo::new(
                    ErrorKind::Timeout,
                    format!("{locator} is outside the viewport after scrolling: {why}"),
                )
                .hint(snapshot().why("it may be inside a closed or clipped container"))
            }
            Failure::Obscured(obscured) => {
                return self
                    .obscured_error(running, page, locator, &obscured, details, force)
                    .note("waited", waited_text);
            }
        };
        info.note("waited", waited_text).details(details)
    }

    /// `Obscured`: what covers the element, with a ref, and the button
    /// that likely dismisses it.
    fn obscured_error(
        &self,
        running: &Running,
        page: &Page,
        locator: &Locator,
        obscured: &Obscured,
        mut details: Value,
        force: Option<Cmd>,
    ) -> ErrorInfo {
        let reference = |listed: &Listed| {
            let key = listed.key.as_ref()?;
            running
                .tabs
                .snapshot_memory(page.id, |m| {
                    (Some(m.refs.generation()) == obscured.generation).then(|| {
                        let number = m.refs.assign_from(
                            key,
                            &listed.role,
                            &listed.name,
                            SnapshotEngine::Dom,
                        );
                        format!("e{number}")
                    })
                })
                .flatten()
        };
        let describe = |listed: &Listed, reference: Option<&str>| {
            let mut text = if listed.role == "generic" || listed.role.is_empty() {
                listed.tag.clone()
            } else {
                listed.role.clone()
            };
            if !listed.name.is_empty() {
                let _ = write!(text, " \"{}\"", listed.name.replace('"', "\\\""));
            }
            if let Some(reference) = reference {
                let _ = write!(text, " [{reference}]");
            }
            text
        };
        let json_of = |listed: &Listed, reference: Option<&str>| json!({ "ref": reference, "role": listed.role, "name": listed.name, "tag": listed.tag });
        let (x, y) = (obscured.x.round(), obscured.y.round());
        details["check"] = json!("hit");
        details["point"] = json!({ "x": x, "y": y });
        if obscured.inert {
            let why = match obscured.pointer_events.as_deref() {
                Some("none") => "it has pointer-events: none".to_owned(),
                _ => "a click there lands on an element around it".to_owned(),
            };
            details["why"] = json!(why);
            let mut info = ErrorInfo::new(
                ErrorKind::Obscured,
                format!("{locator} does not receive pointer events: {why}"),
            );
            if let Some(covering) = obscured
                .covering
                .as_ref()
                .filter(|c| c.tag != "body" && c.tag != "html")
            {
                let reference = reference(covering);
                info = info.note("click lands on", describe(covering, reference.as_deref()));
                details["covering"] = json_of(covering, reference.as_deref());
            }
            let hint = Hint::text("act on the element around it instead");
            let hint = match force {
                Some(force) => hint.or(force.to_string()),
                None => hint,
            };
            return info.hint(hint).details(details);
        }
        let mut info = ErrorInfo::new(
            ErrorKind::Obscured,
            format!("{locator} is covered by another element"),
        );
        let covering_ref = obscured.covering.as_ref().and_then(reference);
        if let Some(covering) = &obscured.covering {
            info = info.note("covering", describe(covering, covering_ref.as_deref()));
            details["covering"] = json_of(covering, covering_ref.as_deref());
        }
        let dismiss = obscured
            .dismiss
            .as_ref()
            .and_then(|d| Some((d, reference(d)?)));
        let hint = if let Some((dismiss, dismiss_ref)) = dismiss {
            details["dismiss"] = json_of(dismiss, Some(&dismiss_ref));
            Hint::run(Cmd::new("click").arg(&dismiss_ref).scoped(&self.session))
                .why(format!("{} inside it", describe(dismiss, None)))
        } else if let Some(covering_ref) = &covering_ref {
            Hint::run(
                Cmd::new("snapshot")
                    .flag("scope", covering_ref)
                    .scoped(&self.session),
            )
            .why("shows what covers it; close it first")
        } else {
            Hint::run(Cmd::new("snapshot").scoped(&self.session))
                .why("shows what covers it; close it first")
        };
        let hint = match force {
            Some(force) => hint.or(force.to_string()),
            None => hint,
        };
        info.hint(hint).details(details)
    }
}
