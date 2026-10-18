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

use engine::{Engine, OpenInitScript, OpenRequest};
use protocol::rpc::{Request, Response, RpcError};
use protocol::schema::{ClosedSession, SessionState, StatusData};
use protocol::{ErrorInfo, ErrorKind, Output};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::Notify;

use crate::history::{self, History};
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

/// The calls `context new` makes in its context before it answers
/// (`test`'s setup, plan 22.2): the settings of a context, no more.
const SETUP_METHODS: [&str; 3] = [rpc::MOCK_USE, rpc::EMULATE, rpc::INIT_SCRIPT_ADD];

/// How long working out a ref's stable locator for the history may take.
const STABLE_WAIT: Duration = Duration::from_secs(2);

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
        history: Mutex::new(History::default()),
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
    /// The commands the session ran (plan 11.8).
    history: Mutex<History>,
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
        let req = match protocol::rpc::decode_request(line) {
            Ok(req) => req,
            Err(error) => return (Response::err(None, error), false),
        };
        let _busy = self.busy(&req.method);
        let id = req.id;
        let mut exit = false;
        // `--context`: the engine picks the context's tab and settings.
        let context = req.context.clone();
        let line = req.line.clone().filter(|_| req.method != rpc::HISTORY);
        // Only the commands a scenario keeps need their refs' locators.
        let kept = matches!(
            req.method.as_str(),
            rpc::ACT
                | rpc::FILL_FORM
                | rpc::EXPECT
                | rpc::WAIT
                | rpc::EXPECT_SNAPSHOT
                | rpc::EXPECT_SCREENSHOT
        );
        let root = req.root.clone().map(std::path::PathBuf::from);
        let testid = req.testid_attribute.clone();
        let wanted = req
            .fingerprint
            .clone()
            .map(|locator| (locator, testid.clone()));
        // A `fill`, `type` or `fill-form` kept in the history: what the
        // page's password fields hold after it, which a repro bundle must
        // not.
        let typing = line.is_some()
            && (req.method == rpc::FILL_FORM
                || (req.method == rpc::ACT
                    && matches!(req.params["action"]["kind"].as_str(), Some("fill" | "type"))));
        let tab = req.params["tab"].as_str().map(str::to_owned);
        let (refs, (result, fingerprint), passwords) = engine::in_context(
            context.clone(),
            Box::pin(engine::in_root(root, async {
                // A ref's element may be gone once the command ran.
                let refs = match &line {
                    Some(words) if kept => self.stable_locators(words, testid.as_deref()).await,
                    _ => Vec::new(),
                };
                let call = engine::noting(self.call(req, &mut exit));
                let ((result, notices), fingerprint) = Box::pin(self.engine.fingerprinting(
                    wanted,
                    call,
                    |(result, _)| result.is_ok(),
                ))
                .await;
                let passwords = if typing {
                    self.password_values(tab.as_deref()).await
                } else {
                    Vec::new()
                };
                (
                    refs,
                    (with_notices(result, notices), fingerprint),
                    passwords,
                )
            })),
        )
        .await;
        if let Some(words) = line {
            let error = result.as_ref().err().map(|e| {
                let kind = e.data.as_ref().map_or_else(
                    || "ProtocolError".to_owned(),
                    |info| {
                        serde_json::to_value(info.kind)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_default()
                    },
                );
                // The message with its notes (`expected: …`, `actual: …`),
                // which say what went wrong.
                let mut message = e.message.clone();
                for note in e.data.iter().flat_map(|info| &info.notes) {
                    message.push_str("\n  ");
                    message.push_str(&note.label);
                    message.push_str(": ");
                    message.push_str(&note.text);
                }
                (kind, message)
            });
            let redacted = crate::history::redact(&words, &passwords);
            lock_ignoring_poison(&self.history).push(words, context, error, refs, redacted);
        }
        let mut response = match result {
            Ok(value) => Response::ok(id, value),
            Err(error) => Response::err(Some(id), error),
        };
        response.fingerprint = fingerprint.map(Box::new);
        (response, exit)
    }

    /// The values of the password fields of tab `tab` (the active one if
    /// `None`), through open shadow roots: what a `fill`, `type` or
    /// `fill-form` may just have typed.
    async fn password_values(&self, tab: Option<&str>) -> Vec<String> {
        const VALUES: &str = "(() => { const out = []; \
             const walk = (root) => { for (const e of root.querySelectorAll('*')) { \
               if (e.localName === 'input' && e.type === 'password' && e.value) out.push(e.value); \
               if (e.shadowRoot) walk(e.shadowRoot); } }; \
             walk(document); return out; })()";
        self.engine
            .evaluate(
                tab,
                None,
                engine::World::Isolated,
                VALUES,
                Duration::from_secs(2),
            )
            .await
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }

    /// The stable locators of the refs `words` name, in the tab its
    /// `--tab` names; `testid=` ones read `testid` (the CLI's configured
    /// attribute), else `data-testid`.
    async fn stable_locators(
        &self,
        words: &[String],
        testid: Option<&str>,
    ) -> Vec<protocol::schema::RefLocator> {
        let mut refs: Vec<String> = Vec::new();
        for w in words {
            if engine::is_ref(w) && !refs.contains(w) {
                refs.push(w.clone());
            }
        }
        if refs.is_empty() || self.engine.state() == SessionState::Idle {
            return Vec::new();
        }
        let tab = history::flag(words, "tab");
        let testid = history::flag(words, "testid-attribute").or(testid);
        self.engine
            .stable_locators(tab, &refs, testid, STABLE_WAIT)
            .await
    }

    /// Makes `setup`'s calls in the context `context new` made (`made`),
    /// in order, until one fails; `made` gets how each went, as `setup`.
    async fn set_up(&self, mut made: Output, setup: Vec<engine::SetupCall>) -> Output {
        let context = made.data["context"]["id"].as_str().map(str::to_owned);
        let mut done: Vec<engine::SetupDone> = Vec::new();
        for call in setup {
            let started = Instant::now();
            let result = if SETUP_METHODS.contains(&call.method.as_str()) {
                let req = Request::new(0, &call.method, call.params).in_context(context.clone());
                let mut exit = false;
                let answer =
                    engine::in_context(context.clone(), Box::pin(self.call(req, &mut exit))).await;
                answer
                    .map_err(protocol::rpc::RpcError::into_error_info)
                    .and_then(|value| {
                        serde_json::from_value::<Output>(value).map_err(|e| {
                            ErrorInfo::new(ErrorKind::ProtocolError, format!("bad answer: {e}"))
                        })
                    })
            } else {
                Err(ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!("{} cannot be part of a context's setup", call.method),
                ))
            };
            let ms = millis(started.elapsed());
            let failed = result.is_err();
            let (output, error) = match result {
                Ok(output) => (Some(output), None),
                Err(error) => (None, Some(error)),
            };
            done.push(engine::SetupDone { ms, output, error });
            if failed {
                break;
            }
        }
        made.data["setup"] = json!(done);
        made
    }

    /// Runs one call; sets `exit` when the daemon should exit after
    /// answering.
    async fn call(&self, req: Request, exit: &mut bool) -> Result<Value, RpcError> {
        match req.method.as_str() {
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
                *exit = true;
                Ok(json!(self.close_session().await))
            }
            rpc::CONTEXT_NEW => match params::<engine::ContextNewRequest>(req.params) {
                Ok(mut p) => {
                    let setup = std::mem::take(&mut p.setup);
                    let (ahead, timeout_ms) = (p.ahead, p.timeout_ms);
                    let made = match self.engine.context_new(p).await {
                        Ok(out) if !setup.is_empty() => Ok(self.set_up(out, setup).await),
                        made => made,
                    };
                    if ahead {
                        self.engine.make_ahead(timeout_ms);
                    }
                    output(made)
                }
                Err(e) => Err(e),
            },
            rpc::CONTEXT_LIST => match params(req.params) {
                Ok(p) => output(self.engine.context_list(p).await),
                Err(e) => Err(e),
            },
            rpc::CONTEXT_CLOSE => match params(req.params) {
                Ok(p) => output(self.engine.context_close(p).await),
                Err(e) => Err(e),
            },
            rpc::TABS => match params(req.params) {
                Ok(p) => output(self.engine.tabs(p).await),
                Err(e) => Err(e),
            },
            rpc::FRAMES => match params(req.params) {
                Ok(p) => output(self.engine.list_frames(p).await),
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
            rpc::EXPECT => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.expect(p)).await),
                Err(e) => Err(e),
            },
            rpc::MARK => match params(req.params) {
                Ok(p) => output(self.engine.mark(p).await),
                Err(e) => Err(e),
            },
            rpc::EXPECT_SNAPSHOT => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.expect_snapshot(p)).await),
                Err(e) => Err(e),
            },
            rpc::EXPECT_SCREENSHOT => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.expect_screenshot(p)).await),
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
            rpc::LOCATOR => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.locator(p)).await),
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
            rpc::ISSUES => match params(req.params) {
                Ok(p) => output(self.engine.issues(p).await),
                Err(e) => Err(e),
            },
            rpc::AUDIT => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.audit(p)).await),
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
            rpc::NETWORK_FRAMES => match params(req.params) {
                Ok(p) => output(self.engine.network_frames(p).await),
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
                Ok(p) => output(self.engine.routes(&p)),
                Err(e) => Err(e),
            },
            rpc::UNROUTE => match params(req.params) {
                Ok(p) => output(self.engine.unroute(p).await),
                Err(e) => Err(e),
            },
            rpc::MOCK_USE => match params(req.params) {
                Ok(p) => output(self.engine.mock_use(p).await),
                Err(e) => Err(e),
            },
            rpc::MOCK_OFF => match params(req.params) {
                Ok(p) => output(self.engine.mock_off(p).await),
                Err(e) => Err(e),
            },
            rpc::MOCK_LIST => match params(req.params) {
                Ok(p) => output(self.engine.mock_list(&p)),
                Err(e) => Err(e),
            },
            rpc::MOCK_LOG => match params(req.params) {
                Ok(p) => output(self.engine.mock_log(&p)),
                Err(e) => Err(e),
            },
            rpc::MOCK_RESET => output(self.engine.mock_reset()),
            rpc::MOCK_STATE => match params(req.params) {
                Ok(p) => output(self.engine.mock_state(&p)),
                Err(e) => Err(e),
            },
            rpc::MOCK_SEED => match params(req.params) {
                Ok(p) => output(self.engine.mock_seed(&p)),
                Err(e) => Err(e),
            },
            rpc::MOCK_EXPLAIN => match params(req.params) {
                Ok(p) => output(self.engine.mock_explain(&p)),
                Err(e) => Err(e),
            },
            rpc::MOCK_WS_SEND => match params(req.params) {
                Ok(p) => output(self.engine.mock_ws_send(p).await),
                Err(e) => Err(e),
            },
            rpc::MOCK_SSE_SEND => match params(req.params) {
                Ok(p) => output(self.engine.mock_sse_send(p).await),
                Err(e) => Err(e),
            },
            rpc::HTTP => match params(req.params) {
                Ok(p) => output(self.engine.http(p).await),
                Err(e) => Err(e),
            },
            rpc::INIT_SCRIPT_ADD => match params(req.params) {
                Ok(p) => output(self.engine.init_script_add(p).await),
                Err(e) => Err(e),
            },
            rpc::INIT_SCRIPT_LIST => match params(req.params) {
                Ok(p) => output(self.engine.init_script_list(&p)),
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
            rpc::EXTENSIONS => match params(req.params) {
                Ok(p) => output(self.engine.extensions(p).await),
                Err(e) => Err(e),
            },
            rpc::COOKIES => match params(req.params) {
                Ok(p) => output(self.engine.cookies(p).await),
                Err(e) => Err(e),
            },
            rpc::STORAGE => match params(req.params) {
                Ok(p) => output(self.engine.storage(p).await),
                Err(e) => Err(e),
            },
            rpc::STATE_SAVE => match params(req.params) {
                Ok(p) => output(self.engine.state_save(p).await),
                Err(e) => Err(e),
            },
            rpc::STATE_LOAD => match params(req.params) {
                Ok(p) => output(self.engine.state_load(p).await),
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
            rpc::HISTORY => {
                let clear = req.params.get("clear").and_then(Value::as_bool) == Some(true);
                let mut history = lock_ignoring_poison(&self.history);
                let data = history.data();
                if clear {
                    history.clear();
                }
                Ok(json!(Output::typed(&data, String::new())))
            }
            rpc::HEAL_MATCH => match params(req.params) {
                Ok(p) => output(self.engine.heal_match(p).await),
                Err(e) => Err(e),
            },
            rpc::REPRO_START => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.repro_start(p)).await),
                Err(e) => Err(e),
            },
            rpc::REPRO_INFO => output(self.engine.repro_info()),
            rpc::RECORD_START => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.record_start(p)).await),
                Err(e) => Err(e),
            },
            rpc::RECORD_STATUS => output(self.engine.record_status()),
            rpc::RECORD_STOP => match params(req.params) {
                Ok(p) => output(self.engine.record_stop(p).await),
                Err(e) => Err(e),
            },
            rpc::HAR_EXPORT => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.har_export(p)).await),
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
            rpc::DEBUG_LOG => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.debug_log(p)).await),
                Err(e) => Err(e),
            },
            rpc::DEBUG_LIST => match params(req.params) {
                Ok(p) => output(self.engine.debug_list(p)),
                Err(e) => Err(e),
            },
            rpc::DEBUG_CLEAR => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.debug_clear(p)).await),
                Err(e) => Err(e),
            },
            rpc::DEBUG_LOCALS => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.debug_locals(p)).await),
                Err(e) => Err(e),
            },
            rpc::SOURCE => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.source(p)).await),
                Err(e) => Err(e),
            },
            rpc::WHY => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.why(p)).await),
                Err(e) => Err(e),
            },
            rpc::FORMS => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.forms(p)).await),
                Err(e) => Err(e),
            },
            rpc::FILL_FORM => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.fill_form(p)).await),
                Err(e) => Err(e),
            },
            rpc::COMPARE => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.compare(p)).await),
                Err(e) => Err(e),
            },
            rpc::COMPARE_CAPTURE => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.compare_capture(p)).await),
                Err(e) => Err(e),
            },
            rpc::COMPARE_NAME => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.compare_name(p)).await),
                Err(e) => Err(e),
            },
            rpc::EXPLORE => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.explore(p)).await),
                Err(e) => Err(e),
            },
            rpc::LAYOUT => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.layout(p)).await),
                Err(e) => Err(e),
            },
            rpc::COMPONENTS => match params(req.params) {
                Ok(p) => output(Box::pin(self.engine.components(p)).await),
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
                            *exit = true;
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
        }
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
            replaced_daemon: false,
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
            worker_events_dropped: (st.worker_events_dropped > 0)
                .then_some(st.worker_events_dropped),
            held_keys: st.held_keys,
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
    if newer.node_stacks.is_some() {
        launch.node_stacks = newer.node_stacks;
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
    if newer.downloads_dir.is_some() {
        launch.downloads_dir.clone_from(&newer.downloads_dir);
    }
    if newer.device.is_some() {
        launch.device.clone_from(&newer.device);
    }
    launch.init_scripts = OpenInitScript::merged(&launch.init_scripts, &newer.init_scripts);
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Reads a method's params; `null` counts as `{}`.
fn params<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    protocol::rpc::decode_params(params)
}

