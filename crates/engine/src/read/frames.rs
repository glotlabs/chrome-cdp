//! Reading a page with its iframes (plan 13, milestone 13.5): `text` and
//! `markdown` without a locator read each frame's document in its own
//! isolated world, through its own session (an out-of-process iframe's
//! too), and put a visible iframe's content where its `<iframe>` sits,
//! between `[iframe "pay"]` and `[/iframe]` lines.

use std::time::Duration;

use cdp::domains::{dom, runtime};
use futures_util::future::BoxFuture;
use protocol::{ErrorInfo, ErrorKind};
use serde_json::Value;

use crate::errors::from_cdp;
use crate::frames::{FrameInfo, World, iframe_label};
use crate::injected::{self, GUARD};
use crate::tabs::{FrameContext, Page};
use crate::{Engine, Running};

/// The object group of the `<iframe>` handles a read passes in.
const GROUP: &str = "chrome-cdp-read-frames";

/// How long a child frame's document may take to have a context before
/// the read goes on without it (it is still loading).
const CHILD_WAIT: Duration = Duration::from_secs(1);

/// How deep iframes in iframes are read.
const MAX_DEPTH: usize = 8;

/// A read of every frame: `function` is called in each frame's isolated
/// world with `this` its document and one argument, the `<iframe>`
/// elements of its child frames; where it returns (or, with `field`, in
/// that field of what it returns) a child's mark (the bundle's
/// `frameMark`), the child's own read goes.
pub(crate) struct FramesRead<'a> {
    pub(crate) function: &'a str,
    pub(crate) field: Option<&'a str>,
}

impl Engine {
    /// Runs `read` in the main frame of `page` and, recursively, in the
    /// frames whose `<iframe>`s show; what the main frame returned, with
    /// the children's reads in place. Call it holding the tab's lock.
    pub(crate) async fn read_frames(
        &self,
        running: &Running,
        page: &Page,
        read: &FramesRead<'_>,
        timeout: Duration,
    ) -> Result<Value, ErrorInfo> {
        let (main, context) = self
            .frame_and_context(running, page, None, World::Isolated, timeout)
            .await?;
        let frames = running.tabs.frames(page.id);
        self.read_frame(running, page, &frames, &main, context, read, timeout, 0)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    fn read_frame<'a>(
        &'a self,
        running: &'a Running,
        page: &'a Page,
        frames: &'a [FrameInfo],
        frame: &'a str,
        context: FrameContext,
        read: &'a FramesRead<'a>,
        timeout: Duration,
        depth: usize,
    ) -> BoxFuture<'a, Result<Value, ErrorInfo>> {
        Box::pin(async move {
            let session = context.session.with_timeout(timeout);
            let children: Vec<&FrameInfo> = if depth < MAX_DEPTH {
                frames
                    .iter()
                    .filter(|f| f.parent.as_deref() == Some(frame))
                    .collect()
            } else {
                Vec::new()
            };
            // Each child's `<iframe>`, as a handle in this world.
            let mut owners: Vec<(&FrameInfo, String)> = Vec::new();
            for child in children {
                let Ok(owner) = session
                    .call(dom::GetFrameOwnerParams::new(child.id.clone()))
                    .await
                else {
                    continue;
                };
                let resolve = dom::ResolveNodeParams {
                    backend_node_id: Some(owner.backend_node_id),
                    execution_context_id: Some(context.context_id),
                    object_group: Some(GROUP.to_owned()),
                    ..dom::ResolveNodeParams::default()
                };
                if let Ok(Some(object)) = session.call(resolve).await.map(|r| r.object.object_id) {
                    owners.push((child, object));
                }
            }
            let mut params = runtime::CallFunctionOnParams::new(format!(
                "function (...owners) {{ {GUARD} return ({}).call(document, owners); }}",
                read.function
            ));
            params.arguments = Some(
                owners
                    .iter()
                    .map(|(_, object)| runtime::CallArgument {
                        value: None,
                        unserializable_value: None,
                        object_id: Some(object.clone()),
                    })
                    .collect(),
            );
            params.return_by_value = Some(true);
            params.await_promise = Some(true);
            let result = injected::call(&session, context.context_id, params).await;
            if !owners.is_empty() {
                let _ = session
                    .call(runtime::ReleaseObjectGroupParams::new(GROUP.to_owned()))
                    .await;
            }
            let result = result.map_err(|e| from_cdp(&e, &self.session))?;
            if let Some(exception) = result.exception_details {
                let message = exception
                    .exception
                    .and_then(|e| e.description)
                    .unwrap_or(exception.text);
                return Err(ErrorInfo::new(
                    ErrorKind::EvalError,
                    format!(
                        "reading the page failed: {}",
                        message.lines().next().unwrap_or_default()
                    ),
                ));
            }
            let mut value = result.result.value.unwrap_or(Value::Null);
            let slot = match read.field {
                Some(field) => value.get_mut(field),
                None => Some(&mut value),
            };
            let Some(Value::String(text)) = slot else {
                return Ok(value);
            };
            for (i, (child, _)) in owners.iter().enumerate() {
                let mark = format!("\u{0}{i}\u{0}");
                if !text.contains(&mark) {
                    continue;
                }
                let inner = match running
                    .tabs
                    .context(page.id, &child.id, World::Isolated, CHILD_WAIT.min(timeout))
                    .await
                {
                    Ok(context) => self
                        .read_frame(
                            running,
                            page,
                            frames,
                            &child.id,
                            context,
                            read,
                            timeout,
                            depth + 1,
                        )
                        .await
                        .ok(),
                    Err(_) => None,
                };
                let inner = inner.as_ref().and_then(|v| match read.field {
                    Some(field) => v.get(field),
                    None => Some(v),
                });
                let block = match inner.and_then(Value::as_str).map(str::trim) {
                    Some(inner) if !inner.is_empty() => {
                        format!("[{}]\n{inner}\n[/iframe]", iframe_label(child))
                    }
                    _ => String::new(),
                };
                *text = text.replace(&mark, &block);
            }
            if !owners.is_empty() {
                *text = tidy_lines(text);
            }
            Ok(value)
        })
    }
}

/// Drops marks left over (an `<iframe>` whose frame went away) and more
/// than one empty line in a row, and trims.
fn tidy_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut empty = 0;
    let mut marked = false;
    for (i, part) in text.split('\u{0}').enumerate() {
        // Odd parts are a mark's number.
        if i % 2 == 1 {
            marked = true;
            continue;
        }
        out.push_str(part);
    }
    if !marked && !out.contains("\n\n\n") {
        return out.trim().to_owned();
    }
    let mut tidy = String::with_capacity(out.len());
    for line in out.lines() {
        if line.trim().is_empty() {
            empty += 1;
            if empty > 1 {
                continue;
            }
        } else {
            empty = 0;
        }
        tidy.push_str(line);
        tidy.push('\n');
    }
    tidy.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::tidy_lines;

    #[test]
    fn marks_left_over_and_empty_lines_go() {
        assert_eq!(tidy_lines("a\n\n\u{0}1\u{0}\n\nb"), "a\n\nb");
        assert_eq!(tidy_lines("  a\nb  "), "a\nb");
        assert_eq!(tidy_lines("a\n\n\n\nb\n"), "a\n\nb");
    }
}
