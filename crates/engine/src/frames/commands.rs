//! What page commands share about frames: finding the frame a command
//! names and the context to run a script in, with errors that list the
//! tab's frames. Plan 04's `frame=` locators and plan 06's
//! `eval --frame --world` build on [`Engine::frame_context`] and
//! [`Engine::evaluate`].

use std::collections::HashMap;
use std::time::Duration;

use cdp::domains::{dom, runtime};
use futures_util::future::join_all;
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, format};
use serde_json::{Value, json};

use super::{FindError, FrameInfo, World};
use crate::context::Scoped as _;
use crate::errors::from_cdp;
use crate::tabs::{FrameContext, FrameError, Page, tab_name};
use crate::{Engine, Running};

/// How many frames an error lists.
const LISTED_FRAMES: usize = 10;

/// How often a call is retried in a newer context.
const STALE_RETRIES: u32 = 3;

/// The longest [`Engine::place_frames`] waits for a document to answer.
const PLACE_TIMEOUT: Duration = Duration::from_secs(2);

/// The object group of the `<iframe>` elements [`Engine::place_frames`]
/// looks at, released when it is done.
const PLACE_GROUP: &str = "chrome-cdp-frames";

/// Where the element it is called on is in its document: the index of
/// each node on the path down to it, a shadow root being `-1` under its
/// host.
const PATH_JS: &str = "function () {
  const path = [];
  for (let node = this; node && node.parentNode; ) {
    const parent = node.parentNode;
    path.push(Array.prototype.indexOf.call(parent.childNodes, node));
    if (parent.nodeType === 11 && parent.host) {
      path.push(-1);
      node = parent.host;
    } else {
      node = parent;
    }
  }
  return path.reverse();
}";

impl Engine {
    /// The frames of tab `tab` (`t2`; the active tab if `None`), parents
    /// before their children, siblings in the order of their `<iframe>`
    /// elements.
    pub async fn frames(
        &self,
        tab: Option<&str>,
        timeout: Duration,
    ) -> Result<Vec<FrameInfo>, ErrorInfo> {
        let running = self.running()?;
        let page = self.page(&running, tab, timeout).await?;
        self.place_frames(&running, &page, timeout).await;
        Ok(running.tabs.frames(page.id))
    }

    /// Looks up where the `<iframe>` of each frame of `page` that has
    /// siblings is in its parent's document (`DOM.getFrameOwner`), so the
    /// tab's frames are listed in document order, not in the order
    /// chrome-cdp first saw them (which differs from run to run after
    /// `connect`). A frame it cannot place keeps its place after the
    /// others.
    pub(crate) async fn place_frames(&self, running: &Running, page: &Page, timeout: Duration) {
        let timeout = timeout.min(PLACE_TIMEOUT);
        let frames = running.tabs.frames(page.id);
        let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
        for frame in &frames {
            if let Some(parent) = &frame.parent {
                children.entry(parent).or_default().push(&frame.id);
            }
        }
        let places = children
            .into_iter()
            .filter(|(_, siblings)| siblings.len() > 1)
            .map(|(parent, siblings)| {
                self.place_siblings(running, page, parent, siblings, timeout)
            });
        let positions: Vec<(String, Vec<i64>)> =
            join_all(places).await.into_iter().flatten().collect();
        running.tabs.set_frame_positions(page.id, positions);
    }

    /// Where the `<iframe>` of each of `siblings` is in the document of
    /// frame `parent`; those it found.
    async fn place_siblings(
        &self,
        running: &Running,
        page: &Page,
        parent: &str,
        siblings: Vec<&str>,
        timeout: Duration,
    ) -> Vec<(String, Vec<i64>)> {
        let Ok(context) = self
            .frame_context(running, page, Some(parent), World::Isolated, timeout)
            .await
        else {
            return Vec::new();
        };
        let session = context.session.with_timeout(timeout);
        let places = siblings.into_iter().map(|id| {
            let session = session.clone();
            async move {
                let owner = session
                    .call(dom::GetFrameOwnerParams::new(id.to_owned()))
                    .await
                    .ok()?;
                let resolve = dom::ResolveNodeParams {
                    backend_node_id: Some(owner.backend_node_id),
                    execution_context_id: Some(context.context_id),
                    object_group: Some(PLACE_GROUP.to_owned()),
                    ..dom::ResolveNodeParams::default()
                };
                let object = session.call(resolve).await.ok()?.object.object_id?;
                let mut call = runtime::CallFunctionOnParams::new(PATH_JS.to_owned());
                call.object_id = Some(object);
                call.return_by_value = Some(true);
                let path = session.call(call).await.ok()?.result.value?;
                Some((id.to_owned(), serde_json::from_value(path).ok()?))
            }
        });
        let positions = join_all(places).await.into_iter().flatten().collect();
        let _ = session
            .call(runtime::ReleaseObjectGroupParams::new(
                PLACE_GROUP.to_owned(),
            ))
            .await;
        positions
    }

