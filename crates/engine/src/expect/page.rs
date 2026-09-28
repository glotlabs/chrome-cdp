//! The page checks of `expect` that look once: `--no-errors`,
//! `--no-failed-requests` (since the page loaded, or since a `mark`) and
//! `--a11y` (the injected `a11y.ts` rules).

use std::time::Duration;

use cdp::domains::runtime;
use protocol::schema::Event;
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Failed, GROUP, Named, Passed, expected};
use crate::frames::World;
use crate::injected::{self, GUARD};
use crate::input::origin;
use crate::locator::{LocateOptions, Locator, read_listed};
use crate::request::{A11yLevel, ExpectRequest, SnapshotEngine};
use crate::tabs::{Page, tab_name};
use crate::{Engine, Running};

/// How many errors, failed requests or violations a failure lists.
const LISTED: usize = 5;

/// How many elements `a11y.ts` lists per rule (its `LISTED`) and in all.
const PER_RULE: usize = 5;
const ELEMENTS: usize = 60;

/// What `a11y.ts` found.
#[derive(Debug, Default, Deserialize)]
struct Audit {
    rules: usize,
    violations: Vec<Violation>,
}

#[derive(Debug, Default, Deserialize)]
struct Violation {
    rule: String,
    wcag: String,
    level: String,
    help: String,
    count: usize,
    #[serde(default)]
    page: Option<String>,
    nodes: Vec<Node>,
}

#[derive(Debug, Default, Deserialize)]
struct Node {
    candidate: Named,
    #[serde(default)]
    why: Option<String>,
    index: usize,
}

