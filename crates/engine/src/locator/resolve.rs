//! Finding what a locator matches (plan 04, milestone 4.4).
//!
//! The engine resolves a locator's leading steps itself: a ref through the
//! ref registry ([`Engine::resolve_ref`], with its role+name fallback and
//! errors), `frame=` through the tab's frames. That gives a [`Place`]: a
//! frame's isolated world and, for a ref, the element to start from. A ref
//! to an iframe followed by more steps looks inside the iframe's document,
//! like `frame=`. The injected bundle then evaluates the other steps there
//! in one call.

use std::fmt::Write as _;
use std::time::Duration;

use cdp::domains::{dom, runtime};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::parse::{Locator, Step, parse_locator};
use crate::errors::from_cdp;
use crate::frames::World;
use crate::frames::commands::find_error;
use crate::injected::{self, GUARD, own_properties};
use crate::snapshot::{NodeKey, RefCall, Target, parse_ref};
use crate::tabs::{FrameContext, Page, tab_name};
use crate::{Engine, Running};

/// How commands that take locators resolve them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocateOptions {
    /// A ref whose element is gone fails, instead of being re-resolved by
    /// role and name (`--strict-refs`).
    #[serde(default)]
    pub strict_refs: bool,
    /// The attribute `testid=` reads; `data-testid` if `None`.
    #[serde(default)]
    pub testid_attribute: Option<String>,
}

impl LocateOptions {
    /// Parses `text` with these options.
    pub fn parse(&self, text: &str) -> Result<Locator, ErrorInfo> {
        parse_locator(text).map(|l| l.with_testid_attribute(self.testid_attribute.as_deref()))
    }
}

/// Where a locator's in-frame steps run.
#[derive(Debug, Clone)]
pub(crate) struct Place {
    /// The frame's id.
    pub(crate) frame: String,
    pub(crate) context: FrameContext,
    /// The element the steps start from: a ref's, or an iframe (the
    /// owner) when the locator names only a frame; the document if
    /// `None`.
    pub(crate) root: Option<Root>,
    /// Set when a ref was re-resolved by role and name: the note to show.
    pub(crate) note: Option<String>,
}

/// An element a locator starts from.
#[derive(Debug, Clone)]
pub(crate) struct Root {
    pub(crate) object_id: String,
    pub(crate) key: NodeKey,
}

impl Place {
    /// The `root` argument of the bundle's functions.
    pub(crate) fn root_argument(&self) -> runtime::CallArgument {
        match &self.root {
            Some(root) => runtime::CallArgument {
                value: None,
                unserializable_value: None,
                object_id: Some(root.object_id.clone()),
            },
            None => value(Value::Null),
        }
    }

    /// Lets go of the root's handle (without waiting).
    pub(crate) fn release(self) {
        if let Some(root) = self.root {
            let session = self.context.session;
            tokio::spawn(async move {
                let _ = session
                    .call(runtime::ReleaseObjectParams {
                        object_id: root.object_id,
                    })
                    .await;
            });
        }
    }
}

/// What a locator that names only a frame stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameOnly {
    /// Its `<iframe>` element, in the parent frame (actions, `--scope`).
    Owner,
    /// Its document (`wait --text … --in frame=…`).
    Document,
}

/// How many candidates an `Ambiguous` error lists (the bundle's
/// `LISTED`).
const LISTED: usize = 5;

/// The object group of a locator's results until they are read.
const GROUP: &str = "chrome-cdp-locate";

/// One candidate of an `Ambiguous` error, as the bundle describes it.
#[derive(Debug, Clone, Default, Deserialize)]
struct Candidate {
    role: String,
    name: String,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    visible: bool,
}

#[derive(Debug, Default, Deserialize)]
struct NotOne {
    count: usize,
    candidates: Vec<Candidate>,
}

impl Engine {
    /// Calls `function` with `this` set to the one element `locator`
    /// matches in tab `tab`, in chrome-cdp's isolated world, and returns
    /// its value (awaited if it is a promise). Fails with `NotFound` or
    /// `Ambiguous` unless exactly one element matches. Holds the tab's
    /// lock. The building block for commands that act on an element.
    pub async fn call_on_locator(
        &self,
        tab: Option<&str>,
        locator: &str,
        options: &LocateOptions,
        function: &str,
        timeout: Duration,
    ) -> Result<RefCall, ErrorInfo> {
        let locator = options.parse(locator)?;
        let running = self.running()?;
        let page = self.page(&running, tab, timeout).await?;
        let _tab = self.tabs.lock(&page.target_id).await;
        let target = self
            .locate_one(&running, &page, &locator, options, None, timeout)
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
        Ok(RefCall {
            value: function_value(result?)?,
            note,
        })
    }

