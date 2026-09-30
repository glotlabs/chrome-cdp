//! `audit` and `expect --audit` (plan 17, milestone 17.4): one report over
//! a page, from every check chrome-cdp has, each finding in one shape
//! ([`Finding`]) with a key that stays the same across runs, ports and
//! reloads.
//!
//! The categories, in the report's order ([`rules::CATEGORIES`]):
//! Chrome's issues (`observe::issues`), the accessibility rules of
//! `a11y.ts` at level AA, `layout`'s findings at the tab's viewport, the
//! console's errors and the failed requests since the page loaded, and
//! the `security` and `meta` checks of [`checks`], from the document
//! (`injected/src/audit.ts`), its response's headers in the network log
//! and its cookies. A LOCATOR scopes what can be scoped (`a11y`,
//! `layout`, the forms of `security`); the page-wide categories run with
//! it only when `--category` names them.
//!
//! A baseline ([`baseline`]) accepts findings by key; `expect --audit`
//! fails on those it does not accept, after waiting for `load`. It looks
//! once: issues and errors are recorded, not polled.

mod baseline;
mod checks;
mod rules;

pub use baseline::{BaselineProblem, VERSION as BASELINE_VERSION, parse_baseline};
pub use rules::{AuditRule, CATEGORIES, RULES, SCOPED};

use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::Duration;

use cdp::domains::{network, runtime};
use protocol::schema::{
    AuditBaselineFile, AuditBaselineResult, AuditBaselineState, AuditData, ExpectAudit, ExpectData,
    Finding, FindingTarget, Issue, LayoutFinding, Severity,
};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format, suggest};
use serde_json::{Value, json};
use tokio::time::Instant;

use crate::context::Scoped as _;
use crate::errors::from_cdp;
use crate::expect::is_favicon;
use crate::frames::World;
use crate::injected::{self, GUARD};
use crate::input::origin;
use crate::locator::{LocateOptions, Locator, read_listed};
use crate::observe::commands::shown_url;
use crate::observe::issue_commands::issue_about;
use crate::request::{A11yLevel, AuditRequest, SnapshotEngine, millis};
use crate::tabs::{FrameContext, Page, TabError, tab_name};
use crate::wait::Condition;
use crate::{Engine, Running};
use checks::{CookieFacts, DocFacts, finding, page_place, url_place};

/// How many elements the accessibility rules list per rule (then a
/// warning says how many more there are).
const A11Y_PER_RULE: usize = 50;

/// How many forms `auditPage` lists (its `FORMS`).
const FORMS: usize = 50;

/// How long naming one element (its stable locator) may take.
const NAME_WAIT: Duration = Duration::from_secs(1);

/// How wide the `category/rule` column may get.
const RULE_COLUMN: usize = 40;

/// How much of a console message a key keeps.
const KEY_MESSAGE: usize = 100;

/// The object group of `auditPage`'s answer.
const GROUP: &str = "chrome-cdp-audit";

/// Checks `audit`'s arguments without a browser: the categories and the
/// locator.
pub fn check_audit(req: &AuditRequest) -> Result<(), ErrorInfo> {
    categories(req)?;
    if let Some(locator) = &req.locator {
        options(req).parse(locator)?;
    }
    if req.all_tabs && req.tab.is_some() {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            "--all-tabs audits every tab; it takes no --tab",
        )
        .hint(Hint::run(Cmd::new("audit").switch("all-tabs"))));
    }
    Ok(())
}

fn options(req: &AuditRequest) -> LocateOptions {
    LocateOptions {
        strict_refs: req.strict_refs,
        testid_attribute: req.testid_attribute.clone(),
    }
}

/// The categories to check, in the report's order: `--category` (comma
/// lists too), else all of them, or with a LOCATOR the ones it scopes.
fn categories(req: &AuditRequest) -> Result<Vec<&'static str>, ErrorInfo> {
    let mut wanted = Vec::new();
    for name in req.categories.iter().flat_map(|c| c.split(',')) {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let Some(&known) = CATEGORIES.iter().find(|c| **c == name) else {
            let mut info = ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("{name:?} is not a category of audit"),
            )
            .note("categories", CATEGORIES.join(", "));
            let close = suggest::similar(name, CATEGORIES, 1);
            if let Some(first) = close.first() {
                info = info.note("did you mean", *first);
            }
            return Err(info.hint(Hint::run(
                Cmd::new("audit").flag("category", close.first().copied().unwrap_or("a11y")),
            )));
        };
        wanted.push(known);
    }
    if wanted.is_empty() {
        wanted = if req.locator.is_some() {
            SCOPED.to_vec()
        } else {
            CATEGORIES.to_vec()
        };
    }
    Ok(CATEGORIES
        .iter()
        .copied()
        .filter(|c| wanted.contains(c))
        .collect())
}

/// What one tab's audit found.
struct Audited {
    tab: String,
    url: String,
    findings: Vec<Finding>,
    warnings: Vec<String>,
}

/// The baseline as read, and what the run does with it.
struct Loaded {
    file: Option<AuditBaselineFile>,
}

