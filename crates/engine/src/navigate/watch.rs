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
    /// A navigation to a new document started (or one committed): the
    /// page is being left.
    leaving: bool,
    /// The URL the last navigation that started goes to.
    pub(crate) started_url: Option<String>,
    /// The main frame stopped loading after `leaving`, before a new
    /// document committed: the navigation became a download, or got no
    /// content (204), or was canceled.
    stopped: bool,
}

/// How [`Watch::until_loaded`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Loaded {
    /// The new document's load event fired (or it came back from the
    /// back/forward cache).
    Load,
    /// The navigation ended without a new document.
    Stopped,
    /// The deadline passed (or the session ended) first.
    TimedOut,
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
            leaving: false,
            started_url: None,
            stopped: false,
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
            // The wait engine's network idle follows (see `navigate_page`).
            WaitUntil::DomContentLoaded | WaitUntil::NetworkIdle => seen("DOMContentLoaded"),
            // The settle follows.
            WaitUntil::Load | WaitUntil::Settle => seen("load"),
        }
    }

    /// The `--wait` conditions the current document reached, for a
    /// timeout error: `commit`, `domcontentloaded`, `load`.
    pub(crate) fn seen(&self) -> Vec<&'static str> {
        let Some(loader) = &self.loader else {
            return Vec::new();
        };
        [
            ("init", "commit"),
            ("DOMContentLoaded", "domcontentloaded"),
            ("load", "load"),
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

    /// Whether the main frame started to navigate to a new document (or
    /// committed one) since the watch began.
    pub(crate) fn leaving(&self) -> bool {
        self.leaving || self.committed || self.restored
    }

    /// Reads the events that already arrived, without waiting.
    pub(crate) async fn drain(&mut self) {
        let now = Instant::now();
        // `timeout_at` polls the stream before it looks at the clock.
        while let Ok(Some(event)) = tokio::time::timeout_at(now, self.events.next()).await {
            self.apply(&event);
        }
    }

    /// Reads events until the main frame starts to leave its document
    /// ([`Watch::leaving`]); never returns if it does not, unless the
    /// session ends.
    pub(crate) async fn until_leaving(&mut self) {
        while !self.leaving() {
            match self.events.next().await {
                Some(event) => self.apply(&event),
                None => return,
            }
        }
    }

    /// After [`Watch::leaving`]: reads events until the new document's
    /// load event, or the navigation ends without one, or `deadline`.
    pub(crate) async fn until_loaded(&mut self, deadline: Instant) -> Loaded {
        loop {
            if self.restored || (self.committed && self.reached(WaitUntil::Load)) {
                return Loaded::Load;
            }
            if self.stopped && !self.committed {
                return Loaded::Stopped;
            }
            match tokio::time::timeout_at(deadline, self.events.next()).await {
                Ok(Some(event)) => self.apply(&event),
                Ok(None) | Err(_) => return Loaded::TimedOut,
            }
        }
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
                    let kind = params["navigationType"].as_str().unwrap_or_default();
                    if !matches!(kind, "sameDocument" | "historySameDocument") {
                        self.leaving = true;
                        self.stopped = false;
                        self.started_url = params["url"].as_str().map(str::to_owned);
                    }
                }
            }
            page::FrameStoppedLoading::NAME => {
                if self.leaving && params["frameId"].as_str() == Some(&self.frame) {
                    self.stopped = true;
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
