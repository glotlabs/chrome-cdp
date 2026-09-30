//! From page to source (plan 15): which line of the page's code made an
//! element.
//!
//! `source <loc>` (milestone 15.2) asks, in order, and stops at the first
//! that answers:
//!
//! 1. the framework's development build, through the adapters of
//!    `injected/src/frameworks.ts` (React 18 and 19, Vue 3, Svelte 5), run
//!    in the page's **main** world, where the page's properties on DOM
//!    nodes are (an isolated world cannot see them);
//! 2. the node's creation stack, as Chrome recorded it (see
//!    `tabs::node_stacks`), mapped through source maps to the files on
//!    disk, the frameworks' and libraries' frames left out;
//! 3. the document's HTML, for an element parsed from it.
//!
//! What the adapters return comes from the page's own world, so a page
//! can lie: it is shown as the page's debug info, and nothing acts on it.
//! `snapshot --source` asks the same for every node with a ref, in one
//! call per frame.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use cdp::Session;
use cdp::domains::{dom, runtime};
use futures_util::future::join_all;
use protocol::schema::{
    ActedElement, NodeSource, SourceComponent, SourceData, SourcePlace, StackFrame,
};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::Instant;

use crate::context::Scoped as _;
use crate::errors::from_cdp;
use crate::frames::World;
use crate::input::origin;
use crate::locator::{LocateOptions, until_found};
use crate::observe::format::stack_frames;
use crate::observe::shown_source;
use crate::places;
use crate::request::{SourceRequest, millis};
use crate::snapshot::{NodeKey, Target};
use crate::tabs::node_stacks::{self, Recording};
use crate::tabs::{Page, tab_name};
use crate::{Engine, Running};

/// The framework adapters, built from `injected/src/frameworks.ts`.
const FRAMEWORKS: &str = include_str!("injected/frameworks.js");

/// How many frames of a creation stack are read, across async parents.
const MAX_FRAMES: usize = 64;

/// How many elements one `snapshot --source` call asks about.
const BATCH: usize = 100;

/// The object group of the handles `source` makes in the main world.
const GROUP: &str = "chrome-cdp-source";

/// Where a frame's code comes from, if it is a library's, a framework's
/// or the dev server's rather than the app's own.
const NOT_THE_APP: [&str; 6] = [
    "/node_modules/",
    "/.vite/deps/",
    "/@vite/",
    "/@react-refresh",
    "/webpack/runtime/",
    "/__vite",
];

/// Functions that make JSX elements: a React 19 debug stack starts in
/// one of them, even where no source map says it is React's.
const JSX_FACTORIES: [&str; 5] = ["jsxDEV", "jsx", "jsxs", "createElement", "cloneElement"];

/// The function `Runtime.callFunctionOn` runs: the adapters, then `call`
/// (an expression of `__ccdpFrameworks`, `this` and `args`). The bundle
/// is a local of the function, so nothing is left on the page. (Its
/// `"use strict"` rules out a rest parameter: the arguments are read
/// from `arguments`.)
pub(crate) fn adapters(call: &str) -> String {
    format!("function () {{\n{FRAMEWORKS}\nconst args = Array.from(arguments);\nreturn {call};\n}}")
}

