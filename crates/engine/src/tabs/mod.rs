//! The target & tab manager (plan 03, milestone 3.1).
//!
//! [`Tabs`] auto-attaches to every target of the browser: at the browser
//! level (tabs, popups, service and shared workers) and again on each page
//! and out-of-process iframe session (their OOPIFs and dedicated workers),
//! always with `waitForDebuggerOnStart`. A new target is paused before it
//! runs any script; [`setup`] applies the session's settings to it (init
//! scripts, routes and emulation), turns on the events
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
//!
//! Plan 07 adds request routing: while the session has routes, every page
//! and iframe session intercepts its requests (see [`fetch`]); and
//! emulation, which every page and iframe session gets (see
//! [`emulation`]), and so do init scripts and exposed functions (see
//! [`inject`]).
//!
//! Plan 13 keeps every attached target, of every kind, in one map (see
//! [`targets`]), and what each kind gets in one registry of features
//! (see [`features`]): [`setup`] and every change of a setting go through
//! it.

mod auth;
pub(crate) mod commands;
pub(crate) mod coverage;
pub(crate) mod dialogs;
mod downloads;
pub(crate) mod emulation;
mod features;
mod fetch;
mod inject;
mod locks;
mod mock_shim;
pub(crate) mod tap;
mod targets;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use cdp::domains::{browser, page, runtime, security, target};
use cdp::{Connection, Event as _, LosslessEventStream, RawEvent, Session};
use futures_util::future::join_all;
use protocol::schema::{Event, HeldKeys, TabInfo, Viewport, Worker, WorkerInfo, WorkerKind};

use self::dialogs::OpenDialog;
use self::downloads::Download;
use self::features::{Feature, NewTarget, REGISTRY};
use self::targets::{Owner, Target, TargetKind};
use crate::emulate::{Aspects, Emulator};
use crate::frames::{self, Context, FindError, FrameInfo, FrameTree, World};
use crate::inject::{Injector, MOCK_BINDING, SEED_BINDING};
use crate::injected;
use crate::input::{Key, Pointer};
use crate::network::{Exchange, Failure, Idle, InFlight, Network};
use crate::observe::console::{self, Console, ConsoleError, FromWorker};
use crate::observe::netlog::{self, NetLog};
use crate::request::{DialogPolicy, HttpCredentials};
use crate::route::Router;
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

/// How often [`Tabs::context`] looks again for a frame's new document
/// when the one it was making a world in went away.
const GONE_POLL: Duration = Duration::from_millis(10);

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
    downloads_dir: Mutex<Option<PathBuf>>,
    /// When the downloads list starts (ms since the epoch), if a daemon
    /// an upgrade replaced had one before.
    downloads_since: Mutex<Option<u64>>,
    /// How many console entries each tab keeps (`open --console-buffer`).
    console_buffer: AtomicUsize,
    /// How many requests each tab's network log keeps (`open
    /// --network-buffer`).
    network_buffer: AtomicUsize,
    /// Whether requests skip service workers without routes too (`open
    /// --bypass-service-worker`).
    bypass_service_worker: AtomicBool,
    /// The routing rules (plan 07).
    pub(crate) routes: Router,
    /// What pages emulate, and the permissions (plan 07).
    pub(crate) emulation: Emulator,
    /// Init scripts and exposed functions (plan 07).
    pub(crate) injections: Injector,
    /// The browser contexts made with `context new` (plan 11).
    pub(crate) contexts: crate::context::Contexts,
}

