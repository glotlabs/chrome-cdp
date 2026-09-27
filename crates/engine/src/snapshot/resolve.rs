//! Resolving a ref to the element it names (plan 04, milestone 4.2).
//!
//! A ref names a node by `(frame, backendNodeId)`. It resolves when that
//! node is still in its frame's current document. When it is not (a
//! framework re-rendered it), the ref falls back to the one node of its
//! frame with the same role and name, if there is exactly one, unless
//! `--strict-refs` is on or the frame changed its route since the ref was
//! shown. Anything else is an error that says why: `NotFound` for a
//! number the tab never gave, `RefStale` for one from an earlier document,
//! `RefDetached` for a node that is gone.

use std::time::Duration;

use cdp::CdpError;
use cdp::domains::{dom, runtime};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};
use serde_json::{Value, json};

use super::prune::{self, View};
use super::refs::{Lookup, RefEntry, parse_ref};
use super::tree::{Node, NodeKey};
use super::{Capture, ax};
use crate::errors::from_cdp;
use crate::frames::World;
use crate::tabs::{FrameContext, Page, tab_name};
use crate::{Engine, Running};

/// A ref resolved to a live element: a handle on it in chrome-cdp's
/// isolated world of its frame.
#[derive(Debug, Clone)]
pub(crate) struct Target {
    pub(crate) key: NodeKey,
    pub(crate) context: FrameContext,
    pub(crate) object_id: String,
    /// Set when the role+name fallback found it: the note to show.
    pub(crate) note: Option<String>,
}

impl Target {
    /// Lets go of the handle (without waiting).
    pub(crate) fn release(self) {
        let session = self.context.session;
        let object_id = self.object_id;
        tokio::spawn(async move {
            let _ = session
                .call(runtime::ReleaseObjectParams { object_id })
                .await;
        });
    }
}

/// Whether a resolved node is in its frame's current document (a node of
/// an earlier document is still connected to that document).
const LIVE_JS: &str =
    "function () { return this.isConnected && this.getRootNode({composed: true}) === document; }";

/// What [`Engine::call_on_ref`] returned.
#[derive(Debug, Clone, PartialEq)]
pub struct RefCall {
    /// The function's value (JSON).
    pub value: Value,
    /// `e6 re-resolved by role+name` when the fallback found the element.
    pub note: Option<String>,
}

impl Engine {
    /// Calls `function` with `this` set to the element ref `reference`
    /// (`e6`, `@e6`) of tab `tab` names, in chrome-cdp's isolated world,
    /// and returns its value (awaited if it is a promise). Holds the tab's
    /// lock. The building block for commands that act on a ref.
    pub async fn call_on_ref(
        &self,
        tab: Option<&str>,
        reference: &str,
        strict: bool,
        function: &str,
        timeout: Duration,
    ) -> Result<RefCall, ErrorInfo> {
        let number = parse_ref(reference).ok_or_else(|| {
            ErrorInfo::new(ErrorKind::BadLocator, format!("{reference} is not a ref"))
                .hint(Hint::text("a ref is e<number>, as the snapshot shows it"))
        })?;
        let running = self.running()?;
        let page = self.page(&running, tab, timeout).await?;
        let _tab = self.tabs.lock(&page.target_id).await;
        let target = self
            .resolve_ref(&running, &page, number, strict, timeout)
            .await?;
        let mut params = runtime::CallFunctionOnParams::new(function.to_owned());
        params.object_id = Some(target.object_id.clone());
        params.return_by_value = Some(true);
        params.await_promise = Some(true);
        let result = target
            .context
            .session
            .with_timeout(timeout)
            .call(params)
            .await
            .map_err(|e| from_cdp(&e, &self.session));
        let note = target.note.clone();
        target.release();
        let result = result?;
        if let Some(exception) = result.exception_details {
            let message = exception
                .exception
                .and_then(|e| e.description)
                .unwrap_or(exception.text);
            let first = message.lines().next().unwrap_or_default().to_owned();
            return Err(ErrorInfo::new(ErrorKind::EvalError, first)
                .hint(Hint::text("fix the function; the page is unchanged"))
                .details(json!({ "exception": message })));
        }
        Ok(RefCall {
            value: result.result.value.unwrap_or(Value::Null),
            note,
        })
    }

