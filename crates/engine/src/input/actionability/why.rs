//! `why <loc>` (plan 16, milestone 16.1): the pipeline's explain mode.
//!
//! It runs the checks an action needs once, now, without acting, waiting
//! or retrying, and reports every one of them with its cause, where the
//! pipeline stops at the first that fails. The checks are the pipeline's
//! own: the page-side rules of `injected/src/actionable.ts` (which
//! `injected/src/why.ts` runs all at once, naming the element behind each
//! failure) and [`Engine::land`], which scrolls, measures and hit-tests
//! as an action does.

use std::fmt::Write as _;

use cdp::domains::runtime;
use protocol::schema::{WhyCheck, WhyData, WhyElement, WhyStatus, WhyVerdict};
use protocol::{Cmd, ErrorInfo, ErrorKind, Output};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Attempt, Candidate, Checks, Failure, Listed, Obscured, argument, returned};
use crate::context::Scoped as _;
use crate::injected::{self, GUARD};
use crate::input::upload::PROBE;
use crate::locator::{LocateOptions, Locator, read_listed};
use crate::request::{SnapshotEngine, WhyFor, WhyRequest, millis};
use crate::snapshot::{NodeKey, Target};
use crate::tabs::{Page, tab_name};
use crate::{Engine, Running};

/// The object group of `why`'s handles.
const GROUP: &str = "chrome-cdp-why";

/// At most this many elements an answer of the page names get refs.
const NAMED: usize = 16;

/// The checks `action` runs, as its command asks the pipeline for them
/// (`input/mouse.rs`, `form.rs`, `keyboard.rs`, `drag.rs`, `upload.rs`).
pub(crate) fn checks_for(action: WhyFor) -> Checks {
    let (enabled, editable, no_hit_test) = match action {
        WhyFor::Click | WhyFor::Check | WhyFor::Drag | WhyFor::Upload => (true, false, false),
        WhyFor::Hover => (false, false, false),
        WhyFor::Fill | WhyFor::Type => (true, true, true),
        WhyFor::Select => (true, false, true),
    };
    Checks {
        enabled,
        editable,
        no_hit_test,
        ..Checks::default()
    }
}

/// `explainActionable()`'s answer.
#[derive(Debug, Default, Deserialize)]
struct Explained {
    #[serde(default)]
    checks: Vec<PageCheck>,
    pointer: Option<Caused>,
    /// The element itself.
    own: Option<Candidate>,
    #[serde(default)]
    names: Vec<Candidate>,
}

/// One check the page ran; `text` names `names[el]` where it says `{el}`.
#[derive(Debug, Deserialize)]
struct PageCheck {
    name: String,
    ok: bool,
    text: Option<String>,
    el: Option<usize>,
    #[serde(default)]
    r#final: bool,
}

/// A cause the page found (`clipped()`, and the pointer events of
/// `explainActionable()`).
#[derive(Debug, Default, Deserialize)]
struct Caused {
    text: Option<String>,
    el: Option<usize>,
    #[serde(default)]
    names: Vec<Candidate>,
}

/// `scrolledSince()`'s answer.
#[derive(Debug, Default, Deserialize)]
struct Moved {
    #[serde(default)]
    moved: Vec<Scrolled>,
    #[serde(default)]
    names: Vec<Candidate>,
}

#[derive(Debug, Deserialize)]
struct Scrolled {
    dx: i64,
    dy: i64,
    el: Option<usize>,
}

/// `checkState()`'s answer, as `check` reads it.
#[derive(Debug, Default, Deserialize)]
struct CheckState {
    kind: Option<String>,
    #[serde(default)]
    state: Value,
    click: Option<Candidate>,
}

/// The file input probe of `upload`.
#[derive(Debug, Default, Deserialize)]
struct Probe {
    #[serde(default)]
    file: bool,
    #[serde(default)]
    disabled: bool,
}

/// What `why` found: the element and every check.
struct Report {
    element: Option<WhyElement>,
    checks: Vec<WhyCheck>,
    /// Why the action would fail at once, without waiting.
    fails: Option<String>,
    note: Option<String>,
}

