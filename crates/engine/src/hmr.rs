//! Dev-server awareness (plan 15, milestone 15.6): does the page have
//! the agent's last edit?
//!
//! Each tab tracks its dev server's hot updates (see
//! [`crate::observe::hmr`]). This module says what they mean to a
//! command: `wait --hmr` waits for one, an action's report lists those
//! that came while it ran (`! hot update applied (src/cart.ts)`), and
//! any command on a tab warns once about those that came since the tab's
//! previous command: the agent edited a file between commands, and the
//! warning says whether the page caught up. Each outcome is reported
//! once: a tab keeps the number of the last one a command reported.
//!
//! The warnings of a call are collected while it runs ([`noting`], which
//! the daemon wraps each call in) and added to its output at the end, so
//! no command has to add them itself.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::future::Future;
use std::path::Path;
use std::time::Duration;

use protocol::schema::{Event, HotFile, HotOutcome, HotUpdate, SourcePosition};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};
use serde_json::json;
use tokio::time::Instant;

use crate::context::Scoped as _;
use crate::observe::hmr::{Client, Outcome};
use crate::places;
use crate::tabs::{Page, tab_name};
use crate::wait::{Met, Unmet};
use crate::{Engine, Running};

/// How many files an outcome's line names; the rest are counted.
const LISTED_FILES: usize = 3;

/// How long the updates must stay quiet before `wait --hmr` answers
/// with the latest: one save can make two rounds (an editor that
/// truncates, then writes), and the second is the page's state.
const QUIET: Duration = Duration::from_millis(200);

/// The same while a Vite update's other files are still being applied
/// (the client applies them one after the other).
const PENDING_GRACE: Duration = Duration::from_millis(500);

tokio::task_local! {
    /// The hot updates the call being handled reports as warnings.
    static NOTICES: RefCell<Notices>;
}

/// What a call reports besides its own output: the hot updates that
/// reached its tab since the tab's previous command.
#[derive(Debug, Default)]
pub struct Notices {
    /// One line each, for the output's warnings (or an error's notes).
    pub warnings: Vec<String>,
    /// The same as `hot_update` events.
    pub events: Vec<Event>,
}

impl Notices {
    pub fn is_empty(&self) -> bool {
        self.warnings.is_empty()
    }
}

/// Runs `fut` (a call's handling) and returns what it noted besides its
/// output ([`Notices`]).
pub async fn noting<F: Future>(fut: F) -> (F::Output, Notices) {
    NOTICES
        .scope(RefCell::new(Notices::default()), async {
            let output = fut.await;
            let notices = NOTICES.with(RefCell::take);
            (output, notices)
        })
        .await
}

/// Notes, for the call being handled, the hot updates that reached tab
/// `page` since a command last reported them. Outside [`noting`] (a
/// test calling the engine) it does nothing and they stay unreported.
pub(crate) fn note_since_previous(running: &Running, page: u32) {
    if NOTICES.try_with(|_| ()).is_err() {
        return;
    }
    let outcomes = running.tabs.report_hot_updates(page, 0);
    if outcomes.is_empty() {
        return;
    }
    let tab = tab_name(page);
    let _ = NOTICES.try_with(|notices| {
        let mut notices = notices.borrow_mut();
        for outcome in &outcomes {
            let update = update(outcome);
            notices.warnings.push(line(&update));
            notices.events.push(Event::HotUpdate {
                tab: tab.clone(),
                update,
            });
        }
    });
}

/// Drops what the call noted so far: `wait --hmr` shows the update
/// itself.
fn forget_notes() {
    let _ = NOTICES.try_with(RefCell::take);
}

/// The `hot_update` event of tab `tab`'s `outcome`, its files found on
/// disk under the call's project root.
pub(crate) fn event(tab: &str, outcome: &Outcome) -> Event {
    Event::HotUpdate {
        tab: tab.to_owned(),
        update: update(outcome),
    }
}