    /// Resolves ref `number` of `page` to its element (see the module
    /// docs). Call it holding the tab's lock.
    pub(crate) async fn resolve_ref(
        &self,
        running: &Running,
        page: &Page,
        number: u32,
        strict: bool,
        timeout: Duration,
    ) -> Result<Target, ErrorInfo> {
        let lookup = running
            .tabs
            .snapshot_memory(page.id, |m| m.refs.lookup(number))
            .ok_or_else(|| {
                ErrorInfo::new(
                    ErrorKind::NotFound,
                    format!("tab {} went away", tab_name(page.id)),
                )
            })?;
        let (entry, fallback) = match lookup {
            Lookup::Live(entry, fallback) => (entry, fallback),
            Lookup::Unknown => return Err(self.unknown_ref(page, number)),
            Lookup::StalePage => {
                return Err(self.stale_ref(
                    number,
                    format!(
                        "ref e{number} is from an earlier page of tab {}",
                        tab_name(page.id)
                    ),
                ));
            }
            Lookup::StaleFrame => {
                return Err(self.stale_ref(
                    number,
                    format!("ref e{number} is from an earlier document of its frame"),
                ));
            }
        };
        if let Some((context, object_id)) =
            self.live_node(running, page, &entry.key, timeout).await?
        {
            return Ok(Target {
                context,
                key: entry.key,
                object_id,
                note: None,
            });
        }
        if strict {
            return Err(self.detached(number, &entry, Why::Strict));
        }
        if !fallback {
            return Err(self.detached(number, &entry, Why::Route));
        }
        let found = self
            .same_role_and_name(running, page, &entry, timeout)
            .await?;
        let [key] = found.as_slice() else {
            return Err(self.detached(number, &entry, Why::Matches(found.len())));
        };
        let Some((context, object_id)) = self.live_node(running, page, key, timeout).await? else {
            return Err(self.detached(number, &entry, Why::Matches(0)));
        };
        running
            .tabs
            .snapshot_memory(page.id, |m| m.refs.repoint(number, key));
        Ok(Target {
            context,
            key: key.clone(),
            object_id,
            note: Some(format!("e{number} re-resolved by role+name")),
        })
    }

    /// A handle on the node `key` names in the isolated world of its
    /// frame (and that world), or `None` if the node or its frame is
    /// gone.
    async fn live_node(
        &self,
        running: &Running,
        page: &Page,
        key: &NodeKey,
        timeout: Duration,
    ) -> Result<Option<(FrameContext, String)>, ErrorInfo> {
        if !running
            .tabs
            .frames(page.id)
            .iter()
            .any(|f| f.id == key.frame)
        {
            return Ok(None);
        }
        let backend = key.backend;
        self.in_frame(
            running,
            page,
            Some(&key.frame),
            World::Isolated,
            timeout,
            |context| {
                let context = context.clone();
                async move {
                    let session = context.session.with_timeout(timeout);
                    let params = dom::ResolveNodeParams {
                        backend_node_id: Some(backend),
                        execution_context_id: Some(context.context_id),
                        ..dom::ResolveNodeParams::default()
                    };
                    let object = match session.call(params).await {
                        Ok(resolved) => resolved.object.object_id,
                        Err(CdpError::Protocol { message, .. }) if no_such_node(&message) => None,
                        Err(e) => return Err(e),
                    };
                    let Some(object_id) = object else {
                        return Ok(None);
                    };
                    let mut params = runtime::CallFunctionOnParams::new(LIVE_JS.to_owned());
                    params.object_id = Some(object_id.clone());
                    params.return_by_value = Some(true);
                    let live = session.call(params).await?;
                    if live.result.value == Some(Value::Bool(true)) {
                        return Ok(Some((context, object_id)));
                    }
                    let _ = session
                        .call(runtime::ReleaseObjectParams { object_id })
                        .await;
                    Ok(None)
                }
            },
        )
        .await?
        .map_err(|e| from_cdp(&e, &self.session))
    }

