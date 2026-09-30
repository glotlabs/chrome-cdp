//! Logpoints as a feature of the registry (plan 15, milestone 15.4):
//! printf debugging without editing the page's code.
//!
//! A logpoint names a line of an original source (`src/cart.ts:42`).
//! While a tab has one, every page, iframe and worker session of it has
//! `Debugger` on, and a CDP binding whose name the page cannot guess
//! ([`Debugging::binding`]). Each script a session reports
//! (`Debugger.scriptParsed`) is kept with its source map's URL.
//!
//! **Binding.** For each script whose source map lists the logpoint's
//! source (matched by path, see [`places::same_file`]), the original
//! line is looked up the other way ([`Mapped::generated`]: its first
//! mapped column, or the next line with code) and a breakpoint is set by
//! the script's URL (`Debugger.setBreakpointByUrl`), so V8 sets it again
//! in the script of the next document (a reload) before it runs. A
//! script without a map is matched by its own URL, at the line given.
//! The breakpoint's condition runs `logpoint.js` (see
//! `injected/src/logpoint.ts`): it previews each expression's value in
//! its own `try`, sends them on the binding and returns `false`, so the
//! page never stops. V8 names a breakpoint by its URL, line and column,
//! so the logpoints at one place share one, whose condition runs each;
//! it is set again when one comes or goes ([`replace`]).
//!
//! **Scripts loaded later.** While Debugger is on, each script with a
//! source map pauses before it runs
//! (`beforeScriptWithSourceMapExecution`): the logpoints its map has are
//! bound, then it resumes ([`paused`]). A script without one is bound
//! when it is reported (its top level has run by then). A logpoint that
//! matches no script is `pending` until one does.
//!
//! **Hits** go to the tab's console as entries of source `logpoint` (so
//! `console --grep` and `--follow` see them), from which the action
//! report and its `logpoint` events take them. A logpoint stops after
//! its `--limit` hits (its breakpoints removed), and says so once in the
//! console. **Other pauses** (a `debugger;` statement) are resumed at
//! once and noted in the console.
//!
//! **Locals** (milestone 15.5) are the second reason for Debugger to be
//! on in a session: where locals capture is on, it pauses at uncaught
//! errors (see `super::locals`). Each session's [`Mode`] says which
//! reasons it has; [`switch`] sends what turning each on or off takes.
//! Where Debugger is on, async call stacks are recorded too, so a
//! request's initiator goes on across `await`s and timers.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::future::Future;
use std::hash::{BuildHasher as _, Hasher as _};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cdp::domains::{debugger, page, runtime};
use cdp::{Event as _, RawEvent, Session};
use futures_util::FutureExt as _;
use futures_util::future::join_all;
use protocol::schema::{self, Level, LogpointHit, LogpointValue};
use serde::Deserialize;
use tokio::time::Instant;

use super::features::{Feature, NewTarget};
use super::targets::Target;
use super::{Shared, State, Step, Tabs, locals, step, tab_name};
use crate::observe::console;
use crate::observe::{Mapped, RESOLVE_LIMIT, is_local};
use crate::places;
use crate::request::DebugLogRequest;

/// How long taking out a logpoint that reached its limit may take.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// How often `debug log` looks whether its logpoint's binds ended.
const BUSY_POLL: Duration = Duration::from_millis(10);

/// The logpoints' helper, put in each breakpoint's condition.
const HELPER: &str = include_str!("../injected/logpoint.js");

/// How many async parents a stack keeps where Debugger is on.
const ASYNC_DEPTH: i64 = 8;

/// The session's logpoints and where Debugger is on for them.
#[derive(Debug)]
pub(super) struct Debugging {
    /// The binding hits come on: a name the page cannot guess.
    binding: String,
    next_id: u32,
    logpoints: BTreeMap<u32, Logpoint>,
    /// The sessions Debugger is on in, by id, with their scripts.
    pub(super) sessions: HashMap<String, Scripts>,
    /// The project root of the latest `debug` command: places in hits
    /// and notes print relative to it.
    root: Option<PathBuf>,
    /// The breakpoints set, by where they are. The logpoints at one
    /// place share one (V8 names a breakpoint by its URL, line and
    /// column, so a second one there fails), whose condition runs each.
    placed: HashMap<Spot, Placed>,
    /// Held while a breakpoint is set again (see [`replace`]).
    changing: Arc<tokio::sync::Mutex<()>>,
    /// `debug locals`: on or off everywhere; `None` for `localhost`
    /// pages only.
    pub(super) locals: Option<bool>,
}

/// Why Debugger is on in a session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Mode {
    /// It runs scripts of a tab with a logpoint.
    pub(super) logpoints: bool,
    /// Locals capture is on there.
    pub(super) locals: bool,
}

impl Mode {
    fn any(self) -> bool {
        self.logpoints || self.locals
    }
}

/// Where a breakpoint is: a session, a script's URL, and the 0-based
/// line and column in it.
type Spot = (String, String, u32, u32);

/// A breakpoint and the logpoints it runs.
#[derive(Debug)]
struct Placed {
    breakpoint: String,
    ids: Vec<u32>,
}

impl Default for Debugging {
    fn default() -> Self {
        let random = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
        Self {
            binding: format!("__ccdpLogpoint{random:016x}"),
            next_id: 1,
            logpoints: BTreeMap::new(),
            sessions: HashMap::new(),
            root: None,
            placed: HashMap::new(),
            changing: Arc::default(),
            locals: None,
        }
    }
}

/// A session Debugger is on in: why, and the scripts it reported, by
/// script id.
#[derive(Debug, Default)]
pub(super) struct Scripts {
    pub(super) mode: Mode,
    pub(super) by_id: HashMap<String, Script>,
    /// The instrumentation breakpoint logpoints set, once set.
    instrument: Option<String>,
    /// Locals read at uncaught errors, for their console entries.
    pub(super) pauses: locals::Pauses,
}

#[derive(Debug, Clone)]
pub(super) struct Script {
    pub(super) url: String,
    /// Its source map's URL (relative to it, or `data:`); empty if none.
    map: String,
    context: i64,
}

