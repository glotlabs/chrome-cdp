//! `wait --console REGEX` and `wait --binding NAME` (plan 14, milestone
//! 14.5): the first console entry of the tab, logged since the previous
//! command began (as `--request` counts requests), whose text or URL
//! matches; for `--binding`, a call of that exposed function (its
//! `binding NAME(…)` entry). Event-driven on the tab's console; it takes
//! no tab lock.

use std::collections::VecDeque;

use protocol::schema::ConsoleEntry;
use regex::Regex;
use serde_json::json;
use tokio::time::Instant;

use super::{Met, Unmet};
use crate::observe::entry_text;
use crate::tabs::{Page, TabError};
use crate::{Engine, Running};

/// How many of the entries that did not match a timeout lists.
const LISTED: usize = 5;

impl Engine {
    /// Waits until `page`'s console has a matching entry, up to
    /// `deadline`; `Err` lists the last entries seen.
    pub(super) async fn console_until(
        &self,
        running: &Running,
        page: &Page,
        regex: &Regex,
        binding: Option<&str>,
        deadline: Instant,
    ) -> Result<Result<Met, Unmet>, protocol::ErrorInfo> {
        let wanted = |e: &ConsoleEntry| match binding {
            Some(name) => e.source == "binding" && e.kind.as_deref() == Some(name),
            None => regex.is_match(&e.text) || e.url.as_deref().is_some_and(|u| regex.is_match(u)),
        };
        let mut checked = running.tabs.previous_console_mark(page.id);
        let mut seen: VecDeque<ConsoleEntry> = VecDeque::new();
        loop {
            let found = running
                .tabs
                .console(page.id, |console| {
                    let mut found = None;
                    for entry in console.entries().filter(|e| e.seq > checked) {
                        if wanted(entry) {
                            found = Some(entry.clone());
                            break;
                        }
                        if seen.len() == LISTED {
                            seen.pop_front();
                        }
                        seen.push_back(entry.clone());
                    }
                    checked = found.as_ref().map_or(console.seq(), |e| e.seq);
                    found
                })
                .ok_or_else(|| self.tab_error(running, TabError::NoSuchTab(page.id)))?;
            if let Some(entry) = found {
                return Ok(Ok(Met {
                    text: entry.text.lines().next().unwrap_or_default().to_owned(),
                    console: Some(entry),
                    ..Met::default()
                }));
            }
            if !running
                .tabs
                .console_logged(page.id, checked, deadline)
                .await
            {
                return Ok(Err(unmet(page, running, &seen)));
            }
        }
    }
}

/// What a timeout saw: the last entries logged, or that there were none.
fn unmet(page: &Page, running: &Running, seen: &VecDeque<ConsoleEntry>) -> Unmet {
    let unmet = Unmet::default().observed("entries", json!(seen));
    if seen.is_empty() {
        return unmet.note(
            "observed",
            "no console entries since the previous command began",
        );
    }
    let url = running
        .tabs
        .frames(page.id)
        .into_iter()
        .next()
        .map(|f| f.url)
        .unwrap_or_default();
    let page_origin = crate::input::origin(&url);
    seen.iter().fold(unmet, |unmet, entry| {
        let mut line = String::new();
        entry_text(&mut line, entry, page_origin);
        let first = line.lines().next().unwrap_or_default().to_owned();
        unmet.note("seen", first)
    })
}
