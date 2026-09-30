//! `issues` (plan 17, milestone 17.1): Chrome's issues about a tab (see
//! [`super::issues`]), read like `console`: a read cursor per tab, so
//! `--since last` (the default) shows only what it has not shown before,
//! `--since` a number, a duration or a mark, `--grep`, `--limit` and
//! `--follow`, with the console's selection (see [`super::commands`]).
//! It reads no page and takes no tab lock, but it asks the page for its
//! form issues first (`Audits.checkFormsIssues`), so those are current.
//!
//! What an issue is about is named here, where waiting is fine: its
//! element gets a ref (as actions give one) and a stable locator, which
//! makes its key; its request, the network log's id.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::time::Duration;

use cdp::domains::audits;
use protocol::schema::{Issue, IssuesData, Severity};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use regex::Regex;
use serde_json::json;
use tokio::time::Instant;

use super::commands::{
    dropped_after, grep_regex, keep_newest, now_ms, shown_url, since_arg, window,
};
use super::issues::{self, Issues, Stored};
use crate::context::Scoped as _;
use crate::input::{describe, origin};
use crate::request::{IssuesRequest, Since, millis};
use crate::snapshot::{NodeKey, Target};
use crate::tabs::{Page, TabError, parse_tab_id, tab_name};
use crate::{Engine, Running};

/// How long naming one issue's element may take.
const NAME_WAIT: Duration = Duration::from_secs(1);

/// How long naming the elements of one read may take in all.
const NAMING_WAIT: Duration = Duration::from_secs(3);

/// How long asking a session for its form issues may take.
const FORMS_WAIT: Duration = Duration::from_secs(2);

/// How wide the rule column may get.
const RULE_COLUMN: usize = 40;

/// What one read picked from a tab's issues.
#[derive(Debug, Default)]
struct Picked {
    stored: Vec<Stored>,
    /// Issues after the start that `--min-severity` hides.
    hidden: usize,
    /// Older matches `--limit` left out.
    skipped: usize,
    /// Issues after the start the buffer dropped.
    dropped: u64,
    /// Where the read started (a sequence number).
    base: u64,
    /// The read cursor after it.
    cursor: u64,
    /// The last issue's number.
    latest: u64,
}

