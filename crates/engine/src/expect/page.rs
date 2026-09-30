//! The page checks of `expect` that look once: `--no-errors`,
//! `--no-failed-requests` (since the page loaded, or since a `mark`) and
//! `--a11y` (the injected `a11y.ts` rules).

use std::collections::HashSet;
use std::time::Duration;

use cdp::domains::runtime;
use protocol::schema::{Event, ExpectData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::Instant;

use super::{Failed, GROUP, Named, Passed, assertion_text, expected};
use crate::context::Scoped as _;
use crate::frames::{World, iframe_label};
use crate::injected::{self, GUARD};
use crate::input::origin;
use crate::locator::{LocateOptions, Locator, read_listed};
use crate::network::Failure;
use crate::request::{A11yLevel, ExpectRequest, SnapshotEngine};
use crate::tabs::{FrameContext, Page, parse_tab_id, tab_name};
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
    /// The iframe it is in (its id and label), unless the main frame.
    #[serde(skip)]
    frame: Option<(String, String)>,
}

impl Audit {
    /// Adds `other`'s violations (another frame's) to these, rule by rule.
    fn merge(&mut self, other: Self) {
        self.rules = self.rules.max(other.rules);
        for v in other.violations {
            match self.violations.iter_mut().find(|w| w.rule == v.rule) {
                Some(w) => {
                    w.count += v.count;
                    w.nodes.extend(v.nodes);
                }
                None => self.violations.push(v),
            }
        }
    }
}

