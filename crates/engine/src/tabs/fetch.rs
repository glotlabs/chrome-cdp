//! Request interception in page, iframe and worker sessions: plan 07's
//! routes (see [`crate::route`]) and plan 03's HTTP authentication (see
//! [`super::auth`]) share the `Fetch` domain.
//!
//! `Fetch` is on in a session while the session has routes or HTTP
//! credentials, with one pattern (`*`, request stage): which rule applies
//! is decided here, not by Chrome's wildcard patterns. While routes exist,
//! requests also skip service workers (which would answer before `Fetch`
//! sees them) and the HTTP cache (whose hits `Fetch` never sees), as
//! Playwright does. Without routes or credentials, nothing is paused.
//!
//! Chrome 154 has no `Fetch` domain in a dedicated worker: its requests
//! are paused in the session of the frame that started it, with the
//! frame's tab. A shared or service worker's are intercepted in its own
//! session (plan 13.2), with its scope and no tab, which is all routes and
//! mock sets need. Page requests skip service workers while routes exist,
//! so they still reach the routes; what a service worker fetches itself
//! is routed in its session.
//!
//! Chrome takes `Fetch.enable` in a shared worker only before it starts
//! (later it answers "'Fetch.enable' wasn't found"), so a shared worker
//! intercepts every request from its start, auth challenges too, and what
//! needs no route or credentials goes on at once ([`enable_early`]).
//!
//! Each session's `Fetch` events come through a lossless stream and are
//! handled in their own task ([`watch_fetch`]): a paused request whose
//! event were dropped would hang its page.

use std::sync::Arc;

use base64::Engine as _;
use std::time::Duration;

use cdp::domains::{fetch, network};
use cdp::{LosslessEventStream, RawEvent, Session};
use futures_util::future::join_all;

use super::auth::Auth;
use super::features::Feature;
use super::targets::TargetKind;
use super::{Shared, Step, lock, step};
use crate::route::{self, Decision, PendingPatches};

/// What a page, iframe or worker session intercepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Interception {
    /// `Fetch` is on.
    fetch: bool,
    /// Requests skip service workers.
    bypass: bool,
    /// Requests skip the HTTP cache.
    no_cache: bool,
}

/// Whether `Fetch` can be turned on and off in a running target of
/// `kind`: not in a shared worker (see the module documentation).
fn switchable(kind: TargetKind) -> bool {
    kind != TargetKind::SharedWorker
}

/// What the session's settings ask of a target of `kind` now.
fn wanted(shared: &Shared, kind: TargetKind) -> Interception {
    let settings = &shared.settings;
    let routes = settings.routes.is_active();
    Interception {
        fetch: routes || settings.http_credentials().is_some() || !switchable(kind),
        bypass: routes || settings.bypass_service_worker(),
        no_cache: routes,
    }
}

/// `Fetch.enable` for a target of `kind`: every request, paused before it
/// is sent; challenges too with credentials (always in a shared worker,
/// which cannot be told later).
fn enable(shared: &Shared, kind: TargetKind) -> fetch::EnableParams {
    fetch::EnableParams {
        patterns: Some(vec![fetch::RequestPattern {
            url_pattern: Some("*".into()),
            resource_type: None,
            request_stage: Some(fetch::RequestStage::Request),
        }]),
        handle_auth_requests: Some(
            shared.settings.http_credentials().is_some() || !switchable(kind),
        ),
    }
}

/// Turns `Fetch` on in a new shared worker, as soon as it attached:
/// Chrome makes the worker's loader as its script request starts,
/// microseconds after, whether or not the worker waits for the debugger,
/// and requests of a loader made before `Fetch.enable` are never paused.
/// The sooner, the likelier it comes first; nothing makes it certain (see
/// plan 13's decisions log).
pub(super) fn enable_early(shared: &Arc<Shared>, session: &Session, kind: TargetKind) {
    if switchable(kind) {
        return;
    }
    let call = session.call(enable(shared, kind));
    shared.spawn(async move {
        if let Err(e) = call.await {
            tracing::debug!("cannot intercept a shared worker's requests: {e}");
        }
    });
}

