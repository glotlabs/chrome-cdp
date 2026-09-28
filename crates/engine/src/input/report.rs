//! The post-action report (plan 05, milestone 5.5).
//!
//! Before an action sends its input, [`Actor::before`] starts watching
//! the tab's main frame, marks its network and console logs, and marks
//! where focus is. Afterwards, [`Actor::report`] waits for what the input
//! caused, unless `--no-wait-after`: a navigation to a new document is
//! followed to its load event and the settle `goto` runs; otherwise a
//! brief settle (the requests the action started done and the DOM quiet
//! for a moment, at most [`BRIEF_SETTLE`]) lets the page react, and a
//! navigation that starts during it is followed instead. A download the
//! action started gets [`DOWNLOAD_WAIT`] to finish.
//!
//! Then it adds a line for each thing that happened, and only then: a
//! navigation (`→ navigated to …`), a new tab, a dialog answered, a
//! download, a new console error, a failed request to the page's own
//! origin, or focus moving to another form (`! …`). `--json` lists them
//! in `events`. `--snap` appends `snapshot --diff -i`.

use std::time::Duration;

use cdp::domains::runtime;
use protocol::schema::Event;
use protocol::{Cmd, ErrorInfo, Output};
use serde_json::Value;
use tokio::time::Instant;

use super::Actor;
use crate::frames::World;
use crate::injected::{self, GUARD};
use crate::navigate::watch::{Loaded, Watch};
use crate::navigate::{describe as describe_page, error_code};
use crate::request::SnapshotRequest;
use crate::tabs::BLANK;
use crate::wait::NAVIGATION_SETTLE;

/// The longest the brief settle after an action that did not navigate
/// waits.
const BRIEF_SETTLE: Duration = Duration::from_millis(1500);

/// The longest a navigation an action started is waited for (from when
/// the action ends; the action's `--timeout` if shorter). The CLI waits
/// `--timeout` plus 30s for the daemon's answer, so both fit.
const NAVIGATION_WAIT: Duration = Duration::from_secs(20);

/// How long a download the action started gets to finish before the
/// report says it goes on.
const DOWNLOAD_WAIT: Duration = Duration::from_secs(2);

/// How long marking focus may take before the action (skipped if the
/// page has no context yet).
const FOCUS_MARK_WAIT: Duration = Duration::from_secs(2);

/// How many console errors and failed requests the report lists (each);
/// a line counts the rest.
const LISTED: usize = 5;

/// The snapshot `--snap` shows: `snapshot --diff -i`'s text limit.
const SNAP_TEXT_LIMIT: usize = 200;

/// What was marked before an action.
pub(super) struct Before {
    /// When the input began: requests since then are the action's.
    start: Instant,
    /// The main frame, watched from before the input.
    watch: Watch,
    /// The tab's network and console log marks.
    logs: (u64, u64),
    /// The main frame's origin before the action.
    origin: Option<String>,
}