impl Engine {
    /// The network and console marks `--since` names (the page's
    /// document if `None`), and how the output says it.
    fn since(
        &self,
        running: &Running,
        page: &Page,
        since: Option<&str>,
    ) -> Result<((u64, u64), String), ErrorInfo> {
        let Some(name) = since else {
            return Ok((
                running.tabs.document_marks(page.id),
                "since the page loaded".into(),
            ));
        };
        match running.tabs.named_mark(page.id, name) {
            Ok(marks) => Ok((marks, format!("since mark {name:?}"))),
            Err(names) => {
                let mut info = ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!("tab {} has no mark {name:?}", tab_name(page.id)),
                );
                if !names.is_empty() {
                    info = info.note("marks", names.join(", "));
                }
                Err(info.hint(
                    Hint::run(Cmd::new("mark").arg(name).session(&self.session))
                        .why("sets it; then act, then expect … --since it"),
                ))
            }
        }
    }

    /// `--no-errors`: no uncaught exception or console error since the
    /// page loaded (or `since`).
    pub(super) fn no_errors(
        &self,
        running: &Running,
        page: &Page,
        req: &ExpectRequest,
        since: Option<&str>,
    ) -> Result<Passed, ErrorInfo> {
        let ((_, console), when) = self.since(running, page, since)?;
        let errors = running.tabs.console_errors_since(page.id, console);
        if errors.is_empty() {
            return Ok(Passed {
                actual: json!(0),
                quiet: "0".into(),
                count: Some(0),
                retries: 0,
            });
        }
        let tab = tab_name(page.id);
        let events: Vec<Event> = errors
            .iter()
            .map(|e| Event::ConsoleError {
                tab: tab.clone(),
                message: e.message.clone(),
                url: e.url.clone(),
                line: e.line,
                column: e.column,
                uncaught: e.uncaught,
            })
            .collect();
        let mut failed = Failed {
            expected: format!("{} {when}", expected(req, None)),
            actual: plural(errors.len(), "error", "errors"),
            ..Failed::default()
        };
        for event in events.iter().take(LISTED) {
            let line = self.event_line(event);
            let line = line.strip_prefix("console error: ").unwrap_or(&line);
            failed.notes.push(("error".into(), line.to_owned()));
        }
        if errors.len() > LISTED {
            failed.notes.push((
                "error".into(),
                format!("… and {} more", errors.len() - LISTED),
            ));
        }
        failed.details.insert("errors".into(), json!(events));
        let mut cmd = Cmd::new("errors").flag("since", console.to_string());
        if running.tabs.active() != Some(page.id) {
            cmd = cmd.flag("tab", &tab);
        }
        failed.hint =
            Some(Hint::run(cmd.session(&self.session)).why("shows them with their stacks"));
        Err(self.one_shot_failure(page, req, failed))
    }

    /// `--no-failed-requests`: no request failed or got a status of 400
    /// or more since the page loaded (or `since`); only those to
    /// `origin_filter` (`self`: the page's origin) if given. Chrome's own
    /// `/favicon.ico` request does not count.
    pub(super) fn no_failed_requests(
        &self,
        running: &Running,
        page: &Page,
        req: &ExpectRequest,
        since: Option<&str>,
        origin_filter: Option<&str>,
    ) -> Result<Passed, ErrorInfo> {
        let ((network, _), when) = self.since(running, page, since)?;
        let main_url = running
            .tabs
            .frames(page.id)
            .first()
            .map(|f| f.url.clone())
            .unwrap_or_default();
        let wanted = match origin_filter {
            Some("self") => Some(origin(&main_url).unwrap_or_default().to_owned()),
            Some(other) => origin(other).map(str::to_owned),
            None => None,
        };
        let failures: Vec<_> = running
            .tabs
            .failures_since(page.id, network)
            .into_iter()
            .filter(|f| wanted.as_deref().is_none_or(|w| origin(&f.url) == Some(w)))
            .filter(|f| !is_favicon(&f.url))
            .collect();
        if failures.is_empty() {
            return Ok(Passed {
                actual: json!(0),
                quiet: "0".into(),
                count: Some(0),
                retries: 0,
            });
        }
        let tab = tab_name(page.id);
        let events: Vec<Event> = failures
            .iter()
            .map(|f| Event::RequestFailed {
                tab: tab.clone(),
                id: running
                    .tabs
                    .netlog(page.id, |log| log.id_of(&f.session, &f.request_id))
                    .flatten(),
                method: f.method.clone(),
                url: f.url.clone(),
                resource_type: f.kind.clone(),
                status: f.status,
                error: f.error.clone(),
            })
            .collect();
        let mut failed = Failed {
            expected: format!("{} {when}", expected(req, None)),
            actual: plural(failures.len(), "failed request", "failed requests"),
            ..Failed::default()
        };
        for event in events.iter().take(LISTED) {
            let line = self.event_line(event);
            let line = line.strip_prefix("request failed: ").unwrap_or(&line);
            failed.notes.push(("failed".into(), line.to_owned()));
        }
        if failures.len() > LISTED {
            failed.notes.push((
                "failed".into(),
                format!("… and {} more", failures.len() - LISTED),
            ));
        }
        failed.details.insert("requests".into(), json!(events));
        let first = events.iter().find_map(|e| match e {
            Event::RequestFailed { id: Some(id), .. } => Some(*id),
            _ => None,
        });
        failed.hint = Some(match first {
            Some(id) => {
                let mut cmd = Cmd::new("network").arg("show").arg(id.to_string());
                if running.tabs.active() != Some(page.id) {
                    cmd = cmd.flag("tab", &tab);
                }
                Hint::run(cmd.session(&self.session))
                    .why("the first one, with its headers and body")
            }
            None => Hint::run(
                Cmd::new("network")
                    .switch("failed")
                    .flag("since", "0")
                    .session(&self.session),
            ),
        });
        Err(self.one_shot_failure(page, req, failed))
    }

    /// `--a11y`: the built-in rules of `level` in the page, or in the one
    /// element `scope` matches.
    #[allow(clippy::too_many_arguments, clippy::single_match_else)]
    pub(super) async fn a11y(
        &self,
        running: &Running,
        page: &Page,
        req: &ExpectRequest,
        scope: Option<&Locator>,
        level: A11yLevel,
        options: &LocateOptions,
        timeout: Duration,
    ) -> Result<Passed, ErrorInfo> {
        let generation = running
            .tabs
            .snapshot_memory(page.id, |m| m.refs.generation());
        let declaration = format!(
            "function (level) {{ {GUARD} \
             return globalThis.__ccdp.a11y(this === globalThis ? null : this, level); }}"
        );
        let mut params = runtime::CallFunctionOnParams::new(declaration);
        params.arguments = Some(vec![runtime::CallArgument {
            value: Some(json!(level.as_str())),
            unserializable_value: None,
            object_id: None,
        }]);
        params.object_group = Some(GROUP.to_owned());
        let (context, frame, target) = match scope {
            Some(scope) => {
                let target = self
                    .locate_one(running, page, scope, options, None, timeout)
                    .await?;
                params.object_id = Some(target.object_id.clone());
                (
                    target.context.clone(),
                    target.key.frame.clone(),
                    Some(target),
                )
            }
            None => {
                let context = self
                    .frame_context(running, page, None, World::Isolated, timeout)
                    .await?;
                let frame = running
                    .tabs
                    .frames(page.id)
                    .first()
                    .map(|f| f.id.clone())
                    .unwrap_or_default();
                (context, frame, None)
            }
        };
        let session = context.session.with_timeout(timeout);
        let result = injected::call(&session, context.context_id, params).await;
        let listed = async {
            let result = result.map_err(|e| crate::errors::from_cdp(&e, &self.session))?;
            if let Some(exception) = result.exception_details {
                return Err(ErrorInfo::new(
                    ErrorKind::EvalError,
                    format!(
                        "checking the page's accessibility failed: {}",
                        exception.text
                    ),
                ));
            }
            let object_id = result.result.object_id.ok_or_else(|| {
                ErrorInfo::new(
                    ErrorKind::ProtocolError,
                    "the accessibility check returned nothing",
                )
            })?;
            read_listed(&session, &object_id, &frame, ELEMENTS)
                .await
                .map_err(|e| crate::errors::from_cdp(&e, &self.session))
        }
        .await;
        let _ = session
            .call(runtime::ReleaseObjectGroupParams::new(GROUP.to_owned()))
            .await;
        if let Some(target) = target {
            target.release();
        }
        let (json, keys) = listed?;
        let mut audit: Audit = serde_json::from_str(&json).unwrap_or_default();
        running.tabs.snapshot_memory(page.id, |m| {
            if Some(m.refs.generation()) != generation {
                return;
            }
            for node in audit.violations.iter_mut().flat_map(|v| v.nodes.iter_mut()) {
                if let Some(Some(key)) = keys.get(node.index) {
                    let c = &node.candidate;
                    let number = m.refs.get(key).unwrap_or_else(|| {
                        m.refs
                            .assign_from(key, &c.role, &c.name, SnapshotEngine::Dom)
                    });
                    node.candidate.reference = Some(format!("e{number}"));
                }
            }
        });
        if audit.violations.is_empty() {
            return Ok(Passed {
                actual: json!(0),
                quiet: "0".into(),
                count: Some(0),
                retries: 0,
            });
        }
        let total: usize = audit.violations.iter().map(|v| v.count).sum();
        let mut failed = Failed {
            expected: expected(req, Some((audit.rules, level))),
            actual: format!(
                "{} of {}",
                plural(total, "violation", "violations"),
                plural(audit.violations.len(), "rule", "rules")
            ),
            ..Failed::default()
        };
        let mut lines = 0;
        for v in &audit.violations {
            let wcag = format!("WCAG {} {}", v.wcag, v.level);
            if let Some(page_why) = &v.page {
                if lines < LISTED * 2 {
                    failed
                        .notes
                        .push((v.rule.clone(), format!("{page_why} ({wcag})")));
                    lines += 1;
                }
                continue;
            }
            for node in v.nodes.iter().take(PER_RULE) {
                if lines >= LISTED * 2 {
                    break;
                }
                let why = node
                    .why
                    .as_deref()
                    .map(|w| format!(": {w}"))
                    .unwrap_or_default();
                failed.notes.push((
                    v.rule.clone(),
                    format!("{} {}{why} ({wcag})", node.candidate.line(), v.help),
                ));
                lines += 1;
            }
            if v.count > v.nodes.len().min(PER_RULE) && lines < LISTED * 2 {
                failed.notes.push((
                    v.rule.clone(),
                    format!("… and {} more", v.count - v.nodes.len().min(PER_RULE)),
                ));
                lines += 1;
            }
        }
        let shown: usize = failed.notes.len();
        if shown >= LISTED * 2 {
            failed
                .notes
                .push(("more".into(), "the rest are in --json's details".into()));
        }
        let violations: Vec<Value> = audit
            .violations
            .iter()
            .map(|v| {
                json!({
                    "rule": v.rule,
                    "wcag": v.wcag,
                    "level": v.level,
                    "help": v.help,
                    "count": v.count,
                    "page": v.page,
                    "elements": v.nodes.iter().map(|n| {
                        let mut e = n.candidate.json();
                        e["why"] = json!(n.why);
                        e
                    }).collect::<Vec<_>>(),
                })
            })
            .collect();
        failed.details.insert("rules".into(), json!(audit.rules));
        failed
            .details
            .insert("violations".into(), json!(violations));
        let first = audit
            .violations
            .iter()
            .flat_map(|v| &v.nodes)
            .find_map(|n| n.candidate.reference.clone());
        failed.hint = Some(match first {
            Some(reference) => Hint::run(
                Cmd::new("html")
                    .arg(&reference)
                    .switch("outer")
                    .session(&self.session),
            )
            .why("the first one's markup, to fix"),
            None => {
                Hint::run(Cmd::new("html").session(&self.session)).why("the page's markup, to fix")
            }
        });
        Err(self.one_shot_failure(page, req, failed))
    }

    /// The failure of a check that looks once (no timeout to report).
    fn one_shot_failure(&self, page: &Page, req: &ExpectRequest, failed: Failed) -> ErrorInfo {
        self.failure(page, req, failed, None)
    }
}

/// Whether `url` is a site's `/favicon.ico`, which Chrome asks for by
/// itself: a missing one is not the page's failure.
fn is_favicon(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or_default();
    path.ends_with("/favicon.ico")
}

/// `1 error`, `2 errors`.
fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}
