//! `cdp --listen --all-sessions` (plan 13, milestone 13.4): the events of
//! every session of a tab, its OOPIFs' and workers' too.
//!
//! A [`Tap`] subscribes to each session the tab has when it is made, and
//! `attached` subscribes it to every session of the tab that attaches
//! while it lives, before the target is set up and resumed, so it misses
//! none of a new target's events. Each event comes with the session it
//! came from and a name for it (`main`, `iframe "pay"`, `dedicated worker
//! w.js`), looked up when it arrives.

use std::sync::{Arc, Mutex};

use cdp::{RawEvent, Session};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use super::targets::{Target, TargetKind};
use super::{Shared, State, Tabs, lock};
use crate::frames::iframe_label;

/// One event a [`Tap`] heard.
#[derive(Debug)]
pub(crate) struct Tapped {
    /// What sent it: `main`, `iframe "pay"`, `dedicated worker w.js`, ….
    pub(crate) from: String,
    /// The id of its CDP session.
    pub(crate) session: String,
    pub(crate) event: Arc<RawEvent>,
}

/// Where the sessions of a tab send their events while a [`Tap`] lives.
#[derive(Debug)]
pub(super) struct Hook {
    id: u64,
    tab: u32,
    tx: mpsc::UnboundedSender<Tapped>,
    /// The tasks that forward each session's events.
    tasks: Arc<Mutex<Vec<AbortHandle>>>,
}

impl Hook {
    pub(super) fn tab(&self) -> u32 {
        self.tab
    }
}

/// The events of every session of a tab, from when it was made until it
/// is dropped.
#[derive(Debug)]
pub(crate) struct Tap {
    id: u64,
    shared: Arc<Shared>,
    rx: mpsc::UnboundedReceiver<Tapped>,
    tasks: Arc<Mutex<Vec<AbortHandle>>>,
}

impl Tap {
    /// The next event; `None` once the tab and every session it had are
    /// gone.
    pub(crate) async fn next(&mut self) -> Option<Tapped> {
        self.rx.recv().await
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        self.shared.lock().hooks.retain(|h| h.id != self.id);
        for task in lock(&self.tasks).drain(..) {
            task.abort();
        }
    }
}

impl Tabs {
    /// Starts hearing the events of every session of tab `id`: its page's,
    /// its OOPIFs', its workers' (shared and service workers of its
    /// origins too), and those that attach from now on.
    pub(crate) fn tap(&self, id: u32) -> Tap {
        let (tx, rx) = mpsc::unbounded_channel();
        let tasks: Arc<Mutex<Vec<AbortHandle>>> = Arc::default();
        let mut state = self.shared.lock();
        state.next_hook += 1;
        let hook = Hook {
            id: state.next_hook,
            tab: id,
            tx,
            tasks: tasks.clone(),
        };
        let sessions: Vec<(String, Session)> = state
            .targets
            .iter()
            .filter(|(_, t)| heard(t) && state.tabs_of(t).contains(&id))
            .map(|(session_id, t)| (session_id.clone(), t.session.clone()))
            .collect();
        for (session_id, session) in sessions {
            forward(&self.shared, &hook, session_id, &session);
        }
        state.hooks.push(hook);
        Tap {
            id: state.next_hook,
            shared: self.shared.clone(),
            rx,
            tasks,
        }
    }
}

/// Whether a tap hears a target of this kind: pages, iframes and workers.
fn heard(target: &Target) -> bool {
    target.kind.is_host() || target.kind.worker().is_some()
}

/// Subscribes the taps of the tabs `target` (of session `session_id`, just
/// attached) belongs to. Called before the target is set up.
pub(super) fn attached(shared: &Arc<Shared>, state: &mut State, session_id: &str) {
    state.hooks.retain(|h| !h.tx.is_closed());
    let Some(target) = state.targets.get(session_id).filter(|t| heard(t)) else {
        return;
    };
    let tabs = state.tabs_of(target);
    for hook in state.hooks.iter().filter(|h| tabs.contains(&h.tab)) {
        forward(shared, hook, session_id.to_owned(), &target.session);
    }
}

/// Forwards the events of `session` to `hook`'s tap, each named after the
/// target it came from.
fn forward(shared: &Arc<Shared>, hook: &Hook, session_id: String, session: &Session) {
    let mut events = session.events_lossless("");
    let tx = hook.tx.clone();
    let tab = hook.tab;
    let shared = shared.clone();
    let task = tokio::spawn(async move {
        let mut from = String::new();
        while let Some(event) = events.next().await {
            if let Some(name) = name(&shared.lock(), tab, &session_id) {
                from = name;
            }
            let tapped = Tapped {
                from: from.clone(),
                session: session_id.clone(),
                event,
            };
            if tx.send(tapped).is_err() {
                break;
            }
        }
    });
    let mut tasks = lock(&hook.tasks);
    tasks.retain(|t| !t.is_finished());
    tasks.push(task.abort_handle());
}

/// What a tap calls the target of session `session_id` of tab `tab`:
/// `main` (the tab's page), `iframe "pay"` (or its URL, without the query),
/// `dedicated worker w.js`; `None` once it is gone.
fn name(state: &State, tab: u32, session_id: &str) -> Option<String> {
    let target = state.targets.get(session_id)?;
    Some(match target.kind {
        TargetKind::Page => "main".to_owned(),
        TargetKind::Iframe => {
            let frame = state
                .tabs
                .get(&tab)
                .and_then(|t| t.frames.list().into_iter().find(|f| f.id == target.id));
            match frame {
                Some(frame) if !frame.name.is_empty() || !frame.url.is_empty() => {
                    iframe_label(&frame)
                }
                _ => format!("iframe {}", short(&target.url)),
            }
        }
        _ => {
            let label = target.worker()?.label();
            label
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_owned()
        }
    })
}

/// A URL without its query and fragment.
fn short(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or_default()
}