    /// Calls `function` with `this` set to the array of every element
    /// `locator` matches in tab `tab` (in document order, maybe empty), in
    /// chrome-cdp's isolated world, and returns its value. For read-only
    /// commands (`count`, `text`), which accept many matches. Holds the
    /// tab's lock.
    pub async fn call_on_all(
        &self,
        tab: Option<&str>,
        locator: &str,
        options: &LocateOptions,
        function: &str,
        timeout: Duration,
    ) -> Result<RefCall, ErrorInfo> {
        let locator = options.parse(locator)?;
        let running = self.running()?;
        let page = self.page(&running, tab, timeout).await?;
        let _tab = self.tabs.lock(&page.target_id).await;
        let place = self
            .place(
                &running,
                &page,
                &locator,
                options,
                FrameOnly::Owner,
                timeout,
            )
            .await?;
        let declaration = format!(
            "function (steps, root) {{ {GUARD} const els = globalThis.__ccdp.query(steps, root);\n\
             return ({function}).call(els); }}"
        );
        let mut params = runtime::CallFunctionOnParams::new(declaration);
        params.arguments = Some(vec![steps_argument(&locator), place.root_argument()]);
        params.return_by_value = Some(true);
        params.await_promise = Some(true);
        let result = injected::call(
            &place.context.session.with_timeout(timeout),
            place.context.context_id,
            params,
        )
        .await
        .map_err(|e| from_cdp(&e, &self.session));
        let note = place.note.clone();
        place.release();
        let result = result?;
        if let Some(message) = locator_error(&result) {
            return Err(bad_locator(&locator, &message));
        }
        Ok(RefCall {
            value: function_value(result)?,
            note,
        })
    }

    /// The one element `locator` matches in `page`. `retry` gives the
    /// command to run with another locator, for the `Ambiguous` error's
    /// hint. Call it holding the tab's lock.
    pub(crate) async fn locate_one(
        &self,
        running: &Running,
        page: &Page,
        locator: &Locator,
        options: &LocateOptions,
        retry: Option<&(dyn Fn(&str) -> Cmd + Sync)>,
        timeout: Duration,
    ) -> Result<Target, ErrorInfo> {
        let place = self
            .place(running, page, locator, options, FrameOnly::Owner, timeout)
            .await?;
        let steps = locator.in_frame_steps();
        if steps.is_empty()
            && let Some(root) = &place.root
        {
            return Ok(Target {
                key: root.key.clone(),
                context: place.context.clone(),
                object_id: root.object_id.clone(),
                note: place.note.clone(),
            });
        }
        let session = place.context.session.with_timeout(timeout);
        let mut params = runtime::CallFunctionOnParams::new(format!(
            "function (steps, root) {{ {GUARD} return globalThis.__ccdp.one(steps, root); }}"
        ));
        params.arguments = Some(vec![steps_argument(locator), place.root_argument()]);
        params.object_group = Some(GROUP.to_owned());
        let result = injected::call(&session, place.context.context_id, params).await;
        let outcome = async {
            let result = result.map_err(|e| from_cdp(&e, &self.session))?;
            if let Some(message) = locator_error(&result) {
                return Err(bad_locator(locator, &message));
            }
            if let Some(exception) = result.exception_details {
                return Err(ErrorInfo::new(
                    ErrorKind::EvalError,
                    format!("looking for {locator} failed: {}", exception.text),
                ));
            }
            let object = result.result;
            let Some(object_id) = object.object_id else {
                return Err(self.not_found(page, locator));
            };
            if object.subtype == Some(runtime::RemoteObjectSubtype::Node) {
                let backend = describe(&session, &object_id)
                    .await
                    .map_err(|e| from_cdp(&e, &self.session))?
                    .backend_node_id;
                return Ok(Target {
                    key: NodeKey {
                        frame: place.frame.clone(),
                        backend,
                    },
                    context: place.context.clone(),
                    object_id,
                    note: place.note.clone(),
                });
            }
            Err(self
                .not_one(running, page, &place, locator, &object_id, retry)
                .await)
        }
        .await;
        // The element's handle stays (the target releases it); the rest
        // of the call's objects, and the root it started from, go.
        if outcome.is_err() {
            let _ = session
                .call(runtime::ReleaseObjectGroupParams::new(GROUP.to_owned()))
                .await;
        }
        place.release();
        outcome
    }

