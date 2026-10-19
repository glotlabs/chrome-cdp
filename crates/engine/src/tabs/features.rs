//! The feature registry (plan 13, milestone 13.1): what each kind of
//! target gets.
//!
//! A feature declares the [`TargetKind`]s it applies to, once
//! ([`Feature::kinds`]). A new target, paused, gets the setup steps of
//! every feature of [`REGISTRY`] that applies to its kind, in the
//! registry's order ([`Feature::setup_steps`], sent by `super::setup`).
//! A feature whose setting changes later brings the targets it applies to
//! in line (`fetch::apply`, `emulation::apply`, `inject::apply`), and finds
//! them through the same kinds (`State::applies`), so the targets a
//! feature sets up and the ones it changes are the same.
//!
//! A feature that should reach another kind of target adds it to its
//! kinds, and its steps handle it (workers have no `Page` domain).

use std::sync::Arc;

use cdp::Session;
use cdp::domains::{audits, network, page, runtime};

use super::targets::{KindSet, Target, TargetKind};
use super::{
    Shared, State, Step, auto_attach, coverage, debug, emulation, fetch, inject, node_stacks, seed,
    step,
};
use crate::{frames, injected, observe};

/// A feature: something every target of some kinds gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Feature {
    /// Auto-attach for the target's children, `Page` and its lifecycle
    /// events.
    Base,
    /// `Runtime` and `Log`: execution contexts, the console, uncaught
    /// exceptions and the browser's messages.
    Runtime,
    /// `Audits`: Chrome's issues (see [`observe::issues`]).
    Audits,
    /// `Network`: requests, for network idle, waits and the network log,
    /// in frames and workers.
    Network,
    /// The frame tree and chrome-cdp's isolated world, with `perf`'s
    /// observer.
    World,
    /// Init scripts, exposed functions, storage seeds and the mock shim
    /// (see [`inject`]); workers get the mock shim.
    Inject,
    /// Routes, mocks and HTTP authentication, in frames and workers (see
    /// [`fetch`]).
    Fetch,
    /// Devices, viewport, locale, the frozen clock, … (see
    /// [`emulation`]); workers get the user agent, locale, time zone and
    /// clock.
    Emulation,
    /// JavaScript and CSS coverage, while `coverage start` measures the
    /// target's tab (see [`coverage`]); workers have no CSS.
    Coverage,
    /// Node creation stacks, for documents `--node-stacks` covers (see
    /// [`node_stacks`]).
    NodeStacks,
    /// `Debugger`, while the target's tab has a logpoint (see [`debug`]).
    Debugger,
}

/// Every feature, in the order a new target gets their steps.
pub(crate) const REGISTRY: [Feature; 11] = [
    Feature::Base,
    Feature::Runtime,
    Feature::Audits,
    Feature::Network,
    Feature::World,
    Feature::Inject,
    Feature::Fetch,
    Feature::Emulation,
    Feature::Coverage,
    Feature::NodeStacks,
    Feature::Debugger,
];

/// A new target, paused, as the features see it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NewTarget<'a> {
    pub(crate) session: &'a Session,
    pub(crate) kind: TargetKind,
    /// The tab it belongs to.
    pub(crate) tab: Option<u32>,
}

