//! The daemon process, `chrome-cdp __daemon --session <name>`.
//!
//! It holds `daemon.lock` for its whole life (that is how clients tell a
//! live daemon from a dead one), listens on the session socket, and answers
//! line-delimited JSON-RPC calls. Each connection runs in its own task, so
//! calls from several CLIs run at the same time; the engine serialises what
//! must not interleave. It exits on `close`, SIGTERM or SIGINT, or after
//! `--idle-timeout` without work, closing Chrome first.

use std::fs::File;
use std::io;
use std::os::unix::fs::OpenOptionsExt as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use engine::{Engine, OpenRequest};
use protocol::rpc::{Request, Response, RpcError};
use protocol::schema::{ClosedSession, SessionState, StatusData};
use protocol::{ErrorInfo, Output};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::Notify;

use crate::meta::Meta;
use crate::output;
use crate::paths::{Env, SessionPaths};
use crate::rpc::{self, HelloResult, OpenParams, StatusParams};
use crate::transport::unix::{self, Stream};

/// The idle timeout until an `open` sets one.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_mins(30);

/// How long a daemon without a browser (its launch failed, or no `open`
/// came) waits for work before it exits.
pub const NO_BROWSER_GRACE: Duration = Duration::from_secs(10);

/// How long a starting daemon waits for another one to let go of the lock
/// (a client probing it holds it for an instant).
const LOCK_WAIT: Duration = Duration::from_secs(2);

/// The exit code of a daemon that found another one running.
pub const EXIT_ALREADY_RUNNING: u8 = 3;

/// How the daemon was started.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    pub session: String,
    /// The build of the CLI that started it (see `client`).
    pub build: String,
    /// [`NO_BROWSER_GRACE`], shorter in tests.
    pub no_browser_grace: Duration,
}

/// Runs the daemon to the end and returns its exit code.
pub fn run(opts: &ServeOptions) -> u8 {
    // One thread: Chrome is launched from it and, on Linux, dies with the
    // thread that spawned it (PR_SET_PDEATHSIG), so it must be one that
    // lives as long as the daemon.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            tracing::error!("cannot start the runtime: {e}");
            return 1;
        }
    };
    match runtime.block_on(serve(opts)) {
        Ok(code) => code,
        Err(e) => {
            tracing::error!("{e}");
            1
        }
    }
}

async fn serve(opts: &ServeOptions) -> Result<u8, String> {
    let env = Env::current();
    crate::paths::check_name(&opts.session).map_err(|e| e.message)?;
    let paths = SessionPaths::new(&env, &opts.session);
    paths.create(&env).map_err(|e| e.message)?;
    let Some(_lock) = lock(&paths).map_err(|e| format!("cannot lock: {e}"))? else {
        tracing::warn!("another daemon runs session {}; exiting", opts.session);
        return Ok(EXIT_ALREADY_RUNNING);
    };
    let pid = std::process::id();
    std::fs::write(&paths.pid, format!("{pid}\n"))
        .map_err(|e| format!("cannot write the pid: {e}"))?;
    let engine = Engine::new(
        &opts.session,
        Some(paths.chrome_log.clone()),
        Some(paths.dir.join("downloads")),
    );
    // Screenshots and PDFs go to `shots/` and `pdfs/` next to downloads.
    engine.set_output_dir(paths.dir.clone());
    let daemon = Arc::new(Daemon {
        engine,
        build: opts.build.clone(),
        pid,
        started_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        idle_timeout: Mutex::new(DEFAULT_IDLE_TIMEOUT),
        no_browser_grace: opts.no_browser_grace,
        last_activity: Mutex::new(Instant::now()),
        in_flight: AtomicUsize::new(0),
        connections: AtomicUsize::new(0),
        shutdown: Notify::new(),
        opened_with: Mutex::new(Opened::default()),
        paths,
    });
    daemon.write_meta();
    let listener = unix::bind(&daemon.paths.sock)
        .map_err(|e| format!("cannot listen on {}: {e}", daemon.paths.sock.display()))?;
    tracing::info!(pid, build = %opts.build, "session {} listening", opts.session);

    let mut term = signal(SignalKind::terminate()).map_err(|e| e.to_string())?;
    let mut int = signal(SignalKind::interrupt()).map_err(|e| e.to_string())?;
    // Registered so a closing terminal cannot kill the daemon.
    let mut hup = signal(SignalKind::hangup()).map_err(|e| e.to_string())?;
    loop {
        let wake = match daemon.idle_deadline() {
            Some(deadline) if deadline <= Instant::now() => {
                tracing::info!("idle; shutting down");
                break;
            }
            Some(deadline) => deadline,
            // Busy: check again soon, as nothing wakes this loop when the
            // last call or connection ends.
            None => Instant::now() + Duration::from_secs(1),
        };
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    tokio::spawn(handle(daemon.clone(), stream));
                }
                Err(e) => tracing::warn!("accept failed: {e}"),
            },
            () = daemon.shutdown.notified() => break,
            () = tokio::time::sleep_until(wake.into()) => {}
            _ = term.recv() => { tracing::info!("SIGTERM; shutting down"); break; }
            _ = int.recv() => { tracing::info!("SIGINT; shutting down"); break; }
            _ = hup.recv() => tracing::info!("ignoring SIGHUP"),
        }
    }

    // No new clients from here on.
    drop(listener);
    let _ = std::fs::remove_file(&daemon.paths.sock);
    let closed = daemon.engine.close().await;
    tracing::info!("browser {}; daemon exiting", closed.as_str());
    let _ = std::fs::remove_file(&daemon.paths.meta);
    let _ = std::fs::remove_file(&daemon.paths.pid);
    Ok(0)
}