impl Engine {
    /// `audit [LOCATOR]`, and `expect --audit` (`req.expect`); see the
    /// module docs.
    pub async fn audit(&self, req: AuditRequest) -> Result<Output, ErrorInfo> {
        check_audit(&req)?;
        let checked = categories(&req)?;
        let start = Instant::now();
        let loaded = Loaded {
            file: match &req.baseline {
                Some(b) => baseline::read(&b.path)?,
                None => None,
            },
        };
        if req.expect
            && let Some(b) = &req.baseline
            && loaded.file.is_none()
            && !b.update
            && !b.create
        {
            return Err(ErrorInfo::new(
                ErrorKind::ExpectFailed,
                format!(
                    "expect {} failed: there is no baseline yet",
                    assertion(&req)
                ),
            )
            .note("baseline", b.path.display().to_string())
            .hint(
                Hint::run(self.audit_cmd(&req).switch("update").scoped(&self.session))
                    .why("stores the findings as the baseline; commit the file"),
            )
            .details(json!({ "baseline": b.path.display().to_string() })));
        }
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let ids = if req.all_tabs {
            self.all_tabs(&running)?
        } else {
            vec![self.page(&running, req.tab.as_deref(), timeout).await?.id]
        };
        let mut audited = Vec::new();
        for id in ids {
            let page = match running.tabs.page(Some(id), timeout).await {
                Ok(page) => page,
                Err(TabError::NoSuchTab(_)) if req.all_tabs => continue,
                Err(e) => return Err(self.tab_error(&running, e)),
            };
            audited.push(
                self.audit_tab(&running, &page, &req, &checked, timeout)
                    .await?,
            );
        }
        self.audit_output(&req, &checked, &loaded, &audited, start)
    }

    /// Audits one tab: waits for `load` (failing `expect` if it does not
    /// come), then runs each category, holding the tab's lock.
    async fn audit_tab(
        &self,
        running: &Running,
        page: &Page,
        req: &AuditRequest,
        checked: &[&str],
        timeout: Duration,
    ) -> Result<Audited, ErrorInfo> {
        self.check_not_blocked(running, page)?;
        let options = options(req);
        let mut warnings = Vec::new();
        let load = Condition::Load {
            dom_content_loaded: false,
        };
        let deadline = Instant::now() + timeout;
        if let Err(unmet) = self
            .probe_until(running, page, &load, &options, deadline)
            .await?
        {
            let state = unmet
                .observed
                .get("readyState")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned();
            if req.expect {
                return Err(ErrorInfo::new(
                    ErrorKind::ExpectFailed,
                    format!("expect {} failed: the page did not load", assertion(req)),
                )
                .note(
                    "actual",
                    format!(
                        "document.readyState is {state:?} (after {})",
                        format::duration(timeout)
                    ),
                )
                .hint(Hint::run(
                    Cmd::new("wait").switch("load").scoped(&self.session),
                )));
            }
            warnings.push(format!(
                "the page had not loaded after {} (document.readyState {state:?}); audited it as it was",
                format::duration(timeout)
            ));
        }
        let tab = tab_name(page.id);
        let url = running
            .tabs
            .frames(page.id)
            .first()
            .map(|f| f.url.clone())
            .unwrap_or_default();
        let mark = running.tabs.mark();
        let _tab = self.tabs.lock(&page.target_id).await;
        let work = async {
            let mut findings = Vec::new();
            let scope = match &req.locator {
                Some(text) => Some(options.parse(text)?),
                None => None,
            };
            for &category in checked {
                let found = match category {
                    "issues" => self.issue_findings(running, page, &url, timeout).await,
                    "a11y" => {
                        self.a11y_findings(
                            running,
                            page,
                            scope.as_ref(),
                            &options,
                            &mut warnings,
                            timeout,
                        )
                        .await?
                    }
                    "layout" => {
                        let (found, more) = self
                            .layout_findings(running, page, req.locator.clone(), &options, timeout)
                            .await?;
                        warnings.extend(more);
                        found.into_iter().map(layout_finding).collect()
                    }
                    "console" => self.console_findings(running, page, &url),
                    "network" => self.network_findings(running, page, &url),
                    // Read below, together.
                    _ => Vec::new(),
                };
                findings.extend(found);
            }
            let security = checked.contains(&"security");
            let meta = checked.contains(&"meta");
            if security || meta {
                let (doc, headers, cookies) = self
                    .page_facts(
                        running,
                        page,
                        scope.as_ref(),
                        &options,
                        &url,
                        security,
                        timeout,
                    )
                    .await?;
                if security {
                    let page_only = req.locator.is_none()
                        || req
                            .categories
                            .iter()
                            .any(|c| c.split(',').any(|c| c.trim() == "security"));
                    if headers.is_none() && page_only {
                        warnings.push(
                            "chrome-cdp did not see the document's response (it attached after the \
                             page loaded), so its headers (CSP, HSTS) were not checked; reload for them"
                                .to_owned(),
                        );
                    }
                    findings.extend(checks::security(
                        &url,
                        &doc,
                        headers.as_deref(),
                        &cookies,
                        page_only,
                    ));
                }
                if meta {
                    findings.extend(checks::meta(&url, &doc));
                }
            }
            Ok::<_, ErrorInfo>(findings)
        };
        let findings = Box::pin(self.unless_blocked(running, page, mark, work)).await?;
        let mut findings = fold(findings);
        if req.all_tabs {
            for f in &mut findings {
                f.tab = Some(tab.clone());
            }
        }
        Ok(Audited {
            tab,
            url,
            findings,
            warnings,
        })
    }