/// What an adapter found (see `Found` in `frameworks.ts`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Found {
    framework: String,
    version: String,
    dev: bool,
    at: Option<PagePlace>,
    components: Vec<PageComponent>,
    more: u64,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct PagePlace {
    file: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
    frames: Vec<PageFrame>,
    #[serde(rename = "match")]
    matching: Option<Match>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
struct PageFrame {
    #[serde(rename = "fn")]
    function: String,
    url: String,
    line: u32,
    column: u32,
}

/// What to look for in a Vue component's `<template>`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
struct Match {
    tag: String,
    index: Option<usize>,
    count: Option<usize>,
    text: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PageComponent {
    pub(crate) name: String,
    pub(crate) place: Option<PagePlace>,
}

/// What `source` found for an element.
#[derive(Debug, Clone, Default)]
struct Answer {
    at: Option<SourcePlace>,
    components: Vec<SourceComponent>,
    more: u64,
    via: String,
    version: Option<String>,
    warnings: Vec<String>,
}

/// Why nothing says where an element came from.
#[derive(Debug, Clone)]
enum NoSource {
    /// The framework is there, its debug info is not.
    NotDev { framework: String, version: String },
    /// Chrome recorded no stack: it began recording after the node was
    /// made.
    BeforeStacks,
    /// Chrome does not record stacks in the element's frame.
    StacksOff,
    /// Only libraries' frames are on its creation stack.
    LibraryOnly { innermost: Option<SourcePlace> },
}

impl Engine {
    /// `source`: the file and line an element came from, and the
    /// components around it.
    pub async fn source(&self, req: SourceRequest) -> Result<Output, ErrorInfo> {
        let options = LocateOptions {
            strict_refs: req.strict_refs,
            testid_attribute: req.testid_attribute.clone(),
        };
        let locator = options.parse(&req.locator)?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let deadline = Instant::now() + timeout;
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let work = async {
            // Elements seen from now on get refs only while this holds.
            let generation = running
                .tabs
                .snapshot_memory(page.id, |m| m.refs.generation());
            let retry = |r: &str| Cmd::new("source").arg(r);
            let retry = &retry;
            let target = until_found(deadline, timeout, || {
                let (running, page, locator, options) = (&running, &page, &locator, &options);
                async move {
                    let _tab = self.tabs.lock(&page.target_id).await;
                    self.locate_one(running, page, locator, options, Some(retry), timeout)
                        .await
                }
            })
            .await?;
            let element = self.name_target(&running, &page, &target, generation).await;
            let answer = self.answer(&running, &page, &target, timeout).await;
            let note = target.note.clone();
            target.release();
            Ok((element, note, answer?))
        };
        let (element, note, answer) =
            Box::pin(self.unless_blocked(&running, &page, mark, work)).await?;
        let answer = answer.map_err(|why| self.no_source(&element, why))?;
        let page_origin = Self::page_origin(&running, &page);
        let depth = usize::try_from(req.depth).unwrap_or(usize::MAX);
        let mut components = answer.components;
        let cut = components.len().saturating_sub(depth);
        components.truncate(depth);
        let more = answer.more + cut as u64;
        let text = source_text(
            &element,
            answer.at.as_ref(),
            &components,
            more,
            &answer.via,
            answer.version.as_deref(),
            page_origin.as_deref(),
        );
        let quiet = answer
            .at
            .as_ref()
            .or_else(|| components.iter().find_map(|c| c.place.as_ref()))
            .map(|p| place_text(p, true, page_origin.as_deref()))
            .unwrap_or_default();
        let data = SourceData {
            tab: tab_name(page.id),
            element,
            at: answer.at,
            components,
            more,
            via: answer.via,
            version: answer.version,
        };
        let mut output = Output::typed(&data, text).quiet(quiet);
        if let Some(note) = note {
            output = output.warning(note);
        }
        for warning in answer.warnings {
            output = output.warning(warning);
        }
        Ok(output)
    }

    /// The stack that created the element `locator` matches in tab `tab`
    /// (the active one if `None`), innermost first: each frame
    /// source-mapped and found on disk (see `map_stack`), without the
    /// frames of libraries and frameworks (`node_modules/`, Vite's
    /// prebundled deps, dev-server runtimes). `None` when Chrome has no
    /// stack for it: made before node stacks were on (`open
    /// --node-stacks`), or parsed from the HTML.
    pub async fn node_stack(
        &self,
        locator: &str,
        tab: Option<&str>,
        timeout_ms: u64,
    ) -> Result<Option<Vec<StackFrame>>, ErrorInfo> {
        let options = LocateOptions::default();
        let locator = options.parse(locator)?;
        let running = self.running()?;
        let timeout = millis(timeout_ms);
        let page = self.page(&running, tab, timeout).await?;
        let target = {
            let _tab = self.tabs.lock(&page.target_id).await;
            self.locate_one(&running, &page, &locator, &options, None, timeout)
                .await?
        };
        let session = target.context.session.with_timeout(timeout);
        let stack = node_stacks::node_stack(&session, &target.object_id).await;
        target.release();
        let Some(stack) = stack.map_err(|e| from_cdp(&e, &self.session))? else {
            return Ok(None);
        };
        let mut frames = self.creation_frames(&running, &page, &stack, timeout).await;
        frames.retain(|frame| !not_the_app(frame));
        Ok(Some(frames))
    }

    /// A creation stack's frames, across its async parents (at most
    /// [`MAX_FRAMES`]), each source-mapped (whatever the page) and found
    /// on disk.
    async fn creation_frames(
        &self,
        running: &Running,
        page: &Page,
        stack: &runtime::StackTrace,
        timeout: Duration,
    ) -> Vec<StackFrame> {
        let mut frames = stack_of(stack);
        let mut refs: Vec<&mut StackFrame> = frames.iter_mut().collect();
        self.map_stack(running, page, &mut refs, true, timeout)
            .await;
        frames
    }

    /// `snapshot --source`: where each node of `refs` (ref numbers and
    /// their nodes) came from, by ref number: the framework's debug info,
    /// else its creation stack. One adapter call per frame (in batches),
    /// one document request per frame for the stacks, and one pass over
    /// the source maps for all of them; nodes nothing answers for are
    /// left out.
    pub(crate) async fn node_sources(
        &self,
        running: &Running,
        page: &Page,
        refs: &[(u32, NodeKey)],
        timeout: Duration,
    ) -> HashMap<u32, NodeSource> {
        let mut by_frame: BTreeMap<&str, Vec<(u32, dom::BackendNodeId)>> = BTreeMap::new();
        for (reference, key) in refs {
            by_frame
                .entry(key.frame.as_str())
                .or_default()
                .push((*reference, key.backend));
        }
        let mut founds: Vec<(u32, Found)> = Vec::new();
        // A node's stack, and whether a framework without its debug info
        // (a production build) rendered it.
        let mut stacks: Vec<(u32, bool, runtime::StackTrace)> = Vec::new();
        for (frame, nodes) in by_frame {
            let Ok(context) = self
                .frame_context(running, page, Some(frame), World::Main, timeout)
                .await
            else {
                continue;
            };
            let session = context.session.with_timeout(timeout);
            let backends: Vec<dom::BackendNodeId> = nodes.iter().map(|n| n.1).collect();
            let objects = resolve_all(&session, context.context_id, &backends).await;
            let found = adapters_on(&session, context.context_id, &objects).await;
            let mut rest = Vec::new();
            for (((reference, _), object), found) in nodes.iter().zip(&objects).zip(found) {
                match (found, object) {
                    (Some(found), _) if found.dev => founds.push((*reference, found)),
                    (found, Some(object)) => {
                        rest.push((*reference, found.is_some(), object.clone()));
                    }
                    (_, None) => {}
                }
            }
            let objects: Vec<String> = rest.iter().map(|r| r.2.clone()).collect();
            let traces = node_stacks::node_stacks_of(&session, &objects).await;
            for ((reference, not_dev, _), trace) in rest.into_iter().zip(traces) {
                if let Some(trace) = trace {
                    stacks.push((reference, not_dev, trace));
                }
            }
            release(&session);
        }
        let mut sources = HashMap::new();
        let answers = self
            .framework_answers(
                running,
                page,
                &founds.iter().map(|f| &f.1).collect::<Vec<_>>(),
                timeout,
            )
            .await;
        for ((reference, _), answer) in founds.iter().zip(&answers) {
            if let Some(source) = node_source(answer) {
                sources.insert(*reference, source);
            }
        }
        let mut frames: Vec<Vec<StackFrame>> = stacks.iter().map(|s| stack_of(&s.2)).collect();
        {
            let mut all: Vec<&mut StackFrame> = frames.iter_mut().flatten().collect();
            if !all.is_empty() {
                self.map_stack(running, page, &mut all, true, timeout).await;
            }
        }
        for ((reference, not_dev, _), frames) in stacks.iter().zip(&frames) {
            let app = frames
                .iter()
                .filter(|f| !not_the_app(f))
                .find(|f| !not_dev || f.original.is_some());
            if let Some(frame) = app {
                sources.insert(
                    *reference,
                    NodeSource {
                        component: None,
                        place: Some(frame_place(frame)),
                    },
                );
            }
        }
        sources
    }

    /// Where `target` came from: the framework's debug info, else its
    /// creation stack, else the document's HTML. The outer error is a
    /// failed call; the inner one why nothing answered.
    async fn answer(
        &self,
        running: &Running,
        page: &Page,
        target: &Target,
        timeout: Duration,
    ) -> Result<Result<Answer, NoSource>, ErrorInfo> {
        let found = self
            .frameworks(
                running,
                page,
                &target.key.frame,
                &[target.key.backend],
                timeout,
            )
            .await?
            .pop()
            .flatten();
        if let Some(found) = found.as_ref().filter(|f| f.dev) {
            return Ok(Ok(self
                .framework_answer(running, page, found, timeout)
                .await));
        }
        // A framework without its debug info: a production build.
        let not_dev = found.map(|f| NoSource::NotDev {
            framework: f.framework,
            version: f.version,
        });
        let session = target.context.session.with_timeout(timeout);
        let stack = node_stacks::node_stack(&session, &target.object_id)
            .await
            .map_err(|e| from_cdp(&e, &self.session))?;
        let Some(stack) = stack else {
            if let Some(not_dev) = not_dev {
                return Ok(Err(not_dev));
            }
            return Ok(
                match running.tabs.node_stacks_recording(&target.context.session) {
                    Recording::SinceLoad => Ok(Self::html_answer(running, page, &target.key.frame)),
                    Recording::Later => Err(NoSource::BeforeStacks),
                    Recording::Off => Err(NoSource::StacksOff),
                },
            );
        };
        let frames = self.creation_frames(running, page, &stack, timeout).await;
        // In a production build, the framework's frames and the app's are
        // one bundle: only a source map tells them apart.
        let app = frames
            .iter()
            .filter(|f| !not_the_app(f))
            .find(|f| not_dev.is_none() || f.original.is_some());
        let Some(frame) = app else {
            return Ok(Err(not_dev.unwrap_or_else(|| NoSource::LibraryOnly {
                innermost: frames.first().map(frame_place),
            })));
        };
        let mut warnings = Vec::new();
        if frame.original.is_none() && frame.file.is_none() {
            warnings.push(format!(
                "no source map for {}: the place is in the script the page ran",
                frame.url
            ));
        }
        Ok(Ok(Answer {
            at: Some(frame_place(frame)),
            via: "node_stack".to_owned(),
            warnings,
            ..Answer::default()
        }))
    }

    /// An element parsed from the HTML: its frame's document.
    fn html_answer(running: &Running, page: &Page, frame: &str) -> Answer {
        let url = running
            .tabs
            .frames(page.id)
            .into_iter()
            .find(|f| f.id == frame)
            .map(|f| f.url)
            .unwrap_or_default();
        let root = places::root();
        let file = places::to_disk(&url, root.as_deref()).map(|f| f.to_string_lossy().into_owned());
        Answer {
            at: Some(SourcePlace {
                source: url,
                line: None,
                column: None,
                file,
                matched: false,
            }),
            via: "html".to_owned(),
            ..Answer::default()
        }
    }

    /// What the adapters find for the elements `backends` of frame
    /// `frame`, in the page's main world: `None` for an element no
    /// framework rendered (or one gone).
    pub(crate) async fn frameworks(
        &self,
        running: &Running,
        page: &Page,
        frame: &str,
        backends: &[dom::BackendNodeId],
        timeout: Duration,
    ) -> Result<Vec<Option<Found>>, ErrorInfo> {
        let values = self
            .on_elements(
                running,
                page,
                frame,
                backends,
                "__ccdpFrameworks.sources(args)",
                timeout,
            )
            .await?;
        Ok(values.into_iter().map(found_of).collect())
    }

    /// Runs `call` of the adapters (an expression of `__ccdpFrameworks`
    /// and `args`, one value per element) on the elements `backends` of
    /// frame `frame`, in the page's main world: `None` for an element
    /// gone, or when the adapters threw.
    pub(crate) async fn on_elements(
        &self,
        running: &Running,
        page: &Page,
        frame: &str,
        backends: &[dom::BackendNodeId],
        call: &str,
        timeout: Duration,
    ) -> Result<Vec<Option<Value>>, ErrorInfo> {
        let context = self
            .frame_context(running, page, Some(frame), World::Main, timeout)
            .await?;
        let session = context.session.with_timeout(timeout);
        let objects = resolve_all(&session, context.context_id, backends).await;
        let values = run_adapters(&session, context.context_id, &objects, call).await;
        release(&session);
        Ok(values)
    }

    /// The answer of a framework's debug info.
    async fn framework_answer(
        &self,
        running: &Running,
        page: &Page,
        found: &Found,
        timeout: Duration,
    ) -> Answer {
        self.framework_answers(running, page, &[found], timeout)
            .await
            .pop()
            .unwrap_or_default()
    }

    /// The answers of frameworks' debug info, their places resolved in
    /// one go (one pass over the source maps).
    async fn framework_answers(
        &self,
        running: &Running,
        page: &Page,
        founds: &[&Found],
        timeout: Duration,
    ) -> Vec<Answer> {
        let places: Vec<&PagePlace> = founds
            .iter()
            .flat_map(|found| {
                found
                    .at
                    .iter()
                    .chain(found.components.iter().filter_map(|c| c.place.as_ref()))
            })
            .collect();
        let mut resolved = self
            .resolve_places(running, page, &places, timeout)
            .await
            .into_iter();
        founds
            .iter()
            .map(|found| {
                let at = found.at.as_ref().and_then(|_| resolved.next().flatten());
                let components: Vec<SourceComponent> = found
                    .components
                    .iter()
                    .map(|c| SourceComponent {
                        name: c.name.clone(),
                        place: c.place.as_ref().and_then(|_| resolved.next().flatten()),
                    })
                    .collect();
                let mut warnings = Vec::new();
                if let Some(at) = &at
                    && at.line.is_none()
                {
                    let name = Path::new(&at.source)
                        .file_name()
                        .map_or_else(|| at.source.clone(), |n| n.to_string_lossy().into_owned());
                    warnings.push(format!(
                        "{} keeps no line numbers, and chrome-cdp could not tell which element \
                         of {name}'s template this is: only the file is shown",
                        found.framework
                    ));
                }
                Answer {
                    at,
                    components,
                    more: found.more,
                    via: found.framework.clone(),
                    version: Some(found.version.clone()).filter(|v| !v.is_empty()),
                    warnings,
                }
            })
            .collect()
    }

    /// The places the page gave, as files on disk: a stack's first frame
    /// of the app (source-mapped; all stacks in one go), a file name, or
    /// a file and what to find in its template.
    pub(crate) async fn resolve_places(
        &self,
        running: &Running,
        page: &Page,
        places: &[&PagePlace],
        timeout: Duration,
    ) -> Vec<Option<SourcePlace>> {
        let mut stacks: Vec<Vec<StackFrame>> = places
            .iter()
            .map(|place| {
                place
                    .frames
                    .iter()
                    .map(|f| StackFrame {
                        function: f.function.clone(),
                        url: f.url.clone(),
                        line: f.line,
                        column: f.column,
                        original: None,
                        file: None,
                        async_boundary: None,
                        locals: None,
                    })
                    .collect()
            })
            .collect();
        {
            let mut refs: Vec<&mut StackFrame> = stacks.iter_mut().flatten().collect();
            if !refs.is_empty() {
                self.map_stack(running, page, &mut refs, true, timeout)
                    .await;
            }
        }
        let root = places::root();
        places
            .iter()
            .zip(stacks)
            .map(|(place, frames)| {
                if !place.frames.is_empty() {
                    return first_of_the_app(&frames).map(frame_place);
                }
                file_place(place, root.as_deref())
            })
            .collect()
    }

    /// The main frame's origin, for places on it shown as paths.
    pub(crate) fn page_origin(running: &Running, page: &Page) -> Option<String> {
        let frames = running.tabs.frames(page.id);
        let main_url = frames.first().map(|f| f.url.clone()).unwrap_or_default();
        origin(&main_url).map(str::to_owned)
    }

    /// The `NoSource` error for `element`.
    fn no_source(&self, element: &ActedElement, why: NoSource) -> ErrorInfo {
        let named = crate::input::describe(element);
        let target = element.reference.clone().unwrap_or_default();
        let again = Cmd::new("source").arg(&target).scoped(&self.session);
        let (message, hint, reason) = match why {
            NoSource::NotDev { framework, version } => {
                let framework = [framework, version].join(" ");
                (
                    format!(
                        "{named} was rendered by a production build of {}: it keeps no debug info",
                        framework.trim()
                    ),
                    Hint::text(
                        "open the app from its dev server (a development build keeps where each \
                         element came from), then run source again",
                    ),
                    "not_dev_build",
                )
            }
            NoSource::BeforeStacks => (
                format!(
                    "Chrome has no record of how {named} was made: it was made before \
                     chrome-cdp recorded node stacks"
                ),
                Hint::run(Cmd::new("reload").scoped(&self.session))
                    .why(format!("then run {again} again")),
                "before_stacks",
            ),
            NoSource::StacksOff => (
                format!(
                    "Chrome has no record of how {named} was made: node stacks are off for \
                     this page (by default they are on for localhost only)"
                ),
                Hint::run(
                    Cmd::new("open")
                        .flag("node-stacks", "on")
                        .scoped(&self.session),
                )
                .why(format!(
                    "then {} and run source again",
                    Cmd::new("reload").scoped(&self.session)
                )),
                "stacks_off",
            ),
            NoSource::LibraryOnly { innermost } => {
                let mut message = format!("only library code is on the stack that made {named}");
                if let Some(place) = innermost {
                    let _ = write!(message, " (innermost: {})", place_text(&place, true, None));
                }
                (
                    message,
                    Hint::text(
                        "a library made it; ask about the element around it that the app renders",
                    ),
                    "library_only",
                )
            }
        };
        ErrorInfo::new(ErrorKind::NoSource, message)
            .hint(hint)
            .details(json!({ "reason": reason }))
    }
}

/// Handles on elements `backends` in context `context_id` (in object
/// group [`GROUP`]); `None` for one that is gone.
async fn resolve_all(
    session: &Session,
    context_id: runtime::ExecutionContextId,
    backends: &[dom::BackendNodeId],
) -> Vec<Option<String>> {
    join_all(backends.iter().map(|backend| {
        session.call(dom::ResolveNodeParams {
            node_id: None,
            backend_node_id: Some(*backend),
            object_group: Some(GROUP.to_owned()),
            execution_context_id: Some(context_id),
        })
    }))
    .await
    .into_iter()
    .map(|r| r.ok().and_then(|r| r.object.object_id))
    .collect()
}

/// What the adapters find for the elements `objects` (handles in the
/// main world's context `context_id`), [`BATCH`] per call.
async fn adapters_on(
    session: &Session,
    context_id: runtime::ExecutionContextId,
    objects: &[Option<String>],
) -> Vec<Option<Found>> {
    run_adapters(
        session,
        context_id,
        objects,
        "__ccdpFrameworks.sources(args)",
    )
    .await
    .into_iter()
    .map(found_of)
    .collect()
}

/// An adapter's answer for an element, if a framework rendered it.
fn found_of(value: Option<Value>) -> Option<Found> {
    value
        .and_then(|v| serde_json::from_value::<Found>(v).ok())
        .filter(|f| !f.framework.is_empty())
}

/// Runs `call` of the adapters (one value per element of `args`) on the
/// elements `objects` (handles in the main world's context `context_id`),
/// [`BATCH`] per call.
async fn run_adapters(
    session: &Session,
    context_id: runtime::ExecutionContextId,
    objects: &[Option<String>],
    call: &str,
) -> Vec<Option<Value>> {
    let mut found = Vec::with_capacity(objects.len());
    for batch in objects.chunks(BATCH) {
        let arguments: Vec<Value> = batch
            .iter()
            .map(|object| match object {
                Some(id) => json!({ "objectId": id }),
                None => json!({ "value": null }),
            })
            .collect();
        let result = session
            .call_raw(
                "Runtime.callFunctionOn",
                json!({
                    "functionDeclaration": adapters(call),
                    "executionContextId": context_id,
                    "arguments": arguments,
                    "returnByValue": true,
                }),
            )
            .await;
        let values = match result {
            Ok(result) if result.get("exceptionDetails").is_none() => {
                result["result"]["value"].as_array().cloned()
            }
            Ok(result) => {
                tracing::debug!(
                    "the framework adapters threw: {}",
                    result["exceptionDetails"]
                );
                None
            }
            Err(e) => {
                tracing::debug!("the framework adapters failed: {e}");
                None
            }
        }
        .unwrap_or_default();
        found.extend((0..batch.len()).map(|i| values.get(i).filter(|v| !v.is_null()).cloned()));
    }
    found
}

/// Lets go of the handles `source` made in `session` (without waiting).
fn release(session: &Session) {
    let session = session.clone();
    tokio::spawn(async move {
        let _ = session
            .call(runtime::ReleaseObjectGroupParams::new(GROUP.to_owned()))
            .await;
    });
}

/// A creation stack's frames, across its async parents (at most
/// [`MAX_FRAMES`]).
fn stack_of(stack: &runtime::StackTrace) -> Vec<StackFrame> {
    let stack = serde_json::to_value(stack).unwrap_or(Value::Null);
    let mut frames = Vec::new();
    let mut next = Some(&stack);
    while let Some(stack) = next.filter(|_| frames.len() < MAX_FRAMES) {
        frames.extend(stack_frames(stack));
        next = stack.get("parent");
    }
    frames.truncate(MAX_FRAMES);
    frames
}

/// Whether `frame` is a library's, a framework's or the dev server's.
fn not_the_app(frame: &StackFrame) -> bool {
    let place = frame.original.as_ref().map_or(&frame.url, |o| &o.source);
    NOT_THE_APP.iter().any(|part| place.contains(part))
}

/// The first frame of a React debug stack that is the app's: past the
/// JSX factory, not a library's, and with a script.
fn first_of_the_app(frames: &[StackFrame]) -> Option<&StackFrame> {
    frames.iter().find(|f| {
        let name = f.function.rsplit('.').next().unwrap_or_default();
        !f.url.is_empty() && !JSX_FACTORIES.contains(&name) && !not_the_app(f)
    })
}

/// A stack frame's place: the original one if a source map gave it.
fn frame_place(frame: &StackFrame) -> SourcePlace {
    match &frame.original {
        Some(o) => SourcePlace {
            source: o.source.clone(),
            line: Some(o.line),
            column: Some(o.column),
            file: o.file.clone(),
            matched: false,
        },
        None => SourcePlace {
            source: frame.url.clone(),
            line: Some(frame.line),
            column: Some(frame.column),
            file: frame.file.clone(),
            matched: false,
        },
    }
}

/// A place the page named by file: found on disk, and its line found in
/// the template when the page does not know it (Vue).
fn file_place(place: &PagePlace, root: Option<&Path>) -> Option<SourcePlace> {
    let source = place.file.clone()?;
    let file = places::to_disk(&source, root);
    let mut line = place.line;
    let mut column = place.column;
    let mut matched = false;
    if line.is_none()
        && let (Some(wanted), Some(file)) = (&place.matching, &file)
        && file.extension().is_some_and(|e| e == "vue")
    {
        line = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| template_line(&text, wanted));
        matched = line.is_some();
        column = None;
    }
    Some(SourcePlace {
        source,
        line,
        column,
        file: file.map(|f| f.to_string_lossy().into_owned()),
        matched,
    })
}

/// The 1-based line of the tag `wanted` names in the `<template>` of a
/// Vue single-file component: the only such tag there; else the
/// `index`-th of them when the template has as many as the component
/// rendered (`count`, no `v-for` or `v-if` in between); else the only
/// one whose line holds the element's text.
fn template_line(text: &str, wanted: &Match) -> Option<u32> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim_start().starts_with("<template"))?;
    let end = lines
        .iter()
        .rposition(|l| l.contains("</template>"))
        .filter(|end| *end > start)
        .unwrap_or(lines.len() - 1);
    let names = tag_names(&wanted.tag);
    let mut candidates = Vec::new();
    for (i, line) in lines.iter().enumerate().take(end + 1).skip(start + 1) {
        for name in &names {
            let open = format!("<{name}");
            let opens = line.match_indices(&open).filter(|(at, _)| {
                let after = line[at + name.len() + 1..].chars().next();
                after.is_none_or(|c| c.is_whitespace() || c == '>' || c == '/')
            });
            candidates.extend(opens.map(|_| i));
        }
    }
    let line = match candidates.as_slice() {
        [only] => Some(*only),
        _ if wanted.count == Some(candidates.len()) => {
            wanted.index.and_then(|i| candidates.get(i).copied())
        }
        _ => {
            let text = wanted.text.as_deref().unwrap_or_default();
            let with_text: Vec<usize> = candidates
                .iter()
                .copied()
                .filter(|&i| !text.is_empty() && lines[i].contains(&format!(">{text}<")))
                .collect();
            match with_text.as_slice() {
                [only] => Some(*only),
                _ => None,
            }
        }
    }?;
    u32::try_from(line + 1).ok()
}