impl Feature {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Runtime => "runtime",
            Self::Audits => "audits",
            Self::Network => "network",
            Self::World => "world",
            Self::Inject => "inject",
            Self::Fetch => "fetch",
            Self::Emulation => "emulation",
            Self::Coverage => "coverage",
            Self::NodeStacks => "stacks",
            Self::Debugger => "debugger",
        }
    }

    /// The kinds of target it applies to.
    pub(crate) fn kinds(self) -> KindSet {
        match self {
            // A worker's console and requests go to its tabs' logs; it
            // gets the mock shim.
            // Emulation: what means something in a worker (see
            // `emulation`).
            // Chrome raises issues in workers too (deprecations, CORS).
            Self::Runtime
            | Self::Audits
            | Self::Network
            | Self::Inject
            | Self::Emulation
            | Self::Coverage
            | Self::Debugger => KindSet::HOSTS_AND_WORKERS,
            // Chrome has no `Fetch` in a dedicated worker: its requests
            // are paused in the session of the frame that started it.
            Self::Fetch => KindSet::of(&[
                TargetKind::Page,
                TargetKind::Iframe,
                TargetKind::SharedWorker,
                TargetKind::ServiceWorker,
            ]),
            Self::Base | Self::World | Self::NodeStacks => KindSet::HOSTS,
        }
    }

    /// Whether it applies to `target`.
    pub(crate) fn applies(self, target: &Target) -> bool {
        self.kinds().contains(target.kind)
    }

    /// Adds the steps that set up `target` (of a kind it applies to) to
    /// `steps`.
    pub(crate) fn setup_steps(
        self,
        shared: &Arc<Shared>,
        target: NewTarget<'_>,
        steps: &mut Vec<(&'static str, Step)>,
    ) {
        let session = target.session;
        match self {
            Self::Base => {
                steps.push(("auto-attach", step(session.call(auto_attach()))));
                steps.push((
                    "Page.enable",
                    step(session.call(page::EnableParams::default())),
                ));
                // `init`, `DOMContentLoaded`, `load`, … per frame, for
                // navigation waits.
                steps.push((
                    "Page.setLifecycleEventsEnabled",
                    step(session.call(page::SetLifecycleEventsEnabledParams::new(true))),
                ));
            }
            Self::Runtime => {
                // Reports every execution context, the existing ones too,
                // and console calls and exceptions (for the console).
                steps.push((
                    "Runtime.enable",
                    step(session.call(runtime::EnableParams::default())),
                ));
                // The browser's messages: failed loads, CSP violations, …
                steps.push((
                    "Log.enable",
                    step(session.call(cdp::domains::log::EnableParams::default())),
                ));
            }
            // Issues from now on, and those raised before (replayed).
            Self::Audits => steps.push((
                "Audits.enable",
                step(session.call(audits::EnableParams::default())),
            )),
            Self::Network => network_steps(shared, session, steps),
            Self::World => world_steps(shared, target, steps),
            // `init-script` and `expose` (see `inject`).
            Self::Inject => inject::setup_steps(shared, session, target.kind, steps),
            // Routes and HTTP authentication (see `fetch`).
            Self::Fetch => fetch::setup_steps(shared, session, target.kind, steps),
            // Devices, viewport, locale, … (see `emulation`).
            Self::Emulation => emulation::setup_steps(shared, session, target.kind, steps),
            // Only while it measures the target's tab.
            Self::Coverage => coverage::setup_steps(shared, target, steps),
            // Where `--node-stacks` wants them (see `node_stacks`).
            Self::NodeStacks => node_stacks::setup_steps(shared, target, steps),
            // While its tab has a logpoint (see `debug`).
            Self::Debugger => debug::setup_steps(shared, target, steps),
        }
    }
}

impl State {
    /// The targets `feature` applies to, by session id.
    pub(crate) fn applies(&self, feature: Feature) -> impl Iterator<Item = (&String, &Target)> {
        self.targets.iter().filter(move |(_, t)| feature.applies(t))
    }

    /// [`State::applies`], to change them.
    pub(crate) fn applies_mut(
        &mut self,
        feature: Feature,
    ) -> impl Iterator<Item = (&String, &mut Target)> {
        self.targets
            .iter_mut()
            .filter(move |(_, t)| feature.applies(t))
    }
}