/// A check's verdict, before it goes in the report.
fn check(name: &str, status: WhyStatus, why: Option<String>) -> WhyCheck {
    WhyCheck {
        name: name.to_owned(),
        status,
        why,
        element: None,
        dismiss: None,
        candidates: Vec::new(),
        notes: Vec::new(),
    }
}

/// How the output names an element: `button "Pay" [e12]`, or for one
/// without a role, its tag and first class (`div.cookie-banner [e88]`).
pub(crate) fn element_text(element: &WhyElement) -> String {
    let mut text = if element.role == "generic" || element.role.is_empty() {
        let mut text = element.tag.clone();
        if !element.class.is_empty() {
            let _ = write!(text, ".{}", element.class);
        }
        text
    } else {
        let mut text = element.role.clone();
        if !element.name.is_empty() {
            let _ = write!(text, " \"{}\"", element.name.replace('"', "\\\""));
        }
        text
    };
    if let Some(reference) = &element.reference {
        let _ = write!(text, " [{reference}]");
    }
    text
}

/// `text` with `{el}` replaced by how the output names `element`.
fn fill_in(text: &str, element: Option<&WhyElement>) -> String {
    match element {
        Some(element) => text.replace("{el}", &element_text(element)),
        None => text.replace("{el}", "an element"),
    }
}

impl Engine {
    /// `why <loc>`: runs the checks the action `req.action` needs on the
    /// element `req.locator` matches, once, and reports each with its
    /// cause and what the action would do now. A locator that matches
    /// nothing, or several, is a verdict, not an error.
    pub async fn why(&self, req: WhyRequest) -> Result<Output, ErrorInfo> {
        let options = LocateOptions {
            strict_refs: req.strict_refs,
            testid_attribute: req.testid_attribute.clone(),
        };
        let locator = options.parse(&req.locator)?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let _tab = self.tabs.lock(&page.target_id).await;
        let work = self.explain(
            &running,
            &page,
            &locator,
            &options,
            req.action,
            !req.no_scroll,
            timeout,
        );
        let report = Box::pin(self.unless_blocked(&running, &page, mark, work)).await?;
        let data = why_data(
            &req,
            tab_name(page.id),
            report.element,
            report.checks,
            report.fails,
        );
        let text = why_text(&data);
        let quiet = verdict_text(&data);
        let mut output = Output::typed(&data, text).quiet(quiet);
        if let Some(note) = report.note {
            output = output.warning(note);
        }
        Ok(output)
    }

    /// Runs the checks; see [`Engine::why`].
    #[allow(clippy::too_many_arguments)]
    async fn explain(
        &self,
        running: &Running,
        page: &Page,
        locator: &Locator,
        options: &LocateOptions,
        action: WhyFor,
        scroll: bool,
        timeout: std::time::Duration,
    ) -> Result<Report, ErrorInfo> {
        // Elements seen from now on get refs only while this holds.
        let generation = running
            .tabs
            .snapshot_memory(page.id, |m| m.refs.generation());
        let retry = |r: &str| {
            Cmd::new("why")
                .arg(r)
                .flag("for", action.name())
                .scoped(&self.session)
        };
        let target = match self
            .locate_one(running, page, locator, options, Some(&retry), timeout)
            .await
        {
            Ok(target) => target,
            Err(err) => return unresolved(err),
        };
        let result = self
            .explain_target(running, page, &target, action, scroll, generation, timeout)
            .await;
        let note = target.note.clone();
        target.release();
        let mut report = result.map_err(|attempt| match attempt {
            Attempt::Final(err) => err,
            Attempt::Retry(failure) => ErrorInfo::new(
                ErrorKind::NotFound,
                match failure {
                    Failure::Missing(err) => err.message,
                    _ => "the element went away".to_owned(),
                },
            ),
        })?;
        report.note = note;
        Ok(report)
    }