/// The setup steps of a new page, iframe or worker session of `kind`
/// (see `super::setup`).
pub(super) fn setup_steps(
    shared: &Shared,
    session: &Session,
    kind: TargetKind,
    steps: &mut Vec<(&str, Step)>,
) {
    let wanted = wanted(shared, kind);
    if let Some(id) = session.id()
        && let Some(target) = lock(&shared.state).targets.get_mut(id)
    {
        target.interception = Some(wanted);
    }
    // A shared worker's was sent as it attached (see `enable_early`).
    if wanted.fetch && switchable(kind) {
        steps.push(("Fetch.enable", step(session.call(enable(shared, kind)))));
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

/// Brings every target [`Feature::Fetch`] applies to in line with the
/// settings (after
/// routes or credentials changed); new sessions get them in their setup.
/// A session that already intercepts as wanted is left alone: `Fetch.enable`
/// again would drop the requests it holds paused (another context's
/// navigation, say). With `force` (new credentials), every session gets
/// every setting.
pub(super) async fn apply(shared: &Shared, timeout: Duration, force: bool) {
    let targets: Vec<(Session, TargetKind, Interception)> = lock(&shared.state)
        .applies_mut(Feature::Fetch)
        .map(|(_, t)| t)
        .filter_map(|t| {
            let now = wanted(shared, t.kind);
            if !force && t.interception == Some(now) {
                return None;
            }
            t.interception = Some(now);
            Some((t.session.with_timeout(timeout), t.kind, now))
        })
        .collect();
    let mut calls: Vec<(TargetKind, Step)> = Vec::new();
    for (host, kind, now) in &targets {
        let kind = *kind;
        if !switchable(kind) {
            // Intercepting since it started.
        } else if now.fetch {
            // Sent again when already on: the credentials may be new.
            calls.push((kind, step(host.call(enable(shared, kind)))));
        } else {
            calls.push((kind, step(host.call(fetch::DisableParams::default()))));
        }
        let params = network::SetBypassServiceWorkerParams::new(now.bypass);
        calls.push((kind, step(host.call(params))));
        let params = network::SetCacheDisabledParams::new(now.no_cache);
        calls.push((kind, step(host.call(params))));
    }
    let (kinds, calls): (Vec<TargetKind>, Vec<Step>) = calls.into_iter().unzip();
    for (kind, result) in kinds.into_iter().zip(join_all(calls).await) {
        if let Err(e) = result {
            // Usually a target that closed meanwhile.
            tracing::debug!("cannot change the request interception of a {kind:?}: {e}");
        }
    }
}

/// Handles a page, iframe or worker session's `Fetch` events: paused
/// requests go through the session's routes, authentication challenges
/// get its credentials. `tab` is the tab the session belongs to (none for
/// a shared or service worker), `context` its browser context.
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
    tracing::trace!(session = ?session.id(), url = %params["request"]["url"], "paused");
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
    // Rules and mock sets see the scope, not Chrome's own context id.
    let scope = shared.settings.contexts.scope_of(context);
    let context = scope.as_deref();
    let mocks = routes.has_mocks(context);
    let body = if mocks { post_data(request) } else { None };
    let (decision, emptied) = routes.decide(&route::Request {
        url: request["url"].as_str().unwrap_or_default(),
        method: request["method"].as_str().unwrap_or("GET"),
        resource_type: params["resourceType"].as_str().unwrap_or("Other"),
        headers: &request["headers"],
        context,
        tab,
        body: body.as_deref(),
        now_ms: if mocks { now_ms(shared, context) } else { 0 },
    });
    match &decision {
        Decision::Route { rule, .. } => {
            tracing::debug!(url = %request["url"], "routed by rule r{rule}");
        }
        Decision::Mock { .. } => tracing::debug!(url = %request["url"], "answered by a mock"),
        Decision::Pass | Decision::Preflight => {}
    }
    patches.expect(&event, &decision);
    shared.spawn(route::carry_out(session, event.clone(), decision));
    if emptied {
        // The last rule ran out: stop intercepting.
        let shared_task = shared.clone();
        shared.spawn(async move { apply(&shared_task, super::SETUP_TIMEOUT, false).await });
    }
}

/// A paused request's body: `postDataEntries` (base64 parts), or the older
/// `postData`. Chrome leaves large bodies out of both.
fn post_data(request: &serde_json::Value) -> Option<Vec<u8>> {
    if let Some(entries) = request["postDataEntries"].as_array() {
        let mut body = Vec::new();
        for entry in entries {
            if let Some(bytes) = entry["bytes"].as_str() {
                body.extend(
                    base64::engine::general_purpose::STANDARD
                        .decode(bytes)
                        .unwrap_or_default(),
                );
            }
        }
        return Some(body);
    }
    request["postData"].as_str().map(|s| s.as_bytes().to_vec())
}

/// Now for mock templates: the session's frozen time (`time freeze`), or
/// the real one.
pub(super) fn now_ms(shared: &Shared, scope: Option<&str>) -> i64 {
    shared.settings.emulation.get_in(scope).time.map_or_else(
        || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        },
        |t| t.epoch_ms,
    )
}