/// A logpoint.
#[derive(Debug, Clone)]
struct Logpoint {
    id: u32,
    tab: u32,
    /// The place's path as given, and the file it names on disk, if any.
    path: String,
    file: Option<PathBuf>,
    line: u32,
    column: Option<u32>,
    exprs: Vec<String>,
    limit: u64,
    hits: u64,
    stopped: bool,
    /// The source it matched (a URL, without query), once one did, and
    /// that source on disk.
    source: Option<String>,
    source_file: Option<PathBuf>,
    /// The original line it is bound at, when another than `line`.
    bound_line: Option<u32>,
    /// Where it is set.
    bound: Vec<Bound>,
    /// The scripts it is being set in now, by session id and URL.
    busy: HashSet<(String, String)>,
}

/// A script a logpoint is set in, and where in it.
#[derive(Debug, Clone)]
struct Bound {
    session: String,
    url: String,
    spot: Spot,
}

/// What `debug log` asks for.
#[derive(Debug, Clone)]
pub(crate) struct Spec {
    pub(crate) path: String,
    pub(crate) file: Option<PathBuf>,
    pub(crate) line: u32,
    pub(crate) column: Option<u32>,
    pub(crate) exprs: Vec<String>,
    pub(crate) limit: u64,
}

/// Why `debug log` failed.
#[derive(Debug)]
pub(crate) enum LogError {
    /// The session has [`DebugLogRequest::MAX`] logpoints already.
    TooMany,
    /// The place names several sources: these, as text shows them.
    Ambiguous(Vec<String>),
    Cdp(cdp::CdpError),
}

/// What `debug clear` did.
#[derive(Debug)]
pub(crate) struct Cleared {
    pub(crate) ids: Vec<u32>,
    pub(crate) remaining: usize,
    /// Whether Debugger is still on somewhere.
    pub(crate) debugger: bool,
}

/// A script a logpoint may be set in.
#[derive(Clone)]
struct Candidate {
    session_id: String,
    session: Session,
    url: String,
    /// The source it has (the map's URL for it, or the script's own).
    source: String,
    mapped: Option<Arc<Mapped>>,
}

impl Tabs {
    /// `debug log`: a logpoint in tab `tab`'s scripts, bound where its
    /// source is, else pending. `root` is the call's project root.
    pub(crate) async fn debug_log(
        &self,
        tab: u32,
        spec: Spec,
        root: Option<PathBuf>,
        timeout: Duration,
    ) -> Result<schema::Logpoint, LogError> {
        let deadline = Instant::now() + timeout.min(RESOLVE_LIMIT);
        let id = {
            let mut state = self.shared.lock();
            let debug = &mut state.debug;
            if debug.logpoints.len() >= DebugLogRequest::MAX {
                return Err(LogError::TooMany);
            }
            debug.root = root;
            let id = debug.next_id;
            debug.next_id += 1;
            debug.logpoints.insert(
                id,
                Logpoint {
                    id,
                    tab,
                    path: spec.path,
                    file: spec.file,
                    line: spec.line,
                    column: spec.column,
                    exprs: spec.exprs,
                    limit: spec.limit.max(1),
                    hits: 0,
                    stopped: false,
                    source: None,
                    source_file: None,
                    bound_line: None,
                    bound: Vec::new(),
                    busy: HashSet::new(),
                },
            );
            id
        };
        if let Err(e) = sync(&self.shared, timeout).await {
            self.shared.lock().debug.logpoints.remove(&id);
            let _ = sync(&self.shared, timeout).await;
            return Err(LogError::Cdp(e));
        }
        let candidates = candidates(&self.shared, tab, deadline).await;
        let matching = matching(&self.shared, id, &candidates);
        let mut sources: Vec<&str> = matching
            .iter()
            .map(|c| places::without_query(&c.source))
            .collect();
        sources.sort_unstable();
        sources.dedup();
        if sources.len() > 1 {
            let root = self.shared.lock().debug.root.clone();
            let shown = sources
                .iter()
                .map(|s| shown_source(s, root.as_deref()).0)
                .collect();
            self.shared.lock().debug.logpoints.remove(&id);
            let _ = sync(&self.shared, timeout).await;
            return Err(LogError::Ambiguous(shown));
        }
        join_all(matching.iter().map(|c| bind(&self.shared, id, c, deadline))).await;
        // The scripts `Debugger.enable` reported are bound from the
        // session's events too: wait for those binds to end.
        while Instant::now() < deadline
            && self
                .shared
                .lock()
                .debug
                .logpoints
                .get(&id)
                .is_some_and(|l| !l.busy.is_empty())
        {
            tokio::time::sleep(BUSY_POLL).await;
        }
        let state = self.shared.lock();
        let logpoint = &state.debug.logpoints[&id];
        Ok(view(logpoint, state.debug.root.as_deref()))
    }

    /// `debug list`: the session's logpoints, oldest first.
    pub(crate) fn debug_list(&self, root: Option<PathBuf>) -> Vec<schema::Logpoint> {
        let mut state = self.shared.lock();
        state.debug.root = root;
        let root = state.debug.root.as_deref();
        state
            .debug
            .logpoints
            .values()
            .map(|l| view(l, root))
            .collect()
    }

    /// `debug clear`: removes logpoint `id` (every one if `None`), and
    /// turns Debugger off where nothing needs it any more. `Err` names
    /// the ids there are when `id` is not one of them.
    pub(crate) async fn debug_clear(
        &self,
        id: Option<u32>,
        timeout: Duration,
    ) -> Result<Cleared, Vec<u32>> {
        let removed: Vec<Logpoint> = {
            let mut state = self.shared.lock();
            let logpoints = &mut state.debug.logpoints;
            match id {
                Some(id) => match logpoints.remove(&id) {
                    Some(logpoint) => vec![logpoint],
                    None => return Err(logpoints.keys().copied().collect()),
                },
                None => std::mem::take(logpoints).into_values().collect(),
            }
        };
        unbind(&self.shared, &removed, timeout).await;
        if let Err(e) = sync(&self.shared, timeout).await {
            tracing::debug!("debug clear: turning the debugger off failed: {e}");
        }
        let state = self.shared.lock();
        Ok(Cleared {
            ids: removed.iter().map(|l| l.id).collect(),
            remaining: state.debug.logpoints.len(),
            debugger: !state.debug.sessions.is_empty(),
        })
    }