impl Engine {
    /// `issues`: the tab's issues after `--since` (or every tab's, with
    /// `--all-tabs`), at `--min-severity` or above, filtered, the newest
    /// `--limit`. With `wait_ms` and nothing to show, waits that long for
    /// an issue (`--follow`).
    pub async fn issues(&self, mut req: IssuesRequest) -> Result<Output, ErrorInfo> {
        if req.all_tabs && req.tab.is_some() {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                "--all-tabs reads every tab; it takes no --tab",
            )
            .hint(Hint::run(
                Cmd::new("issues").switch("all-tabs").scoped(&self.session),
            )));
        }
        let grep = req.grep.as_deref().map(grep_regex).transpose()?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let ids = if req.all_tabs {
            self.all_tabs(&running)?
        } else {
            let id = req.tab.as_deref().map(parse_tab_id).transpose()?;
            let page = running
                .tabs
                .page(id, timeout)
                .await
                .map_err(|e| self.tab_error(&running, e))?;
            if let Since::Mark { name } = &req.since {
                let marks = running
                    .tabs
                    .named_mark(page.id, name)
                    .map_err(|names| self.no_mark(page.id, name, &names))?;
                req.since = Since::Seq { seq: marks.issues };
            }
            vec![page.id]
        };
        let mut issues = Vec::new();
        let mut picks = Vec::new();
        // A shared or service worker's issue is in each tab of its
        // origin: shown once, in the first.
        let mut seen = HashSet::new();
        for id in ids {
            let page = match running.tabs.page(Some(id), timeout).await {
                Ok(page) => page,
                // Closed meanwhile.
                Err(TabError::NoSuchTab(_)) if req.all_tabs => continue,
                Err(e) => return Err(self.tab_error(&running, e)),
            };
            self.check_forms_issues(&running, page.id, timeout).await;
            let since = match &req.since {
                // Each tab's own mark; a tab without it is read whole.
                Since::Mark { name } => Since::Seq {
                    seq: running.tabs.named_mark(id, name).map_or(0, |m| m.issues),
                },
                since => since.clone(),
            };
            let deadline = Instant::now() + Duration::from_millis(req.wait_ms);
            let mut picked = loop {
                let picked = running
                    .tabs
                    .issues(id, |issues, _| {
                        select(issues, &req, &since, grep.as_ref(), now_ms())
                    })
                    .ok_or_else(|| self.tab_error(&running, TabError::NoSuchTab(id)))?;
                if !picked.stored.is_empty() || Instant::now() >= deadline {
                    break picked;
                }
                if !running.tabs.issue_raised(id, picked.latest, deadline).await {
                    break picked;
                }
            };
            let stored = std::mem::take(&mut picked.stored);
            for issue in self.name_issues(&running, &page, stored).await {
                let once = issue.1;
                if once.is_some_and(|once| !seen.insert(once)) {
                    continue;
                }
                issues.push(issue.0);
            }
            picks.push((page, picked));
        }
        Ok(self.issues_output(&running, &req, issues, &picks))
    }

    /// Asks each page and iframe session of tab `id` for its form issues
    /// (labels, autocomplete, ids) and adds those it has not raised yet.
    pub(crate) async fn check_forms_issues(&self, running: &Running, id: u32, timeout: Duration) {
        let wait = timeout.min(FORMS_WAIT);
        for (session, _) in running.tabs.host_sessions(id) {
            let session_id = session.id().unwrap_or_default().to_owned();
            let found = session
                .with_timeout(wait)
                .call(audits::CheckFormsIssuesParams::default())
                .await;
            // Workers and some frames have none to give.
            let Ok(found) = found else { continue };
            let raw = serde_json::to_value(&found.form_issues).unwrap_or_default();
            let Some(list) = raw.as_array() else { continue };
            running.tabs.issues(id, |issues, page_url| {
                for details in list {
                    let raised = issues::parse(&json!({ "issue": {
                        "code": "GenericIssue",
                        "details": { "genericIssueDetails": details },
                    } }));
                    issues.add_current(raised, &session_id, page_url, issues::CAPACITY);
                }
            });
        }
    }

    /// Tab `page`'s issues of severity `min` or more raised after mark
    /// `after`, named, oldest first (for `expect --no-issues` and the
    /// action report).
    pub(crate) async fn issues_since(
        &self,
        running: &Running,
        page: &Page,
        after: u64,
        min: Severity,
    ) -> Vec<(Issue, Option<(String, u64)>)> {
        let stored = running
            .tabs
            .issues(page.id, |issues, _| issues.since(after, min))
            .unwrap_or_default();
        self.name_issues(running, page, stored).await
    }

    /// `stored` (issues of tab `page`) as shown: the element named (a
    /// ref, a stable locator, and the key made from them, remembered),
    /// the request's id in the network log, the frame left out when it is
    /// the main one. Each with its [`Stored::once`].
    async fn name_issues(
        &self,
        running: &Running,
        page: &Page,
        stored: Vec<Stored>,
    ) -> Vec<(Issue, Option<(String, u64)>)> {
        let tab = tab_name(page.id);
        let main = running
            .tabs
            .frames(page.id)
            .first()
            .map(|f| f.id.clone())
            .unwrap_or_default();
        let page_session = page.session.id().unwrap_or_default();
        let deadline = Instant::now() + NAMING_WAIT;
        let mut out = Vec::with_capacity(stored.len());
        for mut s in stored {
            if !s.named
                && let Some((frame, backend)) = s.node.clone()
                && Instant::now() < deadline
            {
                self.name_element(running, page, &mut s, frame, backend)
                    .await;
            }
            let mut issue = s.issue;
            issue.tab.clone_from(&tab);
            if let Some(request) = &s.request {
                // Chrome raises a worker's request's issue (CORS) on the
                // page's session: the log knows whose request it was.
                let found = running
                    .tabs
                    .netlog(page.id, |log| {
                        let id = log.id_of(&s.session, request).or_else(|| {
                            log.records()
                                .rev()
                                .find(|r| r.request_id == *request)
                                .map(|r| r.entry.id)
                        })?;
                        Some((id, log.get(id)?.entry.worker.clone()))
                    })
                    .flatten();
                if let Some((id, worker)) = found {
                    issue.target.request_id = Some(id);
                    if issue.target.worker.is_none() {
                        issue.target.worker = worker;
                    }
                }
            }
            // An iframe's own session raised it: it is in that frame.
            if issue.target.frame.is_none() && s.session != page_session {
                issue.target.frame = running.tabs.target_of(&s.session);
            }
            if issue.target.frame.as_deref() == Some(main.as_str()) {
                issue.target.frame = None;
            }
            out.push((issue, s.once));
        }
        out
    }

    /// Names the element of issue `stored` (node `backend` of `frame`,
    /// else of the frame of its session that has it): its ref, role and
    /// name, frame, stable locator and key, remembered in the tab's
    /// issues.
    async fn name_element(
        &self,
        running: &Running,
        page: &Page,
        stored: &mut Stored,
        frame: Option<String>,
        backend: i64,
    ) {
        stored.named = true;
        // Chrome names the frame of few: a session's same-site iframes
        // share its node ids, and only the node's own frame resolves it.
        let frames = match frame {
            Some(frame) => vec![frame],
            None => running.tabs.frames_of_session(page.id, &stored.session),
        };
        let generation = running
            .tabs
            .snapshot_memory(page.id, |m| m.refs.generation());
        let mut live = None;
        for frame in frames {
            let key = NodeKey { frame, backend };
            if let Ok(Some(found)) = self.live_node(running, page, &key, NAME_WAIT).await {
                live = Some((key, found));
                break;
            }
        }
        if let Some((key, (context, object_id))) = live {
            stored.issue.target.frame = Some(key.frame.clone());
            let target = Target {
                key,
                context,
                object_id,
                note: None,
            };
            let named = self.name_target(running, page, &target, generation).await;
            target.release();
            let issue = &mut stored.issue;
            issue.target.element = Some(describe(&protocol::schema::ActedElement {
                reference: None,
                ..named.clone()
            }));
            issue.target.reference.clone_from(&named.reference);
            if let Some(reference) = &named.reference {
                let tab = tab_name(page.id);
                // `expect` and actions hold the tab's lock.
                issue.target.locator = self.stable_locator_held(&tab, reference, NAME_WAIT).await;
            }
            let place = issue.target.locator.clone().or_else(|| {
                stored
                    .by_element
                    .then(|| issue.target.element.clone())
                    .flatten()
            });
            if let Some(place) = place {
                issue.key = format!("issues/{}/{place}", issue.rule);
            }
        }
        let seq = stored.issue.seq;
        let named = stored.issue.clone();
        running.tabs.issues(page.id, |issues, _| {
            if let Some(kept) = issues.get_mut(seq) {
                kept.named = true;
                kept.issue.target = named.target;
                kept.issue.key = named.key;
            }
        });
    }

    fn issues_output(
        &self,
        running: &Running,
        req: &IssuesRequest,
        issues: Vec<Issue>,
        picks: &[(Page, Picked)],
    ) -> Output {
        let single = picks.first().filter(|_| !req.all_tabs);
        let page_origin = single.and_then(|(page, _)| {
            let frames = running.tabs.frames(page.id);
            frames
                .first()
                .and_then(|f| origin(&f.url).map(str::to_owned))
        });
        let tab = single
            .map(|(page, _)| tab_name(page.id))
            .unwrap_or_default();
        let width = issues
            .iter()
            .map(|i| i.rule.chars().count())
            .filter(|&n| n <= RULE_COLUMN)
            .max()
            .unwrap_or(0);
        let mut text = String::new();
        if !issues.is_empty() {
            let _ = writeln!(text, "{}", header(&issues, req, picks));
        }
        for issue in &issues {
            let _ = writeln!(
                text,
                "{}",
                issue_line(issue, page_origin.as_deref(), width, req.all_tabs)
            );
        }
        let hidden: usize = picks.iter().map(|(_, p)| p.hidden).sum();
        let skipped: usize = picks.iter().map(|(_, p)| p.skipped).sum();
        let dropped: u64 = picks.iter().map(|(_, p)| p.dropped).sum();
        if skipped > 0 {
            let mut cmd = self
                .issues_cmd(req)
                .flag("limit", (req.limit + skipped).to_string());
            if let Some((_, picked)) = single {
                cmd = cmd.flag("since", since_arg(&req.since, picked.base));
            }
            let _ = writeln!(
                text,
                "… {skipped} earlier {} not shown; {cmd} shows them",
                if skipped == 1 { "issue" } else { "issues" },
            );
        }
        if issues.is_empty() {
            let new = if req.since == Since::Last { "new " } else { "" };
            let place = if req.all_tabs {
                "any tab".to_owned()
            } else {
                tab.clone()
            };
            let _ = write!(text, "no {new}issues in {place}");
            if req.since == Since::Last && picks.iter().any(|(_, p)| p.base > 0) {
                let cmd = self.issues_cmd(req).flag("since", "0");
                let _ = write!(text, "; {cmd} shows the earlier ones");
            }
            text.push('\n');
        }
        if hidden > 0 && req.min_severity != Severity::Info {
            let cmd = self.issues_cmd(req).flag("min-severity", "info");
            let cmd = match single {
                Some((_, picked)) => cmd.flag("since", since_arg(&req.since, picked.base)),
                None => cmd,
            };
            let _ = writeln!(
                text,
                "{hidden} less severe {} hidden; {cmd} shows them",
                if hidden == 1 { "issue" } else { "issues" },
            );
        }
        let quiet: Vec<String> = issues
            .iter()
            .map(|i| format!("{} {}", i.rule, i.message))
            .collect();
        let cursor = single.map_or(0, |(_, p)| p.cursor);
        let mut output = Output::typed(
            &IssuesData {
                tab: tab.clone(),
                issues,
                hidden,
                skipped,
                dropped,
                cursor,
            },
            text.trim_end().to_owned(),
        )
        .quiet(quiet.join("\n"));
        if dropped > 0 {
            output = output.warning(format!(
                "{dropped} {} dropped from the buffer before they were read; a tab keeps the \
                 newest {} distinct issues",
                if dropped == 1 {
                    "issue was"
                } else {
                    "issues were"
                },
                issues::CAPACITY,
            ));
        }
        output.limit(
            req.max_chars,
            "--max-chars 0 for all of it, or --limit N for fewer",
        )
    }

    /// `issues` with the flags of `req` that pick issues (and the
    /// session), for hints.
    fn issues_cmd(&self, req: &IssuesRequest) -> Cmd {
        let mut cmd = Cmd::new("issues");
        if let Some(tab) = &req.tab {
            cmd = cmd.flag("tab", tab);
        }
        if req.all_tabs {
            cmd = cmd.switch("all-tabs");
        }
        if let Some(grep) = &req.grep {
            cmd = cmd.flag("grep", grep);
        }
        cmd.scoped(&self.session)
    }
}