    /// Where `locator`'s in-frame steps run in `page`: resolves its ref
    /// and `frame=` steps. Call it holding the tab's lock.
    pub(crate) async fn place(
        &self,
        running: &Running,
        page: &Page,
        locator: &Locator,
        options: &LocateOptions,
        frame_only: FrameOnly,
        timeout: Duration,
    ) -> Result<Place, ErrorInfo> {
        let rest = locator.in_frame_steps().len();
        let head = &locator.steps[..locator.steps.len() - rest];
        let mut frame = running
            .tabs
            .frames(page.id)
            .into_iter()
            .next()
            .ok_or_else(|| self.tab_gone(page))?
            .id;
        let mut root: Option<(FrameContext, Root)> = None;
        let mut note = None;
        for step in head {
            if let Some((context, root)) = root.take() {
                release(&context, root.object_id);
            }
            match step {
                Step::Ref { number } => {
                    let target = self
                        .resolve_ref(running, page, *number, options.strict_refs, timeout)
                        .await?;
                    note = target.note.clone();
                    frame.clone_from(&target.key.frame);
                    // A ref to an iframe, then steps: look in its document.
                    let inner = if rest > 0 {
                        self.content_frame(&target.context, &target.object_id, timeout)
                            .await?
                    } else {
                        None
                    };
                    match inner {
                        Some(inner) => {
                            frame = inner;
                            target.release();
                        }
                        None => {
                            root = Some((
                                target.context.clone(),
                                Root {
                                    object_id: target.object_id.clone(),
                                    key: target.key.clone(),
                                },
                            ));
                        }
                    }
                }
                Step::Frame { query } => {
                    frame = match parse_ref(query) {
                        Some(number) => {
                            let target = self
                                .resolve_ref(running, page, number, options.strict_refs, timeout)
                                .await?;
                            note = target.note.clone();
                            let inner = self
                                .content_frame(&target.context, &target.object_id, timeout)
                                .await;
                            target.release();
                            inner?.ok_or_else(|| {
                                ErrorInfo::new(
                                    ErrorKind::BadLocator,
                                    format!("frame={query}: e{number} is not an iframe"),
                                )
                                .hint(Hint::text(
                                    "use the ref of an iframe line of the snapshot, or the frame's name or a part of its URL",
                                ))
                            })?
                        }
                        None => {
                            running
                                .tabs
                                .find_frame(page.id, query)
                                .map_err(|e| find_error(page.id, query, e))?
                                .id
                        }
                    };
                }
                _ => {}
            }
        }
        let names_a_frame = rest == 0 && matches!(head.last(), Some(Step::Frame { .. }));
        if names_a_frame && frame_only == FrameOnly::Owner {
            let (parent, context, root) = self.frame_owner(running, page, &frame, timeout).await?;
            return Ok(Place {
                frame: parent,
                context,
                root: Some(root),
                note,
            });
        }
        if let Some((context, root)) = root {
            return Ok(Place {
                frame,
                context,
                root: Some(root),
                note,
            });
        }
        let context = self
            .frame_context(running, page, Some(&frame), World::Isolated, timeout)
            .await?;
        Ok(Place {
            frame,
            context,
            root: None,
            note,
        })
    }

    /// The frame an `<iframe>` element shows, or `None` if it is not one.
    async fn content_frame(
        &self,
        context: &FrameContext,
        object_id: &str,
        timeout: Duration,
    ) -> Result<Option<String>, ErrorInfo> {
        let node = describe(&context.session.with_timeout(timeout), object_id)
            .await
            .map_err(|e| from_cdp(&e, &self.session))?;
        Ok(node
            .frame_id
            .filter(|_| matches!(node.local_name.as_str(), "iframe" | "frame")))
    }