/// Takes `daemon.lock`; `None` if another daemon holds it.
fn lock(paths: &SessionPaths) -> io::Result<Option<File>> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&paths.daemon_lock)?;
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        }
    }
}

#[derive(Debug)]
struct Daemon {
    engine: Arc<Engine>,
    paths: SessionPaths,
    build: String,
    pid: u32,
    started_at: u64,
    /// Zero means never.
    idle_timeout: Mutex<Duration>,
    no_browser_grace: Duration,
    last_activity: Mutex<Instant>,
    in_flight: AtomicUsize,
    /// Open client connections. A CLI keeps its connection while it
    /// finds or downloads Chrome before `open`, so none counts as idle.
    connections: AtomicUsize,
    shutdown: Notify,
    opened_with: Mutex<Opened>,
}

/// How the current browser was opened, for `meta.json`.
#[derive(Debug, Clone, Default)]
struct Opened {
    browser: Option<protocol::schema::BrowserInfo>,
    launch: Option<OpenRequest>,
    endpoint: Option<String>,
}

/// Counts a call in flight; on drop, marks the end of work.
struct Busy<'a> {
    daemon: &'a Daemon,
    work: bool,
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        if self.work {
            *lock_ignoring_poison(&self.daemon.last_activity) = Instant::now();
        }
        self.daemon.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

