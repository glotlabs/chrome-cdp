//! The daemon's brain: target/tab manager, frames, navigation and waits,
//! snapshot and ref registry, locators, input and actionability, observation,
//! emulation and routing (plans 03-07).
//!
//! Milestone 2.4 gives it the session's browser: [`Engine`] launches Chrome
//! (`open`) or attaches to one (`connect`), records why it went away if it
//! crashes, makes raw CDP calls (`cdp`), and serialises page commands with a
//! per-tab async mutex. Milestone 3.1 adds the target & tab manager
//! (`tabs`): every target is auto-attached and set up before it runs, and
//! page commands act on the active tab or `--tab`. Milestone 3.2 tracks
//! each tab's frames and their execution contexts (`frames`), with an
//! isolated world for chrome-cdp's own scripts. Milestone 3.3 adds
//! navigation (`navigate`: `goto`, `back`, `forward`, `reload`, and the
//! page load of `open` and `tab new`). Milestone 3.4 adds the wait engine
//! (`wait`, fed by each tab's `network` tracker), which navigations use
//! for their settle. Milestone 3.5 handles what else can happen to a
//! page: JavaScript dialogs (`dialog`, and `interrupt` for the commands
//! they stall), downloads (`downloads`), crashes, and HTTP
//! authentication. Plan 04 adds `snapshot` (milestones 4.1 and 4.3: two
//! engines, one output format, refs), the locator engine (`locator`, 4.4)
//! and the read commands (`read` and `find`, 4.5). Plan 05 adds `input`,
//! starting with the actionability pipeline every action runs (5.1)
//! and the mouse and pointer actions (5.2).
//! Later plans add modules next to `tabs` and `navigate`: `page`,
//! `observe`, `emulate`, `route`.

mod clipboard;
mod dialog;
mod downloads;
mod endpoint;
mod errors;
mod frames;
mod injected;
mod input;
mod interrupt;
mod locator;
mod navigate;
mod network;
mod observe;
mod pattern;
mod raw;
mod read;
mod request;
mod snapshot;
mod tabs;
mod wait;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use browser::{Chrome, Host, LaunchOptions, Launched, ProfileChoice};
use cdp::domains::browser as cdp_browser;
use cdp::transport::WsTransport;
use cdp::{Connection, ConnectionOptions};
use protocol::schema::{BrowserInfo, BrowserKind, OpenData, ProfileInfo, SessionState};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};
use tokio::sync::Notify;

pub use endpoint::{Endpoint, parse_endpoint};
pub use errors::from_cdp;
pub use frames::{FrameInfo, WORLD_NAME, World};
pub use input::{ActionPoint, Checks, check_action, parse_modifiers};
pub use locator::{DEFAULT_TESTID_ATTRIBUTE, LocateOptions, Locator, parse_locator};
pub use navigate::normalize_url;
pub use read::check_read;
pub use request::{
    Action, ActionRequest, Button, CdpRequest, ClipboardOp, ClipboardRequest, ConnectRequest,
    ConsoleRequest, DialogPolicy, DialogRequest, DownloadsRequest, ElementState, FindRequest,
    HttpCredentials, Modifier, MouseOp, NavigateRequest, Navigation, OpenRequest, Read,
    ReadRequest, ScrollTo, Since, SnapshotEngine, SnapshotRequest, TabCloseRequest, TabNewRequest,
    TabSelectRequest, TabsRequest, Target, WaitFor, WaitRequest, WaitUntil,
};
pub use snapshot::{DEFAULT_ENGINE, RefCall, check_find, check_snapshot};
pub use tabs::commands::TabClosed;
pub use tabs::parse_tab_id;
pub use wait::check_condition;

use request::millis;
use tabs::{Settings, TabLocks, Tabs};

/// One session's browser and everything the commands keep between calls.
#[derive(Debug)]
pub struct Engine {
    session: String,
    /// Where launched Chrome's stderr goes (`<session dir>/chrome.log`).
    chrome_log: Option<PathBuf>,
    /// Serialises `open`, `connect` and `close`.
    lifecycle: tokio::sync::Mutex<()>,
    state: Mutex<State>,
    generation: AtomicU64,
    tabs: TabLocks,
    /// What every new target gets before it runs; kept across browsers.
    settings: Arc<Settings>,
}