    /// Runs `expression` in `world` of frame `frame` (see
    /// [`Engine::frame_context`]) of tab `tab`, and returns its value
    /// (awaited if it is a promise). Holds the tab's lock.
    pub async fn evaluate(
        &self,
        tab: Option<&str>,
        frame: Option<&str>,
        world: World,
        expression: &str,
        timeout: Duration,
    ) -> Result<Value, ErrorInfo> {
        let running = self.running()?;
        let page = self.page(&running, tab, timeout).await?;
        let _tab = self.tabs.lock(&page.target_id).await;
        let result = self
            .in_frame(&running, &page, frame, world, timeout, |context| {
                let mut params = runtime::EvaluateParams::new(expression.to_owned());
                params.context_id = Some(context.context_id);
                params.return_by_value = Some(true);
                params.await_promise = Some(true);
                context.session.with_timeout(timeout).call(params)
            })
            .await?
            .map_err(|e| from_cdp(&e, &self.session))?;
        if let Some(exception) = result.exception_details {
            let message = exception
                .exception
                .as_ref()
                .and_then(|e| e.description.clone())
                .unwrap_or(exception.text);
            let first = message.lines().next().unwrap_or_default().to_owned();
            return Err(ErrorInfo::new(ErrorKind::EvalError, first)
                .hint(Hint::text("fix the expression; the page is unchanged"))
                .details(json!({ "exception": message })));
        }
        Ok(result.result.value.unwrap_or(Value::Null))
    }

    /// Makes `call` in `world` of frame `frame` (see
    /// [`Engine::frame_context`]) and returns its result; the outer error
    /// is about finding the frame. When the context went away before the
    /// call reached it (the frame navigated), the script did not run, so
    /// it retries in the new document's context.
    pub(crate) async fn in_frame<T, F>(
        &self,
        running: &Running,
        page: &Page,
        frame: Option<&str>,
        world: World,
        timeout: Duration,
        mut call: impl FnMut(&FrameContext) -> F,
    ) -> Result<cdp::Result<T>, ErrorInfo>
    where
        F: Future<Output = cdp::Result<T>>,
    {
        let mut attempts = 0;
        loop {
            let context = self
                .frame_context(running, page, frame, world, timeout)
                .await?;
            match call(&context).await {
                Err(e) if attempts < STALE_RETRIES && is_stale_context(&e) => {
                    attempts += 1;
                    running.tabs.forget_context(page.id, &context);
                }
                result => return Ok(result),
            }
        }
    }

    /// Where to run a script in `world` of the frame `frame` names (its
    /// id, `name` or a part of its URL; the main frame if `None`) in
    /// `page`. Waits up to `timeout` for the frame's document to have a
    /// context.
    pub(crate) async fn frame_context(
        &self,
        running: &Running,
        page: &Page,
        frame: Option<&str>,
        world: World,
        timeout: Duration,
    ) -> Result<FrameContext, ErrorInfo> {
        self.frame_and_context(running, page, frame, world, timeout)
            .await
            .map(|(_, context)| context)
    }

    /// [`Engine::frame_context`], with the id of the frame it is in.
    pub(crate) async fn frame_and_context(
        &self,
        running: &Running,
        page: &Page,
        frame: Option<&str>,
        world: World,
        timeout: Duration,
    ) -> Result<(String, FrameContext), ErrorInfo> {
        let frame = if let Some(query) = frame {
            self.find_frame(running, page, query, timeout).await?
        } else {
            // The main frame always comes first.
            let main = running.tabs.frames(page.id).into_iter().next();
            main.ok_or_else(|| {
                ErrorInfo::new(
                    ErrorKind::NotFound,
                    format!("tab {} went away", tab_name(page.id)),
                )
                .hint(Hint::run(Cmd::new("tabs").scoped(&self.session)))
            })?
        };
        running
            .tabs
            .context(page.id, &frame.id, world, timeout)
            .await
            .map(|context| (frame.id.clone(), context))
            .map_err(|e| self.frame_error(page.id, &frame, e, timeout))
    }