/// A command's result as an RPC result.
/// Adds what a call noted besides its output (the hot updates that
/// reached its tab since the tab's previous command) to its result: as
/// warnings and events of an output, as notes of an error.
fn with_notices(
    result: Result<Value, RpcError>,
    notices: engine::Notices,
) -> Result<Value, RpcError> {
    if notices.is_empty() {
        return result;
    }
    match result {
        Ok(mut value) => {
            if let Some(output) = value.as_object_mut()
                && output.contains_key("data")
                && output.contains_key("text")
            {
                let mut push = |key: &str, items: Vec<Value>| {
                    if let Some(list) = output
                        .entry(key)
                        .or_insert_with(|| Value::Array(Vec::new()))
                        .as_array_mut()
                    {
                        list.extend(items);
                    }
                };
                push(
                    "warnings",
                    notices.warnings.into_iter().map(Value::String).collect(),
                );
                push(
                    "events",
                    notices
                        .events
                        .iter()
                        .filter_map(|e| serde_json::to_value(e).ok())
                        .collect(),
                );
            }
            Ok(value)
        }
        Err(mut error) => {
            if let Some(info) = error.data.take() {
                let info = notices
                    .warnings
                    .into_iter()
                    .fold(*info, |info, line| info.note("meanwhile", line));
                error.data = Some(Box::new(info));
            }
            Err(error)
        }
    }
}

