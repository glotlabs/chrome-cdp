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
//!
//! Milestone 3.5 adds what can happen to a tab besides navigating: a
//! JavaScript dialog (see [`dialogs`]), a download (see [`downloads`]), a
//! crash of its page, and HTTP authentication (see [`auth`]). Tab
//! openings, dialogs answered without a command, and downloads share one
//! event sequence, so a command reports what happened since its
//! [`Tabs::mark`].

mod auth;
pub(crate) mod commands;
pub(crate) mod dialogs;
mod downloads;
mod locks;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use cdp::domains::{browser, network, page, runtime, security, target};
use cdp::{Connection, Event as _, RawEvent, RawEventStream, Session};
use protocol::schema::{Event, TabInfo};

use self::auth::Auth;
use self::dialogs::OpenDialog;
use self::downloads::Download;
use crate::frames::{self, Context, FindError, FrameInfo, FrameTree, World};
use crate::injected;
use crate::input::Pointer;
use crate::network::{Exchange, Failure, Idle, InFlight, Network};
use crate::observe::console::{self, Console, ConsoleError};
use crate::observe::netlog::{self, NetLog};
use crate::request::{DialogPolicy, HttpCredentials};
use crate::snapshot::{FrameSource, Memory as SnapshotMemory};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};
use tokio::sync::watch;
use tokio::task::AbortHandle;
use tokio::time::Instant;

pub(crate) use locks::TabLocks;

/// The URL of a new empty tab.
pub(crate) const BLANK: &str = "about:blank";

/// How long a step of a new target's setup may take. It is paused until
/// the setup ends, so a hung call must not hold it forever.
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);

/// How many unreported tab openings (and logged dialogs) are kept.
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
    /// How JavaScript dialogs are answered.
    dialogs: Mutex<DialogPolicy>,
    /// What HTTP authentication challenges are answered with.
    http_credentials: Mutex<Option<HttpCredentials>>,
    /// Where downloads go; `None` leaves Chrome's download behaviour alone.
    pub(crate) downloads_dir: Option<PathBuf>,
    /// How many console entries each tab keeps (`open --console-buffer`).
    console_buffer: AtomicUsize,
    /// How many requests each tab's network log keeps (`open
    /// --network-buffer`).
    network_buffer: AtomicUsize,
}

impl Settings {
    /// Settings with downloads going to `downloads_dir`.
    pub(crate) fn new(downloads_dir: Option<PathBuf>) -> Self {
        Self {
            downloads_dir,
            console_buffer: AtomicUsize::new(console::DEFAULT_CAPACITY),
            network_buffer: AtomicUsize::new(netlog::DEFAULT_CAPACITY),
            ..Self::default()
        }
    }

    pub(crate) fn set_console_buffer(&self, entries: usize) {
        self.console_buffer.store(entries.max(1), Ordering::Relaxed);
    }

    pub(crate) fn console_buffer(&self) -> usize {
        match self.console_buffer.load(Ordering::Relaxed) {
            0 => console::DEFAULT_CAPACITY,
            n => n,
        }
    }

    pub(crate) fn set_network_buffer(&self, requests: usize) {
        self.network_buffer
            .store(requests.max(1), Ordering::Relaxed);
    }

    pub(crate) fn network_buffer(&self) -> usize {
        match self.network_buffer.load(Ordering::Relaxed) {
            0 => netlog::DEFAULT_CAPACITY,
            n => n,
        }
    }

    pub(crate) fn set_dialogs(&self, policy: DialogPolicy) {
        *lock(&self.dialogs) = policy;
    }

    pub(crate) fn dialogs(&self) -> DialogPolicy {
        *lock(&self.dialogs)
    }

    pub(crate) fn set_http_credentials(&self, credentials: HttpCredentials) {
        *lock(&self.http_credentials) = Some(credentials);
    }