    /// The `<iframe>` element of frame `frame`: its parent frame, and a
    /// handle on it there.
    async fn frame_owner(
        &self,
        running: &Running,
        page: &Page,
        frame: &str,
        timeout: Duration,
    ) -> Result<(String, FrameContext, Root), ErrorInfo> {
        let frames = running.tabs.frame_sources(page.id);
        let gone = || {
            ErrorInfo::new(
                ErrorKind::NotFound,
                format!(
                    "the frame's <iframe> is no longer in tab {}",
                    tab_name(page.id)
                ),
            )
            .details(json!({ "tab": tab_name(page.id), "frame": frame }))
            .hint(
                Hint::run(Cmd::new("snapshot").session(&self.session))
                    .why("shows the page's iframes"),
            )
        };
        let Some(parent) = frames
            .iter()
            .find(|f| f.info.id == frame)
            .and_then(|f| f.info.parent.clone())
        else {
            return Err(ErrorInfo::new(
                ErrorKind::BadLocator,
                "frame= names the tab's main frame, which has no <iframe>",
            )
            .hint(Hint::text(
                "name an iframe: frame=<name or part of its URL>",
            )));
        };
        let session = frames
            .iter()
            .find(|f| f.info.id == parent)
            .map(|f| f.session.clone())
            .ok_or_else(gone)?;
        let owner = session
            .with_timeout(timeout)
            .call(dom::GetFrameOwnerParams::new(frame.to_owned()))
            .await
            .map_err(|_| gone())?;
        let key = NodeKey {
            frame: parent.clone(),
            backend: owner.backend_node_id,
        };
        let (context, object_id) = self
            .live_node(running, page, &key, timeout)
            .await?
            .ok_or_else(gone)?;
        Ok((parent, context, Root { object_id, key }))
    }

    /// No element matches.
    fn not_found(&self, page: &Page, locator: &Locator) -> ErrorInfo {
        let mut info = ErrorInfo::new(ErrorKind::NotFound, format!("no element matches {locator}"))
            .hint(
                Hint::run(Cmd::new("snapshot").session(&self.session))
                    .why("shows the page; use a ref from it"),
            )
            .details(
                json!({ "locator": locator.to_string(), "count": 0, "tab": tab_name(page.id) }),
            );
        if locator
            .in_frame_steps()
            .iter()
            .any(|s| matches!(s, Step::Text { .. }))
        {
            info = info.note("note", "text= matches visible elements only");
        }
        info
    }

    /// The error when `locator` matched none or several elements: reads
    /// the bundle's `{json, els}` answer and gives the candidates refs.
    async fn not_one(
        &self,
        running: &Running,
        page: &Page,
        place: &Place,
        locator: &Locator,
        object_id: &str,
        retry: Option<&(dyn Fn(&str) -> Cmd + Sync)>,
    ) -> ErrorInfo {
        let session = &place.context.session;
        let read = async {
            let properties = own_properties(session, object_id).await?;
            let json = properties
                .iter()
                .find(|p| p.name == "json")
                .and_then(|p| {
                    p.value
                        .as_ref()?
                        .value
                        .as_ref()?
                        .as_str()
                        .map(str::to_owned)
                })
                .unwrap_or_default();
            let els = properties
                .iter()
                .find(|p| p.name == "els")
                .and_then(|p| p.value.as_ref()?.object_id.clone());
            let mut keys = vec![None; LISTED];
            if let Some(els) = els {
                for property in own_properties(session, &els).await? {
                    let (Ok(index), Some(id)) = (
                        property.name.parse::<usize>(),
                        property.value.and_then(|v| v.object_id),
                    ) else {
                        continue;
                    };
                    if index < LISTED
                        && let Ok(node) = describe(session, &id).await
                    {
                        keys[index] = Some(NodeKey {
                            frame: place.frame.clone(),
                            backend: node.backend_node_id,
                        });
                    }
                }
            }
            cdp::Result::Ok((json, keys))
        };
        let (json, keys) = match read.await {
            Ok(read) => read,
            Err(e) => return from_cdp(&e, &self.session),
        };
        let not_one: NotOne = serde_json::from_str(&json).unwrap_or_default();
        if not_one.count == 0 {
            return self.not_found(page, locator);
        }
        let refs: Vec<Option<String>> = not_one
            .candidates
            .iter()
            .zip(&keys)
            .map(|(c, key)| {
                let key = key.as_ref()?;
                running
                    .tabs
                    .snapshot_memory(page.id, |m| m.refs.assign(key, &c.role, &c.name))
                    .map(|n| format!("e{n}"))
            })
            .collect();
        ambiguous(locator, &not_one, &refs, retry, &self.session)
    }

    fn tab_gone(&self, page: &Page) -> ErrorInfo {
        ErrorInfo::new(
            ErrorKind::NotFound,
            format!("tab {} went away", tab_name(page.id)),
        )
        .hint(Hint::run(Cmd::new("tabs").session(&self.session)))
    }
}