/// How long `expect --a11y` waits for an iframe's document to have a
/// context before it goes on without it (it is still loading).
const A11Y_FRAME_WAIT: Duration = Duration::from_secs(1);

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
            Ok(marks) => Ok((
                (marks.network, marks.console),
                format!("since mark {name:?}"),
            )),
            Err(names) => Err(self.no_mark(page.id, name, &names)),
        }
    }

    /// The error for `--since <name>` when tab `id` has no such mark but
    /// `names`.
    pub(crate) fn no_mark(&self, id: u32, name: &str, names: &[String]) -> ErrorInfo {
        let mut info = ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("tab {} has no mark {name:?}", tab_name(id)),
        );
        if !names.is_empty() {
            info = info.note("marks", names.join(", "));
        }
        info.hint(
            Hint::run(Cmd::new("mark").arg(name).scoped(&self.session))
                .why("sets it; then act, then look at what happened --since it"),
        )
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
                worker: e.worker.clone(),
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
            Some(Hint::run(cmd.scoped(&self.session)).why("shows them with their stacks"));
        Err(self.one_shot_failure(page, req, failed))
    }

    /// `--no-errors --all-tabs`: no uncaught exception or console error in
    /// any tab of the session (or context) since it opened, whatever it
    /// loaded since. The test runner checks it at the end of each test.
    pub(super) fn no_errors_in_all_tabs(&self, req: &ExpectRequest) -> Result<Output, ErrorInfo> {
        let start = Instant::now();
        let running = self.running()?;
        let tabs = self.all_tabs(&running)?;
        // A shared or service worker's error is in each tab of its
        // origin: counted once, in the first.
        let mut seen = HashSet::new();
        let mut events: Vec<Event> = Vec::new();
        for &id in &tabs {
            for e in running.tabs.console_errors_since(id, 0) {
                if e.once
                    .as_ref()
                    .is_some_and(|once| !seen.insert(once.clone()))
                {
                    continue;
                }
                events.push(Event::ConsoleError {
                    tab: tab_name(id),
                    message: e.message,
                    url: e.url,
                    line: e.line,
                    column: e.column,
                    uncaught: e.uncaught,
                    worker: e.worker,
                });
            }
        }
        let first = events.first().map(|e| event_tab(e).to_owned());
        let hint = first.map(|first| {
            let mut cmd = Cmd::new("errors");
            if running.tabs.active() != parse_tab_id(&first).ok() {
                cmd = cmd.flag("tab", &first);
            }
            Hint::run(cmd.scoped(&self.session)).why("shows them with their stacks")
        });
        let found = Found {
            events,
            one: "error",
            many: "errors",
            label: "error",
            prefix: "console error: ",
            key: "errors",
            hint,
        };
        self.in_all_tabs(req, start, found)
    }

    /// `--no-failed-requests --all-tabs`: no request failed or got a
    /// status of 400 or more in any tab of the session (or context) since
    /// it opened; only those to `origin_filter` if given (`self`: each
    /// tab's page's origin).
    pub(super) fn no_failed_requests_in_all_tabs(
        &self,
        req: &ExpectRequest,
        origin_filter: Option<&str>,
    ) -> Result<Output, ErrorInfo> {
        let start = Instant::now();
        let running = self.running()?;
        let tabs = self.all_tabs(&running)?;
        // A shared or service worker's request is in each tab of its
        // origin: counted once, in the first.
        let mut seen = HashSet::new();
        let mut events: Vec<Event> = Vec::new();
        for &id in &tabs {
            let wanted = wanted_origin(&running, id, origin_filter);
            for f in running.tabs.failures_since(id, 0) {
                if !wanted.as_deref().is_none_or(|w| origin(&f.url) == Some(w))
                    || is_favicon(&f.url)
                    || !seen.insert((f.session.clone(), f.request_id.clone()))
                {
                    continue;
                }
                events.push(failure_event(&running, id, &f));
            }
        }
        let hint = events.iter().find_map(|e| match e {
            Event::RequestFailed {
                tab, id: Some(id), ..
            } => Some(self.network_show_hint(&running, tab, *id)),
            _ => None,
        });
        let found = Found {
            events,
            one: "failed request",
            many: "failed requests",
            label: "failed",
            prefix: "request failed: ",
            key: "requests",
            hint: hint.or_else(|| Some(self.network_failed_hint())),
        };
        self.in_all_tabs(req, start, found)
    }

    /// The tabs `--all-tabs` looks at: the session's, or its context's,
    /// in order.
    fn all_tabs(&self, running: &Running) -> Result<Vec<u32>, ErrorInfo> {
        let scope = self.context_key()?;
        let mut tabs = running.tabs.tab_ids_in(scope.as_deref());
        tabs.sort_unstable();
        Ok(tabs)
    }

    /// The outcome of an `--all-tabs` check that found `found`.
    fn in_all_tabs(
        &self,
        req: &ExpectRequest,
        start: Instant,
        found: Found,
    ) -> Result<Output, ErrorInfo> {
        let assertion = assertion_text(req);
        let expected = format!("{} in any tab", expected(req, None));
        let events = found.events;
        if events.is_empty() {
            let elapsed = start.elapsed();
            let data = ExpectData {
                tab: String::new(),
                assertion: assertion.clone(),
                expected,
                actual: json!(0),
                count: Some(0),
                elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                retries: 0,
            };
            let text = format!("passed: {assertion} ({})", format::duration(elapsed));
            return Ok(Output::typed(&data, text)
                .quiet("0")
                .limit(req.max_chars, "--max-chars 0 for all of it"));
        }
        let mut info = ErrorInfo::new(
            ErrorKind::ExpectFailed,
            format!("expect failed: {assertion}"),
        )
        .note("expected", &expected)
        .note("actual", plural(events.len(), found.one, found.many));
        for event in events.iter().take(LISTED) {
            let line = self.event_line(event);
            let line = line.strip_prefix(found.prefix).unwrap_or(&line);
            info = info.note(found.label, format!("{}: {line}", event_tab(event)));
        }
        if events.len() > LISTED {
            info = info.note(found.label, format!("… and {} more", events.len() - LISTED));
        }
        if let Some(hint) = found.hint {
            info = info.hint(hint);
        }
        Err(info.details(json!({
            "assertion": assertion,
            "expected": expected,
            found.key: events,
        })))
    }

    /// `network show ID` (with `--tab` unless `tab` is the active one).
    fn network_show_hint(&self, running: &Running, tab: &str, id: u64) -> Hint {
        let mut cmd = Cmd::new("network").arg("show").arg(id.to_string());
        if running.tabs.active() != parse_tab_id(tab).ok() {
            cmd = cmd.flag("tab", tab);
        }
        Hint::run(cmd.scoped(&self.session)).why("the first one, with its headers and body")
    }

    /// `network --failed --since 0`.
    fn network_failed_hint(&self) -> Hint {
        Hint::run(
            Cmd::new("network")
                .switch("failed")
                .flag("since", "0")
                .scoped(&self.session),
        )
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
        let wanted = wanted_origin(running, page.id, origin_filter);
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
            .map(|f| failure_event(running, page.id, f))
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
            Some(id) => self.network_show_hint(running, &tab, id),
            None => self.network_failed_hint(),
        });
        Err(self.one_shot_failure(page, req, failed))
    }

    /// `--a11y`: the built-in rules of `level` in the page and each of its
    /// iframes (out-of-process ones too), or in the one element `scope`
    /// matches.
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
        let audit = match scope {
            Some(scope) => {
                let target = self
                    .locate_one(running, page, scope, options, None, timeout)
                    .await?;
                let audit = self
                    .audit(
                        running,
                        page,
                        (&target.context, &target.key.frame),
                        Some(&target.object_id),
                        level,
                        timeout,
                    )
                    .await;
                target.release();
                audit?
            }
            None => {
                let (main, context) = self
                    .frame_and_context(running, page, None, World::Isolated, timeout)
                    .await?;
                let mut audit = self
                    .audit(running, page, (&context, &main), None, level, timeout)
                    .await?;
                for frame in running.tabs.frames(page.id).into_iter().skip(1) {
                    let Ok(context) = running
                        .tabs
                        .context(page.id, &frame.id, World::Isolated, A11Y_FRAME_WAIT)
                        .await
                    else {
                        continue;
                    };
                    // A frame that goes away meanwhile has nothing to check.
                    let Ok(mut found) = self
                        .audit(running, page, (&context, &frame.id), None, level, timeout)
                        .await
                    else {
                        continue;
                    };
                    let label = iframe_label(&frame);
                    for node in found.violations.iter_mut().flat_map(|v| v.nodes.iter_mut()) {
                        node.frame = Some((frame.id.clone(), label.clone()));
                    }
                    audit.merge(found);
                }
                audit
            }
        };
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
                let place = node
                    .frame
                    .as_ref()
                    .map(|(_, label)| format!(" in {label}"))
                    .unwrap_or_default();
                failed.notes.push((
                    v.rule.clone(),
                    format!("{}{place} {}{why} ({wcag})", node.candidate.line(), v.help),
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
                        if let Some((frame, _)) = &n.frame {
                            e["frame"] = json!(frame);
                        }
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
                    .scoped(&self.session),
            )
            .why("the first one's markup, to fix"),
            None => {
                Hint::run(Cmd::new("html").scoped(&self.session)).why("the page's markup, to fix")
            }
        });
        Err(self.one_shot_failure(page, req, failed))
    }

    /// Runs the rules of `level` in a frame's isolated world (`at`: its
    /// context and id), on the element `object` or the document; page
    /// rules only in the main frame's document. The listed elements get
    /// refs (unless a frame loaded a new document meanwhile).
    async fn audit(
        &self,
        running: &Running,
        page: &Page,
        at: (&FrameContext, &str),
        object: Option<&str>,
        level: A11yLevel,
        timeout: Duration,
    ) -> Result<Audit, ErrorInfo> {
        let (context, frame) = at;
        let generation = running
            .tabs
            .snapshot_memory(page.id, |m| m.refs.generation());
        let main = running
            .tabs
            .frames(page.id)
            .first()
            .is_some_and(|f| f.id == frame);
        let declaration = format!(
            "function (level, page) {{ {GUARD} \
             return globalThis.__ccdp.a11y(this === globalThis ? null : this, level, page); }}"
        );
        let mut params = runtime::CallFunctionOnParams::new(declaration);
        params.arguments = Some(
            [json!(level.as_str()), json!(main)]
                .into_iter()
                .map(|value| runtime::CallArgument {
                    value: Some(value),
                    unserializable_value: None,
                    object_id: None,
                })
                .collect(),
        );
        params.object_group = Some(GROUP.to_owned());
        params.object_id = object.map(str::to_owned);
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
            read_listed(&session, &object_id, frame, ELEMENTS)
                .await
                .map_err(|e| crate::errors::from_cdp(&e, &self.session))
        }
        .await;
        let _ = session
            .call(runtime::ReleaseObjectGroupParams::new(GROUP.to_owned()))
            .await;
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
        Ok(audit)
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