fn network_steps(shared: &Arc<Shared>, session: &Session, steps: &mut Vec<(&'static str, Step)>) {
    let few_bodies = shared.settings.load_run();
    // Response bodies kept in the browser process, so `network show
    // --body` still has them after the page navigates. Sent before
    // `Network.enable`, but not waited for with the other steps: in a new
    // popup Chrome answers it only much later, and the tab must not wait
    // for that to be ready. A load run's users read no bodies.
    if !few_bodies {
        let durable = session.call(durable_messages());
        shared.spawn(async move {
            if let Err(e) = durable.await {
                tracing::debug!("cannot keep response bodies durably: {e}");
            }
        });
    }
    // Requests, for network idle, `wait --request/--response` and the
    // network log.
    steps.push((
        "Network.enable",
        step(session.call(network_enable(few_bodies))),
    ));
    // A load run's `[load] block`, failed by Chrome itself: no request
    // is paused for it.
    let blocked = shared.settings.blocked_urls();
    if !blocked.is_empty() {
        steps.push((
            "Network.setBlockedURLs",
            step(session.call(blocked_urls(&blocked))),
        ));
    }
}

/// `Network.setBlockedURLs` for `patterns`: the first that matches a URL
/// decides whether Chrome fails it.
pub(super) fn blocked_urls(
    patterns: &[crate::request::BlockedUrl],
) -> network::SetBlockedUrLsParams {
    network::SetBlockedUrLsParams {
        url_patterns: Some(
            patterns
                .iter()
                .map(|p| network::BlockPattern {
                    url_pattern: p.pattern.clone(),
                    block: p.block,
                })
                .collect(),
        ),
        urls: None,
    }
}

fn world_steps(shared: &Arc<Shared>, target: NewTarget<'_>, steps: &mut Vec<(&'static str, Step)>) {
    let session = target.session;
    // The frames that exist already; events report later ones.
    let tree = session.call(page::GetFrameTreeParams::default());
    let seed_shared = shared.clone();
    let session_id = session.id().unwrap_or_default().to_owned();
    let tab = target.tab;
    steps.push((
        "Page.getFrameTree",
        Box::pin(async move {
            let tree = tree.await?.frame_tree;
            if let Some(tab) = tab {
                seed(&seed_shared, tab, &session_id, &tree);
            }
            Ok(())
        }),
    ));
    // chrome-cdp's isolated world, with its helper bundle, in every
    // document from now on, and in the ones that exist already.
    let mut world = page::AddScriptToEvaluateOnNewDocumentParams::new(injected::BUNDLE.to_owned());
    world.world_name = Some(frames::WORLD_NAME.to_owned());
    world.run_immediately = Some(true);
    steps.push(("the isolated world", step(session.call(world))));
    // `perf`'s observer of LCP, layout shifts, interactions and long
    // tasks, in the same world, from each document's start.
    let mut perf =
        page::AddScriptToEvaluateOnNewDocumentParams::new(observe::PERF_OBSERVER.to_owned());
    perf.world_name = Some(frames::WORLD_NAME.to_owned());
    steps.push(("the perf observer", step(session.call(perf))));
}

/// How much of the page's traffic Chrome keeps for `network show`: the
/// bodies of its responses (evicted oldest first past the total), and
/// request bodies up to 64KB in the request events. With `few_bodies` (a
/// load run's Chrome, whose users may keep a page for the whole run),
/// [`FEW_BODIES`] at most.
fn network_enable(few_bodies: bool) -> network::EnableParams {
    let (total, one) = if few_bodies {
        (FEW_BODIES, FEW_BODIES)
    } else {
        (BODIES, BODY)
    };
    network::EnableParams {
        max_total_buffer_size: Some(total),
        max_resource_buffer_size: Some(one),
        max_post_data_size: Some(64 * 1024),
        ..network::EnableParams::default()
    }
}

/// Keeps response bodies (up to [`BODIES`] a session, [`BODY`] each)
/// outside the renderer, where a navigation does not drop them.
fn durable_messages() -> network::ConfigureDurableMessagesParams {
    network::ConfigureDurableMessagesParams {
        max_total_buffer_size: Some(BODIES),
        max_resource_buffer_size: Some(BODY),
    }
}

/// How many bytes of response bodies Chrome keeps for a page or iframe
/// session, and for one response.
const BODIES: i64 = 64 * 1024 * 1024;
const BODY: i64 = 10 * 1024 * 1024;
/// … and for a load run's page.
const FEW_BODIES: i64 = 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::REGISTRY;
    use crate::tabs::targets::TargetKind;

    /// What each kind of target gets today. A feature that reaches
    /// another kind changes this table on purpose.
    #[test]
    fn kinds_per_feature() {
        let kinds = [
            TargetKind::Page,
            TargetKind::Iframe,
            TargetKind::DedicatedWorker,
            TargetKind::SharedWorker,
            TargetKind::ServiceWorker,
            TargetKind::Other,
        ];
        let table: Vec<String> = REGISTRY
            .iter()
            .map(|f| {
                let cells: String = kinds
                    .iter()
                    .map(|&k| if f.kinds().contains(k) { 'x' } else { '.' })
                    .collect();
                format!("{:<9} {cells}", f.name())
            })
            .collect();
        assert_eq!(
            table,
            [
                "base      xx....",
                "runtime   xxxxx.",
                "audits    xxxxx.",
                "network   xxxxx.",
                "world     xx....",
                "inject    xxxxx.",
                "fetch     xx.xx.",
                "emulation xxxxx.",
                "coverage  xxxxx.",
                "stacks    xx....",
                "debugger  xxxxx.",
            ]
        );
    }
}
