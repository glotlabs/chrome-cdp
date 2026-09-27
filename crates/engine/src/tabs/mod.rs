//! The target & tab manager (plan 03, milestone 3.1).
//!
//! [`Tabs`] auto-attaches to every target of the browser: at the browser
//! level (tabs, popups, service and shared workers) and again on each page
//! and out-of-process iframe session (their OOPIFs and dedicated workers),
//! always with `waitForDebuggerOnStart`. A new target is paused before it
//! runs any script; [`setup`] applies the session's settings to it (init
//! scripts today; emulation and routes in plan 07), turns on the events
//! that track its frames and execution contexts (milestone 3.2, see
//! [`crate::frames`]), and only then resumes it. That is what makes
//! settings apply to a popup's first script.
//!
//! Page targets are tabs. Each gets a short id (`t1`, `t2`, …) in the order
//! Chrome announced it, never reused within the session. One tab is
//! *active*: commands act on it unless `--tab` names another. A tab no
//! command asked for (a popup) is recorded, so the command that caused it
//! can report `opened tab t3`; it becomes active only when the session
//! follows popups.

pub(crate) mod commands;
mod locks;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use cdp::domains::{page, runtime, security, target};
use cdp::{Connection, Event as _, RawEvent, RawEventStream, Session};
use protocol::schema::{Event, TabInfo};

use crate::frames::{self, Context, FindError, FrameInfo, FrameTree, World};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};
use tokio::sync::watch;
use tokio::task::AbortHandle;

pub(crate) use locks::TabLocks;

/// The URL of a new empty tab.
pub(crate) const BLANK: &str = "about:blank";

/// How long a step of a new target's setup may take. It is paused until
/// the setup ends, so a hung call must not hold it forever.
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);

/// How many unreported tab openings are kept.
const OPENED_MEMORY: usize = 64;

/// A tab commands can act on.
#[derive(Debug, Clone)]
pub(crate) struct Page {
    /// The number in its id (`3` for `t3`).
    pub(crate) id: u32,
    pub(crate) target_id: String,
    pub(crate) session: Session,
}

/// `t3`.
pub(crate) fn tab_name(id: u32) -> String {
    format!("t{id}")
}

/// Parses a tab id as typed: `t3` or `3`.
pub fn parse_tab_id(text: &str) -> Result<u32, ErrorInfo> {
    let text = text.trim();
    let digits = text.strip_prefix('t').unwrap_or(text);
    digits
        .parse::<u32>()
        .ok()
        .filter(|&n| n > 0 && !digits.starts_with('+'))
        .ok_or_else(|| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("`{text}` is not a tab id; tab ids look like t1, t2, …"),
            )
            .hint(Hint::run(Cmd::new("tabs")).why("lists them"))
        })
}

/// Settings every new target gets before it runs (see [`setup`]).
#[derive(Debug, Default)]
pub(crate) struct Settings {
    /// Scripts that run before any of a document's own.
    init_scripts: Mutex<Vec<String>>,
    /// Whether a popup becomes the active tab.
    follow_popups: AtomicBool,
    /// Whether pages load despite certificate errors
    /// (`Security.setIgnoreCertificateErrors`, browser-wide).
    ignore_https_errors: AtomicBool,
}

impl Settings {
    pub(crate) fn set_follow_popups(&self, follow: bool) {
        self.follow_popups.store(follow, Ordering::Relaxed);
    }

    pub(crate) fn set_ignore_https_errors(&self, ignore: bool) {
        self.ignore_https_errors.store(ignore, Ordering::Relaxed);
    }

    pub(crate) fn add_init_script(&self, source: &str) {
        lock(&self.init_scripts).push(source.to_owned());
    }

    fn init_scripts(&self) -> Vec<String> {
        lock(&self.init_scripts).clone()
    }
}

/// The tabs of one browser connection. Dropping it stops watching.
#[derive(Debug)]
pub(crate) struct Tabs {
    conn: Connection,
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    conn: Connection,
    settings: Arc<Settings>,
    state: Mutex<State>,
    /// Bumped on every change, for waiters.
    changed: watch::Sender<u64>,
    /// Whether this browser was told to ignore certificate errors.
    security_applied: AtomicBool,
    /// The watcher and setup tasks. They hold the connection, so they are
    /// stopped with the manager (a connected browser's WebSocket closes
    /// only when the last handle goes).
    tasks: Mutex<Vec<AbortHandle>>,
}

