//! Following one navigation of a tab's main frame through its events,
//! until the condition `--wait` names holds.
//!
//! A [`Watch`] subscribes to the page session's events *before* the
//! navigation is sent, so nothing is missed however Chrome orders the
//! answer and the events. It follows the main frame's latest committed
//! document: a client-side redirect that commits before the wait is met
//! is waited for instead. Only documents it expects, or whose navigation
//! it saw start (`Page.frameStartedNavigating`), count: Chrome commits a
//! failed navigation's error page a moment after `Page.navigate` answered,
//! so the next navigation must not take that commit for its own.

use std::collections::HashSet;

use cdp::domains::page;
use cdp::{Event as _, RawEvent, RawEventStream, Session};
use serde::Deserialize as _;
use tokio::time::Instant;

use crate::request::WaitUntil;

/// What happened to the main frame since the watch began.
#[derive(Debug)]
pub(crate) struct Watch {
    events: RawEventStream,
    /// The main frame's id (its target id).
    frame: String,
    /// The loader (document) being waited for: the one `Page.navigate`
    /// returned, or the last one to commit.
    loader: Option<String>,
    /// Lifecycle events seen, by loader.
    lifecycle: HashSet<(String, String)>,
    /// Loaders of the navigations that started since the watch began.
    started: HashSet<String>,
    /// A new document committed in the main frame.
    committed: bool,
    /// The navigation stayed in the document (`#fragment`, `pushState`
    /// history entries).
    pub(crate) same_document: bool,
    /// The page came back from the back/forward cache.
    pub(crate) restored: bool,
    /// The committed document is Chrome's error page for this URL.
    pub(crate) unreachable: Option<String>,
    /// Whether a same-document navigation ends the wait: not when a new
    /// document is expected (the old one may still `pushState`).
    same_document_counts: bool,
}

impl Watch {
    /// Starts watching `session`'s main frame `frame`.
    pub(crate) fn new(session: &Session, frame: &str) -> Self {
        Self {
            events: session.events(),
            frame: frame.to_owned(),
            loader: None,
            lifecycle: HashSet::new(),
            started: HashSet::new(),
            committed: false,
            same_document: false,
            restored: false,
            unreachable: None,
            same_document_counts: true,
        }
    }

    /// Waits for the document `loader` (from `Page.navigate`) rather than
    /// the next one to commit.
    pub(crate) fn expect(&mut self, loader: &str) {
        self.loader = Some(loader.to_owned());
        self.same_document_counts = false;
    }

    /// Waits for a new document only (a reload).
    pub(crate) fn new_document(&mut self) {
        self.same_document_counts = false;
    }

    /// Marks the navigation as same-document (`Page.navigate` returned no
    /// loader).
    pub(crate) fn stayed_in_document(&mut self) {
        self.same_document = true;
    }

    /// Whether `wait` holds.
    pub(crate) fn reached(&self, wait: WaitUntil) -> bool {
        if wait == WaitUntil::None || self.same_document || self.restored {
            return true;
        }
        let Some(loader) = &self.loader else {
            return false;
        };
        let seen = |name: &str| self.lifecycle.contains(&(loader.clone(), name.to_owned()));
        match wait {
            WaitUntil::None => true,
            // Chrome reports `init` when a document commits; `committed`
            // covers a document whose events came before `expect`.
            WaitUntil::Commit => self.committed || seen("init"),
            WaitUntil::DomContentLoaded => seen("DOMContentLoaded"),
            WaitUntil::Load => seen("load"),
            WaitUntil::NetworkIdle => seen("networkIdle"),
        }
    }

    /// The `--wait` conditions the current document reached, for a
    /// timeout error: `commit`, `domcontentloaded`, `load`, `networkidle`.
    pub(crate) fn seen(&self) -> Vec<&'static str> {
        let Some(loader) = &self.loader else {
            return Vec::new();
        };
        [
            ("init", "commit"),
            ("DOMContentLoaded", "domcontentloaded"),
            ("load", "load"),
            ("networkIdle", "networkidle"),
        ]
        .into_iter()
        .filter(|(event, _)| {
            self.lifecycle
                .contains(&(loader.clone(), (*event).to_owned()))
        })
        .map(|(_, wait)| wait)
        .collect()
    }

    /// Whether a new document committed (or the navigation stayed in the
    /// document).
    pub(crate) fn committed(&self) -> bool {
        self.committed || self.same_document || self.restored || !self.seen().is_empty()
    }

    /// Reads events until `wait` holds (`true`) or `deadline` passes, or
    /// the session ends (`false`).
    pub(crate) async fn until(&mut self, wait: WaitUntil, deadline: Instant) -> bool {
        loop {
            if self.reached(wait) {
                return true;
            }
            match tokio::time::timeout_at(deadline, self.events.next()).await {
                Ok(Some(event)) => self.apply(&event),
                Ok(None) | Err(_) => return false,
            }
        }
    }

    fn apply(&mut self, event: &RawEvent) {
        let params = &event.params;
        match event.method.as_str() {
            page::FrameNavigated::NAME => {
                let Ok(nav) = page::FrameNavigated::deserialize(params) else {
                    return;
                };
                let restored = nav.r#type == page::NavigationType::BackForwardCacheRestore;
                let loader = &nav.frame.loader_id;
                let ours = self.loader.as_ref() == Some(loader) || self.started.contains(loader);
                if nav.frame.id != self.frame || !(ours || restored) {
                    return;
                }
                self.loader = Some(loader.clone());
                self.committed = true;
                self.restored = restored;
                self.unreachable.clone_from(&nav.frame.unreachable_url);
            }
            page::FrameStartedNavigating::NAME => {
                if params["frameId"].as_str() == Some(&self.frame)
                    && let Some(loader) = params["loaderId"].as_str()
                {
                    self.started.insert(loader.to_owned());
                }
            }
            page::NavigatedWithinDocument::NAME => {
                if self.same_document_counts && params["frameId"].as_str() == Some(&self.frame) {
                    self.same_document = true;
                }
            }
            page::LifecycleEvent::NAME => {
                let Ok(event) = page::LifecycleEvent::deserialize(params) else {
                    return;
                };
                if event.frame_id == self.frame {
                    self.lifecycle.insert((event.loader_id, event.name));
                }
            }
            _ => {}
        }
    }
}
