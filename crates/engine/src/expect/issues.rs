//! `expect --no-issues` (plan 17, milestone 17.1): no issue of Chrome's
//! (see `crate::observe::issues`) at `--min-severity` or above since the
//! page loaded, or since a `mark`, or with `--all-tabs` in any tab since
//! it opened. Like `--no-errors`, it looks once: issues are recorded,
//! not polled. It asks the page for its form issues first.

use std::collections::HashSet;
use std::time::Duration;

use protocol::schema::{Event, Severity};
use protocol::{Cmd, ErrorInfo, Hint, Output};
use serde_json::json;
use tokio::time::Instant;

use super::page::{Found, LISTED, event_tab};
use super::{Failed, Passed, expected};
use crate::context::Scoped as _;
use crate::request::{ExpectRequest, millis};
use crate::tabs::{Page, parse_tab_id, tab_name};
use crate::{Engine, Running};

impl Engine {
    /// `--no-issues`: none of `min` or more since the page loaded (or
    /// mark `since`).
    pub(super) async fn no_issues(
        &self,
        running: &Running,
        page: &Page,
        req: &ExpectRequest,
        since: Option<&str>,
        min: Severity,
        timeout: Duration,
    ) -> Result<Passed, ErrorInfo> {
        let (after, when) = match since {
            None => (
                running
                    .tabs
                    .issues(page.id, |issues, _| issues.document())
                    .unwrap_or(0),
                "since the page loaded".to_owned(),
            ),
            Some(name) => match running.tabs.named_mark(page.id, name) {
                Ok(marks) => (marks.issues, format!("since mark {name:?}")),
                Err(names) => return Err(self.no_mark(page.id, name, &names)),
            },
        };
        self.check_forms_issues(running, page.id, timeout).await;
        let issues = self.issues_since(running, page, after, min).await;
        if issues.is_empty() {
            return Ok(Passed {
                actual: json!(0),
                quiet: "0".into(),
                count: Some(0),
                retries: 0,
            });
        }
        let count = issues.len();
        let events: Vec<Event> = issues
            .into_iter()
            .map(|(issue, _)| Event::Issue(Box::new(issue)))
            .collect();
        let mut failed = Failed {
            expected: format!("{} {when}", expected(req, None)),
            actual: plural(count),
            ..Failed::default()
        };
        for event in events.iter().take(LISTED) {
            failed.notes.push(("issue".into(), self.issue_note(event)));
        }
        if count > LISTED {
            failed
                .notes
                .push(("issue".into(), format!("… and {} more", count - LISTED)));
        }
        failed.details.insert("issues".into(), json!(events));
        let mut cmd = Cmd::new("issues").flag("since", after.to_string());
        if running.tabs.active() != Some(page.id) {
            cmd = cmd.flag("tab", tab_name(page.id));
        }
        if min != Severity::Warning {
            cmd = cmd.flag("min-severity", min.as_str());
        }
        failed.hint =
            Some(Hint::run(cmd.scoped(&self.session)).why("shows them with what they are about"));
        Err(self.one_shot_failure(page, req, failed))
    }

    /// `--no-issues --all-tabs`: none of `min` or more in any tab of the
    /// session (or context) since it opened.
    pub(super) async fn no_issues_in_all_tabs(
        &self,
        req: &ExpectRequest,
        min: Severity,
    ) -> Result<Output, ErrorInfo> {
        let start = Instant::now();
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let tabs = self.all_tabs(&running)?;
        // A shared or service worker's issue is in each tab of its
        // origin: counted once, in the first.
        let mut seen = HashSet::new();
        let mut events = Vec::new();
        for id in tabs {
            let Ok(page) = running.tabs.page(Some(id), timeout).await else {
                continue;
            };
            self.check_forms_issues(&running, id, timeout).await;
            for (issue, once) in self.issues_since(&running, &page, 0, min).await {
                if once.is_some_and(|once| !seen.insert(once)) {
                    continue;
                }
                events.push(Event::Issue(Box::new(issue)));
            }
        }
        let hint = events.first().map(|first| {
            let tab = event_tab(first);
            let mut cmd = Cmd::new("issues").flag("since", "0");
            if running.tabs.active() != parse_tab_id(tab).ok() {
                cmd = cmd.flag("tab", tab);
            }
            Hint::run(cmd.scoped(&self.session)).why("shows them with what they are about")
        });
        let found = Found {
            events,
            one: "issue",
            many: "issues",
            label: "issue",
            prefix: "issue: ",
            key: "issues",
            hint,
        };
        self.in_all_tabs(req, start, found)
    }

    /// An issue's note in a failure: `E mixed-content-blocked script
    /// http://… blocked on an https: page (×2)`.
    fn issue_note(&self, event: &Event) -> String {
        let line = self.event_line(event);
        let line = line.strip_prefix("issue: ").unwrap_or(&line);
        match event {
            Event::Issue(issue) if issue.count > 1 => format!("{line} (×{})", issue.count),
            _ => line.to_owned(),
        }
    }
}

fn plural(count: usize) -> String {
    if count == 1 {
        "1 issue".to_owned()
    } else {
        format!("{count} issues")
    }
}