#[derive(Debug)]
enum State {
    Empty,
    Running(Arc<Running>),
    Crashed(Box<Crash>),
}

/// A browser that went away without `close`.
#[derive(Debug, Clone)]
struct Crash {
    reason: String,
    info: BrowserInfo,
}

#[derive(Debug)]
struct Running {
    generation: u64,
    conn: Connection,
    info: BrowserInfo,
    spec: Spec,
    started: Instant,
    tabs: Tabs,
    /// The browser we launched; `None` when connected (or once closing).
    launched: Mutex<Option<Launched>>,
    /// Stops the crash watcher before a deliberate close.
    stop: Arc<Notify>,
    /// Whether the browser runs on macOS (for `Mod` and editing
    /// commands), once asked.
    mac: std::sync::OnceLock<bool>,
    /// The source maps console stack frames were resolved with.
    sourcemaps: observe::SourceMaps,
}

/// What the running browser was started with, to compare with a later
/// `open`.
#[derive(Debug, Clone, PartialEq)]
enum Spec {
    Launched {
        chrome: PathBuf,
        headed: bool,
        profile: ProfileChoice,
        args: Vec<String>,
    },
    Connected {
        endpoint: String,
    },
}

/// The result of `open` or `connect`.
#[derive(Debug, Clone)]
pub struct Opened {
    pub data: OpenData,
    pub warnings: Vec<String>,
}

/// What `status` reports about the browser.
#[derive(Debug, Clone)]
pub struct EngineStatus {
    pub state: SessionState,
    pub browser: Option<BrowserInfo>,
    pub crash: Option<String>,
    pub tabs: Option<u32>,
    pub uptime: Option<Duration>,
}

/// What `close` did to the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closed {
    /// A launched browser was shut down.
    Closed,
    /// A connected browser was left running.
    Detached,
    /// There was none.
    None,
}

impl Closed {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Detached => "detached",
            Self::None => "none",
        }
    }
}