    fn http_credentials(&self) -> Option<HttpCredentials> {
        lock(&self.http_credentials).clone()
    }

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
    /// Dialogs answered without a command, newest last, with a sequence
    /// number.
    logged: VecDeque<(u64, Event)>,
    /// The session's downloads, oldest first.
    downloads: Vec<Download>,
    /// The last sequence number given to a tab opening, dialog or
    /// download.
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
    /// Its requests, also those of its OOPIF sessions.
    network: Network,
    /// Its console, also that of its OOPIF sessions and dedicated
    /// workers.
    console: Console,
    /// Its network log, also that of its OOPIF sessions.
    netlog: NetLog,
    /// The network log's marks when the page command before the current
    /// one, and the current one, began.
    command_marks: (u64, u64),
    /// The same for the event sequence (downloads).
    event_marks: (u64, u64),
    /// A dialog waiting for `chrome-cdp dialog`.
    dialog: Option<OpenDialog>,
    /// When its page crashed (in the event sequence), until it is loaded
    /// again.
    crashed: Option<u64>,
    /// Its refs and last snapshot.
    snapshot: SnapshotMemory,
    /// Where the mouse is and what it holds, as chrome-cdp sent it.
    pointer: Pointer,
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
                logged: VecDeque::new(),
                downloads: Vec::new(),
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
        self.manage_downloads(timeout).await;
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

    /// Sends downloads to the session's directory (created, user-only),
    /// with events. A browser that refuses keeps its own behaviour.
    async fn manage_downloads(&self, timeout: Duration) {
        let Some(dir) = &self.shared.settings.downloads_dir else {
            return;
        };
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        if let Err(e) = builder.create(dir) {
            tracing::warn!("cannot create {}: {e}", dir.display());
            return;
        }
        let mut params = browser::SetDownloadBehaviorParams::new(
            browser::SetDownloadBehaviorBehavior::AllowAndName,
        );
        params.download_path = Some(dir.display().to_string());
        params.events_enabled = Some(true);
        let result = self.conn.browser().with_timeout(timeout).call(params).await;
        if let Err(e) = result {
            tracing::warn!("cannot send downloads to {}: {e}", dir.display());
        }
    }

    /// Gives a browser we leave running (`close` of a connected session)
    /// its own download behaviour back.
    pub(crate) async fn release(&self, timeout: Duration) {
        if self.shared.settings.downloads_dir.is_none() || self.conn.is_closed() {
            return;
        }
        let params =
            browser::SetDownloadBehaviorParams::new(browser::SetDownloadBehaviorBehavior::Default);
        let result = self.conn.browser().with_timeout(timeout).call(params).await;
        if let Err(e) = result {
            tracing::debug!("cannot reset the download behaviour: {e}");
        }
    }