/// What an `--all-tabs` check found, and how its failure says it.
struct Found {
    events: Vec<Event>,
    /// `error` and `errors`.
    one: &'static str,
    many: &'static str,
    /// Each listed event's note label, and what its line starts with.
    label: &'static str,
    prefix: &'static str,
    /// The events' key in `details`.
    key: &'static str,
    hint: Option<Hint>,
}

/// The tab an error or failed request was in.
fn event_tab(event: &Event) -> &str {
    match event {
        Event::ConsoleError { tab, .. } | Event::RequestFailed { tab, .. } => tab,
        _ => "",
    }
}

/// The origin `--origin` keeps requests to in tab `id` (`self`: its
/// page's), if given.
fn wanted_origin(running: &Running, id: u32, origin_filter: Option<&str>) -> Option<String> {
    match origin_filter {
        Some("self") => {
            let main_url = running
                .tabs
                .frames(id)
                .first()
                .map(|f| f.url.clone())
                .unwrap_or_default();
            Some(origin(&main_url).unwrap_or_default().to_owned())
        }
        Some(other) => origin(other).map(str::to_owned),
        None => None,
    }
}

/// Failed request `f` of tab `id`, as an event.
fn failure_event(running: &Running, id: u32, f: &Failure) -> Event {
    Event::RequestFailed {
        tab: tab_name(id),
        id: running
            .tabs
            .netlog(id, |log| log.id_of(&f.session, &f.request_id))
            .flatten(),
        method: f.method.clone(),
        url: f.url.clone(),
        resource_type: f.kind.clone(),
        status: f.status,
        error: f.error.clone(),
    }
}