/// How a tag can be written in a template: as it is, and a component's
/// `CartItem` also as `cart-item`.
fn tag_names(tag: &str) -> Vec<String> {
    let mut names = vec![tag.to_owned()];
    if tag.chars().any(|c| c.is_ascii_uppercase()) {
        let mut kebab = String::new();
        for (i, c) in tag.chars().enumerate() {
            if c.is_ascii_uppercase() {
                if i > 0 {
                    kebab.push('-');
                }
                kebab.push(c.to_ascii_lowercase());
            } else {
                kebab.push(c);
            }
        }
        names.push(kebab);
    }
    names
}

/// A place as text: the file relative to the project root (else the
/// URL, without the page's origin), `:line`, and `:column` if `column`.
pub(crate) fn place_text(place: &SourcePlace, column: bool, page_origin: Option<&str>) -> String {
    let mut text = shown_source(&place.source, place.file.as_deref(), page_origin);
    if let Some(line) = place.line {
        let _ = write!(text, ":{line}");
        if column && let Some(col) = place.column {
            let _ = write!(text, ":{col}");
        }
    }
    text
}

/// `source`'s text:
///
/// ```text
/// e12 button "Add"
///   at src/components/CartItem.tsx:15:7
///   in <CartItem>  src/components/CartItem.tsx:15
///   in <CartList>  src/components/CartList.tsx:14
/// via react 19 (the page's debug info)
/// ```
fn source_text(
    element: &ActedElement,
    at: Option<&SourcePlace>,
    components: &[SourceComponent],
    more: u64,
    via: &str,
    version: Option<&str>,
    page_origin: Option<&str>,
) -> String {
    let mut text = crate::input::describe(element);
    if via == "html" {
        if let Some(at) = at {
            let _ = write!(
                text,
                "\n  in the HTML of {}",
                place_text(at, false, page_origin)
            );
        }
        return text;
    }
    if let Some(at) = at {
        let _ = write!(text, "\n  at {}", place_text(at, true, page_origin));
    }
    let width = components
        .iter()
        .map(|c| c.name.chars().count() + 2)
        .max()
        .unwrap_or_default();
    for component in components {
        let name = format!("<{}>", component.name);
        match &component.place {
            Some(place) => {
                let _ = write!(
                    text,
                    "\n  in {name:width$}  {}",
                    place_text(place, false, page_origin)
                );
            }
            None => {
                let _ = write!(text, "\n  in {name}");
            }
        }
    }
    if more > 0 {
        let _ = write!(text, "\n  … {more} more (--depth shows them)");
    }
    text.push_str("\nvia ");
    if via == "node_stack" {
        text.push_str("node creation stack");
    } else {
        text.push_str(via);
        if let Some(version) = version {
            let _ = write!(text, " {version}");
        }
        text.push_str(" (the page's debug info");
        let matched = at
            .into_iter()
            .chain(components.iter().filter_map(|c| c.place.as_ref()))
            .any(|p| p.matched);
        if matched {
            text.push_str("; lines found in the templates");
        }
        text.push(')');
    }
    text
}