/// The issues of `issues` that `req` asks for (from `since`, its mark
/// resolved), and moves the read cursor past them.
fn select(
    issues: &mut Issues,
    req: &IssuesRequest,
    since: &Since,
    grep: Option<&Regex>,
    now_ms: u64,
) -> Picked {
    let (base, since_ms) = window(since, issues.cursor, now_ms);
    let matches = |s: &&Stored| {
        let i = &s.issue;
        s.issue.seq > base
            && i.time_ms >= since_ms
            && grep.is_none_or(|re| {
                re.is_match(&i.rule)
                    || re.is_match(&i.message)
                    || i.target.url.as_deref().is_some_and(|u| re.is_match(u))
            })
    };
    let mut hidden = 0;
    let mut stored: Vec<Stored> = issues
        .entries()
        .filter(matches)
        .filter(|s| {
            let shown = s.issue.severity <= req.min_severity;
            hidden += usize::from(!shown);
            shown
        })
        .cloned()
        .collect();
    let skipped = keep_newest(&mut stored, req.limit);
    let dropped = dropped_after(since, issues.evicted(), base);
    let shown = stored.last().map_or(0, |s| s.issue.seq);
    issues.cursor = issues.cursor.max(shown);
    Picked {
        stored,
        hidden,
        skipped,
        dropped,
        base,
        cursor: issues.cursor,
        latest: issues.seq(),
    }
}