impl Settings {
    /// Settings with downloads going to `downloads_dir`.
    pub(crate) fn new(downloads_dir: Option<PathBuf>) -> Self {
        Self {
            downloads_dir: Mutex::new(downloads_dir),
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

    /// Where downloads go (`None`: Chrome's own behaviour).
    pub(crate) fn downloads_dir(&self) -> Option<PathBuf> {
        lock(&self.downloads_dir).clone()
    }

    pub(crate) fn set_downloads_dir(&self, dir: PathBuf) {
        *lock(&self.downloads_dir) = Some(dir);
    }

    /// When the downloads list starts, if a replaced daemon had one.
    pub(crate) fn downloads_since(&self) -> Option<u64> {
        *lock(&self.downloads_since)
    }

    /// The downloads list starts at `ms` (the first call counts).
    pub(crate) fn set_downloads_since(&self, ms: u64) {
        lock(&self.downloads_since).get_or_insert(ms);
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

    pub(crate) fn http_credentials(&self) -> Option<HttpCredentials> {
        lock(&self.http_credentials).clone()
    }

    pub(crate) fn set_bypass_service_worker(&self, bypass: bool) {
        self.bypass_service_worker.store(bypass, Ordering::Relaxed);
    }

    fn bypass_service_worker(&self) -> bool {
        self.bypass_service_worker.load(Ordering::Relaxed)
    }

    pub(crate) fn set_follow_popups(&self, follow: bool) {
        self.follow_popups.store(follow, Ordering::Relaxed);
    }

    pub(crate) fn set_ignore_https_errors(&self, ignore: bool) {
        self.ignore_https_errors.store(ignore, Ordering::Relaxed);
    }

    pub(crate) fn ignores_https_errors(&self) -> bool {
        self.ignore_https_errors.load(Ordering::Relaxed)
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
    /// The viewport of pages without their own (headless mode).
    default_viewport: Option<Viewport>,
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
    /// The active tab of each context of ours, by Chrome's context id.
    context_active: HashMap<String, u32>,
    /// Every attached target, by session id.
    targets: HashMap<String, Target>,
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
    /// The origins each scope's frames loaded a document of (Chrome's id
    /// of a context of ours, `None` for the others), first visit first,
    /// for `state save`.
    visited: HashMap<Option<String>, Vec<String>>,
    /// Console messages and requests of shared and service workers no
    /// tab had a frame on the origin of (`status`).
    dropped: u64,
    /// The taps of `cdp --listen --all-sessions` (see [`tap`]), and the
    /// last id given to one.
    hooks: Vec<tap::Hook>,
    next_hook: u64,
    /// The running `coverage start`, if any (see [`coverage`]).
    coverage: Option<coverage::Measuring>,
}

#[derive(Debug)]
struct Tab {
    target_id: String,
    session: Session,
    url: String,
    title: String,
    opener_target: Option<String>,
    /// Chrome's id of its browser context, when it is one of ours
    /// (`context new`), with our id for it (`c1`).
    context: Option<(String, String)>,
    /// Set up and resumed; commands may use it.
    ready: bool,
    /// Its frames, also those of its OOPIF sessions.
    frames: FrameTree,
    /// Its requests, also those of its OOPIF sessions.
    network: Network,
    /// Its console, also that of its OOPIF sessions and workers (see
    /// `watch_worker`).
    console: Console,
    /// Its network log, also that of its OOPIF sessions.
    netlog: NetLog,
    /// The network log's marks when the page command before the current
    /// one, and the current one, began.
    command_marks: (u64, u64),
    /// The same for the event sequence (downloads).
    event_marks: (u64, u64),
    /// The network and console marks when the main frame's document
    /// committed: where `expect --no-errors` starts by default.
    document_marks: (u64, u64),
    /// `mark <name>`: named log marks.
    marks: HashMap<String, Marks>,
    /// A dialog waiting for `chrome-cdp dialog`.
    dialog: Option<OpenDialog>,
    /// When its page crashed (in the event sequence), until it is loaded
    /// again.
    crashed: Option<u64>,
    /// Its refs and last snapshot.
    snapshot: SnapshotMemory,
    /// Where the mouse is and what it holds, as chrome-cdp sent it.
    pointer: Pointer,
    /// The keys `keyboard down` holds, in the order they went down. They
    /// stay across navigations and go with the tab.
    keys: Vec<Key>,
}

impl Tab {
    /// A tab for page `info`, not set up yet.
    fn new(
        info: &target::TargetInfo,
        session: &Session,
        session_id: &str,
        context: Option<(String, String)>,
    ) -> Self {
        Self {
            target_id: info.target_id.clone(),
            session: session.clone(),
            url: info.url.clone(),
            title: info.title.clone(),
            opener_target: info.opener_id.clone(),
            context,
            ready: false,
            frames: FrameTree::new(&info.target_id, session_id),
            network: Network::default(),
            netlog: NetLog::default(),
            console: Console::default(),
            command_marks: (0, 0),
            document_marks: (0, 0),
            marks: HashMap::new(),
            event_marks: (0, 0),
            dialog: None,
            crashed: None,
            snapshot: SnapshotMemory::default(),
            pointer: Pointer::default(),
            keys: Vec::new(),
        }
    }
}

/// Where a tab's logs were at a `mark`: the network tracker's
/// sequence ([`Tabs::failures_since`]), the console's
/// ([`Tabs::console_errors_since`], `console --since`) and the network
/// log's last request id (`network --since`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Marks {
    pub(crate) network: u64,
    pub(crate) console: u64,
    pub(crate) netlog: u64,
    /// When it was set (ms since the Unix epoch), for the other tabs'
    /// requests (`expect request --since`).
    pub(crate) time_ms: u64,
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
    /// Starts watching `conn`'s targets. Call [`Tabs::init`] next. Pages
    /// without a viewport of their own get `default_viewport`.
    pub(crate) fn start(
        conn: &Connection,
        settings: Arc<Settings>,
        default_viewport: Option<Viewport>,
    ) -> Self {
        let shared = Arc::new(Shared {
            conn: conn.clone(),
            settings,
            state: Mutex::new(State::new()),
            changed: watch::Sender::new(0),
            security_applied: AtomicBool::new(false),
            default_viewport,
            tasks: Mutex::default(),
        });
        // Subscribe before anything can attach. Lossless, as a missed
        // target event loses a tab.
        let events = conn.browser().events_lossless("");
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
        let _ = self.manage_downloads(timeout).await;
        emulation::apply_permissions(&self.shared, timeout).await;
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
    /// with events. A browser that refuses keeps its own behaviour; the
    /// error says why.
    pub(crate) async fn manage_downloads(&self, timeout: Duration) -> Result<(), String> {
        let Some(dir) = self.shared.settings.downloads_dir() else {
            return Ok(());
        };
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        if let Err(e) = builder.create(&dir) {
            let why = format!("cannot create {}: {e}", dir.display());
            tracing::warn!("{why}");
            return Err(why);
        }
        let mut params = browser::SetDownloadBehaviorParams::new(
            browser::SetDownloadBehaviorBehavior::AllowAndName,
        );
        params.download_path = Some(dir.display().to_string());
        params.events_enabled = Some(true);
        let result = self.conn.browser().with_timeout(timeout).call(params).await;
        if let Err(e) = result {
            let why = format!("cannot send downloads to {}: {e}", dir.display());
            tracing::warn!("{why}");
            return Err(why);
        }
        Ok(())
    }

    /// Gives a browser we leave running (`close` of a connected session)
    /// its own download behaviour back.
    pub(crate) async fn release(&self, timeout: Duration) {
        if self.shared.settings.downloads_dir().is_none() || self.conn.is_closed() {
            return;
        }
        let params =
            browser::SetDownloadBehaviorParams::new(browser::SetDownloadBehaviorBehavior::Default);
        let result = self.conn.browser().with_timeout(timeout).call(params).await;
        if let Err(e) = result {
            tracing::debug!("cannot reset the download behaviour: {e}");
        }
    }

    /// Brings request interception in every page and frame session in
    /// line with the session's routes, HTTP credentials and
    /// `--bypass-service-worker` (new sessions get it during [`setup`]).
    /// Only sessions whose interception changes get calls, unless `force`
    /// (the credentials may be new).
    pub(crate) async fn apply_interception(&self, timeout: Duration, force: bool) {
        fetch::apply(&self.shared, timeout, force).await;
    }

    /// The value of `expression` in each worker of `scope` (that has
    /// one: a worker that closed meanwhile has none).
    pub(crate) async fn in_workers(
        &self,
        scope: Option<&str>,
        expression: &str,
        timeout: Duration,
    ) -> Vec<(Worker, serde_json::Value)> {
        let workers: Vec<(Worker, Session)> = self
            .shared
            .lock()
            .targets
            .values()
            .filter(|t| t.scope.as_deref() == scope)
            .filter_map(|t| Some((t.worker()?, t.session.with_timeout(timeout))))
            .collect();
        let values = workers.into_iter().map(|(worker, session)| async move {
            let mut params = runtime::EvaluateParams::new(expression.to_owned());
            params.return_by_value = Some(true);
            let value = session.call(params).await.ok()?.result.value?;
            Some((worker, value))
        });
        join_all(values).await.into_iter().flatten().collect()
    }

    /// `expression`'s value in every page, iframe and worker session of
    /// `scope` emulation reaches (a session's main frame only); those
    /// that answer.
    pub(crate) async fn in_emulated(
        &self,
        scope: Option<&str>,
        expression: &str,
        timeout: Duration,
    ) -> Vec<serde_json::Value> {
        let sessions: Vec<Session> = self
            .shared
            .lock()
            .applies(Feature::Emulation)
            .filter(|(_, t)| t.scope.as_deref() == scope)
            .map(|(_, t)| t.session.with_timeout(timeout))
            .collect();
        let values = sessions.into_iter().map(|session| async move {
            let mut params = runtime::EvaluateParams::new(expression.to_owned());
            params.return_by_value = Some(true);
            session.call(params).await.ok()?.result.value
        });
        join_all(values).await.into_iter().flatten().collect()
    }

    /// Sends a change of the session's emulation (`aspects`) to every page,
    /// iframe and worker session (new ones get it during [`setup`]).
    /// Returns the errors Chrome gave.
    pub(crate) async fn apply_emulation(
        &self,
        aspects: Aspects,
        scope: Option<&str>,
        timeout: Duration,
    ) -> Vec<String> {
        emulation::apply(&self.shared, aspects, scope, timeout).await
    }

    /// Makes `page`'s viewport `size` CSS pixels until
    /// [`Tabs::restore_viewport`]; `dpr` if it emulates no viewport.
    pub(crate) async fn stretch_viewport(
        &self,
        page: &Page,
        size: (u32, u32),
        dpr: f64,
        timeout: Duration,
    ) -> cdp::Result<()> {
        let session = page.session.with_timeout(timeout);
        emulation::stretch_viewport(&self.shared, &session, size, dpr).await
    }

    /// Gives `page` back the viewport it emulates (or none).
    pub(crate) async fn restore_viewport(&self, page: &Page, timeout: Duration) -> cdp::Result<()> {
        let session = page.session.with_timeout(timeout);
        emulation::restore_viewport(&self.shared, &session).await
    }

    /// The viewport of pages without their own, if there is one.
    pub(crate) fn default_viewport(&self) -> Option<Viewport> {
        self.shared.default_viewport.clone()
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
        self.create_in(url, None, timeout).await
    }

    /// Opens a tab at `url` in Chrome's context `browser_context` (the
    /// default one with `None`), and waits until it is set up. It does
    /// not become active.
    pub(crate) async fn create_in(
        &self,
        url: &str,
        browser_context: Option<&str>,
        timeout: Duration,
    ) -> Result<Page, TabError> {
        let mut params = target::CreateTargetParams::new(url.to_owned());
        params.browser_context_id = browser_context.map(str::to_owned);
        let created = self
            .conn
            .browser()
            .with_timeout(timeout)
            .call(params)
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

    /// The tab `id`, which must be in Chrome's context `browser_id` (of
    /// ours), or the context's active one. Without any tab in the
    /// context, opens a blank one there.
    pub(crate) async fn page_in(
        &self,
        browser_id: &str,
        id: Option<u32>,
        timeout: Duration,
    ) -> Result<Page, TabError> {
        if let Some(id) = id {
            let page = self.page(Some(id), timeout).await?;
            if !self.shared.lock().in_scope(id, Some(browser_id)) {
                return Err(TabError::OtherContext(id));
            }
            return Ok(page);
        }
        {
            let mut state = self.shared.lock();
            if let Some(id) = state.current_in(browser_id) {
                state.activate_in(browser_id, id);
                if let Some(page) = state.page(id) {
                    return Ok(page);
                }
            }
        }
        let page = self.create_in(BLANK, Some(browser_id), timeout).await?;
        self.shared.lock().activate_in(browser_id, page.id);
        Ok(page)
    }

    /// Makes `id` the active tab of Chrome's context `browser_id` (of
    /// ours), leaving the session's active tab alone.
    pub(crate) fn activate_in(&self, browser_id: &str, id: u32) {
        self.shared.lock().activate_in(browser_id, id);
        self.shared.bump();
    }

    /// The active tab of Chrome's context `browser_id` (of ours).
    pub(crate) fn active_in(&self, browser_id: &str) -> Option<u32> {
        self.shared.lock().current_in(browser_id)
    }

    /// The ready tabs of Chrome's context `browser_id` (of ours), by id.
    pub(crate) fn in_context(&self, browser_id: &str) -> Vec<u32> {
        let state = self.shared.lock();
        state
            .tabs
            .iter()
            .filter(|&(&id, t)| t.ready && state.in_scope(id, Some(browser_id)))
            .map(|(&id, _)| id)
            .collect()
    }

    /// The ready tabs of `scope` (Chrome's id of a context of ours, or
    /// `None` for the others), by id.
    pub(crate) fn tab_ids_in(&self, scope: Option<&str>) -> Vec<u32> {
        let state = self.shared.lock();
        state
            .tabs
            .iter()
            .filter(|&(&id, t)| t.ready && state.in_scope(id, scope))
            .map(|(&id, _)| id)
            .collect()
    }

    /// Forgets the tabs of Chrome's context `browser_id` (disposed of).
    pub(crate) fn drop_context_tabs(&self, browser_id: &str) {
        let mut state = self.shared.lock();
        let ids: Vec<u32> = state
            .tabs
            .keys()
            .copied()
            .filter(|&id| state.in_scope(id, Some(browser_id)))
            .collect();
        for id in ids {
            state.remove(id);
        }
        state.context_active.remove(browser_id);
        state.visited.remove(&Some(browser_id.to_owned()));
        drop(state);
        self.shared.bump();
    }

    /// The origins frames of `scope` (Chrome's id of a context of ours,
    /// or `None` for the others) loaded a document of, in tabs open or
    /// closed, first visit first.
    pub(crate) fn visited_in(&self, scope: Option<&str>) -> Vec<String> {
        self.shared
            .lock()
            .visited
            .get(&scope.map(str::to_owned))
            .cloned()
            .unwrap_or_default()
    }

    /// Makes `id` the active tab and brings it to the front.
    pub(crate) async fn activate(&self, id: u32, timeout: Duration) -> Result<Page, TabError> {
        let page = self.page(Some(id), timeout).await?;
        self.shared.lock().activate(id);
        self.shared.bump();
        self.bring_to_front(id, timeout).await?;
        Ok(page)
    }

    /// Brings tab `id` to the front of its window, so its page is visible
    /// (`document.visibilityState`) and not throttled. `Page.bringToFront`
    /// and not `Target.activateTarget`, which leaves a tab of a context of
    /// ours hidden in headless Chrome.
    pub(crate) async fn bring_to_front(&self, id: u32, timeout: Duration) -> Result<(), TabError> {
        let page = self.page(Some(id), timeout).await?;
        page.session
            .with_timeout(timeout)
            .call(page::BringToFrontParams::default())
            .await?;
        Ok(())
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

    /// Waits up to `timeout` until no popup is still being set up, so a
    /// command that opened one reports it (and the active tab is back in
    /// front, see [`front_after_popup`]).
    pub(crate) async fn wait_for_popups(&self, timeout: Duration) {
        self.wait(timeout, |state| {
            state
                .tabs
                .values()
                .all(|t| t.ready || t.opener_target.is_none())
        })
        .await;
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

    /// The ids of the ready tabs, in order.
    pub(crate) fn tab_ids(&self) -> Vec<u32> {
        let state = self.shared.lock();
        state
            .tabs
            .iter()
            .filter(|(_, t)| t.ready)
            .map(|(&id, _)| id)
            .collect()
    }

    /// How many tabs are open.
    /// Console messages and requests of shared and service workers that
    /// no tab had a frame on the origin of.
    pub(crate) fn worker_events_dropped(&self) -> u64 {
        self.shared.lock().dropped
    }

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

    /// Tab `id`'s workers: the dedicated workers of its frames, then the
    /// shared and service workers of its origins, each kind by URL.
    pub(crate) fn workers(&self, id: u32) -> Vec<WorkerInfo> {
        let state = self.shared.lock();
        let mut workers: Vec<WorkerInfo> = state
            .targets
            .values()
            .filter(|t| t.kind.worker().is_some() && state.tabs_of(t).contains(&id))
            .filter_map(|t| {
                Some(WorkerInfo {
                    kind: t.kind.worker()?,
                    url: t.url.clone(),
                    id: t.id.clone(),
                    frame: t.frame.clone(),
                })
            })
            .collect();
        let rank = |kind: WorkerKind| match kind {
            WorkerKind::Dedicated => 0,
            WorkerKind::Shared => 1,
            WorkerKind::Service => 2,
        };
        workers.sort_by(|a, b| (rank(a.kind), &a.url, &a.id).cmp(&(rank(b.kind), &b.url, &b.id)));
        workers
    }

    /// Records where the `<iframe>` of each of tab `id`'s frames of
    /// `positions` is (see [`FrameTree::set_positions`]).
    pub(crate) fn set_frame_positions(&self, id: u32, positions: Vec<(String, Vec<i64>)>) {
        if let Some(tab) = self.shared.lock().tabs.get_mut(&id) {
            tab.frames.set_positions(positions);
        }
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
                let session = state.targets.get(owner)?.session.clone();
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

    /// Calls `f` with the keys tab `id` holds (`None` if the tab is gone).
    pub(crate) fn keys<R>(&self, id: u32, f: impl FnOnce(&mut Vec<Key>) -> R) -> Option<R> {
        self.shared
            .lock()
            .tabs
            .get_mut(&id)
            .map(|tab| f(&mut tab.keys))
    }

    /// The keys each tab that holds some holds, by tab id (`status`).
    pub(crate) fn held_keys(&self) -> Vec<HeldKeys> {
        let state = self.shared.lock();
        let mut held: Vec<(u32, Vec<String>)> = state
            .tabs
            .iter()
            .filter(|(_, tab)| tab.ready && !tab.keys.is_empty())
            .map(|(&id, tab)| (id, tab.keys.iter().map(|k| k.key.to_owned()).collect()))
            .collect();
        held.sort_by_key(|(id, _)| *id);
        held.into_iter()
            .map(|(id, keys)| HeldKeys {
                tab: tab_name(id),
                keys,
            })
            .collect()
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
    /// existed before the session attached); if the document goes away
    /// meanwhile, in the next one.
    pub(crate) async fn context(
        &self,
        id: u32,
        frame: &str,
        world: World,
        timeout: Duration,
    ) -> Result<FrameContext, FrameError> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.context_once(id, frame, world, left).await {
                Err(FrameError::Cdp(e)) if document_gone(&e) && !left.is_zero() => {
                    // Until the event that says the old document's
                    // contexts are gone arrives.
                    tokio::time::sleep(GONE_POLL.min(left)).await;
                }
                found => return found,
            }
        }
    }

    async fn context_once(
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
            state.targets.get(session_id).map(|t| t.session.clone())
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

    /// Tab `id`'s network log and console marks when its main frame's
    /// document committed.
    pub(crate) fn document_marks(&self, id: u32) -> (u64, u64) {
        self.shared
            .lock()
            .tabs
            .get(&id)
            .map_or((0, 0), |t| t.document_marks)
    }

    /// Sets tab `id`'s mark `name` to its log marks now; the marks it
    /// replaces, if it had one (`None` also if the tab is gone).
    pub(crate) fn set_mark(&self, id: u32, name: &str) -> Option<Marks> {
        let mut state = self.shared.lock();
        let tab = state.tabs.get_mut(&id)?;
        let now = Marks {
            network: tab.network.seq(),
            console: tab.console.seq(),
            netlog: tab.netlog.seq(),
            time_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
        };
        tab.marks.insert(name.to_owned(), now)
    }

    /// Tab `id`'s mark `name`, or the names it has (sorted).
    pub(crate) fn named_mark(&self, id: u32, name: &str) -> Result<Marks, Vec<String>> {
        let state = self.shared.lock();
        let Some(tab) = state.tabs.get(&id) else {
            return Err(Vec::new());
        };
        tab.marks.get(name).copied().ok_or_else(|| {
            let mut names: Vec<String> = tab.marks.keys().cloned().collect();
            names.sort();
            names
        })
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

    /// Waits until tab `id`'s network log changed after its change count
    /// `after` (or the tab is gone); `false` if `deadline` passed first.
    pub(crate) async fn netlog_changed(&self, id: u32, after: u64, deadline: Instant) -> bool {
        self.watch(deadline, |state, _| match state.tabs.get(&id) {
            Some(tab) if tab.netlog.changes() <= after => Check::Later(None),
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

    /// The session with id `session_id`, if its target is still
    /// attached.
    pub(crate) fn host_session(&self, session_id: &str) -> Option<Session> {
        self.shared
            .lock()
            .targets
            .get(session_id)
            .map(|t| t.session.clone())
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

    /// Brings every page and iframe session in line with the session's
    /// init scripts and exposed functions; with `now`, scripts added run in
    /// the documents there are too. Returns the errors Chrome gave.
    pub(crate) async fn apply_injections(&self, now: bool, timeout: Duration) -> Vec<String> {
        inject::apply(&self.shared, now, timeout).await
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
    /// The tab is not in the context `--context` names.
    OtherContext(u32),
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
    fn new() -> Self {
        Self {
            next_id: 1,
            tabs: BTreeMap::new(),
            active: None,
            recent: Vec::new(),
            context_active: HashMap::new(),
            targets: HashMap::new(),
            ours: HashSet::new(),
            opened: VecDeque::new(),
            logged: VecDeque::new(),
            downloads: Vec::new(),
            seq: 0,
            starting: true,
            visited: HashMap::new(),
            dropped: 0,
            hooks: Vec::new(),
            next_hook: 0,
            coverage: None,
        }
    }

    /// Forgets the target of session `session_id`, and the targets it
    /// announced, whose own detach events came through it; returns them
    /// by session id, its own first (none if it was not attached).
    fn forget_target(&mut self, session_id: &str) -> Vec<(String, Target)> {
        let Some(target) = self.targets.remove(session_id) else {
            return Vec::new();
        };
        let mut forgotten = vec![(session_id.to_owned(), target)];
        let mut gone = vec![session_id.to_owned()];
        while let Some(parent) = gone.pop() {
            let children: Vec<String> = self
                .targets
                .iter()
                .filter(|(_, t)| t.parent.as_deref() == Some(parent.as_str()))
                .map(|(id, _)| id.clone())
                .collect();
            for child in children {
                if let Some(target) = self.targets.remove(&child) {
                    forgotten.push((child.clone(), target));
                }
                gone.push(child);
            }
        }
        forgotten
    }

    /// The tabs `target` belongs to: its own, or for a shared or service
    /// worker, the tabs of its scope with a frame on its origin.
    fn tabs_of(&self, target: &Target) -> Vec<u32> {
        match &target.owner {
            Owner::Tab(id) => vec![*id],
            Owner::None => Vec::new(),
            Owner::Origin { origin } => self
                .tabs
                .iter()
                .filter(|&(&id, tab)| {
                    self.in_scope(id, target.scope.as_deref())
                        && tab.frames.list().iter().any(|f| {
                            crate::state::origin_of(&f.url).as_deref() == Some(origin.as_str())
                        })
                })
                .map(|(&id, _)| id)
                .collect(),
        }
    }

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
        if let Some((browser_id, _)) = self.tabs.get(&id).and_then(|t| t.context.clone()) {
            self.context_active.insert(browser_id, id);
        }
    }

    /// Makes `id` the active tab of its context (of ours) only.
    fn activate_in(&mut self, browser_id: &str, id: u32) {
        self.recent.retain(|&r| r != id);
        self.recent.push(id);
        self.context_active.insert(browser_id.to_owned(), id);
    }

    /// Whether tab `id` is in Chrome's context `browser_id` (of ours), or
    /// with `None`, in none of ours.
    fn in_scope(&self, id: u32, browser_id: Option<&str>) -> bool {
        self.tabs
            .get(&id)
            .is_some_and(|t| t.context.as_ref().map(|c| c.0.as_str()) == browser_id)
    }

    /// The tab commands in context `browser_id` act on: its active one,
    /// else the one active most recently, else its newest.
    fn current_in(&self, browser_id: &str) -> Option<u32> {
        let ready = |id: &u32| {
            self.tabs.get(id).is_some_and(|t| t.ready) && self.in_scope(*id, Some(browser_id))
        };
        self.context_active
            .get(browser_id)
            .copied()
            .filter(ready)
            .or_else(|| self.recent.iter().rev().copied().find(ready))
            .or_else(|| self.tabs.keys().rev().copied().find(ready))
    }

    /// The tab to fall back to when there is no active one: the one active
    /// most recently, else the newest; one outside the contexts of ours
    /// first.
    fn fallback(&self) -> Option<u32> {
        let ready = |id: &u32| self.tabs.get(id).is_some_and(|t| t.ready);
        let default = |id: &u32| ready(id) && self.in_scope(*id, None);
        self.recent
            .iter()
            .rev()
            .copied()
            .find(default)
            .or_else(|| self.tabs.keys().rev().copied().find(default))
            .or_else(|| self.fallback_any(ready))
    }

    fn fallback_any(&self, ready: impl Fn(&u32) -> bool) -> Option<u32> {
        self.recent
            .iter()
            .rev()
            .copied()
            .find(&ready)
            .or_else(|| self.tabs.keys().rev().copied().find(&ready))
    }

    fn remove(&mut self, id: u32) {
        if self.tabs.remove(&id).is_some() {
            self.targets.retain(|_, t| t.owner != Owner::Tab(id));
        }
        // Its taps end once the sessions they hear are gone.
        self.hooks.retain(|h| h.tab() != id);
        self.recent.retain(|&r| r != id);
        if self.active == Some(id) {
            self.active = self.fallback();
        }
    }

    fn update(&mut self, info: &target::TargetInfo) {
        for target in self.targets.values_mut() {
            if target.id == info.target_id {
                target.url.clone_from(&info.url);
            }
        }
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
        let active = match &tab.context {
            Some((browser_id, _)) => self.context_active.get(browser_id) == Some(&id),
            None => self.active == Some(id),
        };
        TabInfo {
            id: tab_name(id),
            active,
            title: tab.title.clone(),
            url: tab.url.clone(),
            opener: tab
                .opener_target
                .as_deref()
                .and_then(|t| self.id_of(t))
                .map(tab_name),
            crashed: tab.crashed.is_some(),
            dialog: tab.dialog.as_ref().map(|d| d.info.clone()),
            context: tab.context.as_ref().map(|c| c.1.clone()),
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

/// Whether a call in a frame's document failed because the document
/// went away (the frame navigated).
fn document_gone(err: &cdp::CdpError) -> bool {
    matches!(err, cdp::CdpError::Protocol { message, .. }
        if message.starts_with("Cannot find context with specified id")
            || message.starts_with("Execution context was destroyed")
            || message.starts_with("Inspected target navigated or closed"))
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
async fn watch_browser(shared: Arc<Shared>, mut events: LosslessEventStream) {
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
                    destroyed(&shared, target_id);
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
/// context events of tab `tab`'s frames it owns, its dialogs and its
/// crash. Its intercepted requests have their own task
/// ([`fetch::watch_fetch`]). It gets every event, however far behind it
/// falls: a skipped one would lose a frame, a console entry, or a
/// request's end (which leaves it in flight and stalls `--idle`).
async fn watch_host(
    shared: Arc<Shared>,
    mut events: LosslessEventStream,
    session: Session,
    tab: Option<u32>,
) {
    let session_id = session.id().unwrap_or_default().to_owned();
    while let Some(event) = events.next().await {
        match event.method.as_str() {
            target::AttachedToTarget::NAME => {
                let parent = Parent::Host {
                    session: &session_id,
                    tab,
                };
                attached(&shared, &event, parent);
            }
            target::DetachedFromTarget::NAME => {
                if let Some(session_id) = event.params["sessionId"].as_str() {
                    detached(&shared, session_id);
                }
            }
            method if method.starts_with("Network.") => {
                if let Some(tab) = tab {
                    network_event(&shared, &session_id, tab, method, &event);
                }
            }
            "Runtime.bindingCalled" if event.params["name"] == SEED_BINDING => {
                seeded(&shared, &session_id, &event);
            }
            "Runtime.bindingCalled" if event.params["name"] == MOCK_BINDING => {
                mock_shim::called(&shared, &session_id, tab, &event);
            }
            "Runtime.consoleAPICalled"
            | "Runtime.exceptionThrown"
            | "Runtime.exceptionRevoked"
            | "Runtime.bindingCalled"
            | "Log.entryAdded" => {
                if let Some(tab) = tab {
                    console_event(&shared, tab, &event);
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

/// A storage seed gave an origin's document its storage: it is taken out
/// of every page and iframe session (plan 07, milestone 7.4).
fn seeded(shared: &Arc<Shared>, session_id: &str, event: &RawEvent) {
    let Some(origin) = event.params["payload"].as_str() else {
        return;
    };
    let scope = shared
        .lock()
        .targets
        .get(session_id)
        .and_then(|t| t.scope.clone());
    if shared.settings.injections.seeded(scope.as_deref(), origin) {
        tracing::debug!("{origin} got its storage; its seed is taken out");
        let shared2 = shared.clone();
        shared.spawn(async move {
            inject::apply(&shared2, false, SETUP_TIMEOUT).await;
        });
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
    let mut guard = shared.lock();
    let state = &mut *guard;
    let Some(tab) = state.tabs.get_mut(&tab) else {
        return;
    };
    let visited = &mut state.visited;
    let frames = &mut tab.frames;
    let decoded = match method {
        page::FrameAttached::NAME => decode::<page::FrameAttached>(event)
            .map(|e| frames.attached(session_id, &e.frame_id, &e.parent_frame_id)),
        page::FrameNavigated::NAME => decode::<page::FrameNavigated>(event).map(|e| {
            visit(visited, tab.context.as_ref(), &e.frame.url);
            let main = e.frame.parent_id.is_none();
            tab.network
                .navigated(session_id, &e.frame.id, &e.frame.loader_id, main);
            if main && session_id == tab.session.id().unwrap_or_default() {
                // A new document: the page runs again after a crash.
                tab.crashed = None;
                tab.document_marks = (tab.network.seq(), tab.console.seq());
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
    drop(guard);
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
        let logged = t
            .netlog
            .event(session_id, method, &event.params, capacity, None);
        followed || logged
    });
    if followed {
        shared.bump();
    }
}

/// Applies a console call, uncaught exception or browser message of a
/// page or iframe session to tab `tab`'s console.
fn console_event(shared: &Shared, tab: u32, event: &RawEvent) {
    let capacity = shared.settings.console_buffer();
    let logged = shared.lock().tabs.get_mut(&tab).is_some_and(|t| {
        t.console
            .event(&event.method, &event.params, None, capacity)
    });
    if logged {
        shared.bump();
    }
}

/// Applies a console call, uncaught exception or browser message of
/// worker session `session_id` to the console of each tab it belongs to,
/// marked with the worker. A shared or service worker's entry is in each
/// tab of its origin, with the same number of the worker's own
/// ([`crate::observe::console::ConsoleError::once`]); with no such tab it
/// is dropped and counted.
fn worker_console_event(shared: &Shared, session_id: &str, event: &RawEvent) {
    let capacity = shared.settings.console_buffer();
    let mut guard = shared.lock();
    let state = &mut *guard;
    let Some(target) = state.targets.get_mut(session_id) else {
        return;
    };
    let Some(worker) = target.worker() else {
        return;
    };
    target.events += 1;
    let once =
        (target.kind != TargetKind::DedicatedWorker).then(|| (target.id.clone(), target.events));
    let tabs = state.tabs_of(&state.targets[session_id]);
    if tabs.is_empty() && is_logged(event) {
        state.dropped += 1;
    }
    let from = FromWorker {
        worker: &worker,
        once: once.as_ref().map(|(id, n)| (id.as_str(), *n)),
    };
    let mut logged = false;
    for id in tabs {
        if let Some(t) = state.tabs.get_mut(&id) {
            logged |= t
                .console
                .event(&event.method, &event.params, Some(from), capacity);
        }
    }
    drop(guard);
    if logged {
        shared.bump();
    }
}

/// Whether a console event is one a tab's console keeps (not a revoked
/// rejection, nor a worker's `Log` repeating its console calls).
fn is_logged(event: &RawEvent) -> bool {
    match event.method.as_str() {
        "Runtime.exceptionRevoked" => false,
        "Log.entryAdded" => event.params["entry"]["source"] != "worker",
        _ => true,
    }
}

/// Handles a worker's events (see "Where worker events go" in plan 13):
/// its requests go to the network, and its console calls, uncaught
/// exceptions and browser messages to the console, of each tab it
/// belongs to (see `State::tabs_of`, asked at each event), and its mock
/// shim's calls to the router.
async fn watch_worker(shared: Arc<Shared>, mut events: LosslessEventStream, session_id: String) {
    while let Some(event) = events.next().await {
        match event.method.as_str() {
            method if method.starts_with("Network.") => {
                worker_network_event(&shared, &session_id, method, &event);
            }
            "Runtime.bindingCalled" if event.params["name"] == MOCK_BINDING => {
                let tab = shared.lock().targets.get(&session_id).and_then(Target::tab);
                mock_shim::called(&shared, &session_id, tab, &event);
            }
            "Runtime.consoleAPICalled"
            | "Runtime.exceptionThrown"
            | "Runtime.exceptionRevoked"
            | "Log.entryAdded" => worker_console_event(&shared, &session_id, &event),
            _ => {}
        }
    }
}

/// Applies a `Network` event of worker session `session_id` to the
/// requests and network log of each tab the worker belongs to, its
/// entries marked with the worker.
fn worker_network_event(shared: &Shared, session_id: &str, method: &str, event: &RawEvent) {
    let capacity = shared.settings.network_buffer();
    let mut guard = shared.lock();
    let state = &mut *guard;
    let Some(target) = state.targets.get(session_id) else {
        return;
    };
    let worker = target.worker();
    let tabs = state.tabs_of(target);
    if tabs.is_empty() && method == "Network.requestWillBeSent" {
        state.dropped += 1;
    }
    let mut followed = false;
    for id in tabs {
        if let Some(t) = state.tabs.get_mut(&id) {
            followed |= t.network.worker_event(session_id, method, &event.params);
            followed |=
                t.netlog
                    .event(session_id, method, &event.params, capacity, worker.as_ref());
        }
    }
    drop(guard);
    if followed {
        shared.bump();
    }
}

fn decode<E: cdp::Event>(event: &RawEvent) -> Result<E, serde_json::Error> {
    E::deserialize(&event.params)
}

/// Which session announced a target.
#[derive(Debug, Clone, Copy)]
enum Parent<'a> {
    Browser,
    /// A page or iframe session, with its tab (none for a prerender).
    Host {
        session: &'a str,
        tab: Option<u32>,
    },
}

/// A target attached (paused if new): record it (and register a tab for a
/// page), then set it up and resume it in its own task.
fn attached(shared: &Arc<Shared>, event: &RawEvent, parent: Parent<'_>) {
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
    let contexts = &shared.settings.contexts;
    let scope = contexts.scope_of(info.browser_context_id.as_deref());
    let context = scope.clone().zip(contexts.name_of(scope.as_deref()));
    let new_tab = if matches!(parent, Parent::Browser) && is_tab(&info) {
        let mut state = shared.lock();
        let id = state.next_id;
        state.next_id += 1;
        state
            .tabs
            .insert(id, Tab::new(&info, &session, &session_id, context));
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
    let mut kind = TargetKind::of(&info.r#type);
    if matches!(parent, Parent::Host { .. })
        && matches!(kind, TargetKind::SharedWorker | TargetKind::ServiceWorker)
    {
        // A service worker is also announced by the pages it controls:
        // the browser's own session of it is the one used, this one is
        // only resumed.
        kind = TargetKind::Other;
    }
    let (tab, parent_session) = match parent {
        Parent::Browser => (new_tab, None),
        Parent::Host { session, tab } => (tab, Some(session.to_owned())),
    };
    let owner = match kind {
        // Shared by the tabs of its scope on its origin.
        TargetKind::SharedWorker | TargetKind::ServiceWorker => crate::state::origin_of(&info.url)
            .map_or(Owner::None, |origin| Owner::Origin { origin }),
        _ => tab.map_or(Owner::None, Owner::Tab),
    };
    // Before auto-attach and the domains are turned on for it, so no
    // child and no frame event is missed.
    let events = (kind.is_host() || kind.worker().is_some()).then(|| session.events_lossless(""));
    let fetch_events = Feature::Fetch
        .kinds()
        .contains(kind)
        .then(|| session.events_lossless("Fetch."));
    let mut target = Target::new(
        session.clone(),
        kind,
        info.target_id.clone(),
        info.url.clone(),
        owner,
        scope,
        parent_session,
    );
    if kind == TargetKind::DedicatedWorker {
        target.frame.clone_from(&info.parent_frame_id);
    }
    {
        let mut state = shared.lock();
        state.targets.insert(session_id.clone(), target);
        tap::attached(shared, &mut state, &session_id);
    }
    if let Some(fetch_events) = fetch_events {
        fetch::enable_early(shared, &session, kind);
        shared.spawn(fetch::watch_fetch(
            shared.clone(),
            fetch_events,
            session.clone(),
            tab,
            info.browser_context_id.clone(),
        ));
    }
    if let Some(events) = events {
        if kind.is_host() {
            shared.spawn(watch_host(shared.clone(), events, session.clone(), tab));
        } else {
            shared.spawn(watch_worker(shared.clone(), events, session_id.clone()));
        }
    }
    let waiting = attached.waiting_for_debugger;
    let task_shared = shared.clone();
    shared.spawn(async move {
        let session = session.with_timeout(SETUP_TIMEOUT);
        let target = NewTarget {
            session: &session,
            kind,
            tab,
        };
        setup(&task_shared, target, waiting).await;
        if let Some(id) = new_tab {
            if let Some(front) = front_after_popup(&task_shared, id) {
                let front = front.with_timeout(SETUP_TIMEOUT);
                if let Err(e) = front.call(page::BringToFrontParams::default()).await {
                    tracing::debug!("bringing the active tab back to the front failed: {e}");
                }
            }
            ready(&task_shared, id);
        }
    });
}

/// Prepares a new target before it runs, then resumes it: every feature
/// that applies to its kind (see [`features`]) adds its steps. A page or
/// iframe gets auto-attach for its children, the domains commands need,
/// frame tracking and chrome-cdp's isolated world, and the session's
/// settings; a worker gets its requests followed and routed and the mock
/// shim (a dedicated worker also its console); the others are only
/// resumed.
///
/// Every step and the resume are sent at once and only then awaited. Chrome
/// runs them in order, but some answers (`Page.enable` in a popup's new
/// renderer) only come once the target runs, so awaiting each step before
/// sending the next would stall until the timeout.
async fn setup(shared: &Arc<Shared>, target: NewTarget<'_>, waiting: bool) {
    let mut steps: Vec<(&str, Step)> = Vec::new();
    for feature in REGISTRY {
        if feature.kinds().contains(target.kind) {
            tracing::trace!(feature = feature.name(), kind = ?target.kind, "setting up");
            feature.setup_steps(shared, target, &mut steps);
        }
    }
    if waiting {
        let resume = runtime::RunIfWaitingForDebuggerParams::default();
        steps.push(("resuming", step(target.session.call(resume))));
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
    let mut guard = shared.lock();
    let state = &mut *guard;
    if let Some(tab) = state.tabs.get_mut(&tab) {
        tab.frames.seed(session_id, tree);
        let mut trees = vec![tree];
        while let Some(tree) = trees.pop() {
            visit(&mut state.visited, tab.context.as_ref(), &tree.frame.url);
            trees.extend(tree.child_frames.iter().flatten());
        }
    }
    drop(guard);
    shared.bump();
}

/// Adds `url`'s origin (http and https only) to what the scope of a tab in
/// `context` visited.
fn visit(
    visited: &mut HashMap<Option<String>, Vec<String>>,
    context: Option<&(String, String)>,
    url: &str,
) {
    let Some(origin) = crate::state::origin_of(url) else {
        return;
    };
    let origins = visited
        .entry(context.map(|(browser_id, _)| browser_id.clone()))
        .or_default();
    if !origins.contains(&origin) {
        origins.push(origin);
    }
}

/// One setup call already sent, its result discarded.
type Step = Pin<Box<dyn Future<Output = cdp::Result<()>> + Send>>;

fn step<T>(call: impl Future<Output = cdp::Result<T>> + Send + 'static) -> Step {
    Box::pin(async move { call.await.map(drop) })
}

/// The page session of the tab to bring back to the front once popup `id` is set up: the
/// active tab of the popup's context, unless the session follows popups.
/// Chrome shows a popup in front of its opener, which then reports
/// `document.visibilityState` "hidden": no `requestAnimationFrame`,
/// throttled timers, geolocation held back. The active tab is the one
/// commands act on, so it stays in front; the popup is a background tab,
/// as `tab <id>` would leave it. `None` for tabs a command opened (no
/// opener) and for the tabs found at start.
fn front_after_popup(shared: &Shared, id: u32) -> Option<Session> {
    let state = shared.lock();
    let popup = state.tabs.get(&id)?;
    if state.starting
        || popup.opener_target.is_none()
        || state.ours.contains(&popup.target_id)
        || shared.settings.follow_popups.load(Ordering::Relaxed)
    {
        return None;
    }
    let ready = |id: &u32| state.tabs.get(id).is_some_and(|t| t.ready);
    let front = match &popup.context {
        Some((browser_id, _)) => state.current_in(browser_id),
        None => state.active.filter(ready).or_else(|| state.fallback()),
    }?;
    let front = state.tabs.get(&front)?;
    (front.target_id != popup.target_id).then(|| front.session.clone())
}

/// Target `target_id` is gone: forget it, and its tab if it is one.
fn destroyed(shared: &Shared, target_id: &str) {
    let mut state = shared.lock();
    state.targets.retain(|_, t| t.id != target_id);
    if let Some(id) = state.id_of(target_id) {
        state.remove(id);
    }
    drop(state);
    shared.bump();
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

/// A session went away: forget its target (and those it announced),
/// their frames and requests, and its tab.
fn detached(shared: &Shared, session_id: &str) {
    let mut state = shared.lock();
    for (id, target) in state.forget_target(session_id) {
        if target.kind.is_host()
            && let Some(tab) = target.tab().and_then(|tab| state.tabs.get_mut(&tab))
        {
            tab.frames.session_gone(&id);
        }
        // A worker's requests may be in any tab's.
        for tab in state.tabs.values_mut() {
            tab.network.session_gone(&id);
        }
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
    use std::sync::Arc;
    use std::time::Duration;

    use cdp::Connection;
    use cdp::transport::MemoryTransport;
    use protocol::schema::RequestState;
    use serde_json::json;
    use tokio::time::Instant;

    use cdp::domains::{page, target};

    use super::targets::{Owner, Target, TargetKind};
    use super::{Settings, State, Tab, Tabs, parse_tab_id};

    /// A burst far past the old 1024-event broadcast buffer, sent while
    /// the tab's watchers cannot run, loses nothing: every console entry
    /// is logged and every request ends.
    #[tokio::test]
    async fn a_lagging_watcher_misses_no_event() {
        const LOGS: usize = 1500;
        const REQUESTS: usize = 600;
        // The connection reads on a runtime of its own; the watchers run
        // on this one, which the test blocks during the burst.
        let io = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (transport, mut peer) = MemoryTransport::pair();
        let conn = {
            let _io = io.enter();
            Connection::new(transport)
        };
        let tabs = Tabs::start(&conn, Arc::new(Settings::default()), None);
        peer.send_json(&json!({"method": "Target.attachedToTarget", "params": {
            "sessionId": "S1",
            "targetInfo": {"targetId": "T1", "type": "page", "title": "", "url": "about:blank",
                           "attached": true, "canAccessOpener": false},
            "waitingForDebugger": false,
        }}));
        // Its setup starts once the tab and its watchers are there.
        assert!(peer.recv_json().await.is_some());
        assert!(tabs.shared.lock().targets.contains_key("S1"));
        let mut probe = conn.session("S1").events_lossless("Probe.");
        let (dispatched, burst_done) = std::sync::mpsc::channel();
        io.spawn(async move {
            probe.next().await;
            let _ = dispatched.send(());
        });
        for n in 0..LOGS {
            peer.send_json(
                &json!({"method": "Runtime.consoleAPICalled", "sessionId": "S1", "params": {
                    "type": "log", "args": [{"type": "string", "value": format!("burst {n}")}],
                    "executionContextId": 1, "timestamp": 0,
                }}),
            );
        }
        for n in 0..REQUESTS {
            let id = format!("R{n}");
            peer.send_json(&json!({"method": "Network.requestWillBeSent", "sessionId": "S1", "params": {
                "requestId": id, "loaderId": "L1", "documentURL": "about:blank", "frameId": "T1",
                "request": {"url": format!("http://x.test/{n}"), "method": "GET", "headers": {}},
                "timestamp": 1.0, "wallTime": 1.0, "initiator": {"type": "other"}, "type": "Fetch",
            }}));
            peer.send_json(
                &json!({"method": "Network.loadingFinished", "sessionId": "S1", "params": {
                    "requestId": id, "timestamp": 2.0, "encodedDataLength": 0,
                }}),
            );
        }
        // Once the probe has it, the reader has handed on the whole burst,
        // and no watcher has run meanwhile.
        peer.send_json(&json!({"method": "Probe.done", "sessionId": "S1", "params": {}}));
        burst_done.recv_timeout(Duration::from_secs(10)).unwrap();
        let done = tabs
            .wait(Duration::from_secs(10), |s| {
                let tab = &s.tabs[&1];
                tab.console.seq() == LOGS as u64 && tab.netlog.seq() == REQUESTS as u64
            })
            .await;
        {
            let state = tabs.shared.lock();
            let tab = &state.tabs[&1];
            assert!(
                done,
                "{} logs, {} requests",
                tab.console.seq(),
                tab.netlog.seq()
            );
            assert!(tab.network.busy(Instant::now()).is_empty());
            assert!(
                tab.netlog
                    .records()
                    .all(|r| r.entry.state == RequestState::Done)
            );
        }
        drop(tabs);
        io.shutdown_background();
    }

    /// A shared or service worker belongs to the tabs of its scope with
    /// a frame on its origin: the main frame's or an iframe's.
    #[tokio::test]
    async fn tabs_of_a_worker() {
        let (transport, _peer) = MemoryTransport::pair();
        let conn = Connection::new(transport);
        let mut state = State::new();
        let frame = |id: &str, parent: Option<&str>, url: &str| -> page::Frame {
            serde_json::from_value(json!({
                "id": id, "parentId": parent, "loaderId": "L", "url": url,
                "domainAndRegistry": "", "securityOrigin": "", "mimeType": "text/html",
                "secureContextType": "Secure", "crossOriginIsolatedContextType": "NotIsolated",
                "gatedAPIFeatures": [],
            }))
            .unwrap()
        };
        // t1: a.test with a b.test iframe; t2: b.test; t3: a.test in
        // context c1; t4: b.test on another port.
        let tabs = [
            ("http://a.test/", Some("http://b.test/frame"), None),
            ("http://b.test/x", None, None),
            ("http://a.test/y", None, Some("C1")),
            ("http://b.test:8080/", None, None),
        ];
        for (n, (url, iframe, context)) in tabs.into_iter().enumerate() {
            let id = u32::try_from(n + 1).unwrap();
            let target_id = format!("T{id}");
            let session_id = format!("S{id}");
            let info: target::TargetInfo = serde_json::from_value(json!({
                "targetId": target_id, "type": "page", "title": "", "url": url,
                "attached": true, "canAccessOpener": false,
            }))
            .unwrap();
            let context = context.map(|c: &str| (c.to_owned(), "c1".to_owned()));
            let session = conn.session(session_id.clone());
            let mut tab = Tab::new(&info, &session, &session_id, context);
            tab.frames
                .navigated(&session_id, &frame(&target_id, None, url));
            if let Some(iframe) = iframe {
                tab.frames.attached(&session_id, "F", &target_id);
                tab.frames
                    .navigated(&session_id, &frame("F", Some(&target_id), iframe));
            }
            state.tabs.insert(id, tab);
        }
        let worker = |origin: &str, scope: Option<&str>| {
            Target::new(
                conn.session("W".to_owned()),
                TargetKind::ServiceWorker,
                "W".to_owned(),
                format!("{origin}/sw.js"),
                Owner::Origin {
                    origin: origin.to_owned(),
                },
                scope.map(str::to_owned),
                None,
            )
        };
        assert_eq!(state.tabs_of(&worker("http://b.test", None)), [1, 2]);
        assert_eq!(state.tabs_of(&worker("http://a.test", None)), [1]);
        assert_eq!(state.tabs_of(&worker("http://a.test", Some("C1"))), [3]);
        assert!(state.tabs_of(&worker("http://c.test", None)).is_empty());
        let mut dedicated = worker("http://b.test", None);
        dedicated.owner = Owner::Tab(4);
        assert_eq!(state.tabs_of(&dedicated), [4]);
        dedicated.owner = Owner::None;
        assert!(state.tabs_of(&dedicated).is_empty());
    }

    /// A target that goes takes the targets it announced with it.
    #[tokio::test]
    async fn forgetting_a_target_forgets_its_children() {
        let (transport, _peer) = MemoryTransport::pair();
        let conn = Connection::new(transport);
        let mut state = State::new();
        for (session, parent) in [("P", None), ("I", Some("P")), ("W", Some("I")), ("X", None)] {
            let target = Target::new(
                conn.session(session.to_owned()),
                TargetKind::Other,
                session.to_owned(),
                String::new(),
                Owner::None,
                None,
                parent.map(str::to_owned),
            );
            state.targets.insert(session.to_owned(), target);
        }
        let gone: Vec<String> = state
            .forget_target("I")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(gone, ["I", "W"]);
        let mut left: Vec<&String> = state.targets.keys().collect();
        left.sort();
        assert_eq!(left, ["P", "X"]);
        assert!(state.forget_target("I").is_empty());
    }

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