    /// The frame of `page` that `query` names (see
    /// [`crate::frames::FrameTree::find`]); when none or several do, an
    /// error that lists the tab's frames in document order.
    pub(crate) async fn find_frame(
        &self,
        running: &Running,
        page: &Page,
        query: &str,
        timeout: Duration,
    ) -> Result<FrameInfo, ErrorInfo> {
        if let Ok(frame) = running.tabs.find_frame(page.id, query) {
            return Ok(frame);
        }
        self.place_frames(running, page, timeout).await;
        running
            .tabs
            .find_frame(page.id, query)
            .map_err(|e| find_error(page.id, query, e, &self.session))
    }

    fn frame_error(
        &self,
        tab: u32,
        frame: &FrameInfo,
        err: FrameError,
        timeout: Duration,
    ) -> ErrorInfo {
        let what = if frame.parent.is_none() {
            format!("the main frame of tab {}", tab_name(tab))
        } else {
            format!("frame {}", describe(frame))
        };
        let details = json!({ "tab": tab_name(tab), "frame": frame });
        match err {
            FrameError::Gone => ErrorInfo::new(ErrorKind::NotFound, format!("{what} went away"))
                .hint(Hint::text(
                    "the page navigated or removed the frame; look at the page again",
                ))
                .details(details),
            FrameError::NoContext => ErrorInfo::new(
                ErrorKind::Timeout,
                format!(
                    "{what} has no document to run scripts in after {}",
                    format::duration(timeout)
                ),
            )
            .note("url", frame.url.clone())
            .hint(format!(
                "retry with a longer timeout, e.g. --timeout {}",
                format::duration(timeout * 2)
            ))
            .details(details),
            FrameError::Cdp(e) => from_cdp(&e, &self.session),
        }
    }
}

pub(crate) fn find_error(tab: u32, query: &str, err: FindError, session: &str) -> ErrorInfo {
    let frames_cmd = || Cmd::new("frames").scoped(session);
    let (info, frames) = match err {
        FindError::NotFound(frames) => (
            ErrorInfo::new(
                ErrorKind::NotFound,
                format!("no frame in tab {} matches {query:?}", tab_name(tab)),
            )
            .hint(Hint::run(frames_cmd()).why(
                "lists the tab's frames; name one by its name attribute, a part of its URL, or \
                 its id",
            )),
            frames,
        ),
        FindError::Ambiguous(frames) => (
            ErrorInfo::new(
                ErrorKind::Ambiguous,
                format!(
                    "{} frames in tab {} match {query:?}",
                    frames.len(),
                    tab_name(tab)
                ),
            )
            .hint(
                Hint::text("use a longer part of the URL, the frame's name, or its id")
                    .or(format!("{} shows the ids", frames_cmd().switch("json"))),
            ),
            frames,
        ),
    };
    let label = if info.kind == ErrorKind::Ambiguous {
        "match"
    } else {
        "frame"
    };
    let info = list(&frames)
        .into_iter()
        .fold(info, |info, line| info.note(label, line));
    info.details(json!({ "tab": tab_name(tab), "frame": query, "frames": frames }))
}

/// `name "pay" https://x/pay.html` or just the URL.
fn describe(frame: &FrameInfo) -> String {
    let url = if frame.url.is_empty() {
        "(no URL yet)"
    } else {
        &frame.url
    };
    if frame.name.is_empty() {
        url.to_owned()
    } else {
        format!("name {:?} {url}", frame.name)
    }
}

/// One note line per frame, at most [`LISTED_FRAMES`] of them.
fn list(frames: &[FrameInfo]) -> Vec<String> {
    let mut lines: Vec<String> = frames
        .iter()
        .take(LISTED_FRAMES)
        .map(|frame| {
            let main = if frame.parent.is_none() {
                " (main)"
            } else {
                ""
            };
            format!("{}{main}", describe(frame))
        })
        .collect();
    if frames.len() > LISTED_FRAMES {
        lines.push(format!("… and {} more", frames.len() - LISTED_FRAMES));
    }
    lines
}

/// Whether Chrome refused a call because its context is gone: the call
/// never ran.
fn is_stale_context(err: &cdp::CdpError) -> bool {
    matches!(err, cdp::CdpError::Protocol { message, .. }
        if message.starts_with("Cannot find context with specified id"))
}