impl Engine {
    /// An engine with no browser yet. Downloads go to `downloads` (made
    /// when a browser starts); with `None`, the browser's own download
    /// behaviour stays.
    pub fn new(
        session: impl Into<String>,
        chrome_log: Option<PathBuf>,
        downloads: Option<PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Self {
            session: session.into(),
            chrome_log,
            lifecycle: tokio::sync::Mutex::new(()),
            state: Mutex::new(State::Empty),
            generation: AtomicU64::new(0),
            tabs: TabLocks::default(),
            settings: Arc::new(Settings::new(downloads)),
        })
    }

    /// The session name.
    pub fn session(&self) -> &str {
        &self.session
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Running, crashed, or idle (no browser yet, or closed).
    pub fn state(&self) -> SessionState {
        match *self.lock_state() {
            State::Running(_) => SessionState::Running,
            State::Crashed(_) => SessionState::Crashed,
            State::Empty => SessionState::Idle,
        }
    }

    /// The running browser, or the error a command gets without one.
    fn running(&self) -> Result<Arc<Running>, ErrorInfo> {
        match &*self.lock_state() {
            State::Running(running) => Ok(running.clone()),
            State::Crashed(crash) => Err(self.crashed_error(crash)),
            State::Empty => Err(ErrorInfo::new(
                ErrorKind::DaemonUnavailable,
                format!("session {} has no browser", self.session),
            )
            .hint(Hint::run(Cmd::new("open").session(&self.session)))),
        }
    }

    fn crashed_error(&self, crash: &Crash) -> ErrorInfo {
        let info = ErrorInfo::new(ErrorKind::BrowserCrashed, crash.reason.clone());
        match (&crash.info.kind, &crash.info.endpoint) {
            (BrowserKind::Connected, Some(endpoint)) => info.hint(
                Hint::run(Cmd::new("connect").arg(endpoint).session(&self.session))
                    .why("attaches again once the browser is back"),
            ),
            _ => info.hint(errors::crashed_hint(&self.session)),
        }
    }

    /// `open`: launches Chrome unless the session runs one that fits the
    /// request, then loads `url`.
    pub async fn open(self: &Arc<Self>, req: OpenRequest) -> Result<Opened, ErrorInfo> {
        let timeout = millis(req.timeout_ms);
        let tab = req.tab.as_deref().map(parse_tab_id).transpose()?;
        let _lifecycle = self.lifecycle.lock().await;
        if let Some(follow) = req.follow_popups {
            self.settings.set_follow_popups(follow);
        }
        if let Some(ignore) = req.ignore_https_errors {
            self.settings.set_ignore_https_errors(ignore);
        }
        if let Some(policy) = req.dialogs {
            self.settings.set_dialogs(policy);
        }
        if let Some(credentials) = &req.http_credentials {
            self.settings.set_http_credentials(credentials.clone());
        }
        if let Some(entries) = req.console_buffer {
            self.settings.set_console_buffer(entries);
        }
        let mut warnings = Vec::new();
        let current = match &*self.lock_state() {
            State::Running(running) => Some(running.clone()),
            State::Crashed(crash) => {
                warnings.push(format!("{}; started a new browser", crash.reason));
                None
            }
            State::Empty => None,
        };
        let (running, started) = match current {
            Some(running) if req.restart => {
                self.shut_down(running).await;
                (self.launch(&req, &mut warnings).await?, true)
            }
            Some(running) => {
                self.check_fits(&running, &req)?;
                if req.ignore_https_errors.is_some() {
                    running
                        .tabs
                        .apply_security(timeout)
                        .await
                        .map_err(|e| from_cdp(&e, &self.session))?;
                }
                if req.http_credentials.is_some() {
                    running.tabs.apply_credentials(timeout).await;
                }
                (running, false)
            }
            None => (self.launch(&req, &mut warnings).await?, true),
        };
        let page = match &req.url {
            Some(url) => {
                let tab = tab.map(tabs::tab_name);
                let page = self.page(&running, tab.as_deref(), timeout).await?;
                let _tab = self.tabs.lock(&page.target_id).await;
                let navigation = Navigation::Goto {
                    url: url.clone(),
                    base_url: req.base_url.clone(),
                };
                let navigated = self
                    .navigate_page(&running, &page, &navigation, WaitUntil::Settle, timeout)
                    .await?;
                warnings.extend(navigated.warnings.iter().cloned());
                Some(navigated.page_info())
            }
            None => None,
        };
        Ok(Opened {
            data: OpenData {
                session: self.session.clone(),
                started,
                browser: running.info.clone(),
                page,
            },
            warnings,
        })
    }

    /// Launches Chrome for `req` and makes it the running browser.
    async fn launch(
        self: &Arc<Self>,
        req: &OpenRequest,
        warnings: &mut Vec<String>,
    ) -> Result<Arc<Running>, ErrorInfo> {
        let host = Host::current();
        let chrome = if let Some(chrome) = &req.chrome {
            chrome.clone()
        } else {
            let chrome = browser::discover(req.chrome_flag.as_deref(), &host)
                .map_err(|e| e.to_error_info())?;
            warnings.extend(chrome.warnings.iter().cloned());
            chrome
        };
        let headed = req.headed.unwrap_or(false);
        let profile = req.profile.clone().unwrap_or_default();
        let options = LaunchOptions {
            headed,
            profile: profile.clone(),
            args: req.chrome_args.clone(),
            log_file: self.chrome_log.clone(),
            timeout: millis(req.timeout_ms),
        };
        tracing::info!(chrome = %chrome.path.display(), "launching Chrome {}", chrome.source.describe());
        // Called from a runtime worker (never `spawn_blocking`): on Linux,
        // Chrome dies with the thread that spawned it.
        let launched = browser::launch(&chrome, &options, &host)
            .await
            .map_err(|info| {
                if info.hint.as_deref() == Some(browser::launch::OUTPUT_HINT) {
                    info.hint(
                        Hint::run(Cmd::new("status").session(&self.session).switch("logs"))
                            .why("shows Chrome's output"),
                    )
                } else {
                    info
                }
            })?;
        warnings.extend(launched.warnings.iter().cloned());
        let info = launched_info(&chrome, &launched, &options);
        let spec = Spec::Launched {
            chrome: chrome.path.clone(),
            headed,
            profile,
            args: req.chrome_args.clone(),
        };
        let conn = launched.connection().clone();
        let running = self.install(conn, info, spec, Some(launched));
        self.init_tabs(running, millis(req.timeout_ms)).await
    }

    /// Starts managing a new browser's tabs; closes it if that fails.
    async fn init_tabs(
        &self,
        running: Arc<Running>,
        timeout: Duration,
    ) -> Result<Arc<Running>, ErrorInfo> {
        if let Err(e) = running.tabs.init(timeout).await {
            let info = from_cdp(&e, &self.session);
            self.shut_down(running).await;
            return Err(info);
        }
        Ok(running)
    }

    /// `connect`: attaches the session to a running browser.
    pub async fn connect(self: &Arc<Self>, req: ConnectRequest) -> Result<Opened, ErrorInfo> {
        let timeout = millis(req.timeout_ms);
        let endpoint = parse_endpoint(&req.endpoint)?;
        let _lifecycle = self.lifecycle.lock().await;
        let mut warnings = Vec::new();
        let current = match &*self.lock_state() {
            State::Running(running) => Some(running.clone()),
            State::Crashed(crash) => {
                warnings.push(format!("{}; attached again", crash.reason));
                None
            }
            State::Empty => None,
        };
        if let Some(running) = current {
            if running.spec
                == (Spec::Connected {
                    endpoint: req.endpoint.clone(),
                })
            {
                return Ok(Opened {
                    data: OpenData {
                        session: self.session.clone(),
                        started: false,
                        browser: running.info.clone(),
                        page: None,
                    },
                    warnings,
                });
            }
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("session {} already has a browser", self.session),
            )
            .note("running", describe(&running.info))
            .hint(
                Hint::run(Cmd::new("close").session(&self.session))
                    .why("then connect again")
                    .or(format!(
                        "use another session: {}",
                        Cmd::new("connect")
                            .arg(&req.endpoint)
                            .raw("--session <name>")
                    )),
            ));
        }

        let url = endpoint::websocket_url(&endpoint, timeout).await?;
        let failed = |why: String| {
            ErrorInfo::new(
                ErrorKind::LaunchFailed,
                format!("cannot connect to {url}: {why}"),
            )
            .hint(Hint::text(
                "check that the browser runs with --remote-debugging-port and that the URL is current",
            ))
        };
        let transport = tokio::time::timeout(timeout, WsTransport::connect(&url))
            .await
            .map_err(|_| failed("no answer".into()))?
            .map_err(|e| failed(e.to_string()))?;
        let conn = Connection::with_options(
            transport,
            ConnectionOptions {
                call_timeout: timeout,
                ..ConnectionOptions::default()
            },
        );
        let version = conn
            .browser()
            .call(cdp_browser::GetVersionParams::default())
            .await
            .map_err(|e| failed(e.to_string()))?;
        let info = BrowserInfo {
            kind: BrowserKind::Connected,
            product: version.product,
            path: None,
            source: None,
            pid: None,
            headless: None,
            profile: None,
            chrome_args: Vec::new(),
            endpoint: Some(req.endpoint.clone()),
        };
        let spec = Spec::Connected {
            endpoint: req.endpoint.clone(),
        };
        let running = self.install(conn, info, spec, None);
        let running = self.init_tabs(running, timeout).await?;
        Ok(Opened {
            data: OpenData {
                session: self.session.clone(),
                started: true,
                browser: running.info.clone(),
                page: None,
            },
            warnings,
        })
    }

    /// Makes a new browser the running one and watches for it going away.
    fn install(
        self: &Arc<Self>,
        conn: Connection,
        info: BrowserInfo,
        spec: Spec,
        launched: Option<Launched>,
    ) -> Arc<Running> {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let stop = Arc::new(Notify::new());
        let running = Arc::new(Running {
            generation,
            conn: conn.clone(),
            info,
            spec,
            started: Instant::now(),
            tabs: Tabs::start(&conn, self.settings.clone()),
            launched: Mutex::new(launched),
            stop: stop.clone(),
            mac: std::sync::OnceLock::new(),
            sourcemaps: observe::SourceMaps::default(),
        });
        *self.lock_state() = State::Running(running.clone());
        let engine = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::select! {
                reason = conn.closed() => {
                    drop(conn);
                    if let Some(engine) = engine.upgrade() {
                        engine.browser_gone(generation, &reason).await;
                    }
                }
                () = stop.notified() => {}
            }
        });
        running
    }

    /// The browser went away on its own: record why and clean up.
    async fn browser_gone(&self, generation: u64, reason: &str) {
        let running = match &*self.lock_state() {
            State::Running(r) if r.generation == generation => r.clone(),
            _ => return,
        };
        let launched = running
            .launched
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let message = match launched {
            Some(launched) => {
                let last = launched
                    .stderr_tail()
                    .into_iter()
                    .rev()
                    .find(|l| !l.trim().is_empty());
                // Kills what is left of its process group and deletes a
                // temporary profile.
                launched.close().await;
                match last {
                    Some(line) => format!("Chrome exited unexpectedly: {}", line.trim()),
                    None => "Chrome exited unexpectedly".to_owned(),
                }
            }
            None => format!("the connection to the browser closed: {reason}"),
        };
        tracing::warn!("{message}");
        let mut state = self.lock_state();
        if matches!(&*state, State::Running(r) if r.generation == generation) {
            *state = State::Crashed(Box::new(Crash {
                reason: message,
                info: running.info.clone(),
            }));
        }
    }

    /// Errors when `req` asks for options the running browser was not
    /// started with.
    fn check_fits(&self, running: &Running, req: &OpenRequest) -> Result<(), ErrorInfo> {
        let mut differences = Vec::new();
        match &running.spec {
            Spec::Launched {
                chrome,
                headed,
                profile,
                args,
            } => {
                let asked_chrome = req
                    .chrome
                    .as_ref()
                    .map(|c| c.path.clone())
                    .or_else(|| req.chrome_flag.clone());
                if req.chrome_flag.is_some()
                    && asked_chrome.is_some_and(|asked| !same_file(&asked, chrome))
                {
                    differences.push("--chrome");
                }
                if req.headed.is_some_and(|h| h != *headed) {
                    differences.push("--headed");
                }
                if req.profile.as_ref().is_some_and(|p| p != profile) {
                    differences.push(match req.profile {
                        Some(ProfileChoice::Dir(_)) => "--user-data-dir",
                        _ => "--profile",
                    });
                }
                if !req.chrome_args.is_empty() && req.chrome_args != *args {
                    differences.push("--chrome-arg");
                }
            }
            Spec::Connected { .. } => {
                let launch_options = req.chrome_flag.is_some()
                    || req.headed.is_some()
                    || req.profile.is_some()
                    || !req.chrome_args.is_empty();
                if launch_options {
                    differences.push("launch options (it is connected to a browser)");
                }
            }
        }
        if differences.is_empty() {
            return Ok(());
        }
        Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!(
                "session {} already runs a browser with other {}",
                self.session,
                if differences.len() == 1 && differences[0].starts_with("launch") {
                    differences[0].to_owned()
                } else {
                    format!("options: {}", differences.join(", "))
                }
            ),
        )
        .note("running", describe(&running.info))
        .hint(
            Hint::run(
                Cmd::new("open")
                    .session(&self.session)
                    .switch("restart")
                    .raw("…"),
            )
            .why("closes it and starts one with the new options")
            .or(format!(
                "use another session: {}",
                Cmd::new("open").raw("--session <name>").raw("…")
            )),
        )
        .details(serde_json::json!({ "differences": differences, "running": running.info })))
    }

    /// Adds a script that runs before any of a document's own, in every
    /// tab, popup and frame from now on, including those of a browser
    /// started later. Plan 07's init scripts build on it.
    pub async fn add_init_script(&self, source: &str, timeout: Duration) -> Result<(), ErrorInfo> {
        let running = match &*self.lock_state() {
            State::Running(running) => Some(running.clone()),
            State::Crashed(_) | State::Empty => None,
        };
        if let Some(running) = running {
            running
                .tabs
                .add_init_script(source, timeout)
                .await
                .map_err(|e| from_cdp(&e, &self.session))
        } else {
            self.settings.add_init_script(source);
            Ok(())
        }
    }

    /// `status`: the browser and its tabs. Takes no tab lock.
    pub fn status(&self) -> EngineStatus {
        let state = match &*self.lock_state() {
            State::Running(running) => Ok(running.clone()),
            State::Crashed(crash) => Err(Some((**crash).clone())),
            State::Empty => Err(None),
        };
        match state {
            Ok(running) => EngineStatus {
                state: SessionState::Running,
                browser: Some(running.info.clone()),
                crash: None,
                tabs: Some(running.tabs.count()),
                uptime: Some(running.started.elapsed()),
            },
            Err(Some(crash)) => EngineStatus {
                state: SessionState::Crashed,
                browser: Some(crash.info),
                crash: Some(crash.reason),
                tabs: None,
                uptime: None,
            },
            Err(None) => EngineStatus {
                state: SessionState::Idle,
                browser: None,
                crash: None,
                tabs: None,
                uptime: None,
            },
        }
    }

    /// `close`: shuts a launched browser down (it has exited when this
    /// returns) or detaches from a connected one.
    pub async fn close(&self) -> Closed {
        let _lifecycle = self.lifecycle.lock().await;
        let previous = std::mem::replace(&mut *self.lock_state(), State::Empty);
        match previous {
            State::Running(running) => self.shut_down(running).await,
            State::Crashed(_) | State::Empty => Closed::None,
        }
    }

    async fn shut_down(&self, running: Arc<Running>) -> Closed {
        running.stop.notify_one();
        {
            let mut state = self.lock_state();
            if matches!(&*state, State::Running(r) if r.generation == running.generation) {
                *state = State::Empty;
            }
        }
        let launched = running
            .launched
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if launched.is_none() {
            // A connected browser keeps running: give it its downloads back.
            running.tabs.release(Duration::from_secs(2)).await;
        }
        drop(running);
        match launched {
            Some(launched) => {
                launched.close().await;
                Closed::Closed
            }
            None => Closed::Detached,
        }
    }
}