/// `outcome` as the schema has it.
pub(crate) fn update(outcome: &Outcome) -> HotUpdate {
    let root = places::root();
    let on_disk = |source: &str| {
        places::to_disk(source, root.as_deref()).map(|f| f.to_string_lossy().into_owned())
    };
    HotUpdate {
        client: outcome.client.name().to_owned(),
        outcome: outcome.outcome,
        files: outcome
            .files
            .iter()
            .map(|source| HotFile {
                source: source.clone(),
                file: on_disk(source),
            })
            .collect(),
        message: outcome.message.clone(),
        place: outcome
            .place
            .as_ref()
            .map(|(source, line, column)| SourcePosition {
                source: source.clone(),
                line: *line,
                column: *column,
                name: None,
                file: on_disk(source),
            }),
    }
}

/// The line that reports a hot update: `hot update applied
/// (src/cart.ts)`, `hot update failed: <message> (src/cart.ts:12)` or
/// `full reload (src/main.ts)`.
pub(crate) fn line(update: &HotUpdate) -> String {
    let root = places::root();
    let shown = |source: &str, file: Option<&str>| match file {
        Some(file) => places::shown(Path::new(file), root.as_deref()),
        None => source.to_owned(),
    };
    let mut files: Vec<String> = update
        .files
        .iter()
        .take(LISTED_FILES)
        .map(|f| shown(&f.source, f.file.as_deref()))
        .collect();
    if update.files.len() > LISTED_FILES {
        let more = update.files.len() - LISTED_FILES;
        if let Some(last) = files.last_mut() {
            let _ = write!(last, " and {more} more");
        }
    }
    if let Some(place) = &update.place {
        files = vec![format!(
            "{}:{}",
            shown(&place.source, place.file.as_deref()),
            place.line
        )];
    }
    let files = if files.is_empty() {
        String::new()
    } else {
        format!(" ({})", files.join(", "))
    };
    match update.outcome {
        HotOutcome::Applied => format!("hot update applied{files}"),
        HotOutcome::Failed => match &update.message {
            Some(message) => format!("hot update failed: {message}{files}"),
            None => format!("hot update failed{files}"),
        },
        HotOutcome::FullReload => format!("full reload{files}"),
    }
}

impl Engine {
    /// `wait --hmr` in `page`: the latest hot update since the previous
    /// command began (and newer than the last one a `wait --hmr`
    /// returned), once it settled (a full reload's page loaded), unless
    /// another began after it; else the next one, up to `deadline`. A failed update fails the wait with its error, and a
    /// page with no hot-reload client fails at once.
    pub(crate) async fn hmr_until(
        &self,
        running: &Running,
        page: &Page,
        deadline: Instant,
    ) -> Result<Result<Met, Unmet>, ErrorInfo> {
        forget_notes();
        // Since the previous command began, but not one a `wait --hmr`
        // returned already (after a failure, the next wait is for the fix).
        let waited = running.tabs.hot_updates(page.id, |h| h.waited).unwrap_or(0);
        let after = running.tabs.previous_hmr_mark(page.id).max(waited);
        // The outcome that may be the answer, and until when it must stay
        // the latest.
        let mut candidate: Option<(u64, Instant)> = None;
        loop {
            let seen = running
                .tabs
                .hot_updates(page.id, |h| {
                    let latest = h.since(after).last().cloned();
                    let newer = match &latest {
                        Some(latest) => h.building_after(latest),
                        None => h.building().is_some(),
                    };
                    let pending = h.building().map(|b| (b.client, b.pending.clone()));
                    (latest, newer, pending, h.client(), h.changes())
                })
                .ok_or_else(|| {
                    self.tab_error(running, crate::tabs::TabError::NoSuchTab(page.id))
                })?;
            let (latest, newer, pending, client, changes) = seen;
            let mut at = None;
            match &latest {
                Some(outcome) if !newer && outcome.settled() => {
                    let now = Instant::now();
                    let until = match candidate {
                        Some((seq, until)) if seq == outcome.seq => until,
                        _ => {
                            // A Vite update whose other files are still
                            // being applied gets longer.
                            let pending = outcome.outcome == HotOutcome::Applied
                                && pending.as_ref().is_some_and(|(_, p)| !p.is_empty());
                            let until = now + if pending { PENDING_GRACE } else { QUIET };
                            candidate = Some((outcome.seq, until));
                            until
                        }
                    };
                    if now >= until {
                        return self.hot_update_met(running, page, outcome).map(Ok);
                    }
                    at = Some(until);
                }
                _ => candidate = None,
            }
            if latest.is_none() && pending.is_none() && client.is_none() {
                return Err(self.no_client(running, page));
            }
            if !running
                .tabs
                .hot_updates_changed(page.id, changes, at, deadline)
                .await
            {
                // Out of time while it stayed the latest: it is the answer.
                if let Some(outcome) = &latest
                    && candidate.is_some_and(|(seq, _)| seq == outcome.seq)
                {
                    return self.hot_update_met(running, page, outcome).map(Ok);
                }
                return Ok(Err(hmr_unmet(latest.as_ref(), pending, client)));
            }
        }
    }