/// A node's `snapshot --source` suffix: `<CartItem src/components/CartItem.tsx:42>`,
/// `<CartItem>` without a place, `<src/cart.ts:12>` without a component.
pub(crate) fn node_source_text(source: &NodeSource, page_origin: Option<&str>) -> String {
    let place = source
        .place
        .as_ref()
        .map(|p| place_text(p, false, page_origin));
    let parts: Vec<String> = source.component.iter().cloned().chain(place).collect();
    format!("<{}>", parts.join(" "))
}

/// A node's source as `snapshot --source` shows it: its innermost
/// component and its own place, else the component's.
fn node_source(answer: &Answer) -> Option<NodeSource> {
    let component = answer.components.first();
    let place = answer
        .at
        .clone()
        .or_else(|| component.and_then(|c| c.place.clone()));
    if component.is_none() && place.is_none() {
        return None;
    }
    Some(NodeSource {
        component: component.map(|c| c.name.clone()),
        place,
    })
}

#[cfg(test)]
mod tests {
    use protocol::schema::{SourcePosition, StackFrame};

    use super::{Match, not_the_app, tag_names, template_line};

    fn frame(url: &str, source: Option<&str>) -> StackFrame {
        StackFrame {
            function: String::new(),
            url: url.into(),
            line: 1,
            column: 1,
            original: source.map(|source| SourcePosition {
                source: source.into(),
                line: 1,
                column: 1,
                name: None,
                file: None,
            }),
            file: None,
            async_boundary: None,
            locals: None,
        }
    }

