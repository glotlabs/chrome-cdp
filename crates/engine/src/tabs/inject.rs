//! Init scripts and exposed functions in page and iframe sessions (plan
//! 07, milestone 7.3; the session's list is [`crate::inject`]'s), and the
//! items workers get too (plan 13.2: the mock shim; see
//! [`Wanted::in_workers`]).
//!
//! Each session records what it was given ([`super::Target::injected`]), by
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
//! like the others, plus one binding in that world they report on; so is
//! the recorder (`record start`, plan 18.6).
//!
//! A worker has no `Page` domain and no documents to come: its items run
//! once with `Runtime.evaluate`. A new worker gets them while paused,
//! before its own script (see `super::setup`); one that runs already gets
//! the mock shim late, and an init script late only with `--now` (as a
//! document that exists does). Taking an init script away cannot undo
//! what it did in a running worker, as in a document.

use std::sync::Arc;
use std::time::Duration;

use cdp::domains::{page, runtime};
use cdp::{RawEvent, Session};
use futures_util::future::join_all;

use super::features::Feature;
use super::targets::TargetKind;
use super::{Shared, Step};
use crate::frames::WORLD_NAME;
use crate::inject::{
    BINDING_PREFIX, Key, MOCK_BINDING, SEED_BINDING, Wanted, answer_script, mock_shim, unwrapper,
    wrapper,
};
use crate::record::RECORD_BINDING;
use crate::request::EvalWorld;

/// The setup steps of a new page, iframe or worker session.
pub(super) fn setup_steps(
    shared: &Arc<Shared>,
    session: &Session,
    kind: TargetKind,
    steps: &mut Vec<(&str, Step)>,
) {
    let session_id = session.id().unwrap_or_default().to_owned();
    // A new worker is paused: what runs now runs before its own script.
    let now = kind.worker().is_some();
    for call in sync(shared, &session_id, session, now) {
        steps.push(("an init script or exposed function", call));
    }
}

/// Brings every target [`Feature::Inject`] applies to in line with the
/// session's list;
/// with `now`, scripts added run in the documents there are too. Returns
/// the errors Chrome gave.
pub(super) async fn apply(shared: &Arc<Shared>, now: bool, timeout: Duration) -> Vec<String> {
    let hosts: Vec<(String, Session)> = shared
        .lock()
        .applies(Feature::Inject)
        .map(|(id, t)| (id.clone(), t.session.with_timeout(timeout)))
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
    let Some(host) = state.targets.get_mut(session_id) else {
        return Vec::new();
    };
    let worker = host.kind.worker().is_some();
    // Each session gets its own context's items (see `crate::context`).
    let wanted: Vec<(Key, Wanted)> = shared
        .settings
        .injections
        .wanted(host.scope.as_deref())
        .into_iter()
        .filter(|(_, what)| !worker || what.in_workers())
        .collect();
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
            calls.push(if worker {
                remove_from_worker(session, &key)
            } else {
                remove(session, &key, identifier)
            });
        }
    }
    for (key, what) in wanted {
        if host.injected.contains_key(&key) {
            continue;
        }
        host.injected.insert(key.clone(), None);
        calls.push(if worker {
            add_to_worker(shared, session_id, session, key, &what, now)
        } else {
            add(shared, session_id, session, key, &what, now)
        });
    }
    calls
}

/// Sends the calls that add `what` (one of [`Wanted::in_workers`]) to
/// worker `session`; the step records it as added (it has no script id).
/// An init script runs only with `now` (a new, paused worker, or
/// `--now`).
fn add_to_worker(
    shared: &Arc<Shared>,
    session_id: &str,
    session: &Session,
    key: Key,
    what: &Wanted,
    now: bool,
) -> Step {
    let (binding, source) = match what {
        Wanted::MockShim => (Some(MOCK_BINDING), Some(mock_shim())),
        Wanted::Script(script) => (None, now.then(|| script.source.clone())),
        Wanted::Binding { .. } | Wanted::SeedBinding | Wanted::Recorder(_) => {
            return Box::pin(async { Ok(()) });
        }
    };
    let binding = binding.map(|name| session.call(runtime::AddBindingParams::new(name.to_owned())));
    let script = source.map(|source| {
        let mut params = runtime::EvaluateParams::new(source);
        params.silent = Some(true);
        session.call(params)
    });
    let shared = shared.clone();
    let session = session.clone();
    let session_id = session_id.to_owned();
    Box::pin(async move {
        if let Some(binding) = binding {
            binding.await?;
        }
        if let Some(script) = script {
            // An init script that throws (it expects a document) is its
            // own business, as in a page.
            script.await?;
        }
        let kept = {
            let mut state = shared.lock();
            match state
                .targets
                .get_mut(&session_id)
                .and_then(|t| t.injected.get_mut(&key))
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
            // Taken away meanwhile.
            remove_from_worker(&session, &key).await
        }
    })
}