    /// The logpoint hits tab `id`'s console logged after mark `after`.
    pub(crate) fn logpoints_since(&self, id: u32, after: u64) -> Vec<LogpointHit> {
        self.console(id, |c| c.logpoints_since(after))
            .unwrap_or_default()
    }
}

/// A logpoint as `debug` commands show it.
fn view(logpoint: &Logpoint, root: Option<&std::path::Path>) -> schema::Logpoint {
    let (place, file) = logpoint_place(logpoint, root);
    let mut scripts: Vec<String> = logpoint.bound.iter().map(|b| b.url.clone()).collect();
    scripts.sort_unstable();
    scripts.dedup();
    let state = if logpoint.stopped {
        "stopped"
    } else if logpoint.bound.is_empty() {
        "pending"
    } else {
        "bound"
    };
    schema::Logpoint {
        id: logpoint.id,
        tab: tab_name(logpoint.tab),
        place: format!("{place}:{}", logpoint.bound_line.unwrap_or(logpoint.line)),
        file: file.map(|f| f.to_string_lossy().into_owned()),
        source: logpoint.source.clone(),
        line: logpoint.line,
        bound_line: logpoint.bound_line,
        column: logpoint.column,
        exprs: logpoint.exprs.clone(),
        state: state.to_owned(),
        scripts,
        hits: logpoint.hits,
        limit: logpoint.limit,
    }
}

/// Where a logpoint is, as text shows it (without the line), and its
/// file on disk: its source's, once it matched one; else the place as
/// asked.
fn logpoint_place(
    logpoint: &Logpoint,
    root: Option<&std::path::Path>,
) -> (String, Option<PathBuf>) {
    if let Some(file) = &logpoint.source_file {
        return (places::shown(file, root), Some(file.clone()));
    }
    if let Some(source) = &logpoint.source {
        return shown_source(source, root);
    }
    match &logpoint.file {
        Some(file) => (places::shown(file, root), Some(file.clone())),
        None => (logpoint.path.clone(), None),
    }
}

/// A source as text shows it: its file on disk relative to `root` when
/// found, else its URL's path.
fn shown_source(source: &str, root: Option<&std::path::Path>) -> (String, Option<PathBuf>) {
    if let Some(file) = places::to_disk(source, root) {
        return (places::shown(&file, root), Some(file));
    }
    let source = places::without_query(source);
    let path = source
        .split_once("://")
        .and_then(|(_, rest)| rest.find('/').map(|at| &rest[at..]))
        .unwrap_or(source);
    (path.to_owned(), None)
}

/// The tabs that have a logpoint that is not stopped.
fn debugged_tabs(state: &State) -> HashSet<u32> {
    state
        .debug
        .logpoints
        .values()
        .filter(|l| !l.stopped)
        .map(|l| l.tab)
        .collect()
}

/// Why Debugger should be on in `target`, whose document is at `url`: it
/// runs scripts of a tab with a logpoint, or locals capture is on there.
fn wanted(state: &State, tabs: &HashSet<u32>, target: &Target, url: &str) -> Mode {
    if !Feature::Debugger.applies(target) {
        return Mode::default();
    }
    Mode {
        logpoints: state.tabs_of(target).iter().any(|t| tabs.contains(t)),
        locals: state.debug.locals.unwrap_or_else(|| is_local(url)),
    }
}

/// The mode Debugger is in in session `id` now.
fn current(state: &State, id: &str) -> Mode {
    state
        .debug
        .sessions
        .get(id)
        .map(|s| s.mode)
        .unwrap_or_default()
}

/// Brings every session's Debugger in line with what it should do. Fails
/// if turning it on for logpoints fails in a tab's page.
async fn sync(shared: &Arc<Shared>, timeout: Duration) -> cdp::Result<()> {
    let changes: Vec<(String, Mode, Mode, Session)> = {
        let state = shared.lock();
        let tabs = debugged_tabs(&state);
        state
            .targets
            .iter()
            .filter_map(|(id, t)| {
                let to = wanted(&state, &tabs, t, &t.url);
                let from = current(&state, id);
                (from != to).then(|| (id.clone(), from, to, t.session.with_timeout(timeout)))
            })
            .collect()
    };
    let pages: HashSet<String> = {
        let state = shared.lock();
        debugged_tabs(&state)
            .iter()
            .filter_map(|t| state.tabs.get(t)?.session.id().map(str::to_owned))
            .collect()
    };
    let calls = changes.iter().map(|(id, from, to, session)| {
        let sent = switch(shared, id, session, *from, *to);
        async move { (id, *from, *to, sent.await) }
    });
    let mut result = Ok(());
    for (id, from, to, outcome) in join_all(calls).await {
        if let Err(e) = outcome {
            tracing::debug!("turning the debugger to {to:?} in {id} failed: {e}");
            if !from.any() && to.any() {
                shared.lock().debug.sessions.remove(id);
            }
            if to.logpoints && !from.logpoints && pages.contains(id) {
                result = Err(e);
            }
        }
    }
    result
}