/// `3 issues (1 error, 2 warnings) since mark "cart"`.
fn header(issues: &[Issue], req: &IssuesRequest, picks: &[(Page, Picked)]) -> String {
    let count = |severity| issues.iter().filter(|i| i.severity == severity).count();
    let kinds: Vec<String> = [
        (Severity::Error, "error", "errors"),
        (Severity::Warning, "warning", "warnings"),
        (Severity::Info, "info", "info"),
    ]
    .into_iter()
    .filter_map(|(severity, one, many)| match count(severity) {
        0 => None,
        1 => Some(format!("1 {one}")),
        n => Some(format!("{n} {many}")),
    })
    .collect();
    let n = issues.len();
    let new = if req.since == Since::Last && picks.iter().any(|(_, p)| p.base > 0) {
        "new "
    } else {
        ""
    };
    let what = if n == 1 { "issue" } else { "issues" };
    let when = match &req.since {
        Since::Last | Since::Seq { seq: 0 } => String::new(),
        Since::Seq { seq } => format!(" after #{seq}"),
        Since::Ago { ms } => format!(
            " in the last {}",
            protocol::format::duration(Duration::from_millis(*ms))
        ),
        Since::Mark { name } => format!(" since mark {name:?}"),
    };
    let tabs = if req.all_tabs {
        let tabs: HashSet<&str> = issues.iter().map(|i| i.tab.as_str()).collect();
        let n = tabs.len();
        format!(" in {n} {}", if n == 1 { "tab" } else { "tabs" })
    } else {
        String::new()
    };
    format!("{n} {new}{what} ({}){when}{tabs}", kinds.join(", "))
}