#[derive(Debug)]
struct State {
    next_id: u32,
    tabs: BTreeMap<u32, Tab>,
    active: Option<u32>,
    /// Tab ids by when they were last active, most recent last.
    recent: Vec<u32>,
    /// Page and iframe sessions (they take init scripts), by session id.
    hosts: HashMap<String, Host>,
    /// Targets commands created (or found at start): not "opened" events.
    ours: HashSet<String>,
    /// Tabs that opened, newest last, with a sequence number.
    opened: VecDeque<(u64, u32)>,
    seq: u64,
    /// Until `init` ends: tabs that attach are the browser's own, not new.
    starting: bool,
}

#[derive(Debug)]
struct Tab {
    target_id: String,
    session: Session,
    url: String,
    title: String,
    opener_target: Option<String>,
    /// Set up and resumed; commands may use it.
    ready: bool,
    /// Its frames, also those of its OOPIF sessions.
    frames: FrameTree,
}

/// A page or iframe session.
#[derive(Debug)]
struct Host {
    session: Session,
    /// The tab whose frames it has (none for a prerender page).
    tab: Option<u32>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Drop for Tabs {
    fn drop(&mut self) {
        for task in lock(&self.shared.tasks).drain(..) {
            task.abort();
        }
    }
}

impl Tabs {
    /// Starts watching `conn`'s targets. Call [`Tabs::init`] next.
    pub(crate) fn start(conn: &Connection, settings: Arc<Settings>) -> Self {
        let shared = Arc::new(Shared {
            conn: conn.clone(),
            settings,
            state: Mutex::new(State {
                next_id: 1,
                tabs: BTreeMap::new(),
                active: None,
                recent: Vec::new(),
                hosts: HashMap::new(),
                ours: HashSet::new(),
                opened: VecDeque::new(),
                seq: 0,
                starting: true,
            }),
            changed: watch::Sender::new(0),
            security_applied: AtomicBool::new(false),
            tasks: Mutex::default(),
        });
        // Subscribe before anything can attach.
        let events = conn.browser().events();
        shared.spawn(watch_browser(shared.clone(), events));
        Self {
            conn: conn.clone(),
            shared,
        }
    }

    /// Turns on discovery and auto-attach at the browser level, and waits
    /// until the pages that exist already are tabs.
    pub(crate) async fn init(&self, timeout: Duration) -> cdp::Result<()> {
        let browser = self.conn.browser().with_timeout(timeout);
        self.apply_security(timeout).await?;
        browser
            .call(target::SetDiscoverTargetsParams::new(true))
            .await?;
        browser.call(auto_attach()).await?;
        let pages: Vec<String> = browser
            .call(target::GetTargetsParams::default())
            .await?
            .target_infos
            .into_iter()
            .filter(is_tab)
            .map(|t| t.target_id)
            .collect();
        let ready = self
            .wait(timeout, |state| {
                pages.iter().all(|target_id| {
                    state
                        .tabs
                        .values()
                        .any(|t| t.ready && &t.target_id == target_id)
                })
            })
            .await;
        if !ready {
            tracing::warn!("not every page was attached within {timeout:?}");
        }
        let mut state = self.shared.lock();
        state.starting = false;
        state.ours.extend(pages);
        if state.active.is_none()
            && let Some(&first) = state.tabs.keys().next()
        {
            state.activate(first);
        }
        drop(state);
        self.shared.bump();
        Ok(())
    }

    /// Applies the session's `--ignore-https-errors` to the browser. It is
    /// only sent when set, or when turning it off again.
    pub(crate) async fn apply_security(&self, timeout: Duration) -> cdp::Result<()> {
        let ignore = self
            .shared
            .settings
            .ignore_https_errors
            .load(Ordering::Relaxed);
        let applied = self.shared.security_applied.swap(ignore, Ordering::Relaxed);
        if ignore || applied {
            self.conn
                .browser()
                .with_timeout(timeout)
                .call(security::SetIgnoreCertificateErrorsParams::new(ignore))
                .await?;
        }
        Ok(())
    }