/// Brings Debugger in session `id` from mode `from` to `to`, and the
/// registry with it. The commands are sent at once (before a new document
/// can run a script); the future awaits them, and records the scripts
/// `Debugger.enable` reported.
fn switch(
    shared: &Arc<Shared>,
    id: &str,
    session: &Session,
    from: Mode,
    to: Mode,
) -> impl Future<Output = cdp::Result<()>> + Send + 'static + use<> {
    let (binding, instrument, placed) = {
        let mut state = shared.lock();
        let debug = &mut state.debug;
        let (instrument, placed) = if to.logpoints {
            (None, Vec::new())
        } else {
            // Its logpoints' breakpoints go (Debugger.disable would take
            // them too).
            let placed: Vec<String> = debug
                .placed
                .iter()
                .filter(|(spot, _)| spot.0 == id)
                .map(|(_, p)| p.breakpoint.clone())
                .collect();
            debug.placed.retain(|spot, _| spot.0 != id);
            for logpoint in debug.logpoints.values_mut() {
                logpoint.bound.retain(|b| b.session != id);
            }
            let instrument = debug.sessions.get_mut(id).and_then(|s| s.instrument.take());
            (instrument, placed)
        };
        if to.any() {
            debug.sessions.entry(id.to_owned()).or_default().mode = to;
        } else {
            debug.sessions.remove(id);
        }
        (debug.binding.clone(), instrument, placed)
    };
    let mut calls: Vec<Step> = Vec::new();
    let mut parsed = None;
    if !from.any() && to.any() {
        parsed = Some(session.events_lossless(debugger::ScriptParsed::NAME));
        calls.push(step(session.call(debugger::EnableParams::default())));
        calls.push(step(
            session.call(runtime::SetAsyncCallStackDepthParams::new(ASYNC_DEPTH)),
        ));
    }
    if to.logpoints && !from.logpoints {
        calls.push(step(
            session.call(runtime::AddBindingParams::new(binding.clone())),
        ));
        let set = session.call(debugger::SetInstrumentationBreakpointParams::new(
            debugger::SetInstrumentationBreakpointInstrumentation::BeforeScriptWithSourceMapExecution,
        ));
        let (shared, id) = (shared.clone(), id.to_owned());
        calls.push(Box::pin(async move {
            let set = set.await?;
            if let Some(s) = shared.lock().debug.sessions.get_mut(&id) {
                s.instrument = Some(set.breakpoint_id);
            }
            Ok(())
        }));
    }
    if from.logpoints && !to.logpoints {
        // Not needed once the calls are sent: a failure is only logged.
        let mut gone = vec![session.call(runtime::RemoveBindingParams::new(binding))];
        let mut removed: Vec<_> = placed
            .into_iter()
            .chain(instrument)
            .filter(|_| to.any())
            .map(|b| session.call(debugger::RemoveBreakpointParams::new(b)))
            .collect();
        calls.push(Box::pin(async move {
            for call in gone.drain(..) {
                let _ = call.await;
            }
            for call in removed.drain(..) {
                if let Err(e) = call.await {
                    tracing::debug!("removing a breakpoint failed: {e}");
                }
            }
            Ok(())
        }));
    }
    if to.any() && to.locals != from.locals {
        let state = if to.locals {
            debugger::SetPauseOnExceptionsState::Uncaught
        } else {
            debugger::SetPauseOnExceptionsState::None
        };
        calls.push(step(
            session.call(debugger::SetPauseOnExceptionsParams::new(state)),
        ));
    }
    if from.any() && !to.any() {
        calls.push(step(session.call(debugger::DisableParams::default())));
        calls.push(step(
            session.call(runtime::SetAsyncCallStackDepthParams::new(0)),
        ));
    }
    let shared = shared.clone();
    let id = id.to_owned();
    async move {
        let mut result = Ok(());
        for call in calls {
            if let Err(e) = call.await
                && result.is_ok()
            {
                result = Err(e);
            }
        }
        if let Some(mut parsed) = parsed {
            while let Some(Some(event)) = parsed.next().now_or_never() {
                record(&shared, &id, &event);
            }
        }
        result
    }
}