/// `Ambiguous`: how many match, and the first few with their refs:
///
/// ```text
/// error: 3 elements match text=Delete: e41, e44, e47
///   e41: button "Delete" (in row "Invoice 12")
///   …
///   hint: chrome-cdp snapshot --scope e41   (the first one), or add >> nth=<index>
/// ```
fn ambiguous(
    locator: &Locator,
    found: &NotOne,
    refs: &[Option<String>],
    retry: Option<&(dyn Fn(&str) -> Cmd + Sync)>,
    session: &str,
) -> ErrorInfo {
    let shown: Vec<&str> = refs.iter().flatten().map(String::as_str).collect();
    let more = if found.count > shown.len() {
        ", …"
    } else {
        ""
    };
    let mut info = ErrorInfo::new(
        ErrorKind::Ambiguous,
        format!(
            "{} elements match {locator}: {}{more}",
            found.count,
            shown.join(", ")
        ),
    );
    let mut candidates = Vec::new();
    for (candidate, reference) in found.candidates.iter().zip(refs) {
        let mut line = candidate.role.clone();
        if !candidate.name.is_empty() {
            let _ = write!(line, " \"{}\"", candidate.name.replace('"', "\\\""));
        }
        let mut notes = Vec::new();
        if let Some(context) = &candidate.context {
            notes.push(format!("in {context}"));
        }
        if !candidate.visible {
            notes.push("hidden".to_owned());
        }
        if !notes.is_empty() {
            let _ = write!(line, " ({})", notes.join(", "));
        }
        let label = reference.clone().unwrap_or_else(|| "?".to_owned());
        info = info.note(label, line);
        candidates.push(json!({
            "ref": reference,
            "role": candidate.role,
            "name": candidate.name,
            "context": candidate.context,
            "visible": candidate.visible,
        }));
    }
    if found.count > found.candidates.len() {
        info = info.note(
            "…",
            format!("{} more", found.count - found.candidates.len()),
        );
    }
    let narrow = format!("add >> nth=<index> to {locator}");
    let hint = match (retry, shown.first()) {
        (Some(retry), Some(first)) => Hint::run(retry(first).session(session))
            .why("the first one")
            .or(narrow),
        _ => Hint::text(format!("use one of the refs, or {narrow}")),
    };
    info.hint(hint).details(json!({
        "locator": locator.to_string(),
        "count": found.count,
        "candidates": candidates,
    }))
}

fn steps_argument(locator: &Locator) -> runtime::CallArgument {
    value(serde_json::to_value(locator.in_frame_steps()).unwrap_or(Value::Null))
}

fn value(value: Value) -> runtime::CallArgument {
    runtime::CallArgument {
        value: Some(value),
        unserializable_value: None,
        object_id: None,
    }
}

async fn describe(session: &cdp::Session, object_id: &str) -> cdp::Result<dom::Node> {
    let params = dom::DescribeNodeParams {
        object_id: Some(object_id.to_owned()),
        ..dom::DescribeNodeParams::default()
    };
    Ok(session.call(params).await?.node)
}

fn release(context: &FrameContext, object_id: String) {
    let session = context.session.clone();
    tokio::spawn(async move {
        let _ = session
            .call(runtime::ReleaseObjectParams { object_id })
            .await;
    });
}

/// The message of a `LocatorError` the bundle threw (a bad selector or
/// regex).
fn locator_error(result: &runtime::CallFunctionOnReturns) -> Option<String> {
    let exception = result.exception_details.as_ref()?;
    let description = exception
        .exception
        .as_ref()
        .and_then(|e| e.description.as_deref())
        .unwrap_or(&exception.text);
    let message = description.strip_prefix("LocatorError: ")?;
    Some(message.lines().next().unwrap_or_default().to_owned())
}

pub(crate) fn bad_locator(locator: &Locator, message: &str) -> ErrorInfo {
    ErrorInfo::new(ErrorKind::BadLocator, format!("{locator}: {message}")).hint(Hint::text(
        "fix the selector; see `chrome-cdp snapshot` for what the page has",
    ))
}

/// The value a user function returned, or `EvalError` if it threw.
fn function_value(result: runtime::CallFunctionOnReturns) -> Result<Value, ErrorInfo> {
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
    Ok(result.result.value.unwrap_or(Value::Null))
}