fn lock_ignoring_poison<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Daemon {
    fn busy(&self, method: &str) -> Busy<'_> {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        // Probes (`hello`, `status`, `close`) are not work: `sessions`
        // asking every daemon must not keep them all alive.
        let work = !matches!(method, rpc::HELLO | rpc::STATUS | rpc::CLOSE);
        if work {
            *lock_ignoring_poison(&self.last_activity) = Instant::now();
        }
        Busy { daemon: self, work }
    }

    /// When the daemon should exit if nothing happens; `None` for never.
    fn idle_deadline(&self) -> Option<Instant> {
        if self.in_flight.load(Ordering::SeqCst) > 0 || self.connections.load(Ordering::SeqCst) > 0
        {
            return None;
        }
        let limit = match self.engine.state() {
            SessionState::Idle => self.no_browser_grace,
            _ => *lock_ignoring_poison(&self.idle_timeout),
        };
        if limit.is_zero() {
            return None;
        }
        Some(*lock_ignoring_poison(&self.last_activity) + limit)
    }

    fn write_meta(&self) {
        let opened = lock_ignoring_poison(&self.opened_with).clone();
        let meta = Meta {
            session: self.paths.name.clone(),
            home: self.paths.home.clone(),
            build: self.build.clone(),
            daemon_pid: self.pid,
            started_at: self.started_at,
            idle_timeout_ms: millis(*lock_ignoring_poison(&self.idle_timeout)),
            browser: opened.browser,
            launch: opened.launch,
            endpoint: opened.endpoint,
        };
        if let Err(e) = meta.write(&self.paths.meta) {
            tracing::warn!("cannot write {}: {e}", self.paths.meta.display());
        }
    }

    fn set_idle_timeout(&self, ms: Option<u64>) {
        if let Some(ms) = ms {
            *lock_ignoring_poison(&self.idle_timeout) = Duration::from_millis(ms);
        }
    }

    /// Answers one request line; `true` when the daemon should exit after
    /// sending the response.
    async fn dispatch(&self, line: &str) -> (Response, bool) {
        let req: Request = match serde_json::from_str(line) {
            Ok(req) => req,
            Err(e) => {
                let error = RpcError::new(RpcError::PARSE_ERROR, format!("not a request: {e}"));
                return (Response::err(None, error), false);
            }
        };
        let _busy = self.busy(&req.method);
        let id = req.id;
        let mut exit = false;
        let result = match req.method.as_str() {
            rpc::HELLO => Ok(json!(HelloResult {
                build: self.build.clone(),
                session: self.paths.name.clone(),
                pid: self.pid,
                state: self.engine.state(),
            })),
            rpc::OPEN => self.open(req.params).await,
            rpc::CONNECT => self.connect(req.params).await,
            rpc::CDP => match params(req.params) {
                Ok(p) => output(self.engine.cdp(p).await),
                Err(e) => Err(e),
            },
            rpc::STATUS => match params::<StatusParams>(req.params) {
                Ok(p) => Ok(json!(output::status(&self.status(&p)))),
                Err(e) => Err(e),
            },
            rpc::CLOSE => {
                exit = true;
                Ok(json!(self.close_session().await))
            }
            rpc::TABS => match params(req.params) {
                Ok(p) => output(self.engine.tabs(p).await),
                Err(e) => Err(e),
            },
            rpc::TAB_NEW => match params(req.params) {
                Ok(p) => output(self.engine.tab_new(p).await),
                Err(e) => Err(e),
            },
            rpc::TAB_SELECT => match params(req.params) {
                Ok(p) => output(self.engine.tab_select(p).await),
                Err(e) => Err(e),
            },
            rpc::NAVIGATE => match params(req.params) {
                Ok(p) => output(self.engine.navigate(p).await),
                Err(e) => Err(e),
            },
            rpc::WAIT => match params(req.params) {
                Ok(p) => output(self.engine.wait(p).await),
                Err(e) => Err(e),
            },
            rpc::DIALOG => match params(req.params) {
                Ok(p) => output(self.engine.dialog(p).await),
                Err(e) => Err(e),
            },
            rpc::SNAPSHOT => match params(req.params) {
                Ok(p) => output(self.engine.snapshot(p).await),
                Err(e) => Err(e),
            },
            rpc::FIND => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.find(p)).await),
                Err(e) => Err(e),
            },
            rpc::READ => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.read(p)).await),
                Err(e) => Err(e),
            },
            rpc::ACT => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.act(p)).await),
                Err(e) => Err(e),
            },
            rpc::CLIPBOARD => match params(req.params) {
                Ok(p) => output(self.engine.clipboard(p).await),
                Err(e) => Err(e),
            },
            rpc::CONSOLE => match params(req.params) {
                Ok(p) => output(self.engine.console(p).await),
                Err(e) => Err(e),
            },
            rpc::NETWORK => match params(req.params) {
                Ok(p) => output(self.engine.network(p).await),
                Err(e) => Err(e),
            },
            rpc::NETWORK_SHOW => match params(req.params) {
                Ok(p) => output(self.engine.network_show(p).await),
                Err(e) => Err(e),
            },
            rpc::NETWORK_CLEAR => match params(req.params) {
                Ok(p) => output(self.engine.network_clear(p).await),
                Err(e) => Err(e),
            },
            rpc::ROUTE => match params(req.params) {
                Ok(p) => output(self.engine.route(p).await),
                Err(e) => Err(e),
            },
            rpc::ROUTES => match params(req.params) {
                Ok(p) => output(Ok(self.engine.routes(&p))),
                Err(e) => Err(e),
            },
            rpc::UNROUTE => match params(req.params) {
                Ok(p) => output(self.engine.unroute(p).await),
                Err(e) => Err(e),
            },
            rpc::INIT_SCRIPT_ADD => match params(req.params) {
                Ok(p) => output(self.engine.init_script_add(p).await),
                Err(e) => Err(e),
            },
            rpc::INIT_SCRIPT_LIST => match params(req.params) {
                Ok(p) => output(Ok(self.engine.init_script_list(&p))),
                Err(e) => Err(e),
            },
            rpc::INIT_SCRIPT_REMOVE => match params(req.params) {
                Ok(p) => output(self.engine.init_script_remove(p).await),
                Err(e) => Err(e),
            },
            rpc::EXPOSE => match params(req.params) {
                Ok(p) => output(self.engine.expose(p).await),
                Err(e) => Err(e),
            },
            rpc::EMULATE => match params(req.params) {
                Ok(p) => output(self.engine.emulate(p).await),
                Err(e) => Err(e),
            },
            rpc::PERMISSIONS => match params(req.params) {
                Ok(p) => output(self.engine.permissions(p).await),
                Err(e) => Err(e),
            },
            rpc::SCREENSHOT => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.screenshot(p)).await),
                Err(e) => Err(e),
            },
            rpc::PDF => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.pdf(p)).await),
                Err(e) => Err(e),
            },
            rpc::EVAL => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.eval(p)).await),
                Err(e) => Err(e),
            },
            rpc::HAR_START => match params(req.params) {
                Ok(p) => output(self.engine.har_start(p).await),
                Err(e) => Err(e),
            },
            rpc::HAR_STOP => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.har_stop(p)).await),
                Err(e) => Err(e),
            },
            rpc::TRACE_START => match params(req.params) {
                Ok(p) => output(self.engine.trace_start(p).await),
                Err(e) => Err(e),
            },
            rpc::TRACE_STOP => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.trace_stop(p)).await),
                Err(e) => Err(e),
            },
            rpc::PERF => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.perf(p)).await),
                Err(e) => Err(e),
            },
            rpc::COVERAGE_START => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.coverage_start(p)).await),
                Err(e) => Err(e),
            },
            rpc::COVERAGE_STOP => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.coverage_stop(p)).await),
                Err(e) => Err(e),
            },
            rpc::HIGHLIGHT => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.highlight(p)).await),
                Err(e) => Err(e),
            },
            rpc::INSPECT => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.inspect(p)).await),
                Err(e) => Err(e),
            },
            rpc::DOWNLOADS => match params(req.params) {
                Ok(p) => output(self.engine.downloads(p)),
                Err(e) => Err(e),
            },
            rpc::TAB_CLOSE => match params(req.params) {
                Ok(p) => match self.engine.tab_close(p).await {
                    Ok(closed) => {
                        if closed.close_session {
                            // `tab close --close-browser` on the last tab.
                            exit = true;
                            self.close_session().await;
                        }
                        output(Ok(closed.output))
                    }
                    Err(info) => output(Err(info)),
                },
                Err(e) => Err(e),
            },
            other => Err(RpcError::new(
                RpcError::METHOD_NOT_FOUND,
                format!("no method `{other}`"),
            )),
        };
        let response = match result {
            Ok(value) => Response::ok(id, value),
            Err(error) => Response::err(Some(id), error),
        };
        (response, exit)
    }

    /// Closes the browser; the daemon exits after answering.
    async fn close_session(&self) -> ClosedSession {
        let closed = self.engine.close().await;
        *lock_ignoring_poison(&self.opened_with) = Opened::default();
        self.write_meta();
        ClosedSession {
            session: self.paths.name.clone(),
            was_running: true,
            browser: closed.as_str().into(),
        }
    }

    async fn open(&self, params: Value) -> Result<Value, RpcError> {
        let p: OpenParams = self::params(params)?;
        self.set_idle_timeout(p.idle_timeout_ms);
        let record = OpenRequest {
            chrome: None,
            url: None,
            base_url: None,
            tab: None,
            restart: false,
            // It holds secrets, and a later daemon must not load it again.
            state: None,
            ..p.request.clone()
        };
        let result = self.engine.open(p.request).await;
        if let Ok(opened) = &result {
            let mut with = lock_ignoring_poison(&self.opened_with);
            if opened.data.started || with.browser.is_none() {
                *with = Opened {
                    browser: Some(opened.data.browser.clone()),
                    launch: Some(record),
                    endpoint: None,
                };
            } else if let Some(launch) = &mut with.launch {
                // Session settings given to a running browser apply at
                // once; keep them for an upgrade too.
                keep_settings(launch, &record);
            }
        }
        self.write_meta();
        output(result.map(|o| output::opened(&o)))
    }

    async fn connect(&self, params: Value) -> Result<Value, RpcError> {
        let p: rpc::ConnectParams = self::params(params)?;
        self.set_idle_timeout(p.idle_timeout_ms);
        let endpoint = p.request.endpoint.clone();
        let result = self.engine.connect(p.request).await;
        if let Ok(opened) = &result {
            *lock_ignoring_poison(&self.opened_with) = Opened {
                browser: Some(opened.data.browser.clone()),
                launch: None,
                endpoint: Some(endpoint),
            };
        }
        self.write_meta();
        output(result.map(|o| output::opened(&o)))
    }

    fn status(&self, p: &StatusParams) -> StatusData {
        let st = self.engine.status();
        StatusData {
            session: self.paths.name.clone(),
            state: st.state,
            browser: st.browser,
            crash: st.crash,
            tabs: st.tabs,
            uptime_ms: st.uptime.map(millis),
            daemon_pid: Some(self.pid),
            idle_timeout_ms: Some(millis(*lock_ignoring_poison(&self.idle_timeout))),
            dir: self.paths.dir.display().to_string(),
            logs: p.logs.map(|n| output::log_tail(&self.paths.chrome_log, n)),
        }
    }
}