    /// Chrome's issues since the page loaded, at any severity.
    async fn issue_findings(
        &self,
        running: &Running,
        page: &Page,
        url: &str,
        timeout: Duration,
    ) -> Vec<Finding> {
        self.check_forms_issues(running, page.id, timeout).await;
        let after = running
            .tabs
            .issues(page.id, |issues, _| issues.document())
            .unwrap_or(0);
        let page_origin = origin(url).map(str::to_owned);
        self.issues_since(running, page, after, Severity::Info)
            .await
            .into_iter()
            .map(|(issue, _)| issue_finding(issue, page_origin.as_deref()))
            .collect()
    }

    /// The accessibility rules at level AA: an element's finding has its
    /// ref and stable locator; a rule with more elements than listed gets
    /// a warning.
    async fn a11y_findings(
        &self,
        running: &Running,
        page: &Page,
        scope: Option<&Locator>,
        options: &LocateOptions,
        warnings: &mut Vec<String>,
        timeout: Duration,
    ) -> Result<Vec<Finding>, ErrorInfo> {
        let audit = self
            .a11y_violations(
                running,
                page,
                scope,
                A11yLevel::AA,
                options,
                A11Y_PER_RULE,
                timeout,
            )
            .await?;
        let tab = tab_name(page.id);
        let url = running
            .tabs
            .frames(page.id)
            .first()
            .map(|f| f.url.clone())
            .unwrap_or_default();
        let mut out = Vec::new();
        for v in audit.violations {
            let severity = if v.level == "A" {
                Severity::Error
            } else {
                Severity::Warning
            };
            let wcag = format!("{} {}", v.wcag, v.level);
            if let Some(why) = &v.page {
                let mut f = finding(
                    "a11y",
                    &v.rule,
                    why.clone(),
                    FindingTarget::default(),
                    &page_place(&url),
                );
                f.severity = severity;
                f.wcag = Some(wcag);
                out.push(f);
                continue;
            }
            if v.count > v.nodes.len() {
                warnings.push(format!(
                    "a11y/{}: {} more elements than listed; expect --a11y --json lists them by rule",
                    v.rule,
                    v.count - v.nodes.len()
                ));
            }
            for node in v.nodes {
                let c = &node.candidate;
                let element = checks::Described {
                    role: c.role.clone(),
                    name: c.name.clone(),
                    tag: c.tag.clone(),
                }
                .text();
                let locator = match &c.reference {
                    Some(reference) => self.stable_locator_held(&tab, reference, NAME_WAIT).await,
                    None => None,
                };
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
                let message = format!("{}{place} {}{why}", c.line(), v.help);
                let target = FindingTarget {
                    reference: c.reference.clone(),
                    element: Some(element.clone()),
                    locator: locator.clone(),
                    frame: node.frame.as_ref().map(|(id, _)| id.clone()),
                    ..FindingTarget::default()
                };
                let key_place = locator.unwrap_or(element);
                let mut f = finding("a11y", &v.rule, message, target, &key_place);
                f.severity = severity;
                f.wcag = Some(wcag.clone());
                out.push(f);
            }
        }
        Ok(out)
    }

    /// Uncaught exceptions and console errors since the page loaded.
    #[allow(clippy::unused_self)]
    fn console_findings(&self, running: &Running, page: &Page, url: &str) -> Vec<Finding> {
        let (_, console) = running.tabs.document_marks(page.id);
        let page_origin = origin(url);
        running
            .tabs
            .console_errors_since(page.id, console)
            .into_iter()
            .map(|e| {
                let rule = if e.uncaught {
                    "uncaught-exception"
                } else {
                    "console-error"
                };
                let source = e.url.as_deref().filter(|u| !u.is_empty());
                let place = match (source, e.line) {
                    (Some(u), Some(line)) => format!("{}:{line}", url_place(u, url)),
                    (Some(u), None) => url_place(u, url),
                    _ => page_place(url),
                };
                let mut message = e.message.clone();
                if let Some(u) = source {
                    let _ = write!(message, "  ({}", shown_url(u, page_origin));
                    if let Some(line) = e.line {
                        let _ = write!(message, ":{line}");
                    }
                    message.push(')');
                }
                let target = FindingTarget {
                    worker: e.worker.clone(),
                    ..FindingTarget::default()
                };
                let key_place = format!("{place} {}", key_message(&e.message, url));
                finding("console", rule, message, target, &key_place)
            })
            .collect()
    }

    /// Failed requests and responses of 400 or more since the page
    /// loaded; the page's own origin's are errors, others warnings.
    #[allow(clippy::unused_self)]
    fn network_findings(&self, running: &Running, page: &Page, url: &str) -> Vec<Finding> {
        let (network, _) = running.tabs.document_marks(page.id);
        let page_origin = origin(url);
        running
            .tabs
            .failures_since(page.id, network)
            .into_iter()
            .filter(|f| !is_favicon(&f.url))
            .map(|f| {
                let rule = if f.status.is_some() {
                    "http-error"
                } else {
                    "request-failed"
                };
                let what = match (f.status, &f.error) {
                    (Some(status), _) => status.to_string(),
                    (None, Some(error)) => error.clone(),
                    (None, None) => "failed".to_owned(),
                };
                let short = f.url.split(['?', '#']).next().unwrap_or_default();
                let message = format!("{} {} {what}", f.method, shown_url(short, page_origin));
                let id = running
                    .tabs
                    .netlog(page.id, |log| log.id_of(&f.session, &f.request_id))
                    .flatten();
                let target = FindingTarget {
                    request_id: id,
                    url: Some(f.url.clone()),
                    ..FindingTarget::default()
                };
                let key_place = format!("{} {}", f.method, url_place(&f.url, url));
                let mut found = finding("network", rule, message, target, &key_place);
                if origin(&f.url) != page_origin {
                    found.severity = Severity::Warning;
                }
                found
            })
            .collect()
    }