    /// The checks on a resolved element.
    #[allow(clippy::too_many_arguments)]
    async fn explain_target(
        &self,
        running: &Running,
        page: &Page,
        target: &Target,
        action: WhyFor,
        scroll: bool,
        generation: Option<u64>,
        timeout: std::time::Duration,
    ) -> Result<Report, Attempt> {
        let acted = self.name_target(running, page, target, generation).await;
        let element = WhyElement {
            reference: acted.reference,
            role: acted.role,
            name: acted.name,
            tag: acted.tag,
            class: String::new(),
        };
        let mut checks = vec![check("resolve", WhyStatus::Ok, Some("1 match".to_owned()))];
        // `upload` sets a file input's files without the pipeline: it only
        // waits for the input to be enabled.
        if action == WhyFor::Upload {
            let value = self.call_page(target, PROBE, Vec::new(), timeout).await?;
            let probe: Probe = serde_json::from_value(value).unwrap_or_default();
            if probe.file {
                checks.push(if probe.disabled {
                    check(
                        "enabled",
                        WhyStatus::Failed,
                        Some("the disabled attribute".to_owned()),
                    )
                } else {
                    check("enabled", WhyStatus::Ok, None)
                });
                return Ok(Report {
                    element: Some(element),
                    checks,
                    fails: None,
                    note: None,
                });
            }
        }
        // `check` does nothing to what is already checked, fails at once
        // on what cannot be checked, and clicks the label of a box styled
        // away behind it: the checks are then the label's.
        let mut label = None;
        if action == WhyFor::Check {
            let function =
                format!("function () {{ {GUARD} return globalThis.__ccdp.checkState(this); }}");
            let (json, keys) = self
                .call_listed(target, &function, Vec::new(), timeout)
                .await?;
            let state: CheckState = serde_json::from_str(&json).unwrap_or_default();
            if state.kind.is_none() {
                return Ok(Report {
                    element: Some(element),
                    checks,
                    fails: Some("it is not a checkbox, radio button or switch".to_owned()),
                    note: None,
                });
            }
            if state.state == Value::Bool(true) {
                checks[0].why = Some("1 match, already checked: check does nothing".to_owned());
                return Ok(Report {
                    element: Some(element),
                    checks,
                    fails: None,
                    note: None,
                });
            }
            let key = keys.get(1).cloned().flatten();
            if let (Some(click), Some(key)) = (&state.click, key)
                && key != target.key
                && let Some((context, object_id)) =
                    self.live_node(running, page, &key, timeout).await?
            {
                let named = refer_all(
                    running,
                    page,
                    std::slice::from_ref(click),
                    &[Some(key.clone())],
                    generation,
                );
                if let Some(named) = named.first() {
                    checks[0].why = Some(format!("1 match; check clicks {}", element_text(named)));
                }
                label = Some(Target {
                    key,
                    context,
                    object_id,
                    note: None,
                });
            }
        }
        let own = label.is_none();
        let result = self
            .run_checks(
                running,
                page,
                label.as_ref().unwrap_or(target),
                own,
                action,
                scroll,
                generation,
                timeout,
                element,
                checks,
            )
            .await;
        if let Some(label) = label {
            label.release();
        }
        result
    }