    /// The nodes of `entry`'s frame that have its role and name now, as a
    /// full snapshot of the frame would show them.
    async fn same_role_and_name(
        &self,
        running: &Running,
        page: &Page,
        entry: &RefEntry,
        timeout: Duration,
    ) -> Result<Vec<NodeKey>, ErrorInfo> {
        let frames: Vec<_> = running
            .tabs
            .frame_sources(page.id)
            .into_iter()
            .filter(|f| f.info.id == entry.key.frame)
            .collect();
        if frames.is_empty() {
            return Ok(Vec::new());
        }
        let Capture { nodes, .. } = ax::capture(&frames, timeout)
            .await
            .map_err(|e| from_cdp(&e, &self.session))?;
        let view = View {
            collapse: false,
            text_limit: 0,
            ..View::default()
        };
        let shaped = prune::shape(nodes, &view);
        let mut found = Vec::new();
        for node in &shaped {
            node.walk(&mut |n: &Node| {
                if let Some(key) = &n.key
                    && key.frame == entry.key.frame
                    && n.role == entry.role
                    && same_name(&entry.name, &n.name)
                    && n.more.is_none()
                    && !found.contains(key)
                {
                    found.push(key.clone());
                }
            });
        }
        Ok(found)
    }

    /// A number the tab never gave.
    pub(crate) fn unknown_ref(&self, page: &Page, number: u32) -> ErrorInfo {
        ErrorInfo::new(
            ErrorKind::NotFound,
            format!("tab {} has no ref e{number}", tab_name(page.id)),
        )
        .hint(
            Hint::run(Cmd::new("snapshot").session(&self.session))
                .why("shows the refs the page has now"),
        )
        .details(json!({ "ref": format!("e{number}") }))
    }

    fn stale_ref(&self, number: u32, message: String) -> ErrorInfo {
        ErrorInfo::new(ErrorKind::RefStale, message)
            .hint(
                Hint::run(Cmd::new("snapshot").session(&self.session))
                    .why("gives the new document's refs"),
            )
            .details(json!({ "ref": format!("e{number}") }))
    }

    fn detached(&self, number: u32, entry: &RefEntry, why: Why) -> ErrorInfo {
        let was = if entry.name.is_empty() {
            entry.role.clone()
        } else {
            format!("{} \"{}\"", entry.role, entry.name.replace('"', "\\\""))
        };
        let mut info = ErrorInfo::new(
            ErrorKind::RefDetached,
            format!("element e{number} is no longer in the page"),
        )
        .note("was", was);
        let mut details = json!({
            "ref": format!("e{number}"),
            "role": entry.role,
            "name": entry.name,
            "frame": entry.key.frame,
        });
        match why {
            Why::Strict | Why::Matches(0) => {}
            Why::Route => {
                info = info.note(
                    "note",
                    "the page changed its URL since it was shown, so it is not looked up by role and name",
                );
            }
            Why::Matches(n) => {
                info = info.note(
                    "note",
                    format!("{n} elements have its role and name now; not guessing which"),
                );
                details["matches"] = json!(n);
            }
        }
        info.hint(
            Hint::run(Cmd::new("snapshot").session(&self.session))
                .why("shows the refs the page has now"),
        )
        .details(details)
    }
}

/// Why a gone node was not re-resolved.
#[derive(Debug, Clone, Copy)]
enum Why {
    /// `--strict-refs`.
    Strict,
    /// A same-document navigation since the ref was shown.
    Route,
    /// This many nodes have its role and name (not one).
    Matches(usize),
}

/// Whether a `DOM.resolveNode` error means the node is gone.
fn no_such_node(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("no node") || message.contains("node with given id")
}

/// Whether `now` is the name a ref kept: the same, or what a shown name
/// cut at `--text-limit` (ending in `…`) began with.
fn same_name(kept: &str, now: &str) -> bool {
    kept == now
        || kept
            .strip_suffix('…')
            .is_some_and(|start| !start.is_empty() && now.starts_with(start))
}

#[cfg(test)]
mod tests {
    use super::{no_such_node, same_name};

    #[test]
    fn names_match_whole_or_as_cut() {
        assert!(same_name("Save", "Save"));
        assert!(!same_name("Save", "Save all"));
        assert!(same_name("A long na…", "A long name indeed"));
        assert!(!same_name("A long na…", "A short name"));
        assert!(!same_name("…", "anything"));
        assert!(same_name("", ""));
    }

    #[test]
    fn gone_nodes_are_told_from_other_errors() {
        assert!(no_such_node("No node with given id found"));
        assert!(no_such_node(
            "Node with given id does not belong to the document"
        ));
        assert!(!no_such_node("Cannot find context with specified id"));
    }
}