    /// What `security` and `meta` read: the document's facts (in `scope`'s
    /// element's frame, inside it, when given), its response's headers
    /// (`None` if the network log did not see it) and, for `security`,
    /// the cookies its requests carry.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::single_match_else)]
    async fn page_facts(
        &self,
        running: &Running,
        page: &Page,
        scope: Option<&Locator>,
        options: &LocateOptions,
        url: &str,
        security: bool,
        timeout: Duration,
    ) -> Result<(DocFacts, Option<Vec<(String, String)>>, Vec<CookieFacts>), ErrorInfo> {
        let mut doc = match scope {
            Some(scope) => {
                let target = self
                    .locate_one(running, page, scope, options, None, timeout)
                    .await?;
                let facts = self
                    .doc_facts(
                        running,
                        page,
                        &target.context,
                        &target.key.frame,
                        Some(&target.object_id),
                        timeout,
                    )
                    .await;
                target.release();
                facts?
            }
            None => {
                let (main, context) = self
                    .frame_and_context(running, page, None, World::Isolated, timeout)
                    .await?;
                self.doc_facts(running, page, &context, &main, None, timeout)
                    .await?
            }
        };
        let tab = tab_name(page.id);
        for form in &mut doc.forms {
            if let Some(reference) = &form.reference {
                form.locator = self.stable_locator_held(&tab, reference, NAME_WAIT).await;
            }
        }
        let headers = running
            .tabs
            .netlog(page.id, |log| {
                log.records()
                    .rev()
                    .find(|r| {
                        r.navigation.is_some()
                            && r.entry.status.is_some()
                            && r.entry.redirect.is_none()
                    })
                    .map(|r| {
                        r.response_headers
                            .iter()
                            .map(|h| (h.name.clone(), h.value.clone()))
                            .collect::<Vec<_>>()
                    })
            })
            .flatten();
        let mut cookies = Vec::new();
        if security && matches!(url.split(':').next(), Some("http" | "https")) {
            let params = network::GetCookiesParams {
                urls: Some(vec![url.to_owned()]),
            };
            let found = page
                .session
                .with_timeout(timeout)
                .call(params)
                .await
                .map_err(|e| from_cdp(&e, &self.session))?;
            cookies = found
                .cookies
                .into_iter()
                .map(|c| CookieFacts {
                    name: c.name,
                    domain: c.domain.trim_start_matches('.').to_owned(),
                    path: c.path,
                    secure: c.secure,
                    http_only: c.http_only,
                    same_site: c.same_site.is_some(),
                })
                .collect();
        }
        Ok((doc, headers, cookies))
    }

    /// Runs `auditPage` in a frame's isolated world (on the element
    /// `object`, or the document), giving the forms refs.
    async fn doc_facts(
        &self,
        running: &Running,
        page: &Page,
        context: &FrameContext,
        frame: &str,
        object: Option<&str>,
        timeout: Duration,
    ) -> Result<DocFacts, ErrorInfo> {
        let generation = running
            .tabs
            .snapshot_memory(page.id, |m| m.refs.generation());
        let declaration = format!(
            "function () {{ {GUARD} \
             return globalThis.__ccdp.auditPage(this === globalThis ? null : this); }}"
        );
        let mut params = runtime::CallFunctionOnParams::new(declaration);
        params.object_group = Some(GROUP.to_owned());
        params.object_id = object.map(str::to_owned);
        let session = context.session.with_timeout(timeout);
        let result = injected::call(&session, context.context_id, params).await;
        let listed = async {
            let result = result.map_err(|e| from_cdp(&e, &self.session))?;
            if let Some(exception) = result.exception_details {
                return Err(ErrorInfo::new(
                    ErrorKind::EvalError,
                    format!("reading the page for the audit failed: {}", exception.text),
                ));
            }
            let object_id = result.result.object_id.ok_or_else(|| {
                ErrorInfo::new(
                    ErrorKind::ProtocolError,
                    "the audit's page read returned nothing",
                )
            })?;
            read_listed(&session, &object_id, frame, FORMS)
                .await
                .map_err(|e| from_cdp(&e, &self.session))
        }
        .await;
        let _ = session
            .call(runtime::ReleaseObjectGroupParams::new(GROUP.to_owned()))
            .await;
        let (json, keys) = listed?;
        let mut doc: DocFacts = serde_json::from_str(&json).unwrap_or_default();
        running.tabs.snapshot_memory(page.id, |m| {
            if Some(m.refs.generation()) != generation {
                return;
            }
            for form in &mut doc.forms {
                if let Some(Some(key)) = keys.get(form.index) {
                    let c = &form.candidate;
                    let number = m.refs.get(key).unwrap_or_else(|| {
                        m.refs
                            .assign_from(key, &c.role, &c.name, SnapshotEngine::Dom)
                    });
                    form.reference = Some(format!("e{number}"));
                }
            }
        });
        Ok(doc)
    }