    /// Waits up to `timeout` for `done` to hold; `false` if it did not.
    async fn wait(&self, timeout: Duration, mut done: impl FnMut(&State) -> bool) -> bool {
        let mut changed = self.shared.changed.subscribe();
        tokio::time::timeout(timeout, async {
            loop {
                if done(&self.shared.lock()) {
                    return;
                }
                if changed.changed().await.is_err() {
                    return;
                }
            }
        })
        .await
        .is_ok()
    }

    /// The tab `id`, or the active one. Without any tab, opens a blank one.
    pub(crate) async fn page(&self, id: Option<u32>, timeout: Duration) -> Result<Page, TabError> {
        if let Some(id) = id {
            // A tab that just attached may still be setting up.
            let exists = self
                .wait(timeout.min(SETUP_TIMEOUT), |state| {
                    state.tabs.get(&id).is_none_or(|t| t.ready)
                })
                .await;
            return match self.shared.lock().page(id) {
                Some(page) if exists => Ok(page),
                _ => Err(TabError::NoSuchTab(id)),
            };
        }
        {
            let mut state = self.shared.lock();
            let current = state
                .active
                .filter(|id| state.tabs.get(id).is_some_and(|t| t.ready))
                .or_else(|| state.fallback());
            if let Some(id) = current {
                state.activate(id);
                if let Some(page) = state.page(id) {
                    return Ok(page);
                }
            }
        }
        let page = self.create(BLANK, timeout).await?;
        self.shared.lock().activate(page.id);
        Ok(page)
    }

    /// Opens a tab at `url` and waits until it is set up. It does not
    /// become active.
    pub(crate) async fn create(&self, url: &str, timeout: Duration) -> Result<Page, TabError> {
        let created = self
            .conn
            .browser()
            .with_timeout(timeout)
            .call(target::CreateTargetParams::new(url.to_owned()))
            .await?;
        let target_id = created.target_id;
        self.shared.lock().ours.insert(target_id.clone());
        self.wait(timeout, |state| {
            state
                .tabs
                .values()
                .any(|t| t.ready && t.target_id == target_id)
        })
        .await;
        let state = self.shared.lock();
        state
            .tabs
            .iter()
            .find(|(_, t)| t.target_id == target_id)
            .and_then(|(&id, _)| state.page(id))
            .ok_or(TabError::NotAttached)
    }

    /// Makes `id` the active tab and brings it to the front.
    pub(crate) async fn activate(&self, id: u32, timeout: Duration) -> Result<Page, TabError> {
        let page = self.page(Some(id), timeout).await?;
        self.shared.lock().activate(id);
        self.shared.bump();
        self.conn
            .browser()
            .with_timeout(timeout)
            .call(target::ActivateTargetParams::new(page.target_id.clone()))
            .await?;
        Ok(page)
    }

    /// Closes tab `id`; another becomes active if it was.
    pub(crate) async fn close(&self, id: u32, timeout: Duration) -> Result<(), TabError> {
        let page = self.page(Some(id), timeout).await?;
        self.conn
            .browser()
            .with_timeout(timeout)
            .call(target::CloseTargetParams::new(page.target_id))
            .await?;
        // Chrome reports the detach too; do not wait for it.
        self.shared.lock().remove(id);
        self.shared.bump();
        Ok(())
    }

    /// Every ready tab, with its current URL and title.
    pub(crate) async fn list(&self, timeout: Duration) -> cdp::Result<Vec<TabInfo>> {
        self.refresh(timeout).await?;
        Ok(self.list_cached())
    }

    /// Every ready tab, with the URL and title last seen.
    pub(crate) fn list_cached(&self) -> Vec<TabInfo> {
        let state = self.shared.lock();
        state
            .tabs
            .iter()
            .filter(|(_, t)| t.ready)
            .map(|(&id, _)| state.info(id))
            .collect()
    }

    /// Tab `id` as `tabs` shows it (URL and title as last seen).
    pub(crate) fn info(&self, id: u32) -> Option<TabInfo> {
        let state = self.shared.lock();
        state.tabs.contains_key(&id).then(|| state.info(id))
    }