fn output(result: Result<Output, ErrorInfo>) -> Result<Value, RpcError> {
    match result {
        Ok(output) => serde_json::to_value(output)
            .map_err(|e| RpcError::new(RpcError::INVALID_REQUEST, e.to_string())),
        Err(info) => Err(RpcError::command(info)),
    }
}

/// Whether the call on `line` is dropped when its client goes away.
fn stops_with_client(line: &str) -> bool {
    protocol::rpc::decode_request(line).is_ok_and(|req| req.method == rpc::EXPLORE)
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
    // Lines that came while a call that stops with its client ran.
    let mut pending: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    loop {
        let line = match pending.pop_front() {
            Some(line) => line,
            None => match lines.next_line().await {
                Ok(Some(line)) => line,
                _ => break,
            },
        };
        if line.trim().is_empty() {
            continue;
        }
        let (response, exit) = if stops_with_client(&line) {
            // `explore` runs for minutes: when its client goes away
            // (Ctrl-C), the call is dropped, and its context closed.
            let dispatch = daemon.dispatch(&line);
            tokio::pin!(dispatch);
            let gone = async {
                loop {
                    match lines.next_line().await {
                        Ok(Some(next)) => pending.push_back(next),
                        _ => return,
                    }
                }
            };
            tokio::select! {
                answer = &mut dispatch => answer,
                () = gone => break,
            }
        } else {
            daemon.dispatch(&line).await
        };
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
    use engine::{DialogPolicy, OpenInitScript, OpenRequest};

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
            downloads_dir: Some("/tmp/dl".into()),
            ..OpenRequest::default()
        };
        keep_settings(&mut launch, &newer);
        assert_eq!(launch.dialogs, Some(DialogPolicy::Dismiss));
        assert_eq!(launch.downloads_dir, newer.downloads_dir);
        assert_eq!(launch.follow_popups, Some(true));
        // Launch options are not settings.
        assert_eq!(launch.headed, Some(true));
    }

    #[test]
    fn devices_and_init_scripts_given_later_are_kept() {
        let script = |source: &str| OpenInitScript {
            source: source.into(),
            file: None,
        };
        let mut launch = OpenRequest {
            init_scripts: vec![script("a()"), script("b()")],
            ..OpenRequest::default()
        };
        let newer = OpenRequest {
            device: Some("iPhone 15".into()),
            init_scripts: vec![script("b()"), script("c()")],
            ..OpenRequest::default()
        };
        keep_settings(&mut launch, &newer);
        assert_eq!(launch.device.as_deref(), Some("iPhone 15"));
        assert_eq!(
            launch.init_scripts,
            vec![script("a()"), script("b()"), script("c()")]
        );
    }
}