    /// The report (or `expect`'s outcome) of the tabs `audited`.
    fn audit_output(
        &self,
        req: &AuditRequest,
        checked: &[&str],
        loaded: &Loaded,
        audited: &[Audited],
        start: Instant,
    ) -> Result<Output, ErrorInfo> {
        let mut warnings: Vec<String> = Vec::new();
        let mut all: Vec<Finding> = Vec::new();
        for a in audited {
            for w in &a.warnings {
                let w = if req.all_tabs {
                    format!("{}: {w}", a.tab)
                } else {
                    w.clone()
                };
                if !warnings.contains(&w) {
                    warnings.push(w);
                }
            }
            all.extend(a.findings.iter().cloned());
        }
        sort(&mut all);
        let hidden = all.iter().filter(|f| f.severity > req.min_severity).count();
        let mut shown: Vec<Finding> = all
            .iter()
            .filter(|f| f.severity <= req.min_severity)
            .cloned()
            .collect();
        let mut state = None;
        let mut stale = Vec::new();
        if let Some(b) = &req.baseline {
            let path = b.path.display().to_string();
            let write = b.update || (loaded.file.is_none() && b.create && req.expect);
            if let Some(file) = &loaded.file {
                stale = baseline::apply(file, &all, &mut shown, checked);
            }
            let result = if write {
                let new = baseline::updated(loaded.file.as_ref(), &all, &shown, checked);
                match &loaded.file {
                    Some(old) if *old == new => AuditBaselineResult::Unchanged,
                    old => {
                        baseline::write(&b.path, &new)?;
                        for f in &mut shown {
                            f.accepted = true;
                        }
                        stale.clear();
                        if old.is_some() {
                            AuditBaselineResult::Updated
                        } else {
                            AuditBaselineResult::Created
                        }
                    }
                }
            } else if loaded.file.is_some() {
                AuditBaselineResult::Compared
            } else {
                AuditBaselineResult::Missing
            };
            if result == AuditBaselineResult::Unchanged {
                for f in &mut shown {
                    f.accepted = true;
                }
                stale.clear();
            }
            state = Some(AuditBaselineState {
                path,
                result,
                stale: stale.clone(),
            });
        }
        let accepted = shown.iter().filter(|f| f.accepted).count();
        let new: Vec<Finding> = shown.iter().filter(|f| !f.accepted).cloned().collect();
        let count = |s| new.iter().filter(|f| f.severity == s).count();
        let (errors, warnings_n, info) = (
            count(Severity::Error),
            count(Severity::Warning),
            count(Severity::Info),
        );
        let (tab, url) = match audited {
            [one] if !req.all_tabs => (one.tab.clone(), one.url.clone()),
            _ => (String::new(), String::new()),
        };
        if req.expect {
            return self.expect_output(req, &tab, &url, &all, &shown, &new, state, warnings, start);
        }
        let listed: Vec<Finding> = if req.show_accepted {
            shown
        } else {
            new.clone()
        };
        let data = AuditData {
            tab: tab.clone(),
            url: url.clone(),
            locator: req.locator.clone(),
            categories: checked.iter().map(|c| (*c).to_owned()).collect(),
            min_severity: req.min_severity,
            findings: listed,
            errors,
            warnings: warnings_n,
            info,
            hidden,
            accepted,
            baseline: state.clone(),
        };
        let mut text = String::new();
        let place = if req.all_tabs {
            let n = audited.len();
            format!("{n} {}", if n == 1 { "tab" } else { "tabs" })
        } else {
            let mut place = url.clone();
            if let Some(locator) = &req.locator {
                let _ = write!(place, " inside {locator}");
            }
            place
        };
        let _ = writeln!(
            text,
            "audit: {place} — {}{}",
            summary(errors, warnings_n, info),
            aside(&all, req.min_severity, accepted)
        );
        text.push_str(&report(&data.findings, req.all_tabs));
        if let Some(state) = &state {
            match state.result {
                AuditBaselineResult::Created => {
                    let _ = writeln!(
                        text,
                        "created baseline {} ({})",
                        state.path,
                        plural(accepted, "finding", "findings")
                    );
                }
                AuditBaselineResult::Updated => {
                    let _ = writeln!(
                        text,
                        "updated baseline {} ({})",
                        state.path,
                        plural(accepted, "finding", "findings")
                    );
                }
                AuditBaselineResult::Unchanged => {
                    let _ = writeln!(text, "baseline {} is up to date", state.path);
                }
                AuditBaselineResult::Missing => {
                    let _ = writeln!(text, "no baseline yet at {}", state.path);
                }
                AuditBaselineResult::Compared => {}
            }
            if !state.stale.is_empty() {
                let _ = writeln!(
                    text,
                    "stale: {} the baseline accepts no longer found",
                    plural(state.stale.len(), "finding", "findings")
                );
                for key in &state.stale {
                    let _ = writeln!(text, "  {key}");
                }
            }
        }
        let wants_update = !new.is_empty() || state.as_ref().is_some_and(|s| !s.stale.is_empty());
        if wants_update && !req.all_tabs {
            let cmd = match &req.baseline {
                Some(_) => self.audit_cmd(req),
                None => self.audit_cmd(req).flag("baseline", baseline_name(&url)),
            };
            let why = if new.is_empty() {
                "drops the stale ones"
            } else {
                "accepts these"
            };
            let _ = writeln!(
                text,
                "hint: {}",
                Hint::run(cmd.switch("update").scoped(&self.session)).why(why)
            );
        }
        let quiet = (errors + warnings_n + info).to_string();
        let output = Output::typed(&data, text.trim_end().to_owned())
            .quiet(quiet)
            .limit(
                req.max_chars,
                "--min-severity error, --category or --max-chars 0 for all of it",
            );
        Ok(warnings.into_iter().fold(output, Output::warning))
    }