    /// Waits up to `timeout` until tabs `ids` have a URL: a popup has none
    /// until its first navigation commits.
    pub(crate) async fn wait_for_urls(&self, ids: &[u32], timeout: Duration) {
        self.wait(timeout, |state| {
            ids.iter().all(|id| {
                state
                    .tabs
                    .get(id)
                    .is_none_or(|t| !t.url.is_empty() && t.url != BLANK)
            })
        })
        .await;
    }

    /// Updates every tab's URL and title from Chrome.
    pub(crate) async fn refresh(&self, timeout: Duration) -> cdp::Result<()> {
        let targets = self
            .conn
            .browser()
            .with_timeout(timeout)
            .call(target::GetTargetsParams::default())
            .await?
            .target_infos;
        let mut state = self.shared.lock();
        for info in targets {
            state.update(&info);
        }
        Ok(())
    }

    /// How many tabs are open.
    pub(crate) fn count(&self) -> u32 {
        let state = self.shared.lock();
        let ready = state.tabs.values().filter(|t| t.ready).count();
        u32::try_from(ready).unwrap_or(u32::MAX)
    }

    /// The id of the active tab, if any.
    pub(crate) fn active(&self) -> Option<u32> {
        self.shared.lock().active
    }

    /// A mark to pass to [`Tabs::opened_since`].
    pub(crate) fn mark(&self) -> u64 {
        self.shared.lock().seq
    }

    /// Tabs that opened after `mark` without a command asking for them.
    pub(crate) fn opened_since(&self, mark: u64) -> Vec<Event> {
        let state = self.shared.lock();
        state
            .opened
            .iter()
            .filter(|&&(seq, _)| seq > mark)
            .filter_map(|&(_, id)| {
                let tab = state.tabs.get(&id)?;
                if state.ours.contains(&tab.target_id) {
                    return None;
                }
                let info = state.info(id);
                Some(Event::TabOpened {
                    tab: info.id,
                    url: info.url,
                    opener: info.opener,
                    active: info.active,
                })
            })
            .collect()
    }

    /// Tab `id`'s frames, parents before their children.
    pub(crate) fn frames(&self, id: u32) -> Vec<FrameInfo> {
        self.shared
            .lock()
            .tabs
            .get(&id)
            .map(|t| t.frames.list())
            .unwrap_or_default()
    }

    /// The frame of tab `id` that `query` names (see
    /// [`FrameTree::find`]).
    pub(crate) fn find_frame(&self, id: u32, query: &str) -> Result<FrameInfo, FindError> {
        match self.shared.lock().tabs.get(&id) {
            Some(tab) => tab.frames.find(query),
            None => Err(FindError::NotFound(Vec::new())),
        }
    }

    /// Where to run a script in `world` of frame `frame` (tab `id`): the
    /// session that owns the frame and the context's id. Waits up to
    /// `timeout` for the frame's document to have one. The isolated world
    /// is made on first use if the frame has none yet (a frame that
    /// existed before the session attached).
    pub(crate) async fn context(
        &self,
        id: u32,
        frame: &str,
        world: World,
        timeout: Duration,
    ) -> Result<FrameContext, FrameError> {
        enum Found {
            Context(Context),
            /// The main world exists, so the document does: make ours.
            Create(String),
            Gone,
        }
        let mut found = None;
        self.wait(timeout, |state| {
            let tree = state.tabs.get(&id).map(|t| &t.frames);
            let Some((tree, owner)) = tree.and_then(|t| Some((t, t.owner(frame)?))) else {
                found = Some(Found::Gone);
                return true;
            };
            if let Some(context) = tree.context(frame, world) {
                found = Some(Found::Context(context.clone()));
            } else if world == World::Isolated && tree.context(frame, World::Main).is_some() {
                found = Some(Found::Create(owner.to_owned()));
            }
            found.is_some()
        })
        .await;
        let session_of = |session_id: &str| {
            let state = self.shared.lock();
            state.hosts.get(session_id).map(|h| h.session.clone())
        };
        match found {
            Some(Found::Context(context)) => Ok(FrameContext {
                session: session_of(&context.session).ok_or(FrameError::Gone)?,
                context_id: context.id,
            }),
            Some(Found::Create(owner)) => {
                let session = session_of(&owner).ok_or(FrameError::Gone)?;
                let mut params = page::CreateIsolatedWorldParams::new(frame.to_owned());
                params.world_name = Some(frames::WORLD_NAME.to_owned());
                let created = session.with_timeout(timeout).call(params).await?;
                let context_id = created.execution_context_id;
                if let Some(tab) = self.shared.lock().tabs.get_mut(&id) {
                    let context = Context {
                        session: owner,
                        id: context_id,
                    };
                    tab.frames.isolated_created(frame, context);
                }
                Ok(FrameContext {
                    session,
                    context_id,
                })
            }
            Some(Found::Gone) => Err(FrameError::Gone),
            None => Err(FrameError::NoContext),
        }
    }

