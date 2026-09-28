//! Init scripts and exposed functions in page and iframe sessions (plan
//! 07, milestone 7.3; the session's list is [`crate::inject`]'s).
//!
//! Each session records what it was given ([`super::Host::injected`]), by
//! a key that is never reused, with the script id Chrome gave back. A new
//! session gets everything during its setup, while paused
//! ([`setup_steps`]); a change compares every session with the list and
//! sends only the difference ([`apply`]). The comparison and the recording
//! happen under the state lock and the calls are sent right there, so a
//! session set up while a change is applied gets each item once. An item
//! taken away while Chrome was still adding it is removed when Chrome's
//! answer comes.
//!
//! Storage seeds (`state load`, milestone 7.4) are isolated-world scripts
//! like the others, plus one binding in that world they report on.

use std::sync::Arc;
use std::time::Duration;

use cdp::Session;
use cdp::domains::{page, runtime};
use futures_util::future::join_all;

use super::{Shared, Step};
use crate::frames::WORLD_NAME;
use crate::inject::{BINDING_PREFIX, Key, SEED_BINDING, Wanted, unwrapper, wrapper};
use crate::request::EvalWorld;

/// The setup steps of a new page or iframe session.
pub(super) fn setup_steps(shared: &Arc<Shared>, session: &Session, steps: &mut Vec<(&str, Step)>) {
    let session_id = session.id().unwrap_or_default().to_owned();
    for call in sync(shared, &session_id, session, false) {
        steps.push(("an init script or exposed function", call));
    }
}

/// Brings every page and iframe session in line with the session's list;
/// with `now`, scripts added run in the documents there are too. Returns
/// the errors Chrome gave.
pub(super) async fn apply(shared: &Arc<Shared>, now: bool, timeout: Duration) -> Vec<String> {
    let hosts: Vec<(String, Session)> = shared
        .lock()
        .hosts
        .iter()
        .map(|(id, h)| (id.clone(), h.session.with_timeout(timeout)))
        .collect();
    let calls: Vec<Step> = hosts
        .iter()
        .flat_map(|(id, session)| sync(shared, id, session, now))
        .collect();
    let errors: Vec<String> = join_all(calls)
        .await
        .into_iter()
        .filter_map(Result::err)
        .map(|e| e.to_string())
        .collect();
    for error in &errors {
        // Usually a target that closed meanwhile.
        tracing::debug!("cannot change a target's init scripts: {error}");
    }
    errors
}

/// Compares what session `session_id` has with the list, records the
/// difference as done, and sends the calls for it (in order). Returns them
/// to await.
fn sync(shared: &Arc<Shared>, session_id: &str, session: &Session, now: bool) -> Vec<Step> {
    let mut state = shared.lock();
    let Some(host) = state.hosts.get_mut(session_id) else {
        return Vec::new();
    };
    // Each session gets its own context's items (see `crate::context`).
    let wanted = shared.settings.injections.wanted(host.scope.as_deref());
    let mut calls = Vec::new();
    let gone: Vec<Key> = host
        .injected
        .keys()
        .filter(|key| !wanted.iter().any(|(k, _)| k == *key))
        .cloned()
        .collect();
    for key in gone {
        let identifier = host.injected.remove(&key).flatten();
        // A script still being added is removed when its id comes.
        if let Some(identifier) = identifier {
            calls.push(remove(session, &key, identifier));
        }
    }
    for (key, what) in wanted {
        if host.injected.contains_key(&key) {
            continue;
        }
        host.injected.insert(key.clone(), None);
        calls.push(add(shared, session_id, session, key, &what, now));
    }
    calls
}

/// Sends the calls that add `what` to `session`; the step records the
/// script id Chrome gives back.
fn add(
    shared: &Arc<Shared>,
    session_id: &str,
    session: &Session,
    key: Key,
    what: &Wanted,
    now: bool,
) -> Step {
    let (binding, params) = match what {
        Wanted::SeedBinding => return add_seed_binding(shared, session_id, session, key),
        Wanted::Script(script) => {
            let mut params =
                page::AddScriptToEvaluateOnNewDocumentParams::new(script.source.clone());
            if script.world == EvalWorld::Isolated {
                params.world_name = Some(WORLD_NAME.to_owned());
            }
            params.run_immediately = now.then_some(true);
            (None, params)
        }
        Wanted::Binding(name) => {
            let binding = runtime::AddBindingParams::new(format!("{BINDING_PREFIX}{name}"));
            // The function goes into the documents there are at once: the
            // binding is in them already.
            let mut params = page::AddScriptToEvaluateOnNewDocumentParams::new(wrapper(name));
            params.run_immediately = Some(true);
            (Some(session.call(binding)), params)
        }
    };
    let script = session.call(params);
    let shared = shared.clone();
    let session = session.clone();
    let session_id = session_id.to_owned();
    Box::pin(async move {
        if let Some(binding) = binding {
            binding.await?;
        }
        let identifier = script.await?.identifier;
        let kept = {
            let mut state = shared.lock();
            match state
                .hosts
                .get_mut(&session_id)
                .and_then(|h| h.injected.get_mut(&key))
            {
                Some(slot) => {
                    *slot = Some(identifier.clone());
                    true
                }
                None => false,
            }
        };
        if kept {
            Ok(())
        } else {
            // Taken away meanwhile.
            remove(&session, &key, identifier).await
        }
    })
}

/// Adds [`SEED_BINDING`] to chrome-cdp's isolated world of `session`; the
/// step records it as added (it has no script id).
fn add_seed_binding(shared: &Arc<Shared>, session_id: &str, session: &Session, key: Key) -> Step {
    let mut params = runtime::AddBindingParams::new(SEED_BINDING.to_owned());
    params.execution_context_name = Some(WORLD_NAME.to_owned());
    let call = session.call(params);
    let shared = shared.clone();
    let session = session.clone();
    let session_id = session_id.to_owned();
    Box::pin(async move {
        call.await?;
        let kept = {
            let mut state = shared.lock();
            match state
                .hosts
                .get_mut(&session_id)
                .and_then(|h| h.injected.get_mut(&key))
            {
                Some(slot) => {
                    *slot = Some(String::new());
                    true
                }
                None => false,
            }
        };
        if kept {
            Ok(())
        } else {
            remove(&session, &key, String::new()).await
        }
    })
}

/// Takes `key` (added as script `identifier`) out of `session`: the
/// script from new documents, and an exposed function from the documents
/// there are too.
fn remove(session: &Session, key: &Key, identifier: String) -> Step {
    if *key == Key::SeedBinding {
        let call = session.call(runtime::RemoveBindingParams::new(SEED_BINDING.to_owned()));
        return Box::pin(async move { call.await.map(drop) });
    }
    let script = session.call(page::RemoveScriptToEvaluateOnNewDocumentParams::new(
        identifier,
    ));
    let Key::Binding { name, .. } = key else {
        return Box::pin(async move { script.await.map(drop) });
    };
    let binding = session.call(runtime::RemoveBindingParams::new(format!(
        "{BINDING_PREFIX}{name}"
    )));
    let mut params = page::AddScriptToEvaluateOnNewDocumentParams::new(unwrapper(name));
    params.run_immediately = Some(true);
    let unwrap = session.call(params);
    let session = session.clone();
    Box::pin(async move {
        script.await?;
        binding.await?;
        // The script that took the function out has done its work.
        let identifier = unwrap.await?.identifier;
        session
            .call(page::RemoveScriptToEvaluateOnNewDocumentParams::new(
                identifier,
            ))
            .await
            .map(drop)
    })
}