    /// The pipeline's checks on `target` (the element, or the label
    /// `check` clicks: `own` is false), after `resolve`'s in `checks`.
    #[allow(clippy::too_many_arguments)]
    async fn run_checks(
        &self,
        running: &Running,
        page: &Page,
        target: &Target,
        own: bool,
        action: WhyFor,
        scroll: bool,
        generation: Option<u64>,
        timeout: std::time::Duration,
        mut element: WhyElement,
        mut checks: Vec<WhyCheck>,
    ) -> Result<Report, Attempt> {
        let needs = checks_for(action);
        let function = format!(
            "function (needs) {{ {GUARD} return globalThis.__ccdp.explainActionable(this, needs); }}"
        );
        let args = vec![argument(json!({
            "enabled": needs.enabled,
            "editable": needs.editable,
        }))];
        let (json, keys) = self.call_listed(target, &function, args, timeout).await?;
        let explained: Explained = serde_json::from_str(&json).unwrap_or_default();
        let names = refer_all(running, page, &explained.names, &keys, generation);
        if let Some(named) = explained.own.as_ref().filter(|_| own) {
            element.class.clone_from(&named.cls);
        }
        let mut fails = None;
        let mut failed_before_editable = false;
        for found in explained.checks {
            let cause = found.el.and_then(|i| names.get(i));
            let why = found.text.as_deref().map(|t| fill_in(t, cause));
            let status = if found.ok {
                WhyStatus::Ok
            } else {
                WhyStatus::Failed
            };
            // The pipeline fails at once on what can never take text, but
            // only once the checks before it pass.
            if found.name == "editable" && found.r#final && !failed_before_editable {
                fails.clone_from(&why);
            }
            if !found.ok && found.name != "editable" {
                failed_before_editable = true;
            }
            let mut checked = check(&found.name, status, why);
            checked.element = cause.cloned();
            checks.push(checked);
        }
        let visible = checks
            .iter()
            .any(|c| c.name == "visible" && c.status == WhyStatus::Ok);
        let hit_test = !needs.no_hit_test;
        let this = |function: &str| {
            format!("function () {{ {GUARD} return globalThis.__ccdp.{function}(this); }}")
        };
        if scroll {
            self.call_page(target, &this("scrollMark"), Vec::new(), timeout)
                .await?;
        }
        let landed = self
            .land(running, page, target, needs, generation, scroll, timeout)
            .await;
        let scrolled = if scroll {
            let function =
                format!("function () {{ {GUARD} return globalThis.__ccdp.scrolledSince(); }}");
            let (json, keys) = self
                .call_listed(target, &function, Vec::new(), timeout)
                .await?;
            let moved: Moved = serde_json::from_str(&json).unwrap_or_default();
            let names = refer_all(running, page, &moved.names, &keys, generation);
            scrolled_text(&moved.moved, &names)
        } else {
            None
        };
        let skipped = |why: &str| check("hit test", WhyStatus::Skipped, Some(why.to_owned()));
        match landed {
            Ok((x, y)) => {
                checks.push(check("in view", WhyStatus::Ok, scrolled));
                if hit_test {
                    let at = format!("reaches it at {},{}", x.round(), y.round());
                    checks.push(check("hit test", WhyStatus::Ok, Some(at)));
                }
            }
            Err(Attempt::Final(err)) => return Err(Attempt::Final(err)),
            Err(Attempt::Retry(Failure::Obscured(obscured))) => {
                checks.push(check("in view", WhyStatus::Ok, scrolled));
                let clip = self
                    .clip(running, page, target, generation, timeout)
                    .await?;
                let pointer = explained.pointer.as_ref().and_then(|p| {
                    let cause = p.el.and_then(|i| names.get(i));
                    Some((fill_in(p.text.as_deref()?, cause), cause.cloned()))
                });
                // Inert (a modal dialog open elsewhere, or `inert`): clicks
                // pass through it, or land on the dialog's backdrop.
                let inert = checks
                    .iter()
                    .find(|c| {
                        c.name == "enabled"
                            && c.why.as_deref().is_some_and(|w| w.starts_with("inert"))
                    })
                    .and_then(|c| Some((c.why.clone()?, c.element.clone()?)));
                checks.push(hit_check(
                    running, page, &obscured, clip, pointer, inert, generation,
                ));
            }
            Err(Attempt::Retry(failure)) => {
                let why = match failure {
                    Failure::OutOfView { why } | Failure::Check { why, .. } => why,
                    Failure::Missing(err) => err.message,
                    Failure::Obscured(_) => unreachable!("handled above"),
                };
                if !visible {
                    checks.push(check(
                        "in view",
                        WhyStatus::Skipped,
                        Some("not visible".to_owned()),
                    ));
                } else if !scroll {
                    let why = format!("{why}; not scrolled (--no-scroll)");
                    checks.push(check("in view", WhyStatus::Skipped, Some(why)));
                } else {
                    let clip = self
                        .clip(running, page, target, generation, timeout)
                        .await?;
                    let mut checked = check("in view", WhyStatus::Failed, None);
                    match clip {
                        Some((text, cause)) => {
                            checked.why = Some(text);
                            checked.element = cause;
                        }
                        None => checked.why = Some(why),
                    }
                    checks.push(checked);
                }
                if hit_test {
                    checks.push(skipped("not in view"));
                }
            }
        }
        Ok(Report {
            element: Some(element),
            checks,
            fails,
            note: None,
        })
    }

    /// Why the element lies where no scroll shows it (`clipped()`), with
    /// the element that clips it.
    async fn clip(
        &self,
        running: &Running,
        page: &Page,
        target: &Target,
        generation: Option<u64>,
        timeout: std::time::Duration,
    ) -> Result<Option<(String, Option<WhyElement>)>, Attempt> {
        let function = format!("function () {{ {GUARD} return globalThis.__ccdp.clipped(this); }}");
        let (json, keys) = self
            .call_listed(target, &function, Vec::new(), timeout)
            .await?;
        let caused: Caused = serde_json::from_str(&json).unwrap_or_default();
        let names = refer_all(running, page, &caused.names, &keys, generation);
        let cause = caused.el.and_then(|i| names.get(i)).cloned();
        Ok(caused
            .text
            .map(|text| (fill_in(&text, cause.as_ref()), cause)))
    }

    /// Calls `function` on the element and returns its value.
    async fn call_page(
        &self,
        target: &Target,
        function: &str,
        arguments: Vec<runtime::CallArgument>,
        timeout: std::time::Duration,
    ) -> Result<Value, Attempt> {
        let mut params = runtime::CallFunctionOnParams::new(function.to_owned());
        params.object_id = Some(target.object_id.clone());
        params.arguments = Some(arguments);
        params.return_by_value = Some(true);
        let session = target.context.session.with_timeout(timeout);
        let result = injected::call(&session, target.context.context_id, params)
            .await
            .map_err(|e| self.gone_or(&e))?;
        returned(result)
    }

    /// Calls `function`, which returns `{json, els}`, on the element: the
    /// JSON text and the keys of the elements it names.
    async fn call_listed(
        &self,
        target: &Target,
        function: &str,
        arguments: Vec<runtime::CallArgument>,
        timeout: std::time::Duration,
    ) -> Result<(String, Vec<Option<NodeKey>>), Attempt> {
        let mut params = runtime::CallFunctionOnParams::new(function.to_owned());
        params.object_id = Some(target.object_id.clone());
        params.arguments = Some(arguments);
        params.await_promise = Some(true);
        params.object_group = Some(GROUP.to_owned());
        let session = target.context.session.with_timeout(timeout);
        let result = async {
            let result = injected::call(&session, target.context.context_id, params)
                .await
                .map_err(|e| self.gone_or(&e))?;
            if let Some(exception) = &result.exception_details {
                return Err(Attempt::Final(ErrorInfo::new(
                    ErrorKind::EvalError,
                    format!("checking the element failed: {}", exception.text),
                )));
            }
            let Some(object_id) = result.result.object_id else {
                return Ok((String::new(), Vec::new()));
            };
            read_listed(&session, &object_id, &target.key.frame, NAMED)
                .await
                .map_err(|e| self.gone_or(&e))
        }
        .await;
        // Released before the next call, which uses the same group.
        let _ = session
            .call(runtime::ReleaseObjectGroupParams::new(GROUP.to_owned()))
            .await;
        result
    }
}

