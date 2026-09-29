//! The targets the session is attached to (plan 13, milestone 13.1).
//!
//! Every attached target, whatever its kind, is one [`Target`] in
//! `State::targets`, keyed by its session id: pages, iframes, dedicated,
//! shared and service workers, and the rest (prerenders, …). A target
//! knows what it is ([`TargetKind`]) and whom it belongs to ([`Owner`]):
//! a tab, or, for shared and service workers, an origin of a scope, whose
//! tabs are found when needed (`State::tabs_of`).
//!
//! What each kind of target gets is up to the features (see
//! [`super::features`]), which also keep their per-target state here.

use std::collections::HashMap;

use cdp::Session;
use protocol::schema::{Worker, WorkerKind};

use super::fetch::Interception;
use crate::inject::Key;

/// What a target is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TargetKind {
    /// A page's own session: a tab, a popup, a prerender.
    Page,
    /// An out-of-process iframe.
    Iframe,
    DedicatedWorker,
    SharedWorker,
    ServiceWorker,
    /// Anything else Chrome attaches to (worklets, …).
    Other,
}

impl TargetKind {
    /// The kind of a target of Chrome's `type`.
    pub(crate) fn of(r#type: &str) -> Self {
        match r#type {
            "page" => Self::Page,
            "iframe" => Self::Iframe,
            "worker" => Self::DedicatedWorker,
            "shared_worker" => Self::SharedWorker,
            "service_worker" => Self::ServiceWorker,
            _ => Self::Other,
        }
    }

    /// Whether it has frames (and the `Page` domain): a page or iframe.
    pub(crate) fn is_host(self) -> bool {
        matches!(self, Self::Page | Self::Iframe)
    }

    /// The kind of worker it is, if it is one.
    pub(crate) fn worker(self) -> Option<WorkerKind> {
        match self {
            Self::DedicatedWorker => Some(WorkerKind::Dedicated),
            Self::SharedWorker => Some(WorkerKind::Shared),
            Self::ServiceWorker => Some(WorkerKind::Service),
            _ => None,
        }
    }

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// A set of [`TargetKind`]s: the kinds a feature applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct KindSet(u8);

impl KindSet {
    /// Pages and iframes: the targets with frames.
    pub(crate) const HOSTS: Self = Self::of(&[TargetKind::Page, TargetKind::Iframe]);

    /// Pages, iframes and every kind of worker: the targets that run a
    /// page's scripts and send its requests.
    pub(crate) const HOSTS_AND_WORKERS: Self = Self::of(&[
        TargetKind::Page,
        TargetKind::Iframe,
        TargetKind::DedicatedWorker,
        TargetKind::SharedWorker,
        TargetKind::ServiceWorker,
    ]);

    pub(crate) const fn of(kinds: &[TargetKind]) -> Self {
        let mut bits = 0;
        let mut i = 0;
        while i < kinds.len() {
            bits |= 1 << kinds[i] as u8;
            i += 1;
        }
        Self(bits)
    }

    pub(crate) fn contains(self, kind: TargetKind) -> bool {
        self.0 & kind.bit() != 0
    }
}

/// Whom a target belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Owner {
    /// A page, its iframes and their dedicated workers belong to a tab
    /// (a worker started in an OOPIF too).
    Tab(u32),
    /// A shared or service worker belongs to an origin of its scope: the
    /// tabs of the scope with a frame on the origin share it.
    Origin { origin: String },
    /// Prerenders, targets of no tab, and the rest.
    None,
}

/// An attached target.
#[derive(Debug)]
pub(crate) struct Target {
    pub(crate) session: Session,
    pub(crate) kind: TargetKind,
    /// Chrome's target id.
    pub(crate) id: String,
    /// Kept current from `Target.targetInfoChanged`.
    pub(crate) url: String,
    pub(crate) owner: Owner,
    /// Its context's settings key: Chrome's context id for a context of
    /// ours, `None` for the session's default one.
    pub(crate) scope: Option<String>,
    /// The id of the session that announced it; `None` for the browser's.
    pub(crate) parent: Option<String>,
    /// A dedicated worker's frame: the one that started it (or the worker
    /// that did).
    pub(crate) frame: Option<String>,
    /// The id of its frozen clock script (`time freeze`), if it has one.
    pub(crate) clock: Option<String>,
    /// What it was last told to intercept.
    pub(crate) interception: Option<Interception>,
    /// The init scripts and exposed functions it has, with the id of
    /// each one's script (`None` while Chrome is adding it).
    pub(crate) injected: HashMap<Key, Option<String>>,
    /// A worker's console events so far: numbers each, the same in every
    /// tab it goes to.
    pub(crate) events: u64,
}

impl Target {
    pub(crate) fn new(
        session: Session,
        kind: TargetKind,
        id: String,
        url: String,
        owner: Owner,
        scope: Option<String>,
        parent: Option<String>,
    ) -> Self {
        Self {
            session,
            kind,
            id,
            url,
            owner,
            scope,
            parent,
            frame: None,
            clock: None,
            interception: None,
            injected: HashMap::new(),
            events: 0,
        }
    }

    /// The tab it belongs to, if it belongs to one.
    pub(crate) fn tab(&self) -> Option<u32> {
        match self.owner {
            Owner::Tab(id) => Some(id),
            _ => None,
        }
    }

    /// It, as the entries of a tab's logs name it, if it is a worker.
    pub(crate) fn worker(&self) -> Option<Worker> {
        Some(Worker {
            kind: self.kind.worker()?,
            url: self.url.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{KindSet, TargetKind};

    #[test]
    fn kinds() {
        assert_eq!(TargetKind::of("page"), TargetKind::Page);
        assert_eq!(TargetKind::of("worker"), TargetKind::DedicatedWorker);
        assert_eq!(TargetKind::of("service_worker"), TargetKind::ServiceWorker);
        assert_eq!(TargetKind::of("auction_worklet"), TargetKind::Other);
        let set = KindSet::of(&[TargetKind::Iframe, TargetKind::SharedWorker]);
        assert!(set.contains(TargetKind::Iframe));
        assert!(set.contains(TargetKind::SharedWorker));
        assert!(!set.contains(TargetKind::Page));
        assert!(!KindSet::default().contains(TargetKind::Other));
        assert!(KindSet::HOSTS.contains(TargetKind::Page));
        assert!(!KindSet::HOSTS.contains(TargetKind::DedicatedWorker));
    }
}