    /// `expect --audit`: fails on new findings, with the report of those.
    #[allow(clippy::too_many_arguments)]
    fn expect_output(
        &self,
        req: &AuditRequest,
        tab: &str,
        url: &str,
        all: &[Finding],
        shown: &[Finding],
        new: &[Finding],
        state: Option<AuditBaselineState>,
        warnings: Vec<String>,
        start: Instant,
    ) -> Result<Output, ErrorInfo> {
        let assertion = assertion(req);
        let elapsed = start.elapsed();
        let stale = state.as_ref().map(|s| s.stale.clone()).unwrap_or_default();
        if !new.is_empty() {
            let count = |s| new.iter().filter(|f| f.severity == s).count();
            let what = if req.baseline.is_some() {
                "the baseline does not accept"
            } else {
                "(no baseline)"
            };
            let mut info = ErrorInfo::new(
                ErrorKind::ExpectFailed,
                format!(
                    "expect {assertion} failed: {} {what}",
                    plural(new.len(), "finding", "findings")
                ),
            );
            if !url.is_empty() {
                info = info.note("page", url);
            }
            info = info.note(
                "found",
                format!(
                    "{}{}",
                    summary(
                        count(Severity::Error),
                        count(Severity::Warning),
                        count(Severity::Info)
                    ),
                    aside(all, req.min_severity, shown.len() - new.len())
                ),
            );
            if let Some(state) = &state {
                info = info.note("baseline", &state.path);
            }
            info = info.block("new", report(new, req.all_tabs).trim_end());
            if !stale.is_empty() {
                info = info.note(
                    "stale",
                    format!(
                        "{} the baseline accepts no longer found: {}",
                        plural(stale.len(), "finding", "findings"),
                        stale.join(", ")
                    ),
                );
            }
            let update = match &req.baseline {
                Some(_) => self.audit_cmd(req),
                None => self.audit_cmd(req).flag("baseline", baseline_name(url)),
            };
            let hint = Hint::run(update.switch("update").scoped(&self.session))
                .why("accepts them, if they are fine")
                .or("fix them");
            return Err(info.hint(hint).details(json!({
                "assertion": assertion,
                "url": url,
                "baseline": state,
                "new": new,
                "found": all.iter().filter(|f| f.severity <= req.min_severity).map(|f| &f.key).collect::<Vec<_>>(),
                "elapsed_ms": u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            })));
        }
        let keys: Vec<String> = shown.iter().map(|f| f.key.clone()).collect();
        let accepted = shown.len();
        let mut text = format!(
            "passed: expect {assertion}: {}",
            match (accepted, &req.baseline) {
                (0, _) => "no findings".to_owned(),
                (n, _) => format!("{}, all accepted", plural(n, "finding", "findings")),
            }
        );
        let _ = write!(text, " ({})", format::duration(elapsed));
        if let Some(state) = &state {
            match state.result {
                AuditBaselineResult::Created => {
                    let _ = write!(text, "\ncreated baseline {}", state.path);
                }
                AuditBaselineResult::Updated => {
                    let _ = write!(text, "\nupdated baseline {}", state.path);
                }
                _ => {}
            }
        }
        let mut warnings = warnings;
        if !stale.is_empty() {
            let shown_keys: Vec<&str> = stale.iter().take(5).map(String::as_str).collect();
            warnings.push(format!(
                "the baseline accepts {} no longer found ({}{}); {} drops them",
                plural(stale.len(), "finding", "findings"),
                shown_keys.join("; "),
                if stale.len() > 5 { "; …" } else { "" },
                self.audit_cmd(req).switch("update").scoped(&self.session)
            ));
        }
        let data = ExpectData {
            tab: tab.to_owned(),
            assertion,
            expected: format!(
                "no findings of severity {} or more {}",
                req.min_severity.as_str(),
                if req.baseline.is_some() {
                    "that the baseline does not accept"
                } else {
                    "at all"
                }
            ),
            actual: json!(0),
            count: Some(keys.len() as u64),
            elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            retries: 0,
            layout: None,
            perf: None,
            audit: Some(ExpectAudit {
                baseline: state,
                findings: keys,
                accepted,
            }),
        };
        Ok(warnings
            .into_iter()
            .fold(Output::typed(&data, text).quiet("0"), Output::warning))
    }

    /// The command again (`audit` or `expect --audit`), for hints.
    #[allow(clippy::unused_self)]
    fn audit_cmd(&self, req: &AuditRequest) -> Cmd {
        let mut cmd = if req.expect {
            Cmd::new("expect").switch("audit")
        } else {
            Cmd::new("audit")
        };
        if let Some(locator) = &req.locator {
            cmd = cmd.arg(locator);
        }
        if let Some(tab) = &req.tab {
            cmd = cmd.flag("tab", tab);
        }
        if !req.categories.is_empty() {
            cmd = cmd.flag("category", req.categories.join(","));
        }
        if req.min_severity != Severity::Warning {
            cmd = cmd.flag("min-severity", req.min_severity.as_str());
        }
        if let Some(b) = &req.baseline {
            cmd = cmd.flag("baseline", &b.name);
            if let Some(dir) = &b.dir {
                cmd = cmd.flag("dir", dir);
            }
        }
        cmd
    }
}