    /// Forgets `context` of tab `id`: Chrome said it is gone before the
    /// event that says so arrived.
    pub(crate) fn forget_context(&self, id: u32, context: &FrameContext) {
        let session_id = context.session.id().unwrap_or_default();
        if let Some(tab) = self.shared.lock().tabs.get_mut(&id) {
            tab.frames.context_destroyed(session_id, context.context_id);
        }
    }

    /// Adds a script that runs before any of a document's own, in every tab
    /// and frame from now on (plan 07's init scripts build on this).
    pub(crate) async fn add_init_script(&self, source: &str, timeout: Duration) -> cdp::Result<()> {
        self.shared.settings.add_init_script(source);
        let hosts: Vec<Session> = self
            .shared
            .lock()
            .hosts
            .values()
            .map(|h| h.session.clone())
            .collect();
        for host in hosts {
            let result = host
                .with_timeout(timeout)
                .call(page::AddScriptToEvaluateOnNewDocumentParams::new(
                    source.to_owned(),
                ))
                .await;
            if let Err(e) = result {
                tracing::debug!("cannot add the init script to a target: {e}");
            }
        }
        Ok(())
    }
}

/// Where a script runs: a frame's session and one of its contexts.
#[derive(Debug, Clone)]
pub(crate) struct FrameContext {
    pub(crate) session: Session,
    pub(crate) context_id: runtime::ExecutionContextId,
}

/// Why a frame has no context to run a script in.
#[derive(Debug)]
pub(crate) enum FrameError {
    /// The frame (or its tab) went away.
    Gone,
    /// Its document has no context yet (still loading, or stuck).
    NoContext,
    Cdp(cdp::CdpError),
}

impl From<cdp::CdpError> for FrameError {
    fn from(e: cdp::CdpError) -> Self {
        Self::Cdp(e)
    }
}

/// Why a tab is not available.
#[derive(Debug)]
pub(crate) enum TabError {
    /// No open tab has this id.
    NoSuchTab(u32),
    /// Chrome created the tab but never announced it.
    NotAttached,
    Cdp(cdp::CdpError),
}

impl From<cdp::CdpError> for TabError {
    fn from(e: cdp::CdpError) -> Self {
        Self::Cdp(e)
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    fn bump(&self) {
        self.changed.send_modify(|n| *n += 1);
    }

    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        let handle = tokio::spawn(task).abort_handle();
        let mut tasks = lock(&self.tasks);
        tasks.retain(|t| !t.is_finished());
        tasks.push(handle);
    }
}

impl State {
    fn page(&self, id: u32) -> Option<Page> {
        let tab = self.tabs.get(&id).filter(|t| t.ready)?;
        Some(Page {
            id,
            target_id: tab.target_id.clone(),
            session: tab.session.clone(),
        })
    }

    fn activate(&mut self, id: u32) {
        self.active = Some(id);
        self.recent.retain(|&r| r != id);
        self.recent.push(id);
    }

    /// The tab to fall back to when there is no active one: the one active
    /// most recently, else the newest.
    fn fallback(&self) -> Option<u32> {
        let ready = |id: &u32| self.tabs.get(id).is_some_and(|t| t.ready);
        self.recent
            .iter()
            .rev()
            .copied()
            .find(ready)
            .or_else(|| self.tabs.keys().rev().copied().find(ready))
    }

    fn remove(&mut self, id: u32) {
        if self.tabs.remove(&id).is_some() {
            self.hosts.retain(|_, h| h.tab != Some(id));
        }
        self.recent.retain(|&r| r != id);
        if self.active == Some(id) {
            self.active = self.fallback();
        }
    }