    #[test]
    fn libraries_are_not_the_app() {
        let bundle = "http://localhost:5173/assets/index.js";
        assert!(!not_the_app(&frame(
            bundle,
            Some("http://localhost:5173/src/cart.ts")
        )));
        assert!(not_the_app(&frame(
            bundle,
            Some("http://localhost:5173/node_modules/react-dom/cjs/react-dom.development.js")
        )));
        assert!(not_the_app(&frame(
            "http://localhost:5173/node_modules/.vite/deps/vue.js?v=1",
            None
        )));
        assert!(not_the_app(&frame(
            "http://localhost:5173/@vite/client",
            None
        )));
        assert!(!not_the_app(&frame(bundle, None)));
    }

    const ITEM: &str = r#"<script setup lang="ts">
const props = defineProps<{ item: Item }>();
</script>

<template>
  <li :data-testid="`item-${item.id}`">
    <span>{{ item.name }}</span> ×
    <span>{{ item.qty }}</span>
    <button @click="add">Add</button>
    <button @click="remove">Remove</button>
    <CartRow v-for="row in rows" :row="row" />
  </li>
</template>
"#;

    fn wanted(tag: &str, index: Option<usize>, count: Option<usize>, text: &str) -> Match {
        Match {
            tag: tag.into(),
            index,
            count,
            text: Some(text.into()),
        }
    }