/// An issue's line: `E mixed-content-blocked  script http://… blocked on
/// an https: page  ×1`, what it is about in parentheses, with its tab
/// first for `--all-tabs`.
pub(crate) fn issue_line(
    issue: &Issue,
    page_origin: Option<&str>,
    width: usize,
    with_tab: bool,
) -> String {
    let mut line = String::new();
    if with_tab {
        let _ = write!(line, "{} ", issue.tab);
    }
    let _ = write!(line, "{} {:<width$}  ", issue.severity.letter(), issue.rule);
    if let Some(worker) = &issue.target.worker {
        let _ = write!(line, "{} ", worker.label());
    }
    line.push_str(&issue.message);
    let mut about = Vec::new();
    if let Some(element) = &issue.target.element {
        about.push(match &issue.target.reference {
            Some(reference) => format!("{reference} {element}"),
            None => element.clone(),
        });
    }
    if let Some(source) = &issue.target.source {
        about.push(format!(
            "{}:{}",
            shown_url(&source.url, page_origin),
            source.line
        ));
    }
    if let Some(id) = issue.target.request_id {
        about.push(format!("request #{id}"));
    }
    if !about.is_empty() {
        let _ = write!(line, "  ({})", about.join(", "));
    }
    let _ = write!(line, "  ×{}", issue.count);
    line
}

#[cfg(test)]
mod tests {
    use protocol::schema::{FindingTarget, Issue, Severity, SourceLine};
    use serde_json::json;

    use super::{issue_line, select};
    use crate::observe::issues::{Issues, parse};
    use crate::request::{IssuesRequest, Since};