    fn update(&mut self, info: &target::TargetInfo) {
        if let Some(tab) = self
            .tabs
            .values_mut()
            .find(|t| t.target_id == info.target_id)
        {
            tab.url.clone_from(&info.url);
            tab.title.clone_from(&info.title);
        }
    }

    fn id_of(&self, target_id: &str) -> Option<u32> {
        self.tabs
            .iter()
            .find(|(_, t)| t.target_id == target_id)
            .map(|(&id, _)| id)
    }

    fn info(&self, id: u32) -> TabInfo {
        let tab = &self.tabs[&id];
        TabInfo {
            id: tab_name(id),
            active: self.active == Some(id),
            title: tab.title.clone(),
            url: tab.url.clone(),
            opener: tab
                .opener_target
                .as_deref()
                .and_then(|t| self.id_of(t))
                .map(tab_name),
        }
    }
}

/// Whether a target is a tab: a page, not a prerender or other special
/// kind.
fn is_tab(info: &target::TargetInfo) -> bool {
    info.r#type == "page" && info.subtype.is_none()
}

fn auto_attach() -> target::SetAutoAttachParams {
    let mut params = target::SetAutoAttachParams::new(true, true);
    params.flatten = Some(true);
    params
}

/// Handles the browser session's target events.
async fn watch_browser(shared: Arc<Shared>, mut events: RawEventStream) {
    while let Some(event) = events.next().await {
        match event.method.as_str() {
            target::AttachedToTarget::NAME => attached(&shared, &event, Parent::Browser),
            target::DetachedFromTarget::NAME => {
                if let Some(session_id) = event.params["sessionId"].as_str() {
                    detached(&shared, session_id);
                }
            }
            target::TargetDestroyed::NAME => {
                if let Some(target_id) = event.params["targetId"].as_str() {
                    let mut state = shared.lock();
                    if let Some(id) = state.id_of(target_id) {
                        state.remove(id);
                        drop(state);
                        shared.bump();
                    }
                }
            }
            target::TargetInfoChanged::NAME => {
                if let Ok(changed) = decode::<target::TargetInfoChanged>(&event) {
                    shared.lock().update(&changed.target_info);
                }
            }
            _ => {}
        }
    }
}

/// Handles a page or iframe session's events: its own auto-attached
/// targets (OOPIFs, workers) and their detaching, and the frame and
/// execution context events of tab `tab`'s frames it owns.
async fn watch_host(
    shared: Arc<Shared>,
    mut events: RawEventStream,
    session_id: String,
    tab: Option<u32>,
) {
    while let Some(event) = events.next().await {
        match event.method.as_str() {
            target::AttachedToTarget::NAME => attached(&shared, &event, Parent::Host(tab)),
            target::DetachedFromTarget::NAME => {
                if let Some(session_id) = event.params["sessionId"].as_str() {
                    detached(&shared, session_id);
                }
            }
            method => {
                if let Some(tab) = tab {
                    frame_event(&shared, &session_id, tab, method, &event);
                }
            }
        }
    }
}

/// Applies a frame or execution context event of `session_id` to tab
/// `tab`'s frame tree.
fn frame_event(shared: &Shared, session_id: &str, tab: u32, method: &str, event: &RawEvent) {
    let mut state = shared.lock();
    let Some(frames) = state.tabs.get_mut(&tab).map(|t| &mut t.frames) else {
        return;
    };
    let decoded = match method {
        page::FrameAttached::NAME => decode::<page::FrameAttached>(event)
            .map(|e| frames.attached(session_id, &e.frame_id, &e.parent_frame_id)),
        page::FrameNavigated::NAME => decode::<page::FrameNavigated>(event).map(|e| {
            if e.r#type == page::NavigationType::BackForwardCacheRestore {
                frames.restored(session_id, &e.frame);
            } else {
                frames.navigated(session_id, &e.frame);
            }
        }),
        page::NavigatedWithinDocument::NAME => decode::<page::NavigatedWithinDocument>(event)
            .map(|e| frames.navigated_within_document(&e.frame_id, &e.url)),
        page::FrameDetached::NAME => decode::<page::FrameDetached>(event).map(|e| {
            let swap = e.reason == page::FrameDetachedReason::Swap;
            frames.detached(&e.frame_id, swap);
        }),
        runtime::ExecutionContextCreated::NAME => decode::<runtime::ExecutionContextCreated>(event)
            .map(|e| frames.context_created(session_id, &e.context)),
        runtime::ExecutionContextDestroyed::NAME => {
            decode::<runtime::ExecutionContextDestroyed>(event)
                .map(|e| frames.context_destroyed(session_id, e.execution_context_id))
        }
        runtime::ExecutionContextsCleared::NAME => {
            frames.contexts_cleared(session_id);
            Ok(())
        }
        _ => return,
    };
    drop(state);
    match decoded {
        Ok(()) => shared.bump(),
        Err(e) => tracing::warn!("cannot decode {method}: {e}"),
    }
}