/// The info `open` and `status` show for a browser we launched.
fn launched_info(chrome: &Chrome, launched: &Launched, options: &LaunchOptions) -> BrowserInfo {
    let (kind, name) = match &options.profile {
        ProfileChoice::Ephemeral => ("temporary", None),
        ProfileChoice::Named(name) => ("named", Some(name.clone())),
        ProfileChoice::Dir(_) => ("dir", None),
    };
    BrowserInfo {
        kind: BrowserKind::Launched,
        product: launched.product().to_owned(),
        path: Some(chrome.path.display().to_string()),
        source: Some(chrome.source.describe()),
        pid: Some(launched.pid()),
        headless: Some(!options.headed),
        profile: Some(ProfileInfo {
            kind: kind.into(),
            name,
            dir: launched.profile_dir().display().to_string(),
        }),
        chrome_args: options.args.clone(),
        endpoint: None,
    }
}

/// One line about a browser: `HeadlessChrome/154.0.8037.57, headless,
/// temporary profile, pid 1234`.
pub fn describe(info: &BrowserInfo) -> String {
    let mut parts = vec![info.product.clone()];
    match info.kind {
        BrowserKind::Launched => {
            if let Some(headless) = info.headless {
                parts.push(if headless { "headless" } else { "headed" }.into());
            }
            if let Some(profile) = &info.profile {
                parts.push(match (&profile.name, profile.kind.as_str()) {
                    (Some(name), _) => format!("profile {name}"),
                    (None, "temporary") => "temporary profile".into(),
                    (None, _) => format!("profile {}", profile.dir),
                });
            }
            if !info.chrome_args.is_empty() {
                parts.push(format!("chrome args {}", info.chrome_args.join(" ")));
            }
            if let Some(pid) = info.pid {
                parts.push(format!("pid {pid}"));
            }
        }
        BrowserKind::Connected => {
            if let Some(endpoint) = &info.endpoint {
                parts.push(format!("connected to {endpoint}"));
            }
        }
    }
    parts.join(", ")
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    a == b
        || match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}