    #[test]
    fn template_lines() {
        // The only <li>.
        assert_eq!(
            template_line(ITEM, &wanted("li", Some(0), Some(1), "")),
            Some(6)
        );
        // The second of two buttons, as the component rendered two.
        assert_eq!(
            template_line(ITEM, &wanted("button", Some(1), Some(2), "Remove")),
            Some(10)
        );
        // A v-for rendered three rows of one tag.
        assert_eq!(
            template_line(ITEM, &wanted("CartRow", Some(2), Some(3), "")),
            Some(11)
        );
        // A component by its kebab-case name too.
        let kebab = ITEM.replace("<CartRow", "<cart-row");
        assert_eq!(
            template_line(&kebab, &wanted("CartRow", None, None, "")),
            Some(11)
        );
        // Counts that differ: the element's text decides.
        assert_eq!(
            template_line(ITEM, &wanted("button", Some(0), Some(5), "Add")),
            Some(9)
        );
        // Nothing decides: no line.
        assert_eq!(
            template_line(ITEM, &wanted("span", Some(0), Some(6), "Mug")),
            None
        );
        // `<b` is not `<button`, and no template, no line.
        assert_eq!(template_line(ITEM, &wanted("b", None, None, "")), None);
        assert_eq!(
            template_line("<div></div>", &wanted("div", None, None, "")),
            None
        );
    }

    #[test]
    fn component_tags_in_kebab_case() {
        assert_eq!(tag_names("CartItem"), ["CartItem", "cart-item"]);
        assert_eq!(tag_names("button"), ["button"]);
    }
}