/// Copies the session settings `newer` gives (not the launch options) into
/// `launch`.
fn keep_settings(launch: &mut OpenRequest, newer: &OpenRequest) {
    if newer.follow_popups.is_some() {
        launch.follow_popups = newer.follow_popups;
    }
    if newer.ignore_https_errors.is_some() {
        launch.ignore_https_errors = newer.ignore_https_errors;
    }
    if newer.bypass_service_worker.is_some() {
        launch.bypass_service_worker = newer.bypass_service_worker;
    }
    if newer.dialogs.is_some() {
        launch.dialogs = newer.dialogs;
    }
    if newer.http_credentials.is_some() {
        launch.http_credentials.clone_from(&newer.http_credentials);
    }
    if newer.console_buffer.is_some() {
        launch.console_buffer = newer.console_buffer;
    }
    if newer.network_buffer.is_some() {
        launch.network_buffer = newer.network_buffer;
    }
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Reads a method's params; `null` counts as `{}`.
fn params<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    let params = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(params)
        .map_err(|e| RpcError::new(RpcError::INVALID_PARAMS, format!("bad params: {e}")))
}

/// A command's result as an RPC result.
fn output(result: Result<Output, ErrorInfo>) -> Result<Value, RpcError> {
    match result {
        Ok(output) => serde_json::to_value(output)
            .map_err(|e| RpcError::new(RpcError::INVALID_REQUEST, e.to_string())),
        Err(info) => Err(RpcError::command(info)),
    }
}