/// The hit test's verdict from what it found instead of the element.
fn hit_check(
    running: &Running,
    page: &Page,
    obscured: &Obscured,
    clip: Option<(String, Option<WhyElement>)>,
    pointer: Option<(String, Option<WhyElement>)>,
    inert: Option<(String, WhyElement)>,
    generation: Option<u64>,
) -> WhyCheck {
    let listed = |l: &Listed| {
        let reference = l.key.as_ref().and_then(|key| {
            refer(
                running,
                page,
                key,
                &l.role,
                &l.name,
                obscured.generation.or(generation),
            )
        });
        WhyElement {
            reference,
            role: l.role.clone(),
            name: l.name.clone(),
            tag: l.tag.clone(),
            class: l.class.clone(),
        }
    };
    let covering = obscured
        .covering
        .as_ref()
        .filter(|c| c.tag != "body" && c.tag != "html")
        .map(listed);
    let mut checked = check("hit test", WhyStatus::Failed, None);
    let mut why = match &clip {
        Some((text, _)) => format!("{text}; "),
        None => String::new(),
    };
    if obscured.inert {
        if let (Some("none"), Some((text, cause))) = (obscured.pointer_events.as_deref(), pointer) {
            why.push_str(&text);
            checked.element = cause;
        } else {
            why.push_str("clicks pass through it");
            if let Some((cause, element)) = inert {
                let _ = write!(why, ": {cause}");
                checked.element = Some(element);
            } else {
                if let Some(covering) = &covering {
                    let _ = write!(why, " to {}", element_text(covering));
                }
                checked.element = covering;
            }
        }
    } else {
        let modal = inert
            .filter(|(cause, _)| cause.contains("modal"))
            .map(|(_, element)| element);
        match &covering {
            Some(covering) => {
                let _ = write!(why, "covered by {}", element_text(covering));
            }
            None => match (&modal, &obscured.covering) {
                (Some(modal), _) => {
                    let _ = write!(why, "covered by the backdrop of {}", element_text(modal));
                }
                (None, Some(page)) => {
                    let _ = write!(why, "a click there lands on <{}>", page.tag);
                }
                (None, None) => why.push_str("nothing at its centre takes the click"),
            },
        }
        let dismiss = obscured.dismiss.as_ref().map(listed);
        if let Some(dismiss) = dismiss.as_ref().filter(|d| d.reference.is_some()) {
            let _ = write!(why, "; its {} likely dismisses it", element_text(dismiss));
        }
        checked.element = covering.or(modal);
        checked.dismiss = dismiss;
    }
    if checked.element.is_none()
        && let Some((_, cause)) = clip
    {
        checked.element = cause;
    }
    checked.why = Some(why);
    checked
}