fn decode<E: cdp::Event>(event: &RawEvent) -> Result<E, serde_json::Error> {
    E::deserialize(&event.params)
}

/// Which session announced a target.
#[derive(Debug, Clone, Copy)]
enum Parent {
    Browser,
    /// A page or iframe session, with its tab (none for a prerender).
    Host(Option<u32>),
}

/// A target attached (paused if new): register a tab for a page, then set
/// it up and resume it in its own task.
fn attached(shared: &Arc<Shared>, event: &RawEvent, parent: Parent) {
    let attached = match decode::<target::AttachedToTarget>(event) {
        Ok(attached) => attached,
        Err(e) => {
            tracing::warn!("cannot decode Target.attachedToTarget: {e}");
            return;
        }
    };
    let info = attached.target_info;
    let session_id = attached.session_id;
    let session = shared.conn.session(session_id.clone());
    let new_tab = if matches!(parent, Parent::Browser) && is_tab(&info) {
        let mut state = shared.lock();
        let id = state.next_id;
        state.next_id += 1;
        state.tabs.insert(
            id,
            Tab {
                target_id: info.target_id.clone(),
                session: session.clone(),
                url: info.url.clone(),
                title: info.title.clone(),
                opener_target: info.opener_id.clone(),
                ready: false,
                frames: FrameTree::new(&info.target_id, &session_id),
            },
        );
        Some(id)
    } else {
        None
    };
    tracing::debug!(
        kind = %info.r#type,
        url = %info.url,
        waiting = attached.waiting_for_debugger,
        "attached to {}",
        info.target_id
    );
    let host = matches!(info.r#type.as_str(), "page" | "iframe");
    let tab = match parent {
        Parent::Browser => new_tab,
        Parent::Host(tab) => tab,
    };
    if host {
        // Before auto-attach and the domains are turned on for it, so no
        // child and no frame event is missed.
        let events = session.events();
        shared.lock().hosts.insert(
            session_id.clone(),
            Host {
                session: session.clone(),
                tab,
            },
        );
        shared.spawn(watch_host(shared.clone(), events, session_id, tab));
    }
    let waiting = attached.waiting_for_debugger;
    let task_shared = shared.clone();
    shared.spawn(async move {
        let session = session.with_timeout(SETUP_TIMEOUT);
        setup(&task_shared, &session, host, tab, waiting).await;
        if let Some(id) = new_tab {
            ready(&task_shared, id);
        }
    });
}

/// Prepares a new target before it runs: auto-attach for its children,
/// the domains commands need, frame tracking and chrome-cdp's isolated
/// world, the session's settings; then resumes it. `host` is set for page
/// and iframe sessions, with `tab` the tab whose frames they have.
///
/// Every step and the resume are sent at once and only then awaited. Chrome
/// runs them in order, but some answers (`Page.enable` in a popup's new
/// renderer) only come once the target runs, so awaiting each step before
/// sending the next would stall until the timeout.
///
/// Later milestones add their per-target steps here (emulation and routes
/// in plan 07).
async fn setup(
    shared: &Arc<Shared>,
    session: &Session,
    host: bool,
    tab: Option<u32>,
    waiting: bool,
) {
    let mut steps: Vec<(&str, Step)> = Vec::new();
    if host {
        steps.push(("auto-attach", step(session.call(auto_attach()))));
        steps.push((
            "Page.enable",
            step(session.call(page::EnableParams::default())),
        ));
        // `init`, `DOMContentLoaded`, `load`, … per frame, for navigation
        // waits.
        steps.push((
            "Page.setLifecycleEventsEnabled",
            step(session.call(page::SetLifecycleEventsEnabledParams::new(true))),
        ));
        // Reports every execution context, the existing ones too.
        steps.push((
            "Runtime.enable",
            step(session.call(runtime::EnableParams::default())),
        ));
        // The frames that exist already; events report later ones.
        let tree = session.call(page::GetFrameTreeParams::default());
        let seed_shared = shared.clone();
        let session_id = session.id().unwrap_or_default().to_owned();
        steps.push((
            "Page.getFrameTree",
            Box::pin(async move {
                let tree = tree.await?.frame_tree;
                if let Some(tab) = tab {
                    seed(&seed_shared, tab, &session_id, &tree);
                }
                Ok(())
            }),
        ));
        // chrome-cdp's isolated world in every document from now on, and
        // in the ones that exist already.
        let mut world = page::AddScriptToEvaluateOnNewDocumentParams::new(String::new());
        world.world_name = Some(frames::WORLD_NAME.to_owned());
        world.run_immediately = Some(true);
        steps.push(("the isolated world", step(session.call(world))));
        for source in shared.settings.init_scripts() {
            let add = page::AddScriptToEvaluateOnNewDocumentParams::new(source);
            steps.push(("an init script", step(session.call(add))));
        }
    }
    if waiting {
        let resume = runtime::RunIfWaitingForDebuggerParams::default();
        steps.push(("resuming", step(session.call(resume))));
    }
    for (what, result) in steps {
        if let Err(e) = result.await {
            // Usually the target closed meanwhile.
            tracing::debug!("setting up a target failed at {what}: {e}");
        }
    }
}

/// Adds the frames `Page.getFrameTree` reported on `session_id` to tab
/// `tab`.
fn seed(shared: &Shared, tab: u32, session_id: &str, tree: &page::FrameTree) {
    if let Some(tab) = shared.lock().tabs.get_mut(&tab) {
        tab.frames.seed(session_id, tree);
    }
    shared.bump();
}

/// One setup call already sent, its result discarded.
type Step = Pin<Box<dyn Future<Output = cdp::Result<()>> + Send>>;

fn step<T>(call: impl Future<Output = cdp::Result<T>> + Send + 'static) -> Step {
    Box::pin(async move { call.await.map(drop) })
}

/// Tab `id` is set up: commands may use it now.
fn ready(shared: &Shared, id: u32) {
    let mut state = shared.lock();
    let Some(tab) = state.tabs.get_mut(&id) else {
        return;
    };
    tab.ready = true;
    let target_id = tab.target_id.clone();
    if !state.starting {
        state.seq += 1;
        let seq = state.seq;
        state.opened.push_back((seq, id));
        if state.opened.len() > OPENED_MEMORY {
            state.opened.pop_front();
        }
        let follow = shared.settings.follow_popups.load(Ordering::Relaxed);
        if follow && !state.ours.contains(&target_id) {
            state.activate(id);
        }
    }
    drop(state);
    shared.bump();
}

/// A session went away: forget its tab or frame.
fn detached(shared: &Shared, session_id: &str) {
    let mut state = shared.lock();
    let host = state.hosts.remove(session_id);
    if let Some(tab) = host.and_then(|h| h.tab)
        && let Some(tab) = state.tabs.get_mut(&tab)
    {
        tab.frames.session_gone(session_id);
    }
    let tab = state
        .tabs
        .iter()
        .find(|(_, t)| t.session.id() == Some(session_id))
        .map(|(&id, _)| id);
    if let Some(id) = tab {
        state.remove(id);
    }
    drop(state);
    shared.bump();
}

#[cfg(test)]
mod tests {
    use super::parse_tab_id;

    #[test]
    fn tab_ids() {
        assert_eq!(parse_tab_id("t3").unwrap(), 3);
        assert_eq!(parse_tab_id("12").unwrap(), 12);
        for bad in ["", "t", "t0", "x3", "t-1", "t+1", "tt1", "3.0"] {
            let err = parse_tab_id(bad).unwrap_err();
            assert_eq!(err.exit_code(), 2, "{bad}");
        }
    }
}