/// `--audit --baseline checkout --category a11y`, as `expect` names it.
fn assertion(req: &AuditRequest) -> String {
    let mut text = "--audit".to_owned();
    if let Some(locator) = &req.locator {
        text = format!("{locator} {text}");
    }
    if let Some(b) = &req.baseline {
        let _ = write!(text, " --baseline {}", b.name);
    }
    if !req.categories.is_empty() {
        let _ = write!(text, " --category {}", req.categories.join(","));
    }
    if req.min_severity != Severity::Warning {
        let _ = write!(text, " --min-severity {}", req.min_severity.as_str());
    }
    text
}

/// A baseline name for the page at `url`: its path's last part without
/// extension (`checkout`), `home` for `/`.
fn baseline_name(url: &str) -> String {
    let path = page_place(url);
    let last = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let stem = last.split('.').next().unwrap_or_default();
    let name: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let name = name.trim_matches('-');
    if name.is_empty() {
        "home".to_owned()
    } else {
        name.to_owned()
    }
}

/// An issue as a finding: its line's message with what it is about.
fn issue_finding(issue: Issue, page_origin: Option<&str>) -> Finding {
    let mut message = issue.message.clone();
    if let Some(about) = issue_about(&issue, page_origin) {
        let _ = write!(message, "  ({about})");
    }
    Finding {
        category: "issues".to_owned(),
        rule: issue.rule,
        severity: issue.severity,
        message,
        target: issue.target,
        key: issue.key,
        count: issue.count,
        docs: issue.docs,
        wcag: None,
        tab: None,
        accepted: false,
    }
}

/// A `layout` finding as an audit's: its key without the viewport.
fn layout_finding(f: LayoutFinding) -> Finding {
    let mut words = f.key.splitn(3, ' ');
    let (kind, _viewport, rest) = (words.next(), words.next(), words.next());
    let place = rest.unwrap_or_default();
    let element = checks::Described {
        role: f.element.role.clone(),
        name: f.element.name.clone(),
        tag: f.element.tag.clone(),
    }
    .text();
    let target = FindingTarget {
        reference: f.element.reference.clone(),
        element: Some(element),
        frame: f.frame.clone(),
        ..FindingTarget::default()
    };
    let rule = kind.unwrap_or(&f.kind);
    finding("layout", rule, f.message, target, place)
}

/// The first line of a console message for a key: URLs as keys name
/// them (no port, no query), at most [`KEY_MESSAGE`] characters.
fn key_message(message: &str, page_url: &str) -> String {
    let line = message.lines().next().unwrap_or_default();
    let words: Vec<String> = line
        .split(' ')
        .map(|w| {
            if w.contains("://") {
                let start = w.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(0);
                let (before, url) = w.split_at(start);
                format!("{before}{}", url_place(url, page_url))
            } else {
                w.to_owned()
            }
        })
        .collect();
    words.join(" ").chars().take(KEY_MESSAGE).collect()
}

/// Findings with the same key (and tab) folded into one, counts added.
fn fold(findings: Vec<Finding>) -> Vec<Finding> {
    let mut out: Vec<Finding> = Vec::with_capacity(findings.len());
    let mut at: HashMap<(String, Option<String>), usize> = HashMap::new();
    for f in findings {
        let id = (f.key.clone(), f.tab.clone());
        if let Some(&i) = at.get(&id) {
            out[i].count += f.count;
        } else {
            at.insert(id, out.len());
            out.push(f);
        }
    }
    out
}

/// Most severe first, then by category (in the report's order), rule,
/// tab and key.
fn sort(findings: &mut [Finding]) {
    let rank = |c: &str| {
        CATEGORIES
            .iter()
            .position(|x| *x == c)
            .unwrap_or(CATEGORIES.len())
    };
    findings.sort_by(|a, b| {
        (a.severity, rank(&a.category), &a.rule, &a.tab, &a.key).cmp(&(
            b.severity,
            rank(&b.category),
            &b.rule,
            &b.tab,
            &b.key,
        ))
    });
}

/// `2 errors, 3 warnings`, or `no findings`.
fn summary(errors: usize, warnings: usize, info: usize) -> String {
    let parts: Vec<String> = [
        (errors, "error", "errors"),
        (warnings, "warning", "warnings"),
        (info, "info", "info"),
    ]
    .into_iter()
    .filter(|(n, _, _)| *n > 0)
    .map(|(n, one, many)| plural(n, one, many))
    .collect();
    if parts.is_empty() {
        "no findings".to_owned()
    } else {
        parts.join(", ")
    }
}