/// The report when the locator did not resolve to one element: no
/// match (the action would wait for one), or an ambiguous locator or
/// a gone ref (it would fail at once). Other errors are errors.
fn unresolved(err: ErrorInfo) -> Result<Report, ErrorInfo> {
    if !matches!(
        err.kind,
        ErrorKind::NotFound | ErrorKind::Ambiguous | ErrorKind::RefStale | ErrorKind::RefDetached
    ) {
        return Err(err);
    }
    let waits = super::waits_for(&err);
    let mut resolve = check("resolve", WhyStatus::Failed, Some(err.message.clone()));
    let candidates = err
        .details
        .as_ref()
        .and_then(|d| d.get("candidates"))
        .and_then(Value::as_array);
    if let Some(candidates) = candidates {
        resolve.candidates = candidates
            .iter()
            .map(|c| WhyElement {
                reference: c["ref"].as_str().map(str::to_owned),
                role: c["role"].as_str().unwrap_or_default().to_owned(),
                name: c["name"].as_str().unwrap_or_default().to_owned(),
                tag: String::new(),
                class: String::new(),
            })
            .collect();
    } else {
        resolve.notes = err
            .notes
            .iter()
            .map(|n| format!("{}: {}", n.label, n.text))
            .collect();
    }
    Ok(Report {
        element: None,
        checks: vec![resolve],
        fails: (!waits).then(|| err.message.clone()),
        note: None,
    })
}

/// The elements a page answer names, with refs (given now, unless a
/// frame loaded a new document since `generation`).
pub(crate) fn refer_all(
    running: &Running,
    page: &Page,
    names: &[Candidate],
    keys: &[Option<NodeKey>],
    generation: Option<u64>,
) -> Vec<WhyElement> {
    names
        .iter()
        .enumerate()
        .map(|(i, c)| WhyElement {
            reference: keys
                .get(i)
                .and_then(Option::as_ref)
                .and_then(|key| refer(running, page, key, &c.role, &c.name, generation)),
            role: c.role.clone(),
            name: c.name.clone(),
            tag: c.tag.clone(),
            class: c.cls.clone(),
        })
        .collect()
}

/// The ref of the element `key`: its own, or a new one while the tab's
/// ref generation is still `generation`.
fn refer(
    running: &Running,
    page: &Page,
    key: &NodeKey,
    role: &str,
    name: &str,
    generation: Option<u64>,
) -> Option<String> {
    running
        .tabs
        .snapshot_memory(page.id, |m| {
            m.refs.get(key).or_else(|| {
                (Some(m.refs.generation()) == generation)
                    .then(|| m.refs.assign_from(key, role, name, SnapshotEngine::Dom))
            })
        })
        .flatten()
        .map(|n| format!("e{n}"))
}