/// A new target's steps: Debugger on, if it runs scripts of a tab with
/// a logpoint or locals capture is on there.
pub(super) fn setup_steps(
    shared: &Arc<Shared>,
    target: NewTarget<'_>,
    steps: &mut Vec<(&'static str, Step)>,
) {
    let id = target.session.id().unwrap_or_default().to_owned();
    let to = {
        let state = shared.lock();
        let tabs = debugged_tabs(&state);
        match state.targets.get(&id) {
            Some(t) => wanted(&state, &tabs, t, &t.url),
            None => return,
        }
    };
    if to.any() {
        steps.push((
            "Debugger.enable",
            Box::pin(switch(shared, &id, target.session, Mode::default(), to)),
        ));
    }
}

/// A frame of session `session_id` started navigating (`event`): if it is
/// the session's top frame, locals capture follows where it goes, at once.
pub(super) fn navigating(shared: &Arc<Shared>, session_id: &str, event: &RawEvent) {
    let Ok(started) = page::FrameStartedNavigating::deserialize(&event.params) else {
        return;
    };
    if matches!(
        started.navigation_type,
        page::FrameStartedNavigatingNavigationType::SameDocument
            | page::FrameStartedNavigatingNavigationType::HistorySameDocument
    ) {
        return;
    }
    let (from, to, session) = {
        let state = shared.lock();
        let Some(target) = state.targets.get(session_id) else {
            return;
        };
        if !target.kind.is_host() || target.id != started.frame_id {
            return;
        }
        let tabs = debugged_tabs(&state);
        let to = wanted(&state, &tabs, target, &started.url);
        (current(&state, session_id), to, target.session.clone())
    };
    if from == to {
        return;
    }
    let sent = switch(
        shared,
        session_id,
        &session.with_timeout(super::SETUP_TIMEOUT),
        from,
        to,
    );
    shared.spawn(async move {
        if let Err(e) = sent.await {
            tracing::debug!("turning the debugger to {to:?} failed: {e}");
        }
    });
}

impl Tabs {
    /// `debug locals`: turns locals capture on or off everywhere
    /// (`Some`), or back to `localhost` pages (`None`). The setting and
    /// how many sessions capture now.
    pub(crate) async fn debug_locals(
        &self,
        locals: Option<bool>,
        timeout: Duration,
    ) -> (Option<bool>, usize) {
        self.shared.lock().debug.locals = locals;
        if let Err(e) = sync(&self.shared, timeout).await {
            tracing::debug!("debug locals: changing the debugger failed: {e}");
        }
        self.locals_now()
    }

    /// Where locals are captured (`None`: `localhost`), and how many
    /// sessions capture them now.
    pub(crate) fn locals_now(&self) -> (Option<bool>, usize) {
        let state = self.shared.lock();
        let capturing = state
            .debug
            .sessions
            .values()
            .filter(|s| s.mode.locals)
            .count();
        (state.debug.locals, capturing)
    }
}

/// The scripts of tab `tab`'s sessions with Debugger on that a logpoint
/// may be set in, with their maps (loaded until `deadline`).
async fn candidates(shared: &Arc<Shared>, tab: u32, deadline: Instant) -> Vec<Candidate> {
    let scripts: Vec<(String, Session, Option<String>, Script)> = {
        let state = shared.lock();
        let mut seen = HashSet::new();
        state
            .debug
            .sessions
            .iter()
            .filter_map(|(id, scripts)| {
                let target = state.targets.get(id)?;
                state
                    .tabs_of(target)
                    .contains(&tab)
                    .then_some((id, target, scripts))
            })
            .flat_map(|(id, target, scripts)| {
                let frame = target.kind.is_host().then(|| target.id.clone());
                scripts
                    .by_id
                    .values()
                    .map(move |s| (id.clone(), target.session.clone(), frame.clone(), s.clone()))
            })
            .filter(|(id, _, _, s)| !s.url.is_empty() && seen.insert((id.clone(), s.url.clone())))
            .collect()
    };
    let loads = scripts
        .into_iter()
        .map(|(id, session, frame, script)| async move {
            let mapped = if script.map.is_empty() {
                None
            } else {
                shared
                    .sourcemaps
                    .of_script(
                        &session,
                        frame.as_deref(),
                        &script.url,
                        &script.map,
                        deadline,
                    )
                    .await
            };
            (id, session, script, mapped)
        });
    let mut candidates = Vec::new();
    for (session_id, session, script, mapped) in join_all(loads).await {
        match &mapped {
            Some(map) => candidates.extend(map.sources().into_iter().map(|source| Candidate {
                session_id: session_id.clone(),
                session: session.clone(),
                url: script.url.clone(),
                source,
                mapped: mapped.clone(),
            })),
            // A script with a map that did not load has no sources we
            // know; one without a map is its own.
            None if script.map.is_empty() => candidates.push(Candidate {
                session_id,
                session,
                source: script.url.clone(),
                url: script.url,
                mapped: None,
            }),
            None => {}
        }
    }
    candidates
}

/// The candidates logpoint `id` is to be set in: those of its source,
/// once it has one, else those whose source its place names.
fn matching(shared: &Shared, id: u32, candidates: &[Candidate]) -> Vec<Candidate> {
    let state = shared.lock();
    let Some(logpoint) = state.debug.logpoints.get(&id) else {
        return Vec::new();
    };
    let root = state.debug.root.as_deref();
    candidates
        .iter()
        .filter(|c| match &logpoint.source {
            Some(source) => places::without_query(&c.source) == source,
            None => places::same_file(&c.source, &logpoint.path, logpoint.file.as_deref(), root),
        })
        .cloned()
        .collect()
}

/// Sets logpoint `id` in the script of `candidate`, unless it is set
/// there already; the first script fixes its source.
async fn bind(shared: &Arc<Shared>, id: u32, candidate: &Candidate, deadline: Instant) {
    let key = (candidate.session_id.clone(), candidate.url.clone());
    let (line, column) = {
        let mut guard = shared.lock();
        let debug = &mut guard.debug;
        let root = debug.root.clone();
        let Some(logpoint) = debug.logpoints.get_mut(&id) else {
            return;
        };
        let source = places::without_query(&candidate.source);
        if logpoint.stopped
            || logpoint.source.as_deref().is_some_and(|s| s != source)
            || logpoint
                .bound
                .iter()
                .any(|b| b.session == key.0 && b.url == key.1)
            || !logpoint.busy.insert(key.clone())
        {
            return;
        }
        if logpoint.source.is_none() {
            logpoint.source = Some(source.to_owned());
            logpoint.source_file = places::to_disk(source, root.as_deref());
        }
        (logpoint.line, logpoint.column)
    };
    let place = match &candidate.mapped {
        Some(mapped) => mapped
            .generated(&candidate.source, line, column)
            .map(|g| (g.line, g.column, g.original_line)),
        None => Some((line - 1, column.map_or(0, |c| c.saturating_sub(1)), line)),
    };
    let mut set = None;
    if let Some((generated_line, generated_column, original_line)) = place {
        let left = deadline.saturating_duration_since(Instant::now());
        let session = candidate
            .session
            .with_timeout(left.max(Duration::from_secs(1)));
        let spot = (
            key.0.clone(),
            key.1.clone(),
            generated_line,
            generated_column,
        );
        match replace(shared, &session, &spot, Some(id), None).await {
            Ok(at) => {
                // Where V8 put it (the next place it can stop at), in the
                // original source.
                let used = at
                    .as_ref()
                    .zip(candidate.mapped.as_ref())
                    .and_then(|(at, mapped)| {
                        let line = u32::try_from(at.line_number).ok()?.checked_add(1)?;
                        let column = u32::try_from(at.column_number?).ok()?.checked_add(1)?;
                        let back = mapped.lookup(line, column)?;
                        (places::without_query(&back.source)
                            == places::without_query(&candidate.source))
                        .then_some(back.line)
                    })
                    .unwrap_or(original_line);
                set = Some((spot, used));
            }
            Err(e) => {
                tracing::debug!("setting logpoint {id} in {} failed: {e}", candidate.url);
            }
        }
    }
    let cleared = {
        let mut guard = shared.lock();
        match guard.debug.logpoints.get_mut(&id) {
            None => true,
            Some(logpoint) => {
                logpoint.busy.remove(&key);
                if let Some((spot, used)) = &set {
                    if logpoint.bound.is_empty() {
                        logpoint.bound_line = (*used != logpoint.line).then_some(*used);
                    }
                    logpoint.bound.push(Bound {
                        session: key.0.clone(),
                        url: key.1.clone(),
                        spot: spot.clone(),
                    });
                }
                false
            }
        }
    };
    if cleared && let Some((spot, _)) = set {
        // Cleared meanwhile.
        let _ = replace(shared, &candidate.session, &spot, None, Some(id)).await;
    }
}

/// Sets the breakpoint at `spot` again for its logpoints, with `add`
/// added and `remove` taken out (and removes it when none is left):
/// where V8 put it, if set.
async fn replace(
    shared: &Shared,
    session: &Session,
    spot: &Spot,
    add: Option<u32>,
    remove: Option<u32>,
) -> cdp::Result<Option<debugger::Location>> {
    let changing = shared.lock().debug.changing.clone();
    let _changing = changing.lock().await;
    let (old, ids, condition) = {
        let mut state = shared.lock();
        let debug = &mut state.debug;
        let old = debug.placed.remove(spot);
        let mut ids: Vec<u32> = old.as_ref().map(|p| p.ids.clone()).unwrap_or_default();
        ids.extend(add.filter(|id| !ids.contains(id)));
        ids.retain(|id| Some(*id) != remove && debug.logpoints.get(id).is_some_and(|l| !l.stopped));
        let logpoints: Vec<(u32, &[String])> = ids
            .iter()
            .filter_map(|id| Some((*id, debug.logpoints.get(id)?.exprs.as_slice())))
            .collect();
        let condition = condition(&debug.binding, &logpoints);
        (old.map(|p| p.breakpoint), ids, condition)
    };
    if let Some(old) = old
        && let Err(e) = session
            .call(debugger::RemoveBreakpointParams::new(old))
            .await
    {
        tracing::debug!("removing a logpoint's breakpoint failed: {e}");
    }
    if ids.is_empty() {
        return Ok(None);
    }
    let mut params = debugger::SetBreakpointByUrlParams::new(i64::from(spot.2));
    params.url = Some(spot.1.clone());
    params.column_number = Some(i64::from(spot.3));
    params.condition = Some(condition);
    match session.call(params).await {
        Ok(set) => {
            shared.lock().debug.placed.insert(
                spot.clone(),
                Placed {
                    breakpoint: set.breakpoint_id,
                    ids,
                },
            );
            Ok(set.locations.into_iter().next())
        }
        Err(e) => {
            // The others there lost it too.
            let mut state = shared.lock();
            for logpoint in state.debug.logpoints.values_mut() {
                logpoint.bound.retain(|b| &b.spot != spot);
            }
            Err(e)
        }
    }
}

/// Takes `logpoints` out of the breakpoints they are set in.
async fn unbind(shared: &Shared, logpoints: &[Logpoint], timeout: Duration) {
    let spots: Vec<(Session, Spot, u32)> = {
        let state = shared.lock();
        logpoints
            .iter()
            .flat_map(|l| l.bound.iter().map(move |b| (l.id, b)))
            .filter_map(|(id, b)| {
                let session = state.targets.get(&b.session)?.session.with_timeout(timeout);
                Some((session, b.spot.clone(), id))
            })
            .collect()
    };
    for (session, spot, id) in spots {
        if let Err(e) = replace(shared, &session, &spot, None, Some(id)).await {
            tracing::debug!("taking logpoint {id} out of its breakpoint failed: {e}");
        }
    }
}

/// The condition of a breakpoint of `logpoints` (ids and expressions):
/// each sends its expressions' values on `binding`, and it returns
/// `false`. Each expression is a closure over the paused code's scope; a
/// newline ends it, in case it ends in a `//` comment.
fn condition(binding: &str, logpoints: &[(u32, &[String])]) -> String {
    let binding = serde_json::Value::from(binding);
    let mut hits = String::new();
    for (id, exprs) in logpoints {
        let values: Vec<String> = exprs.iter().map(|e| format!("() => ({e}\n)")).collect();
        let _ = writeln!(
            hits,
            "__ccdpLogpoint.hit({binding}, {id}, [{}]);",
            values.join(", ")
        );
    }
    format!("(() => {{\ntry {{\n{HELPER}\n{hits}}} catch (e) {{}}\nreturn false;\n}})()")
}

/// Records a script session `session_id` reported, if Debugger is on
/// for logpoints there.
fn record(shared: &Shared, session_id: &str, event: &RawEvent) -> Option<Script> {
    let parsed: debugger::ScriptParsed = serde_json::from_value(event.params.clone()).ok()?;
    let mut state = shared.lock();
    let scripts = state.debug.sessions.get_mut(session_id)?;
    let script = Script {
        url: parsed.url,
        map: parsed.source_map_url.unwrap_or_default(),
        context: parsed.execution_context_id,
    };
    scripts.by_id.insert(parsed.script_id, script.clone());
    Some(script)
}

/// A `Debugger` event of `session` (a page, iframe or worker session).
pub(super) fn event(shared: &Arc<Shared>, session: &Session, event: &RawEvent) {
    let session_id = session.id().unwrap_or_default();
    match event.method.as_str() {
        debugger::ScriptParsed::NAME => {
            // A script with a map pauses before it runs; one without is
            // bound now.
            if let Some(script) = record(shared, session_id, event)
                && script.map.is_empty()
                && !script.url.is_empty()
            {
                let candidate = Candidate {
                    session_id: session_id.to_owned(),
                    session: session.clone(),
                    source: script.url.clone(),
                    url: script.url,
                    mapped: None,
                };
                let shared2 = shared.clone();
                shared.spawn(async move {
                    bind_everywhere(&shared2, &[candidate]).await;
                });
            }
        }
        debugger::Paused::NAME => {
            let shared2 = shared.clone();
            let session = session.clone();
            let params = event.params.clone();
            shared.spawn(async move {
                paused(&shared2, &session, &params).await;
            });
        }
        _ => {}
    }
}

/// Sets each logpoint of the tabs of `candidates`' session that names
/// their sources.
async fn bind_everywhere(shared: &Arc<Shared>, candidates: &[Candidate]) {
    let Some(first) = candidates.first() else {
        return;
    };
    let ids: Vec<u32> = {
        let state = shared.lock();
        let Some(target) = state.targets.get(&first.session_id) else {
            return;
        };
        let tabs = state.tabs_of(target);
        state
            .debug
            .logpoints
            .values()
            .filter(|l| !l.stopped && tabs.contains(&l.tab))
            .map(|l| l.id)
            .collect()
    };
    let deadline = Instant::now() + RESOLVE_LIMIT;
    for id in ids {
        let matching = matching(shared, id, candidates);
        // A pending logpoint takes the first source that matches.
        let source = matching
            .first()
            .map(|c| places::without_query(&c.source).to_owned());
        let calls = matching
            .iter()
            .filter(|c| Some(places::without_query(&c.source)) == source.as_deref())
            .map(|c| bind(shared, id, c, deadline));
        join_all(calls).await;
    }
}

/// The session paused: before a script with a source map runs (its
/// logpoints are bound), or for something chrome-cdp did not ask for (a
/// `debugger;` statement), which is noted in the console. Either way it
/// resumes.
async fn paused(shared: &Arc<Shared>, session: &Session, params: &serde_json::Value) {
    let session_id = session.id().unwrap_or_default().to_owned();
    if matches!(
        params["reason"].as_str(),
        Some("exception" | "promiseRejection")
    ) {
        locals::capture(shared, session, params).await;
        return;
    }
    // A failed `console.assert` pauses too where exceptions do: the
    // console has it already.
    if params["reason"] == "assert" {
        resume(session).await;
        return;
    }
    if params["reason"] == "instrumentation" {
        let data = &params["data"];
        let url = data["url"].as_str().unwrap_or_default().to_owned();
        let map = data["sourceMapURL"].as_str().unwrap_or_default().to_owned();
        let frame = {
            let state = shared.lock();
            state
                .targets
                .get(&session_id)
                .filter(|t| t.kind.is_host())
                .map(|t| t.id.clone())
        };
        if !url.is_empty() && !map.is_empty() {
            let deadline = Instant::now() + RESOLVE_LIMIT;
            let mapped = shared
                .sourcemaps
                .of_script(session, frame.as_deref(), &url, &map, deadline)
                .await;
            if let Some(mapped) = mapped {
                let candidates: Vec<Candidate> = mapped
                    .sources()
                    .into_iter()
                    .map(|source| Candidate {
                        session_id: session_id.clone(),
                        session: session.clone(),
                        url: url.clone(),
                        source,
                        mapped: Some(mapped.clone()),
                    })
                    .collect();
                bind_everywhere(shared, &candidates).await;
            }
        }
        resume(session).await;
        return;
    }
    resume(session).await;
    note_pause(shared, session, params).await;
}

pub(super) async fn resume(session: &Session) {
    let session = session.with_timeout(Duration::from_secs(5));
    if let Err(e) = session.call(debugger::ResumeParams::default()).await {
        tracing::debug!("resuming a pause failed: {e}");
    }
}

/// Notes a pause chrome-cdp did not ask for in the console of the
/// session's tabs: `paused at a debugger statement (src/x.ts:3); resumed`.
async fn note_pause(shared: &Arc<Shared>, session: &Session, params: &serde_json::Value) {
    let session_id = session.id().unwrap_or_default().to_owned();
    let top = &params["callFrames"][0];
    let script_id = top["location"]["scriptId"].as_str().unwrap_or_default();
    let line = top["location"]["lineNumber"].as_u64().unwrap_or(0) + 1;
    let column = top["location"]["columnNumber"].as_u64().unwrap_or(0) + 1;
    let (url, map, frame, root) = {
        let state = shared.lock();
        // The call frame's own `url` is deprecated (and empty): the
        // script's is the one it reported.
        let script = state
            .debug
            .sessions
            .get(&session_id)
            .and_then(|s| s.by_id.get(script_id));
        let url = script.map_or_else(
            || top["url"].as_str().unwrap_or_default().to_owned(),
            |s| s.url.clone(),
        );
        let map = script.map(|s| s.map.clone()).unwrap_or_default();
        let frame = state
            .targets
            .get(&session_id)
            .filter(|t| t.kind.is_host())
            .map(|t| t.id.clone());
        (url, map, frame, state.debug.root.clone())
    };
    let (line, column) = (
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(column).unwrap_or(u32::MAX),
    );
    let deadline = Instant::now() + RESOLVE_LIMIT;
    let original = if map.is_empty() {
        None
    } else {
        shared
            .sourcemaps
            .of_script(session, frame.as_deref(), &url, &map, deadline)
            .await
            .and_then(|m| m.lookup(line, column))
    };
    let place = match &original {
        Some(o) => format!("{}:{}", shown_source(&o.source, root.as_deref()).0, o.line),
        None => format!("{}:{line}", shown_source(&url, root.as_deref()).0),
    };
    let what = match params["reason"].as_str() {
        Some("other")
            if params["hitBreakpoints"]
                .as_array()
                .is_some_and(|b| !b.is_empty()) =>
        {
            "a breakpoint".to_owned()
        }
        Some("other") => "a debugger statement".to_owned(),
        Some(reason) => format!("a pause ({reason})"),
        None => "a pause".to_owned(),
    };
    let text = format!("paused at {what} ({place}); resumed");
    let mut entry = console::entry(Level::Warn, "debugger", None, text);
    entry.url = Some(original.as_ref().map_or(url, |o| o.source.clone()));
    entry.line = Some(original.as_ref().map_or(line, |o| o.line));
    log_to_tabs(shared, &session_id, &entry);
}

/// Logs `entry` in the console of each tab session `session_id` runs
/// scripts of.
fn log_to_tabs(shared: &Shared, session_id: &str, entry: &schema::ConsoleEntry) {
    let capacity = shared.settings.console_buffer();
    let mut guard = shared.lock();
    let state = &mut *guard;
    let tabs = state
        .targets
        .get(session_id)
        .map(|t| state.tabs_of(t))
        .unwrap_or_default();
    for id in tabs {
        if let Some(tab) = state.tabs.get_mut(&id) {
            tab.console.log(entry.clone(), capacity);
        }
    }
    drop(guard);
    shared.bump();
}

/// What a logpoint's condition sends on the binding.
#[derive(Debug, Deserialize)]
struct Payload {
    id: u32,
    #[serde(default)]
    values: Vec<Sent>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Sent {
    value: Option<String>,
    error: Option<String>,
}

/// Whether `event` is a call of the logpoints' binding.
pub(super) fn is_hit(shared: &Shared, event: &RawEvent) -> bool {
    event.params["name"].as_str() == Some(shared.lock().debug.binding.as_str())
}

/// A logpoint was hit: the hit goes to its tab's console. The one that
/// reaches its limit stops: its breakpoints go, and the console says so.
pub(super) fn hit(shared: &Arc<Shared>, event: &RawEvent) {
    let Some(payload) = event.params["payload"]
        .as_str()
        .and_then(|p| serde_json::from_str::<Payload>(p).ok())
    else {
        return;
    };
    let capacity = shared.settings.console_buffer();
    let mut guard = shared.lock();
    let state = &mut *guard;
    let root = state.debug.root.clone();
    let Some(logpoint) = state.debug.logpoints.get_mut(&payload.id) else {
        return;
    };
    if logpoint.stopped {
        return;
    }
    logpoint.hits += 1;
    let (place, file) = logpoint_place(logpoint, root.as_deref());
    let line = logpoint.bound_line.unwrap_or(logpoint.line);
    let mut sent = payload.values.into_iter();
    let values = logpoint
        .exprs
        .iter()
        .map(|expr| {
            let sent = sent.next().unwrap_or_default();
            LogpointValue {
                expr: expr.clone(),
                value: sent.value,
                error: sent.error,
            }
        })
        .collect();
    let hit = LogpointHit {
        id: logpoint.id,
        place: format!("{place}:{line}"),
        file: file.map(|f| f.to_string_lossy().into_owned()),
        line,
        values,
    };
    let mut entry = console::entry(Level::Info, "logpoint", None, hit_text(&hit));
    entry.kind = Some(logpoint.id.to_string());
    entry.url.clone_from(&logpoint.source);
    entry.line = Some(line);
    entry.logpoint = Some(hit);
    let stopped = logpoint.hits >= logpoint.limit;
    let mut notes = vec![entry];
    let mut gone = None;
    if stopped {
        logpoint.stopped = true;
        gone = Some(logpoint.clone());
        logpoint.bound.clear();
        let text = format!(
            "logpoint {} stopped after {} hits (its --limit); chrome-cdp debug clear {} removes it",
            logpoint.id, logpoint.hits, logpoint.id
        );
        let mut note = console::entry(
            Level::Info,
            "logpoint",
            Some(&logpoint.id.to_string()),
            text,
        );
        note.url.clone_from(&logpoint.source);
        notes.push(note);
    }
    let tab = logpoint.tab;
    if let Some(tab) = state.tabs.get_mut(&tab) {
        for note in notes {
            tab.console.log(note, capacity);
        }
    }
    drop(guard);
    shared.bump();
    if let Some(gone) = gone {
        let shared2 = shared.clone();
        shared.spawn(async move {
            unbind(&shared2, &[gone], STOP_TIMEOUT).await;
            let _ = sync(&shared2, STOP_TIMEOUT).await;
        });
    }
}

/// A hit as text shows it: `logpoint 1 src/cart.ts:42: total=12,
/// items.length=3`; an expression that threw shows its error in its
/// place (`total=<ReferenceError: total is not defined>`).
pub(crate) fn hit_text(hit: &LogpointHit) -> String {
    let values: Vec<String> = hit
        .values
        .iter()
        .map(|v| match (&v.value, &v.error) {
            (Some(value), _) => format!("{}={value}", v.expr),
            (None, Some(error)) => format!("{}=<{error}>", v.expr),
            (None, None) => format!("{}=?", v.expr),
        })
        .collect();
    if values.is_empty() {
        format!("logpoint {} {}", hit.id, hit.place)
    } else {
        format!("logpoint {} {}: {}", hit.id, hit.place, values.join(", "))
    }
}

/// Session `session_id`'s execution contexts went (all of them, or
/// `context`): so did their scripts.
pub(super) fn contexts_gone(shared: &Shared, session_id: &str, context: Option<i64>) {
    let mut state = shared.lock();
    if let Some(scripts) = state.debug.sessions.get_mut(session_id) {
        match context {
            Some(context) => scripts.by_id.retain(|_, s| s.context != context),
            None => scripts.by_id.clear(),
        }
    }
}

/// Tab `tab` closed: its logpoints go with it.
pub(super) fn tab_gone(state: &mut State, tab: u32) {
    state.debug.logpoints.retain(|_, l| l.tab != tab);
}

/// Session `session_id` went away.
pub(super) fn detached(state: &mut State, session_id: &str) {
    state.debug.sessions.remove(session_id);
    state.debug.placed.retain(|spot, _| spot.0 != session_id);
    for logpoint in state.debug.logpoints.values_mut() {
        logpoint.bound.retain(|b| b.session != session_id);
    }
}

#[cfg(test)]
mod tests {
    use protocol::schema::{LogpointHit, LogpointValue};

    use super::{condition, hit_text};

    #[test]
    fn hits_as_text() {
        let value = |expr: &str, value: Option<&str>, error: Option<&str>| LogpointValue {
            expr: expr.into(),
            value: value.map(Into::into),
            error: error.map(Into::into),
        };
        let mut hit = LogpointHit {
            id: 1,
            place: "src/cart.ts:42".into(),
            file: None,
            line: 42,
            values: vec![
                value("total", None, Some("ReferenceError: total is not defined")),
                value("items.length", Some("3"), None),
            ],
        };
        assert_eq!(
            hit_text(&hit),
            "logpoint 1 src/cart.ts:42: total=<ReferenceError: total is not defined>, items.length=3"
        );
        hit.values.clear();
        assert_eq!(hit_text(&hit), "logpoint 1 src/cart.ts:42");
    }

    #[test]
    fn conditions_end_each_expression_on_its_own_line() {
        let three = ["a // why".to_owned(), "b".to_owned()];
        let text = condition("b", &[(3, &three), (4, &[])]);
        assert!(
            text.contains(
                "__ccdpLogpoint.hit(\"b\", 3, [() => (a // why\n), () => (b\n)]);\n\
                 __ccdpLogpoint.hit(\"b\", 4, []);\n} catch"
            ),
            "{text}"
        );
        assert!(text.ends_with("return false;\n})()"));
    }
}