/// ` (1 info hidden, 2 accepted)`: what the report leaves out.
fn aside(all: &[Finding], min: Severity, accepted: usize) -> String {
    let below = |s: Severity| all.iter().filter(|f| f.severity == s && s > min).count();
    let mut hidden: Vec<String> = [
        (Severity::Warning, "warning", "warnings"),
        (Severity::Info, "info", "info"),
    ]
    .into_iter()
    .filter_map(|(s, one, many)| match below(s) {
        0 => None,
        n => Some(plural(n, one, many)),
    })
    .collect();
    let mut parts = Vec::new();
    if !hidden.is_empty() {
        let last = hidden.pop().unwrap_or_default();
        let list = if hidden.is_empty() {
            last
        } else {
            format!("{} and {last}", hidden.join(", "))
        };
        parts.push(format!("{list} hidden"));
    }
    if accepted > 0 {
        parts.push(format!("{accepted} accepted"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

/// The findings, under a line per severity: `  a11y/image-alt  img [e3]
/// has no alt text  (WCAG 1.1.1 A)`.
fn report(findings: &[Finding], with_tab: bool) -> String {
    let width = findings
        .iter()
        .map(|f| f.category.len() + 1 + f.rule.chars().count())
        .filter(|&n| n <= RULE_COLUMN)
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    let mut last = None;
    for f in findings {
        if last != Some(f.severity) {
            let heading = match f.severity {
                Severity::Error => "errors",
                Severity::Warning => "warnings",
                Severity::Info => "info",
            };
            let _ = writeln!(out, "{heading}");
            last = Some(f.severity);
        }
        let rule = format!("{}/{}", f.category, f.rule);
        let _ = write!(out, "  ");
        if with_tab && let Some(tab) = &f.tab {
            let _ = write!(out, "{tab} ");
        }
        let _ = write!(out, "{rule:<width$}  {}", f.message);
        if f.count > 1 {
            let _ = write!(out, "  ×{}", f.count);
        }
        if let Some(wcag) = &f.wcag {
            let _ = write!(out, "  (WCAG {wcag})");
        }
        if f.accepted {
            out.push_str("  (accepted)");
        }
        out.push('\n');
    }
    out
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

#[cfg(test)]
mod tests {
    use protocol::schema::{FindingTarget, Severity};

    use super::*;

    fn f(category: &str, rule: &str, severity: Severity, key: &str) -> Finding {
        Finding {
            category: category.into(),
            rule: rule.into(),
            severity,
            message: format!("{rule} message"),
            target: FindingTarget::default(),
            key: key.into(),
            count: 1,
            docs: None,
            wcag: None,
            tab: None,
            accepted: false,
        }
    }

    fn request(categories: &[&str], locator: Option<&str>) -> AuditRequest {
        AuditRequest {
            tab: None,
            all_tabs: false,
            locator: locator.map(str::to_owned),
            categories: categories.iter().map(|c| (*c).to_owned()).collect(),
            min_severity: Severity::Warning,
            baseline: None,
            show_accepted: false,
            expect: false,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: 1000,
            max_chars: 0,
        }
    }

    #[test]
    fn categories_default_and_typos() {
        assert_eq!(categories(&request(&[], None)).unwrap(), CATEGORIES);
        assert_eq!(categories(&request(&[], Some("#main"))).unwrap(), SCOPED);
        assert_eq!(
            categories(&request(&["meta,a11y", "issues"], None)).unwrap(),
            ["issues", "a11y", "meta"]
        );
        let e = categories(&request(&["secruity"], None)).unwrap_err();
        assert!(
            e.to_text().contains("did you mean: security"),
            "{}",
            e.to_text()
        );
    }

    #[test]
    fn sorting_folding_and_report() {
        let mut all = fold(vec![
            f("meta", "no-lang", Severity::Warning, "meta/no-lang//"),
            f(
                "console",
                "console-error",
                Severity::Warning,
                "console/console-error/x",
            ),
            f(
                "console",
                "console-error",
                Severity::Warning,
                "console/console-error/x",
            ),
            f("a11y", "image-alt", Severity::Error, "a11y/image-alt/#logo"),
            f(
                "meta",
                "no-description",
                Severity::Info,
                "meta/no-description//",
            ),
        ]);
        sort(&mut all);
        let keys: Vec<&str> = all.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "a11y/image-alt/#logo",
                "console/console-error/x",
                "meta/no-lang//",
                "meta/no-description//"
            ]
        );
        assert_eq!(all[1].count, 2);
        let text = report(&all[..3], false);
        assert_eq!(
            text,
            "errors\n  a11y/image-alt         image-alt message\n\
             warnings\n  console/console-error  console-error message  ×2\n  \
             meta/no-lang           no-lang message\n"
        );
        assert_eq!(summary(1, 2, 0), "1 error, 2 warnings");
        assert_eq!(
            aside(&all, Severity::Warning, 2),
            " (1 info hidden, 2 accepted)"
        );
        assert_eq!(
            aside(&all, Severity::Error, 0),
            " (2 warnings and 1 info hidden)"
        );
    }

    #[test]
    fn names_and_messages_for_keys() {
        assert_eq!(
            baseline_name("http://x.test:81/shop/checkout.html?a=1"),
            "checkout"
        );
        assert_eq!(baseline_name("http://x.test/"), "home");
        assert_eq!(
            key_message(
                "Failed to load http://127.0.0.1:4000/api/x?id=3 and https://cdn.test:8443/a.js\nstack",
                "http://127.0.0.1:4000/"
            ),
            "Failed to load self/api/x and https://cdn.test/a.js"
        );
        assert_eq!(
            assertion(&AuditRequest {
                baseline: Some(crate::request::AuditBaseline {
                    name: "cart".into(),
                    path: "/x/cart.json".into(),
                    update: false,
                    create: false,
                    dir: None,
                }),
                ..request(&["a11y"], None)
            }),
            "--audit --baseline cart --category a11y"
        );
    }
}