/// `after scrolling main [e3] by 420px`: which containers scrolled to
/// show the element, innermost first.
fn scrolled_text(moved: &[Scrolled], names: &[WhyElement]) -> Option<String> {
    let parts: Vec<String> = moved
        .iter()
        .map(|m| {
            let what = match m.el.and_then(|i| names.get(i)) {
                Some(element) => element_text(element),
                None => "the page".to_owned(),
            };
            let by = match (m.dx, m.dy) {
                (0, dy) => format!("{dy}px"),
                (dx, 0) => format!("{dx}px sideways"),
                (dx, dy) => format!("{dx}px sideways and {dy}px"),
            };
            format!("{what} by {by}")
        })
        .collect();
    (!parts.is_empty()).then(|| format!("after scrolling {}", parts.join(" and ")))
}

/// The report's data, with the verdict.
fn why_data(
    req: &WhyRequest,
    tab: String,
    element: Option<WhyElement>,
    checks: Vec<WhyCheck>,
    fails: Option<String>,
) -> WhyData {
    let waits = if fails.is_some() {
        Vec::new()
    } else {
        checks
            .iter()
            .filter(|c| c.status == WhyStatus::Failed)
            .map(|c| c.name.clone())
            .collect()
    };
    let acts_now = fails.is_none() && waits.is_empty();
    WhyData {
        tab,
        locator: req.locator.clone(),
        action: req.action.name().to_owned(),
        element,
        checks,
        verdict: WhyVerdict {
            acts_now,
            waits,
            fails,
        },
    }
}

/// A failed check in the verdict line: `not stable`, or the hit test's
/// first clause (`covered by div.cookie-banner [e88]`).
fn short(check: &WhyCheck) -> String {
    match check.name.as_str() {
        "resolve" => "no element matches".to_owned(),
        "visible" => "not visible".to_owned(),
        "stable" => "not stable".to_owned(),
        "enabled" if check.why.as_deref().is_some_and(|w| w.starts_with("inert")) => {
            "inert".to_owned()
        }
        "enabled" => "disabled".to_owned(),
        "editable" => "read-only".to_owned(),
        "in view" => "out of view".to_owned(),
        _ => check
            .why
            .as_deref()
            .and_then(|w| w.split("; ").next())
            .unwrap_or("covered")
            .to_owned(),
    }
}

/// The verdict: `click would act now`, `… would wait until the timeout:
/// <checks>` or `… would fail at once: <reason>`.
fn verdict_text(data: &WhyData) -> String {
    let action = &data.action;
    if let Some(reason) = &data.verdict.fails {
        return format!("{action} would fail at once: {reason}");
    }
    if data.verdict.acts_now {
        let skipped: Vec<&str> = data
            .checks
            .iter()
            .filter(|c| c.status == WhyStatus::Skipped)
            .map(|c| c.name.as_str())
            .collect();
        return if skipped.is_empty() {
            format!("{action} would act now")
        } else {
            format!(
                "{action} would act now, unless {} fails",
                skipped.join(" or ")
            )
        };
    }
    let waits: Vec<String> = data
        .checks
        .iter()
        .filter(|c| c.status == WhyStatus::Failed)
        .map(short)
        .collect();
    format!(
        "{action} would wait until the timeout: {}",
        waits.join("; ")
    )
}

