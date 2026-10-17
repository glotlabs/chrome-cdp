//! Node creation stacks as a feature of the registry (plan 15, milestone
//! 15.1): Chrome records the JavaScript stack that created each DOM node
//! (`DOM.setNodeStackTracesEnabled`), so `source` can say which line of
//! the page's code made an element.
//!
//! Chrome only records while `DOM` is on in the session, and only for
//! nodes made after that, so pages and iframes get it before they run a
//! script ([`setup_steps`]). Recording costs page time per node, so with
//! `open --node-stacks localhost` (the default) it is on only for
//! documents on this machine: a session's top frame turns it on or off
//! as each navigation starts ([`navigating`]), before the new document
//! runs anything. `open --node-stacks` on a running browser brings every
//! page and iframe in line ([`Tabs::apply_node_stacks`]).

use std::sync::Arc;
use std::time::Duration;

use cdp::domains::{dom, page, runtime};
use cdp::{RawEvent, Session};
use futures_util::future::join_all;
use serde::Deserialize as _;

use super::features::NewTarget;
use super::{Shared, Step, Tabs, step};

/// A new page's or iframe's steps: recording on, if its document gets
/// it.
pub(super) fn setup_steps(
    shared: &Arc<Shared>,
    target: NewTarget<'_>,
    steps: &mut Vec<(&'static str, Step)>,
) {
    let stacks = shared.settings.node_stacks();
    let mut state = shared.lock();
    let id = target.session.id().unwrap_or_default();
    let Some(new) = state.targets.get_mut(id) else {
        return;
    };
    if stacks.wants(&new.url) {
        new.node_stacks = true;
        new.node_stacks_since_load = true;
        steps.push(("DOM.enable", step(target.session.call(dom_enable()))));
        steps.push((
            "DOM.setNodeStackTracesEnabled",
            step(
                target
                    .session
                    .call(dom::SetNodeStackTracesEnabledParams::new(true)),
            ),
        ));
    }
}

/// A frame of session `session_id` started navigating: if it is the
/// session's top frame, recording follows where it goes.
pub(super) fn navigating(shared: &Arc<Shared>, session_id: &str, event: &RawEvent) {
    let Ok(started) = page::FrameStartedNavigating::deserialize(&event.params) else {
        return;
    };
    let same_document = matches!(
        started.navigation_type,
        page::FrameStartedNavigatingNavigationType::SameDocument
            | page::FrameStartedNavigatingNavigationType::HistorySameDocument
    );
    if same_document {
        return;
    }
    let want = shared.settings.node_stacks().wants(&started.url);
    let (session, keep_dom) = {
        let mut state = shared.lock();
        let Some(target) = state.targets.get_mut(session_id) else {
            return;
        };
        if !target.kind.is_host() || target.id != started.frame_id {
            return;
        }
        // The new document is recorded from its start, or not at all.
        target.node_stacks_since_load = want;
        if target.node_stacks == want {
            return;
        }
        target.node_stacks = want;
        let session = target.session.clone();
        let keep_dom = state.coverage.as_ref().is_some_and(|c| c.uses(session_id));
        (session, keep_dom)
    };
    // Sent now, before the new document can run a script.
    let sent = switch(&session.with_timeout(super::SETUP_TIMEOUT), want, keep_dom);
    shared.spawn(async move {
        if let Err(e) = sent.await {
            tracing::debug!("turning node stacks {} failed: {e}", on_off(want));
        }
    });
}

impl Tabs {
    /// Brings every page and iframe in line with the session's
    /// `--node-stacks` (after `open` changed it). Recording turned on
    /// covers the nodes made from now on.
    pub(crate) async fn apply_node_stacks(&self, timeout: Duration) {
        let stacks = self.shared.settings.node_stacks();
        let changes: Vec<_> = {
            let mut state = self.shared.lock();
            let coverage: Vec<String> = state
                .coverage
                .as_ref()
                .map(super::coverage::Measuring::session_ids)
                .unwrap_or_default();
            state
                .targets
                .iter_mut()
                .filter(|(_, t)| t.kind.is_host())
                .filter_map(|(id, target)| {
                    let want = stacks.wants(&target.url);
                    if target.node_stacks == want {
                        return None;
                    }
                    target.node_stacks = want;
                    // Its document was made before: its nodes so far
                    // have no stacks.
                    target.node_stacks_since_load = false;
                    let session = target.session.with_timeout(timeout);
                    Some(switch(&session, want, coverage.contains(id)))
                })
                .collect()
        };
        for result in join_all(changes).await {
            if let Err(e) = result {
                tracing::debug!("changing node stacks failed: {e}");
            }
        }
    }

    /// Before a `goto` of `session`'s page to `url`: recording switched
    /// for where it goes, and Chrome's answer awaited, so the new document
    /// is recorded from its first node. ([`navigating`] does the same for
    /// navigations the page starts, but its call can arrive after a fast
    /// local page made its nodes.)
    pub(crate) async fn node_stacks_for(&self, session: &Session, url: &str, timeout: Duration) {
        let want = self.shared.settings.node_stacks().wants(url);
        let id = session.id().unwrap_or_default();
        let sent = {
            let mut state = self.shared.lock();
            let keep_dom = state.coverage.as_ref().is_some_and(|c| c.uses(id));
            let Some(target) = state.targets.get_mut(id) else {
                return;
            };
            if !target.kind.is_host() || target.node_stacks == want {
                return;
            }
            target.node_stacks = want;
            // Until the navigation starts, the old document is current.
            target.node_stacks_since_load = false;
            switch(&target.session.with_timeout(timeout), want, keep_dom)
        };
        if let Err(e) = sent.await {
            tracing::debug!("turning node stacks {} failed: {e}", on_off(want));
        }
    }

    /// Whether `DOM` must stay on in `session`: node stacks record there.
    pub(crate) fn node_stacks_use(&self, session: &Session) -> bool {
        let id = session.id().unwrap_or_default();
        self.shared
            .lock()
            .targets
            .get(id)
            .is_some_and(|t| t.node_stacks)
    }
}

/// Whether Chrome records node stacks in a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Recording {
    Off,
    /// On, but only since after its document began to load.
    Later,
    /// On since its document began to load.
    SinceLoad,
}