    /// Turns on answering HTTP authentication challenges in every page and
    /// frame session (new ones get it during [`setup`]).
    pub(crate) async fn apply_credentials(&self, timeout: Duration) {
        if self.shared.settings.http_credentials().is_none() {
            return;
        }
        let hosts: Vec<Session> = self
            .shared
            .lock()
            .hosts
            .values()
            .map(|h| h.session.clone())
            .collect();
        for host in hosts {
            if let Err(e) = host.with_timeout(timeout).call(auth::enable()).await {
                tracing::debug!("cannot intercept requests of a target: {e}");
            }
        }
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

    /// Dialogs answered without a command after `mark`, and downloads
    /// that began after it (as they are now).
    pub(crate) fn happened_since(&self, mark: u64) -> Vec<Event> {
        let state = self.shared.lock();
        let dialogs = state
            .logged
            .iter()
            .filter(|(seq, _)| *seq > mark)
            .map(|(_, event)| event.clone());
        let downloads = state
            .downloads_since(mark)
            .map(|d| Event::Download(d.info()));
        dialogs.chain(downloads).collect()
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

    /// Tab `id`'s frames, parents before their children, each with the
    /// session that owns it (an OOPIF has its own).
    pub(crate) fn frame_sources(&self, id: u32) -> Vec<FrameSource> {
        let state = self.shared.lock();
        let Some(tab) = state.tabs.get(&id) else {
            return Vec::new();
        };
        tab.frames
            .list()
            .into_iter()
            .filter_map(|info| {
                let owner = tab.frames.owner(&info.id)?;
                let session = state.hosts.get(owner)?.session.clone();
                Some(FrameSource { info, session })
            })
            .collect()
    }

    /// Runs `f` on tab `id`'s snapshot memory (refs and last snapshot);
    /// `None` if the tab is gone.
    pub(crate) fn snapshot_memory<R>(
        &self,
        id: u32,
        f: impl FnOnce(&mut SnapshotMemory) -> R,
    ) -> Option<R> {
        self.shared
            .lock()
            .tabs
            .get_mut(&id)
            .map(|tab| f(&mut tab.snapshot))
    }

    /// Calls `f` with tab `id`'s mouse state (`None` if the tab is gone).
    pub(crate) fn pointer<R>(&self, id: u32, f: impl FnOnce(&mut Pointer) -> R) -> Option<R> {
        self.shared
            .lock()
            .tabs
            .get_mut(&id)
            .map(|tab| f(&mut tab.pointer))
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
                injected::load(&session.with_timeout(timeout), context_id).await?;
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

    /// Re-runs `check` on every change to the tabs (frames, requests, …)
    /// and at the instant it asks for, until it gives a value, or `None`
    /// once `deadline` passes. The event-driven half of the wait engine
    /// (see [`crate::wait`]).
    async fn watch<T>(
        &self,
        deadline: Instant,
        mut check: impl FnMut(&State, Instant) -> Check<T>,
    ) -> Option<T> {
        let mut changed = self.shared.changed.subscribe();
        loop {
            let now = Instant::now();
            let later = match check(&self.shared.lock(), now) {
                Check::Done(value) => return Some(value),
                Check::Later(at) => at,
            };
            if now >= deadline {
                return None;
            }
            let wake = later.map_or(deadline, |at| at.min(deadline));
            tokio::select! {
                result = changed.changed() => {
                    if result.is_err() {
                        return None;
                    }
                }
                () = tokio::time::sleep_until(wake) => {}
            }
        }
    }

    /// Marks the start of a page command on tab `id` in its network log
    /// and the event sequence.
    pub(crate) fn begin_command(&self, id: u32) {
        let mut state = self.shared.lock();
        let seq = state.seq;
        if let Some(tab) = state.tabs.get_mut(&id) {
            tab.command_marks = (tab.command_marks.1, tab.network.seq());
            tab.event_marks = (tab.event_marks.1, seq);
        }
    }

    /// The event mark when the page command before the current one on tab
    /// `id` began: `wait --download` also accepts a download that command
    /// started.
    pub(crate) fn previous_event_mark(&self, id: u32) -> u64 {
        self.shared
            .lock()
            .tabs
            .get(&id)
            .map_or(0, |t| t.event_marks.0)
    }

    /// The network log's mark when the page command before the current
    /// one on tab `id` began: `wait --request/--response` also accept what
    /// that command (a click, a `goto`) caused before `wait` ran.
    pub(crate) fn previous_command_mark(&self, id: u32) -> u64 {
        self.shared
            .lock()
            .tabs
            .get(&id)
            .map_or(0, |t| t.command_marks.0)
    }

    /// Waits until tab `id`'s network is idle since `since` (see
    /// [`Network::idle`]). On timeout, returns the requests still in
    /// flight.
    pub(crate) async fn network_idle(
        &self,
        id: u32,
        since: Instant,
        deadline: Instant,
    ) -> Result<(), Vec<InFlight>> {
        let idle = self
            .watch(deadline, |state, now| {
                let Some(tab) = state.tabs.get(&id) else {
                    return Check::Done(());
                };
                match tab.network.idle(since, now) {
                    Idle::Now => Check::Done(()),
                    Idle::At(at) | Idle::Busy(at) => Check::Later(Some(at)),
                }
            })
            .await;
        match idle {
            Some(()) => Ok(()),
            None => Err(self.busy(id)),
        }
    }

    /// Waits until the requests tab `id` started since `since` are done
    /// and its network was quiet for `quiet` (see [`Network::idle_for`]);
    /// `false` if `deadline` passed first.
    pub(crate) async fn own_requests_done(
        &self,
        id: u32,
        since: Instant,
        quiet: Duration,
        deadline: Instant,
    ) -> bool {
        self.watch(deadline, |state, now| {
            let Some(tab) = state.tabs.get(&id) else {
                return Check::Done(());
            };
            match tab.network.idle_for(since, now, quiet, true) {
                Idle::Now => Check::Done(()),
                Idle::At(at) | Idle::Busy(at) => Check::Later(Some(at)),
            }
        })
        .await
        .is_some()
    }

    /// Tab `id`'s network log and console marks now, for
    /// [`Tabs::failures_since`] and [`Tabs::console_errors_since`].
    pub(crate) fn log_marks(&self, id: u32) -> (u64, u64) {
        self.shared
            .lock()
            .tabs
            .get(&id)
            .map_or((0, 0), |t| (t.network.seq(), t.console.seq()))
    }

    /// Tab `id`'s failed requests since network mark `after`.
    pub(crate) fn failures_since(&self, id: u32, after: u64) -> Vec<Failure> {
        self.shared
            .lock()
            .tabs
            .get(&id)
            .map(|t| t.network.failures_since(after))
            .unwrap_or_default()
    }

    /// Tab `id`'s console errors since console mark `after`.
    pub(crate) fn console_errors_since(&self, id: u32, after: u64) -> Vec<ConsoleError> {
        self.shared
            .lock()
            .tabs
            .get(&id)
            .map(|t| t.console.errors_since(after))
            .unwrap_or_default()
    }

    /// Calls `f` with tab `id`'s console (`None` if the tab is gone).
    pub(crate) fn console<R>(&self, id: u32, f: impl FnOnce(&mut Console) -> R) -> Option<R> {
        self.shared
            .lock()
            .tabs
            .get_mut(&id)
            .map(|tab| f(&mut tab.console))
    }

    /// Waits until tab `id`'s console has an entry after mark `after` (or
    /// the tab is gone); `false` if `deadline` passed first.
    pub(crate) async fn console_logged(&self, id: u32, after: u64, deadline: Instant) -> bool {
        self.watch(deadline, |state, _| match state.tabs.get(&id) {
            Some(tab) if tab.console.seq() <= after => Check::Later(None),
            _ => Check::Done(()),
        })
        .await
        .is_some()
    }

    /// Calls `f` with tab `id`'s network log (`None` if the tab is gone).
    pub(crate) fn netlog<R>(&self, id: u32, f: impl FnOnce(&mut NetLog) -> R) -> Option<R> {
        self.shared
            .lock()
            .tabs
            .get_mut(&id)
            .map(|tab| f(&mut tab.netlog))
    }

    /// The page or iframe session with id `session_id`, if it is still
    /// attached.
    pub(crate) fn host_session(&self, session_id: &str) -> Option<Session> {
        self.shared
            .lock()
            .hosts
            .get(session_id)
            .map(|h| h.session.clone())
    }

    /// The requests that keep tab `id`'s network busy now.
    pub(crate) fn busy(&self, id: u32) -> Vec<InFlight> {
        self.shared
            .lock()
            .tabs
            .get(&id)
            .map(|t| t.network.busy(Instant::now()))
            .unwrap_or_default()
    }

    /// Waits for a request (or, with `response`, a response) of tab `id`
    /// logged after mark `after` that `matches`.
    pub(crate) async fn exchange(
        &self,
        id: u32,
        response: bool,
        after: u64,
        matches: impl Fn(&Exchange) -> bool,
        deadline: Instant,
    ) -> Option<Exchange> {
        self.watch(deadline, |state, _| {
            let found = state.tabs.get(&id).and_then(|tab| {
                if response {
                    tab.network.response_since(after, &matches)
                } else {
                    tab.network.request_since(after, &matches)
                }
            });
            found.map_or(Check::Later(None), Check::Done)
        })
        .await
    }

    /// Waits until tab `id`'s main frame URL `matches`, and returns it;
    /// on timeout, the URL it has.
    pub(crate) async fn main_url(
        &self,
        id: u32,
        matches: impl Fn(&str) -> bool,
        deadline: Instant,
    ) -> Result<String, String> {
        let mut last = String::new();
        let found = self
            .watch(deadline, |state, _| {
                let Some(tab) = state.tabs.get(&id) else {
                    return Check::Later(None);
                };
                let url = tab.frames.main_url();
                if matches(url) {
                    return Check::Done(url.to_owned());
                }
                url.clone_into(&mut last);
                Check::Later(None)
            })
            .await;
        found.ok_or(last)
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

/// What a [`Tabs::watch`] check found.
#[derive(Debug)]
enum Check<T> {
    Done(T),
    /// Not yet: look again on the next change, or at this instant.
    Later(Option<Instant>),
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
            crashed: tab.crashed.is_some(),
            dialog: tab.dialog.as_ref().map(|d| d.info.clone()),
        }
    }

    /// Logs `event` (a dialog answered without a command) for the command
    /// that runs meanwhile to report.
    fn log(&mut self, event: Event) {
        self.seq += 1;
        self.logged.push_back((self.seq, event));
        if self.logged.len() > OPENED_MEMORY {
            self.logged.pop_front();
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
            browser::DownloadWillBegin::NAME => downloads::will_begin(&shared, &event),
            browser::DownloadProgress::NAME => downloads::progress(&shared, &event),
            _ => {}
        }
    }
}

/// Handles a page or iframe session's events: its own auto-attached
/// targets (OOPIFs, workers) and their detaching, the frame and execution
/// context events of tab `tab`'s frames it owns, its dialogs, its crash,
/// and its intercepted requests.
async fn watch_host(
    shared: Arc<Shared>,
    mut events: RawEventStream,
    session: Session,
    tab: Option<u32>,
) {
    let session_id = session.id().unwrap_or_default().to_owned();
    let mut auth = Auth::default();
    while let Some(event) = events.next().await {
        match event.method.as_str() {
            target::AttachedToTarget::NAME => attached(&shared, &event, Parent::Host(tab)),
            target::DetachedFromTarget::NAME => {
                if let Some(session_id) = event.params["sessionId"].as_str() {
                    detached(&shared, session_id);
                }
            }
            "Fetch.requestPaused" => Auth::request_paused(&shared, &session, &event),
            "Fetch.authRequired" => auth.auth_required(&shared, &session, &event),
            method if method.starts_with("Network.") => {
                if let Some(tab) = tab {
                    network_event(&shared, &session_id, tab, method, &event);
                }
            }
            "Runtime.consoleAPICalled"
            | "Runtime.exceptionThrown"
            | "Runtime.exceptionRevoked"
            | "Log.entryAdded" => {
                if let Some(tab) = tab {
                    console_event(&shared, tab, &event, None);
                }
            }
            page::JavascriptDialogOpening::NAME => {
                if let Some(tab) = tab {
                    dialogs::opening(&shared, &session, tab, &event);
                }
            }
            page::JavascriptDialogClosed::NAME => {
                if let Some(tab) = tab {
                    dialogs::closed(&shared, tab, &event);
                }
            }
            "Inspector.targetCrashed" => {
                if let Some(tab) = tab {
                    crashed(&shared, &session_id, tab);
                }
            }
            "Inspector.targetReloadedAfterCrash" => {
                if let Some(tab) = tab {
                    recovered(&shared, &session_id, tab);
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

/// The page of tab `tab` crashed, if `session_id` is its own session (an
/// out-of-process iframe's crash leaves the page running): its scripts,
/// dialog and requests are gone.
fn crashed(shared: &Shared, session_id: &str, tab: u32) {
    let mut state = shared.lock();
    state.seq += 1;
    let seq = state.seq;
    let Some(tab) = state.tabs.get_mut(&tab) else {
        return;
    };
    if tab.session.id() != Some(session_id) {
        return;
    }
    tracing::warn!("the page of tab {} crashed", tab.target_id);
    tab.crashed = Some(seq);
    tab.dialog = None;
    tab.frames.contexts_cleared(session_id);
    tab.network.session_gone(session_id);
    drop(state);
    shared.bump();
}

/// Tab `tab`'s page loads again after a crash.
fn recovered(shared: &Shared, session_id: &str, tab: u32) {
    let mut state = shared.lock();
    if let Some(tab) = state.tabs.get_mut(&tab)
        && tab.session.id() == Some(session_id)
    {
        tab.crashed = None;
    }
    drop(state);
    shared.bump();
}

/// Applies a frame or execution context event of `session_id` to tab
/// `tab`'s frame tree.
fn frame_event(shared: &Shared, session_id: &str, tab: u32, method: &str, event: &RawEvent) {
    let mut state = shared.lock();
    let Some(tab) = state.tabs.get_mut(&tab) else {
        return;
    };
    let frames = &mut tab.frames;
    let decoded = match method {
        page::FrameAttached::NAME => decode::<page::FrameAttached>(event)
            .map(|e| frames.attached(session_id, &e.frame_id, &e.parent_frame_id)),
        page::FrameNavigated::NAME => decode::<page::FrameNavigated>(event).map(|e| {
            let main = e.frame.parent_id.is_none();
            tab.network
                .navigated(session_id, &e.frame.id, &e.frame.loader_id, main);
            if main && session_id == tab.session.id().unwrap_or_default() {
                // A new document: the page runs again after a crash.
                tab.crashed = None;
            }
            let gone = if e.r#type == page::NavigationType::BackForwardCacheRestore {
                frames.restored(session_id, &e.frame)
            } else {
                frames.navigated(session_id, &e.frame)
            };
            if !gone.is_empty() {
                let main = gone.iter().any(|id| id == frames.root());
                tab.snapshot.navigated(&gone, main);
            }
        }),
        page::NavigatedWithinDocument::NAME => {
            decode::<page::NavigatedWithinDocument>(event).map(|e| {
                frames.navigated_within_document(&e.frame_id, &e.url);
                tab.snapshot.refs.route_changed(&e.frame_id);
            })
        }
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

/// Applies a `Network` event of `session_id` to tab `tab`'s requests and
/// network log.
fn network_event(shared: &Shared, session_id: &str, tab: u32, method: &str, event: &RawEvent) {
    let capacity = shared.settings.network_buffer();
    let followed = shared.lock().tabs.get_mut(&tab).is_some_and(|t| {
        let followed = t.network.event(session_id, method, &event.params);
        let logged = t.netlog.event(session_id, method, &event.params, capacity);
        followed || logged
    });
    if followed {
        shared.bump();
    }
}

/// Applies a console call, uncaught exception or browser message (of the
/// dedicated worker whose script is `worker`, if given) to tab `tab`'s
/// console.
fn console_event(shared: &Shared, tab: u32, event: &RawEvent, worker: Option<&str>) {
    let capacity = shared.settings.console_buffer();
    let logged = shared.lock().tabs.get_mut(&tab).is_some_and(|t| {
        t.console
            .event(&event.method, &event.params, worker, capacity)
    });
    if logged {
        shared.bump();
    }
}

/// Handles a dedicated worker's events: its console calls, uncaught
/// exceptions and browser messages go to tab `tab`'s console.
async fn watch_worker(shared: Arc<Shared>, mut events: RawEventStream, tab: u32, url: String) {
    while let Some(event) = events.next().await {
        if matches!(
            event.method.as_str(),
            "Runtime.consoleAPICalled"
                | "Runtime.exceptionThrown"
                | "Runtime.exceptionRevoked"
                | "Log.entryAdded"
        ) {
            console_event(&shared, tab, &event, Some(&url));
        }
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
                network: Network::default(),
                netlog: NetLog::default(),
                console: Console::default(),
                command_marks: (0, 0),
                event_marks: (0, 0),
                dialog: None,
                crashed: None,
                snapshot: SnapshotMemory::default(),
                pointer: Pointer::default(),
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
        shared.spawn(watch_host(shared.clone(), events, session.clone(), tab));
    }
    // A dedicated worker of a tab: its console goes to the tab's.
    let worker = info.r#type == "worker" && matches!(parent, Parent::Host(Some(_)));
    if worker && let Some(tab) = tab {
        let events = session.events();
        shared.spawn(watch_worker(shared.clone(), events, tab, info.url.clone()));
    }
    let kind = if host {
        Kind::Host
    } else if worker {
        Kind::Worker
    } else {
        Kind::Other
    };
    let waiting = attached.waiting_for_debugger;
    let task_shared = shared.clone();
    shared.spawn(async move {
        let session = session.with_timeout(SETUP_TIMEOUT);
        setup(&task_shared, &session, kind, tab, waiting).await;
        if let Some(id) = new_tab {
            ready(&task_shared, id);
        }
    });
}

/// How much of the page's traffic Chrome keeps for `network show`: the
/// bodies of its responses (evicted oldest first past the total), and
/// request bodies up to 64KB in the request events.
fn network_enable() -> network::EnableParams {
    network::EnableParams {
        max_total_buffer_size: Some(BODIES),
        max_resource_buffer_size: Some(BODY),
        max_post_data_size: Some(64 * 1024),
        ..network::EnableParams::default()
    }
}

/// Keeps response bodies (up to [`BODIES`] a session, [`BODY`] each)
/// outside the renderer, where a navigation does not drop them.
fn durable_messages() -> network::ConfigureDurableMessagesParams {
    network::ConfigureDurableMessagesParams {
        max_total_buffer_size: Some(BODIES),
        max_resource_buffer_size: Some(BODY),
    }
}

/// How many bytes of response bodies Chrome keeps for a page or iframe
/// session, and for one response.
const BODIES: i64 = 64 * 1024 * 1024;
const BODY: i64 = 10 * 1024 * 1024;

/// What a target is to [`setup`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A page or iframe session.
    Host,
    /// A tab's dedicated worker.
    Worker,
    Other,
}

/// Prepares a new target before it runs: auto-attach for its children,
/// the domains commands need, frame tracking and chrome-cdp's isolated
/// world, the session's settings; then resumes it. `kind` is
/// [`Kind::Host`] for page and iframe sessions, with `tab` the tab whose
/// frames they have; a tab's dedicated worker gets only its console.
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
    kind: Kind,
    tab: Option<u32>,
    waiting: bool,
) {
    let mut steps: Vec<(&str, Step)> = Vec::new();
    if kind == Kind::Worker {
        // Console calls and exceptions, and the browser's messages.
        steps.push((
            "Runtime.enable",
            step(session.call(runtime::EnableParams::default())),
        ));
        steps.push((
            "Log.enable",
            step(session.call(cdp::domains::log::EnableParams::default())),
        ));
    }
    if kind == Kind::Host {
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
        // Reports every execution context, the existing ones too, and
        // console calls and exceptions (for the console).
        steps.push((
            "Runtime.enable",
            step(session.call(runtime::EnableParams::default())),
        ));
        // The browser's messages: failed loads, CSP violations, …
        steps.push((
            "Log.enable",
            step(session.call(cdp::domains::log::EnableParams::default())),
        ));
        // Response bodies kept in the browser process, so `network show
        // --body` still has them after the page navigates.
        steps.push((
            "Network.configureDurableMessages",
            step(session.call(durable_messages())),
        ));
        // Requests, for network idle, `wait --request/--response` and
        // the network log.
        steps.push(("Network.enable", step(session.call(network_enable()))));
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
        // chrome-cdp's isolated world, with its helper bundle, in every
        // document from now on, and in the ones that exist already.
        let mut world =
            page::AddScriptToEvaluateOnNewDocumentParams::new(injected::BUNDLE.to_owned());
        world.world_name = Some(frames::WORLD_NAME.to_owned());
        world.run_immediately = Some(true);
        steps.push(("the isolated world", step(session.call(world))));
        for source in shared.settings.init_scripts() {
            let add = page::AddScriptToEvaluateOnNewDocumentParams::new(source);
            steps.push(("an init script", step(session.call(add))));
        }
        // Answers HTTP authentication challenges (see `auth`).
        if shared.settings.http_credentials().is_some() {
            steps.push(("Fetch.enable", step(session.call(auth::enable()))));
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
        tab.network.session_gone(session_id);
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
