//! Request interception in page and iframe sessions: plan 07's routes
//! (see [`crate::route`]) and plan 03's HTTP authentication (see
//! [`super::auth`]) share the `Fetch` domain.
//!
//! `Fetch` is on in a session while the session has routes or HTTP
//! credentials, with one pattern (`*`, request stage): which rule applies
//! is decided here, not by Chrome's wildcard patterns. While routes exist,
//! requests also skip service workers (which would answer before `Fetch`
//! sees them) and the HTTP cache (whose hits `Fetch` never sees), as
//! Playwright does. Without routes or credentials, nothing is paused.
//!
//! Each host session's `Fetch` events come through a lossless stream and
//! are handled in their own task ([`watch_fetch`]): a paused request whose
//! event were dropped would hang its page.

use std::sync::Arc;
use std::time::Duration;

use cdp::domains::{fetch, network};
use cdp::{LosslessEventStream, RawEvent, Session};
use futures_util::future::join_all;

use super::auth::Auth;
use super::{Shared, Step, lock, step};
use crate::route::{self, Decision, PendingPatches};

/// What a page or iframe session intercepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Interception {
    /// `Fetch` is on.
    fetch: bool,
    /// Requests skip service workers.
    bypass: bool,
    /// Requests skip the HTTP cache.
    no_cache: bool,
}

/// What the session's settings ask for now.
fn wanted(shared: &Shared) -> Interception {
    let settings = &shared.settings;
    let routes = settings.routes.is_active();
    Interception {
        fetch: routes || settings.http_credentials().is_some(),
        bypass: routes || settings.bypass_service_worker(),
        no_cache: routes,
    }
}

/// `Fetch.enable`: every request, paused before it is sent; challenges too
/// with credentials.
fn enable(shared: &Shared) -> fetch::EnableParams {
    fetch::EnableParams {
        patterns: Some(vec![fetch::RequestPattern {
            url_pattern: Some("*".into()),
            resource_type: None,
            request_stage: Some(fetch::RequestStage::Request),
        }]),
        handle_auth_requests: Some(shared.settings.http_credentials().is_some()),
    }
}

/// The setup steps of a new page or iframe session (see `super::setup`).
pub(super) fn setup_steps(shared: &Shared, session: &Session, steps: &mut Vec<(&str, Step)>) {
    let wanted = wanted(shared);
    if wanted.fetch {
        steps.push(("Fetch.enable", step(session.call(enable(shared)))));
    }
    if wanted.bypass {
        let params = network::SetBypassServiceWorkerParams::new(true);
        steps.push(("Network.setBypassServiceWorker", step(session.call(params))));
    }
    if wanted.no_cache {
        let params = network::SetCacheDisabledParams::new(true);
        steps.push(("Network.setCacheDisabled", step(session.call(params))));
    }
}

/// Brings every page and iframe session in line with the settings (after
/// routes or credentials changed); new sessions get them in their setup.
/// Every setting is sent, on or off: a session set up while rules existed
/// (before a restart, say) must be turned off too, and turning off what is
/// off costs a call and no harm.
pub(super) async fn apply(shared: &Shared, timeout: Duration) {
    let now = wanted(shared);
    let hosts: Vec<Session> = lock(&shared.state)
        .hosts
        .values()
        .map(|h| h.session.with_timeout(timeout))
        .collect();
    let mut calls: Vec<Step> = Vec::new();
    for host in &hosts {
        if now.fetch {
            // Sent again when already on: the credentials may be new.
            calls.push(step(host.call(enable(shared))));
        } else {
            calls.push(step(host.call(fetch::DisableParams::default())));
        }
        let params = network::SetBypassServiceWorkerParams::new(now.bypass);
        calls.push(step(host.call(params)));
        let params = network::SetCacheDisabledParams::new(now.no_cache);
        calls.push(step(host.call(params)));
    }
    for result in join_all(calls).await {
        if let Err(e) = result {
            // Usually a target that closed meanwhile.
            tracing::debug!("cannot change a target's request interception: {e}");
        }
    }
}

/// Handles a page or iframe session's `Fetch` events: paused requests go
/// through the session's routes, authentication challenges get its
/// credentials. `tab` is the tab whose frames the session has, `context`
/// its browser context.
pub(super) async fn watch_fetch(
    shared: Arc<Shared>,
    mut events: LosslessEventStream,
    session: Session,
    tab: Option<u32>,
    context: Option<String>,
) {
    let mut auth = Auth::default();
    let mut patches = PendingPatches::default();
    while let Some(event) = events.next().await {
        match event.method.as_str() {
            "Fetch.requestPaused" => {
                paused(
                    &shared,
                    &session,
                    tab,
                    context.as_deref(),
                    &mut patches,
                    event,
                );
            }
            "Fetch.authRequired" => auth.auth_required(&shared, &session, &event),
            _ => {}
        }
    }
}

/// `Fetch.requestPaused`: at the request stage, the routes decide; at the
/// response stage (asked for by `--patch-json`), the response is patched.
fn paused(
    shared: &Arc<Shared>,
    session: &Session,
    tab: Option<u32>,
    context: Option<&str>,
    patches: &mut PendingPatches,
    event: Arc<RawEvent>,
) {
    let params = &event.params;
    let request_id = params["requestId"].as_str().unwrap_or_default();
    let response_stage =
        params.get("responseStatusCode").is_some() || params.get("responseErrorReason").is_some();
    if response_stage {
        let pending = patches.take(request_id);
        let settings = shared.settings.clone();
        let session = session.clone();
        shared.spawn(async move {
            if let Some((handler, seq)) = pending {
                route::patch_response(&settings.routes, &session, &event, &handler, seq).await;
            } else {
                let id = event.params["requestId"].as_str().unwrap_or_default();
                route::continue_paused(&session, id).await;
            }
        });
        return;
    }
    let routes = &shared.settings.routes;
    if !routes.is_active() {
        // Paused for authentication only.
        shared.spawn(route::continue_paused(session, request_id));
        return;
    }
    let request = &params["request"];
    let (decision, emptied) = routes.decide(&route::Request {
        url: request["url"].as_str().unwrap_or_default(),
        method: request["method"].as_str().unwrap_or("GET"),
        resource_type: params["resourceType"].as_str().unwrap_or("Other"),
        headers: &request["headers"],
        context,
        tab,
    });
    if let Decision::Route { rule, .. } = &decision {
        tracing::debug!(url = %request["url"], "routed by rule r{rule}");
    }
    patches.expect(&event, &decision);
    shared.spawn(route::carry_out(session, event.clone(), decision));
    if emptied {
        // The last rule ran out: stop intercepting.
        let shared_task = shared.clone();
        shared.spawn(async move { apply(&shared_task, super::SETUP_TIMEOUT).await });
    }
}