/// Serves one client connection: one request per line, answered in order.
async fn handle(daemon: Arc<Daemon>, stream: Stream) {
    daemon.connections.fetch_add(1, Ordering::SeqCst);
    serve_connection(&daemon, stream).await;
    daemon.connections.fetch_sub(1, Ordering::SeqCst);
}

async fn serve_connection(daemon: &Daemon, stream: Stream) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let (response, exit) = daemon.dispatch(&line).await;
        let mut bytes = serde_json::to_vec(&response).unwrap_or_default();
        bytes.push(b'\n');
        let sent = write.write_all(&bytes).await.is_ok() && write.flush().await.is_ok();
        if exit {
            daemon.shutdown.notify_one();
            break;
        }
        if !sent {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use engine::{DialogPolicy, OpenRequest};

    use super::keep_settings;

    #[test]
    fn settings_for_a_running_browser_are_kept() {
        let mut launch = OpenRequest {
            headed: Some(true),
            follow_popups: Some(true),
            ..OpenRequest::default()
        };
        let newer = OpenRequest {
            headed: Some(false),
            dialogs: Some(DialogPolicy::Dismiss),
            ..OpenRequest::default()
        };
        keep_settings(&mut launch, &newer);
        assert_eq!(launch.dialogs, Some(DialogPolicy::Dismiss));
        assert_eq!(launch.follow_popups, Some(true));
        // Launch options are not settings.
        assert_eq!(launch.headed, Some(true));
    }
}