impl Tabs {
    /// Whether node stacks record in `session`, and since when.
    pub(crate) fn node_stacks_recording(&self, session: &Session) -> Recording {
        let id = session.id().unwrap_or_default();
        match self.shared.lock().targets.get(id) {
            Some(t) if t.node_stacks && t.node_stacks_since_load => Recording::SinceLoad,
            Some(t) if t.node_stacks => Recording::Later,
            _ => Recording::Off,
        }
    }
}

/// The stack that created the node `object_id` (an object of `session`,
/// in any world), as Chrome recorded it; `None` if it has none (made
/// before recording was on, or parsed from the HTML).
pub(crate) async fn node_stack(
    session: &Session,
    object_id: &str,
) -> cdp::Result<Option<runtime::StackTrace>> {
    // Node ids need the document requested first.
    let document = session.call(dom::GetDocumentParams {
        depth: Some(0),
        pierce: None,
    });
    let node = session.call(dom::RequestNodeParams::new(object_id.to_owned()));
    document.await?;
    let node_id = node.await?.node_id;
    let stacks = session
        .call(dom::GetNodeStackTracesParams::new(node_id))
        .await?;
    Ok(stacks.creation)
}

/// [`node_stack`] for many nodes of `session` at once: the document is
/// requested once (a second request would drop the node ids of the
/// first), then each node's stack.
pub(crate) async fn node_stacks_of(
    session: &Session,
    object_ids: &[String],
) -> Vec<Option<runtime::StackTrace>> {
    if object_ids.is_empty() {
        return Vec::new();
    }
    let document = session.call(dom::GetDocumentParams {
        depth: Some(0),
        pierce: None,
    });
    if document.await.is_err() {
        return vec![None; object_ids.len()];
    }
    let nodes = join_all(
        object_ids
            .iter()
            .map(|id| session.call(dom::RequestNodeParams::new(id.clone()))),
    )
    .await;
    join_all(nodes.into_iter().map(|node| async move {
        let node_id = node.ok()?.node_id;
        session
            .call(dom::GetNodeStackTracesParams::new(node_id))
            .await
            .ok()?
            .creation
    }))
    .await
}

/// Turns recording on (with `DOM`) or off (and `DOM` too, unless
/// `keep_dom`) in `session`; both calls are sent before this returns.
fn switch(
    session: &Session,
    on: bool,
    keep_dom: bool,
) -> impl Future<Output = cdp::Result<()>> + Send + use<> {
    let dom = if on {
        Some(step(session.call(dom_enable())))
    } else {
        None
    };
    let record = session.call(dom::SetNodeStackTracesEnabledParams::new(on));
    let off = (!on && !keep_dom).then(|| step(session.call(dom::DisableParams {})));
    async move {
        if let Some(dom) = dom {
            dom.await?;
        }
        record.await?;
        if let Some(off) = off {
            off.await?;
        }
        Ok(())
    }
}

fn dom_enable() -> dom::EnableParams {
    dom::EnableParams::default()
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}