/// The text form (see the plan's example).
fn why_text(data: &WhyData) -> String {
    let mut out = format!("why {}", data.locator);
    if let Some(element) = &data.element {
        let mut named = element.clone();
        let locator = data.locator.trim_start_matches('@');
        if named.reference.as_deref() == Some(locator) {
            named.reference = None;
        }
        let _ = write!(out, " {}", element_text(&named));
    }
    let _ = writeln!(out, " (for {})", data.action);
    for check in &data.checks {
        let mark = match check.status {
            WhyStatus::Ok => '✓',
            WhyStatus::Failed => '✗',
            WhyStatus::Skipped => '-',
        };
        let line = match &check.why {
            Some(why) => format!("{mark} {:<9}  {why}", check.name),
            None => format!("{mark} {}", check.name),
        };
        let _ = writeln!(out, "{}", line.trim_end());
        for candidate in &check.candidates {
            let reference = candidate.reference.as_deref().unwrap_or("?");
            let _ = writeln!(
                out,
                "    {reference}: {}",
                element_text(&WhyElement {
                    reference: None,
                    ..candidate.clone()
                })
            );
        }
        for note in &check.notes {
            let _ = writeln!(out, "    {note}");
        }
    }
    let _ = write!(out, "→ {}", verdict_text(data));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(
        role: &str,
        name: &str,
        tag: &str,
        class: &str,
        reference: Option<&str>,
    ) -> WhyElement {
        WhyElement {
            reference: reference.map(str::to_owned),
            role: role.to_owned(),
            name: name.to_owned(),
            tag: tag.to_owned(),
            class: class.to_owned(),
        }
    }

    #[test]
    fn names_elements_as_the_snapshot_does() {
        assert_eq!(
            element_text(&named("button", "Pay", "button", "", Some("e12"))),
            "button \"Pay\" [e12]"
        );
        assert_eq!(
            element_text(&named(
                "generic",
                "We use cookies",
                "div",
                "cookie-banner",
                Some("e88")
            )),
            "div.cookie-banner [e88]"
        );
        assert_eq!(element_text(&named("main", "", "main", "", None)), "main");
        assert_eq!(
            fill_in(
                "display: none on {el}",
                Some(&named("generic", "", "div", "panel", Some("e3")))
            ),
            "display: none on div.panel [e3]"
        );
    }

    #[test]
    fn says_what_the_action_would_do() {
        let req = WhyRequest {
            tab: None,
            locator: "e12".to_owned(),
            action: WhyFor::Click,
            no_scroll: false,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: 1000,
        };
        let element = Some(named("button", "Pay", "button", "", Some("e12")));
        let mut stable = check(
            "stable",
            WhyStatus::Failed,
            Some("moved 14px between two frames (animation \"pulse\" on it)".to_owned()),
        );
        stable.element = None;
        let hit = check(
            "hit test",
            WhyStatus::Failed,
            Some(
                "covered by div.cookie-banner [e88]; its button \"Accept\" [e90] likely dismisses it"
                    .to_owned(),
            ),
        );
        let checks = vec![
            check("resolve", WhyStatus::Ok, Some("1 match".to_owned())),
            check(
                "visible",
                WhyStatus::Ok,
                Some("120×32 at 40,610".to_owned()),
            ),
            stable,
            check("enabled", WhyStatus::Ok, None),
            check(
                "in view",
                WhyStatus::Ok,
                Some("after scrolling main [e3] by 420px".to_owned()),
            ),
            hit,
        ];
        let data = why_data(&req, "t1".to_owned(), element, checks, None);
        assert_eq!(data.verdict.waits, ["stable", "hit test"]);
        assert_eq!(
            why_text(&data),
            "why e12 button \"Pay\" (for click)\n\
             ✓ resolve    1 match\n\
             ✓ visible    120×32 at 40,610\n\
             ✗ stable     moved 14px between two frames (animation \"pulse\" on it)\n\
             ✓ enabled\n\
             ✓ in view    after scrolling main [e3] by 420px\n\
             ✗ hit test   covered by div.cookie-banner [e88]; its button \"Accept\" [e90] likely dismisses it\n\
             → click would wait until the timeout: not stable; covered by div.cookie-banner [e88]"
        );

        let gone = why_data(
            &req,
            "t1".to_owned(),
            None,
            vec![check(
                "resolve",
                WhyStatus::Failed,
                Some("e12 is stale".to_owned()),
            )],
            Some("e12 is stale".to_owned()),
        );
        assert!(!gone.verdict.acts_now);
        assert!(gone.verdict.waits.is_empty());
        assert_eq!(
            verdict_text(&gone),
            "click would fail at once: e12 is stale"
        );
    }

    #[test]
    fn scrolls_are_named_innermost_first() {
        let names = vec![named("main", "", "main", "", Some("e3"))];
        let moved = [
            Scrolled {
                dx: 0,
                dy: 420,
                el: Some(0),
            },
            Scrolled {
                dx: 30,
                dy: 0,
                el: None,
            },
        ];
        assert_eq!(
            scrolled_text(&moved, &names).unwrap(),
            "after scrolling main [e3] by 420px and the page by 30px sideways"
        );
        assert_eq!(scrolled_text(&[], &names), None);
    }
}