    fn request() -> IssuesRequest {
        IssuesRequest {
            tab: None,
            all_tabs: false,
            since: Since::Last,
            grep: None,
            limit: 50,
            min_severity: Severity::Warning,
            wait_ms: 0,
            timeout_ms: 1000,
            max_chars: 0,
        }
    }

    fn quirks(url: &str) -> serde_json::Value {
        json!({ "issue": { "code": "QuirksModeIssue", "details": { "quirksModeIssueDetails": {
            "isLimitedQuirksMode": false, "documentNodeId": 1, "url": url, "frameId": "F", "loaderId": "L",
        } } } })
    }

    fn deprecation(line: u32) -> serde_json::Value {
        json!({ "issue": { "code": "DeprecationIssue", "details": { "deprecationIssueDetails": {
            "type": "UnloadHandler",
            "sourceCodeLocation": { "url": "http://x.test/app.js", "lineNumber": line, "columnNumber": 0 },
        } } } })
    }

    #[test]
    fn cursors_severities_and_limits() {
        let mut issues = Issues::default();
        let page = "http://x.test/";
        issues.add(parse(&quirks("http://x.test/q")), "S", page, None, 100);
        issues.add(parse(&deprecation(1)), "S", page, None, 100);
        issues.add(parse(&deprecation(2)), "S", page, None, 100);
        let req = request();
        let picked = select(&mut issues, &req, &req.since, None, u64::MAX);
        // Quirks mode is info: hidden at the default --min-severity.
        assert_eq!(picked.stored.len(), 2);
        assert_eq!(picked.hidden, 1);
        assert_eq!(picked.cursor, 3);
        // Each is shown once.
        let again = select(&mut issues, &req, &req.since, None, u64::MAX);
        assert!(again.stored.is_empty());
        // Raised again: shown again, counted.
        issues.add(parse(&deprecation(1)), "S", page, None, 100);
        let again = select(&mut issues, &req, &req.since, None, u64::MAX);
        assert_eq!(again.stored.len(), 1);
        assert_eq!(again.stored[0].issue.count, 2);
        // --since 0 --limit 1 --min-severity info.
        let all = IssuesRequest {
            since: Since::Seq { seq: 0 },
            limit: 1,
            min_severity: Severity::Info,
            ..request()
        };
        let picked = select(&mut issues, &all, &all.since, None, u64::MAX);
        assert_eq!(picked.skipped, 2);
        assert_eq!(picked.stored[0].issue.seq, 4);
        let re = regex::Regex::new("quirks").unwrap();
        let picked = select(&mut issues, &all, &all.since, Some(&re), u64::MAX);
        assert_eq!(picked.stored[0].issue.rule, "quirks-mode");
    }

    #[test]
    fn lines() {
        let issue = Issue {
            seq: 3,
            time_ms: 0,
            tab: "t1".into(),
            category: "issues".into(),
            rule: "deprecation-unload-handler".into(),
            severity: Severity::Warning,
            code: "DeprecationIssue".into(),
            message: "unload handler is deprecated".into(),
            target: FindingTarget {
                source: Some(SourceLine {
                    url: "http://x.test/app.js".into(),
                    line: 12,
                    column: 1,
                }),
                ..FindingTarget::default()
            },
            key: "issues/deprecation-unload-handler/self/app.js:12".into(),
            count: 4,
            docs: None,
        };
        assert_eq!(
            issue_line(&issue, Some("http://x.test"), 28, false),
            "W deprecation-unload-handler    unload handler is deprecated  (/app.js:12)  ×4"
        );
        let element = Issue {
            rule: "form-input-with-no-label".into(),
            severity: Severity::Warning,
            message: "a form field has no label".into(),
            target: FindingTarget {
                reference: Some("e4".into()),
                element: Some("textbox \"Email\"".into()),
                request_id: Some(7),
                ..FindingTarget::default()
            },
            count: 1,
            ..issue
        };
        assert_eq!(
            issue_line(&element, None, 0, true),
            "t1 W form-input-with-no-label  a form field has no label  (e4 textbox \"Email\", request #7)  ×1"
        );
    }
}