/// Takes `key` out of worker `session`: sockets it opens later are real
/// again.
fn remove_from_worker(session: &Session, key: &Key) -> Step {
    if !matches!(key, Key::MockShim(_)) {
        return Box::pin(async { Ok(()) });
    }
    let binding = session.call(runtime::RemoveBindingParams::new(MOCK_BINDING.to_owned()));
    let mut params = runtime::EvaluateParams::new(
        "globalThis[Symbol.for('chrome-cdp.mock')]?.({ off: true });".to_owned(),
    );
    params.silent = Some(true);
    let off = session.call(params);
    Box::pin(async move {
        binding.await?;
        off.await.map(drop)
    })
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
        Wanted::MockShim => {
            let binding = runtime::AddBindingParams::new(MOCK_BINDING.to_owned());
            // Documents there are get it too: their later sockets are
            // mocked.
            let mut params = page::AddScriptToEvaluateOnNewDocumentParams::new(mock_shim());
            params.run_immediately = Some(true);
            (Some(session.call(binding)), params)
        }
        Wanted::Recorder(script) => {
            // The binding only in chrome-cdp's world, where the recorder
            // runs; the documents there are get it at once.
            let mut binding = runtime::AddBindingParams::new(RECORD_BINDING.to_owned());
            binding.execution_context_name = Some(WORLD_NAME.to_owned());
            let mut params =
                page::AddScriptToEvaluateOnNewDocumentParams::new(script.source.clone());
            params.world_name = Some(WORLD_NAME.to_owned());
            params.run_immediately = Some(true);
            (Some(session.call(binding)), params)
        }
        Wanted::Binding { name, answers } => {
            let binding = runtime::AddBindingParams::new(format!("{BINDING_PREFIX}{name}"));
            // The function goes into the documents there are at once: the
            // binding is in them already.
            let mut params =
                page::AddScriptToEvaluateOnNewDocumentParams::new(wrapper(name, *answers));
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
                .targets
                .get_mut(&session_id)
                .and_then(|t| t.injected.get_mut(&key))
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
                .targets
                .get_mut(&session_id)
                .and_then(|t| t.injected.get_mut(&key))
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
    let (binding, undo, world) = match key {
        Key::Binding { name, .. } => (format!("{BINDING_PREFIX}{name}"), unwrapper(name), None),
        // Sockets opened later in the documents there are are real again.
        Key::MockShim(_) => (
            MOCK_BINDING.to_owned(),
            "globalThis[Symbol.for('chrome-cdp.mock')]?.({ off: true });".to_owned(),
            None,
        ),
        // The documents there are stop reporting.
        Key::Recorder(_) => (
            RECORD_BINDING.to_owned(),
            "globalThis.__ccdp?.recordOff();".to_owned(),
            Some(WORLD_NAME.to_owned()),
        ),
        _ => return Box::pin(async move { script.await.map(drop) }),
    };
    let binding = session.call(runtime::RemoveBindingParams::new(binding));
    let mut params = page::AddScriptToEvaluateOnNewDocumentParams::new(undo);
    params.run_immediately = Some(true);
    params.world_name = world;
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

/// How long handing a call its answer may take.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(5);

/// A `Runtime.bindingCalled` from session `session_id`: a call of a
/// function exposed with answers (its payload has the call's number) gets
/// its answer in the context it came from, in a task of its own.
pub(super) fn answer(shared: &Arc<Shared>, session_id: &str, event: &RawEvent) {
    let Some(name) = event.params["name"]
        .as_str()
        .and_then(|n| n.strip_prefix(BINDING_PREFIX))
    else {
        return;
    };
    let Some(call) = event.params["payload"]
        .as_str()
        .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
        .and_then(|p| p["call"].as_u64())
    else {
        return;
    };
    let Some(context) = event.params["executionContextId"].as_i64() else {
        return;
    };
    let Some((session, scope)) = shared
        .lock()
        .targets
        .get(session_id)
        .map(|t| (t.session.clone(), t.scope.clone()))
    else {
        return;
    };
    let Some(answer) = shared.settings.injections.answer(scope.as_deref(), name) else {
        return;
    };
    let mut params = runtime::EvaluateParams::new(answer_script(name, call, &answer));
    params.context_id = Some(context);
    params.silent = Some(true);
    let call = session.with_timeout(ANSWER_TIMEOUT).call(params);
    shared.spawn(async move {
        if let Err(e) = call.await {
            tracing::debug!("cannot hand an exposed function's call its answer: {e}");
        }
    });
}