    /// What `wait --hmr` returns for `outcome`: met, or the failure.
    fn hot_update_met(
        &self,
        running: &Running,
        page: &Page,
        outcome: &Outcome,
    ) -> Result<Met, ErrorInfo> {
        running.tabs.hot_updates(page.id, |h| {
            h.reported = h.reported.max(outcome.seq);
            h.waited = h.waited.max(outcome.seq);
        });
        let update = update(outcome);
        let text = line(&update);
        if update.outcome != HotOutcome::Failed {
            return Ok(Met {
                text,
                hot_update: Some(update),
                ..Met::default()
            });
        }
        let mut info =
            ErrorInfo::new(ErrorKind::HotUpdateFailed, text).note("client", update.client.clone());
        if let Some(place) = &update.place {
            let root = places::root();
            let file = match &place.file {
                Some(file) => places::shown(Path::new(file), root.as_deref()),
                None => place.source.clone(),
            };
            info = info.note("at", format!("{file}:{}:{}", place.line, place.column));
        }
        let mut again = Cmd::new("wait").switch("hmr");
        if running.tabs.active() != Some(page.id) {
            again = again.flag("tab", tab_name(page.id));
        }
        Err(info
            .hint(
                Hint::run(again.scoped(&self.session))
                    .why("after you fix it and save, waits for the next update"),
            )
            .details(json!({ "hot_update": update })))
    }

    /// The error of `wait --hmr` on a page with no hot-reload client.
    fn no_client(&self, running: &Running, page: &Page) -> ErrorInfo {
        let url = running
            .tabs
            .frames(page.id)
            .into_iter()
            .next()
            .map(|f| f.url)
            .unwrap_or_default();
        ErrorInfo::new(
            ErrorKind::NotFound,
            "no hot-reload client on this page (not a Vite, webpack or Next.js dev server?)",
        )
        .note("page", url.clone())
        .hint(Hint::text(format!(
            "open the app from its dev server (npm run dev), or wait for what the edit changes: {}",
            Cmd::new("wait").flag("text", "…").scoped(&self.session)
        )))
        .details(json!({ "reason": "no_client", "url": url }))
    }
}

/// What a `wait --hmr` that timed out saw.
fn hmr_unmet(
    latest: Option<&Outcome>,
    pending: Option<(Client, Vec<String>)>,
    client: Option<Client>,
) -> Unmet {
    let unmet = Unmet::default().observed("client", json!(client.map(Client::name)));
    if let Some((client, files)) = pending {
        let files = if files.is_empty() {
            String::new()
        } else {
            format!(" of {}", files.join(", "))
        };
        return unmet
            .note(
                "observed",
                format!(
                    "{} began a hot update{files}; it did not end",
                    client.name()
                ),
            )
            .observed("building", json!(true));
    }
    match latest {
        Some(outcome) if !outcome.settled() => unmet
            .note(
                "observed",
                format!(
                    "{} began a full reload; the page did not load",
                    outcome.client.name()
                ),
            )
            .observed("reloading", json!(true)),
        _ => unmet.note(
            "observed",
            "no hot update since the previous command began; save the file first",
        ),
    }
}