impl Actor<'_> {
    /// Marks the tab before the action's input (see [`Before`]).
    pub(super) async fn before(&self) -> Before {
        let watch = Watch::new(&self.page.session, &self.page.target_id);
        let logs = self.running.tabs.log_marks(self.page.id);
        let origin = self
            .running
            .tabs
            .frames(self.page.id)
            .into_iter()
            .next()
            .and_then(|f| origin(&f.url).map(str::to_owned));
        self.focus_mark().await;
        Before {
            start: Instant::now(),
            watch,
            logs,
            origin,
        }
    }

    /// Tells the page where focus is, for [`Actor::focus_moved`]. A page
    /// without a context yet has nothing focused to report on.
    async fn focus_mark(&self) {
        let wait = FOCUS_MARK_WAIT.min(self.timeout);
        let Ok(context) = self
            .engine
            .frame_context(self.running, self.page, None, World::Isolated, wait)
            .await
        else {
            return;
        };
        let mut params = runtime::CallFunctionOnParams::new(format!(
            "function () {{ {GUARD} globalThis.__ccdp.focusMark(); }}"
        ));
        params.execution_context_id = Some(context.context_id);
        let _ = injected::call(
            &context.session.with_timeout(wait),
            context.context_id,
            params,
        )
        .await;
    }

    /// Waits for what the action caused (unless `--no-wait-after`) and
    /// adds the report to `output`; `mark` is the tab event mark from
    /// before the action. With `--snap`, adds the snapshot of what
    /// changed.
    pub(super) async fn report(
        &self,
        mut output: Output,
        mut before: Before,
        mark: u64,
    ) -> Result<Output, ErrorInfo> {
        let navigated = if self.req.no_wait_after {
            before.watch.drain().await;
            if before.watch.leaving() {
                // Not waited for: where it is going.
                Some(self.navigation(&before, Loaded::TimedOut).await)
            } else {
                self.same_document(&before).await
            }
        } else {
            self.wait_after(&mut before, mark).await?
        };
        let mut events: Vec<Event> = Vec::new();
        let mut warnings = Vec::new();
        let leaving = before.watch.leaving();
        if let Some((event, warning)) = navigated {
            events.push(event);
            warnings.extend(warning);
        }
        events.extend(self.engine.events_since(self.running, mark, None).await);
        let origins = origins(&before, &events);
        events.extend(self.console_errors(&before));
        events.extend(self.failed_requests(&before, &origins, &events));
        if !leaving && let Some(element) = self.focus_moved().await {
            events.push(Event::FocusMoved {
                tab: self.tab(),
                element,
            });
        }
        let lines = self.lines(&events);
        if !lines.is_empty() {
            output.text.push('\n');
            output.text.push_str(&lines.join("\n"));
        }
        output.warnings.extend(warnings);
        output.events.extend(events);
        if self.req.snap {
            output = self.snap(output, mark).await?;
        }
        Ok(output.limit(self.req.max_chars, "--max-chars 0 for all of it"))
    }

    /// Waits for the action's consequences: a navigation it started (to
    /// its load and settle), else a brief settle, then a download it
    /// started. Returns the navigation's event and warning, if any.
    async fn wait_after(
        &self,
        before: &mut Before,
        mark: u64,
    ) -> Result<Option<(Event, Option<String>)>, ErrorInfo> {
        let brief = Instant::now() + BRIEF_SETTLE;
        let settled = tokio::select! {
            settled = self.engine.action_settle(self.running, self.page, before.start, brief) => {
                Some(settled?)
            }
            () = before.watch.until_leaving() => None,
        };
        before.watch.drain().await;
        let navigated = if before.watch.leaving() {
            let deadline = Instant::now() + self.timeout.min(NAVIGATION_WAIT);
            let loaded = before.watch.until_loaded(deadline).await;
            if loaded == Loaded::Load && before.watch.unreachable.is_none() {
                let limit = deadline.min(Instant::now() + NAVIGATION_SETTLE);
                // Not settling is no failure: the page is there.
                let _ = self
                    .engine
                    .settle(self.running, self.page, before.start, limit)
                    .await?;
            }
            if loaded == Loaded::Stopped {
                // A download or an empty answer: the page stayed. Let it
                // react as to any other action.
                if settled.is_none() {
                    let brief = Instant::now() + BRIEF_SETTLE;
                    self.engine
                        .action_settle(self.running, self.page, before.start, brief)
                        .await?;
                }
                None
            } else {
                Some(self.navigation(before, loaded).await)
            }
        } else {
            self.same_document(before).await
        };
        if self.running.tabs.download_began(mark, Duration::ZERO).await {
            let deadline = Instant::now() + DOWNLOAD_WAIT;
            let _ = self.running.tabs.download_done(mark, None, deadline).await;
        }
        Ok(navigated)
    }

    /// The event of a navigation to a new document that ended as
    /// `loaded`, and a warning if it did not load.
    async fn navigation(&self, before: &Before, loaded: Loaded) -> (Event, Option<String>) {
        let watch = &before.watch;
        let session = self.page.session.with_timeout(Duration::from_secs(2));
        if let Some(unreachable) = &watch.unreachable {
            let mut event = self.navigated(unreachable.clone(), String::new(), None, watch, true);
            if let Event::Navigated { error, .. } = &mut event {
                *error = Some(error_code(&self.page.session).await);
            }
            return (event, None);
        }
        let loaded = loaded == Loaded::Load;
        let shown = if watch.committed() {
            describe_page(&session).await.ok()
        } else {
            None
        };
        let (url, title, status) = match shown {
            Some(shown) => {
                let http = shown.url.starts_with("http://") || shown.url.starts_with("https://");
                let status = shown.status.filter(|_| http && !watch.restored);
                (shown.url, shown.title, status)
            }
            // Not committed yet: where it is going.
            None => (
                watch.started_url.clone().unwrap_or_default(),
                String::new(),
                None,
            ),
        };
        let warning = (!loaded).then(|| {
            let wait = if self.req.no_wait_after {
                "not waited for (--no-wait-after)".to_owned()
            } else {
                format!(
                    "did not load within {}",
                    protocol::format::duration(self.timeout.min(NAVIGATION_WAIT))
                )
            };
            format!(
                "the page the action navigated to {wait}; {} waits for it",
                protocol::Cmd::new("wait")
                    .switch("load")
                    .session(self.session())
            )
        });
        let event = self.navigated(url, title, status, watch, loaded);
        (event, warning)
    }

    /// The event of a same-document navigation (`pushState`,
    /// `#fragment`) the action made, if it made one.
    async fn same_document(&self, before: &Before) -> Option<(Event, Option<String>)> {
        if !before.watch.same_document {
            return None;
        }
        let session = self.page.session.with_timeout(Duration::from_secs(2));
        let shown = describe_page(&session).await.ok()?;
        let event = self.navigated(shown.url, shown.title, None, &before.watch, true);
        Some((event, None))
    }

    fn navigated(
        &self,
        url: String,
        title: String,
        status: Option<u16>,
        watch: &Watch,
        loaded: bool,
    ) -> Event {
        Event::Navigated {
            tab: self.tab(),
            url: if url.is_empty() {
                BLANK.to_owned()
            } else {
                url
            },
            title,
            status,
            same_document: watch.same_document && !watch.leaving(),
            restored: watch.restored,
            error: None,
            loaded,
        }
    }

    /// The console errors logged since the action began.
    fn console_errors(&self, before: &Before) -> Vec<Event> {
        self.running
            .tabs
            .console_errors_since(self.page.id, before.logs.1)
            .into_iter()
            .map(|e| Event::ConsoleError {
                tab: self.tab(),
                message: e.message,
                url: e.url,
                line: e.line,
                column: e.column,
                uncaught: e.uncaught,
            })
            .collect()
    }

    /// The requests to one of `origins` that failed since the action
    /// began, except the document of a navigation `events` report (its
    /// status is on the navigation's line).
    fn failed_requests(&self, before: &Before, origins: &[String], events: &[Event]) -> Vec<Event> {
        let navigated_to = events.iter().find_map(|e| match e {
            Event::Navigated { url, .. } => Some(url.as_str()),
            _ => None,
        });
        self.running
            .tabs
            .failures_since(self.page.id, before.logs.0)
            .into_iter()
            .filter(|f| origin(&f.url).is_some_and(|o| origins.iter().any(|x| x == o)))
            .filter(|f| !(f.kind == "Document" && Some(f.url.as_str()) == navigated_to))
            .map(|f| Event::RequestFailed {
                tab: self.tab(),
                id: self
                    .running
                    .tabs
                    .netlog(self.page.id, |log| log.id_of(&f.session, &f.request_id))
                    .flatten(),
                method: f.method,
                url: f.url,
                resource_type: f.kind,
                status: f.status,
                error: f.error,
            })
            .collect()
    }

    /// The element focus moved to in another form (see `focusMoved` in
    /// the injected `report.ts`), named.
    async fn focus_moved(&self) -> Option<protocol::schema::ActedElement> {
        self.page_element("globalThis.__ccdp.focusMoved()")
            .await
            .ok()
            .flatten()
    }

    /// The report's lines: `→ navigated …` first, then `! …` for the rest,
    /// at most [`LISTED`] console errors and failed requests each.
    fn lines(&self, events: &[Event]) -> Vec<String> {
        let mut lines = Vec::new();
        let (mut errors, mut failures) = (0, 0);
        for event in events {
            let count = match event {
                Event::ConsoleError { .. } => Some(&mut errors),
                Event::RequestFailed { .. } => Some(&mut failures),
                _ => None,
            };
            if let Some(count) = count {
                *count += 1;
                if *count > LISTED {
                    continue;
                }
            }
            let line = self.engine.event_line(event);
            lines.push(match event {
                Event::Navigated { .. } => format!("→ {line}"),
                _ => format!("! {line}"),
            });
        }
        for (count, what, console) in [
            (errors, "console errors", true),
            (failures, "failed requests", false),
        ] {
            if count > LISTED {
                let more = count - LISTED;
                let cmd = if console {
                    Cmd::new("errors")
                } else {
                    Cmd::new("network").switch("failed")
                };
                let cmd = cmd.session(&self.engine.session);
                lines.push(format!("! … and {more} more {what} ({cmd} shows them)"));
            }
        }
        lines
    }

    /// `--snap`: adds `snapshot --diff -i` of the tab to `output`, within
    /// what is left of `--max-chars`.
    async fn snap(&self, mut output: Output, mark: u64) -> Result<Output, ErrorInfo> {
        let used = output.text.chars().count() + 1;
        let max_chars = match self.req.max_chars {
            0 => 0,
            max => max.saturating_sub(used).max(1),
        };
        let req = SnapshotRequest {
            tab: None,
            interactive: true,
            scope: None,
            depth: None,
            diff: true,
            all_refs: false,
            all: false,
            text_limit: SNAP_TEXT_LIMIT,
            no_text: false,
            engine: None,
            strict_refs: self.req.strict_refs,
            testid_attribute: self.req.testid_attribute.clone(),
            timeout_ms: self.req.timeout_ms,
            max_chars,
        };
        let snap = self
            .engine
            .snapshot_page(self.running, self.page, &req, mark)
            .await?;
        output.text.push('\n');
        output.text.push_str(&snap.text);
        if let Value::Object(data) = &mut output.data {
            data.insert("snapshot".to_owned(), snap.data);
        }
        output.warnings.extend(snap.warnings);
        Ok(output)
    }
}

/// The origins whose failed requests count: the page's before the
/// action and after it.
fn origins(before: &Before, events: &[Event]) -> Vec<String> {
    let mut origins: Vec<String> = before.origin.iter().cloned().collect();
    let now = events.iter().find_map(|e| match e {
        Event::Navigated { url, .. } => origin(url).map(str::to_owned),
        _ => None,
    });
    origins.extend(now);
    origins
}

/// A URL's origin, `http://x:8080` (`None` for `about:`, `data:`, …).
pub(crate) fn origin(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme, "http" | "https" | "file") {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some(&url[..scheme.len() + 3 + end])
}

#[cfg(test)]
mod tests {
    use super::origin;

    #[test]
    fn origins() {
        assert_eq!(origin("http://x:8080/a?b#c"), Some("http://x:8080"));
        assert_eq!(origin("https://x"), Some("https://x"));
        assert_eq!(origin("about:blank"), None);
        assert_eq!(origin("chrome-error://chromewebdata/"), None);
    }
}
