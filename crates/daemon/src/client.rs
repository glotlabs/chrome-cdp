//! The CLI's side of the session socket: finding the session's daemon,
//! starting one when there is none, and making calls.
//!
//! - A daemon holds `daemon.lock` while it runs. A lock nobody holds means
//!   no daemon, whatever `sock` and `pid` say; those are then stale and
//!   cleaned up.
//! - Starting one happens under `start.lock`, so of many CLIs that find
//!   no daemon at once, one starts it and the others wait and connect.
//! - Every connection starts with `hello`. A daemon of another build (an
//!   upgrade) is closed and replaced; `meta.json` tells the new one how the
//!   old browser was opened.

use std::fs::File;
use std::os::unix::fs::OpenOptionsExt as _;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use protocol::rpc::{Request, Settle};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, Lines};

use crate::meta::Meta;
use crate::paths::{Env, SessionPaths};
use crate::rpc::{self, Hello, HelloResult};
use crate::transport::unix::{self, ReadHalf, WriteHalf};

/// How long a new daemon gets to answer.
const START_WAIT: Duration = Duration::from_secs(10);
/// How long to wait for another CLI that is starting the daemon.
const START_LOCK_WAIT: Duration = Duration::from_secs(30);
/// How long `hello` may take.
const HELLO_WAIT: Duration = Duration::from_secs(5);
/// How long a closed daemon gets to exit.
const EXIT_WAIT: Duration = Duration::from_secs(15);
/// `daemon.log` is rotated (one `.1` kept) past this size.
const DAEMON_LOG_LIMIT: u64 = 1024 * 1024;

thread_local! {
    /// The words of the command this thread runs, for the session's
    /// history: the next call (other than `hello`) takes them.
    static LINE: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) };
}

thread_local! {
    /// The locator whose element the calls of this thread ask the daemon
    /// to fingerprint (`test`'s steps, plan 18.2), and the last
    /// fingerprint an answer carried.
    static FINGERPRINT: std::cell::RefCell<(Option<String>, Option<protocol::schema::Fingerprint>)> =
        const { std::cell::RefCell::new((None, None)) };
}

/// Makes the calls of this thread ask for the fingerprint of the element
/// `locator` finds (`None`: stop asking), and forgets the last one.
pub fn ask_fingerprint(locator: Option<String>) {
    FINGERPRINT.with(|f| *f.borrow_mut() = (locator, None));
}

/// The fingerprint the last answer on this thread carried, if any.
pub fn take_fingerprint() -> Option<protocol::schema::Fingerprint> {
    FINGERPRINT.with(|f| f.borrow_mut().1.take())
}

thread_local! {
    /// A `test` worker's connection (plan 22.2): while this thread keeps
    /// connections, a [`Client`] that is dropped in good order is kept
    /// here, and the next [`connect`] to the same daemon takes it instead
    /// of connecting and saying hello again.
    static KEPT: std::cell::RefCell<Keeping> = const { std::cell::RefCell::new(Keeping::Off) };
}

/// Whether this thread keeps its connection, and the one it keeps.
#[derive(Debug)]
enum Keeping {
    Off,
    On(Option<Box<Kept>>),
}

/// A connection kept for the next call of the thread.
#[derive(Debug)]
struct Kept {
    paths: SessionPaths,
    build: String,
    hello: HelloResult,
    conn: Conn,
}

/// Makes this thread keep one daemon connection between commands (`test`'s
/// workers, which run on a runtime of their own that outlives them), or
/// stop keeping it, closing the kept one.
pub fn keep_connection(on: bool) {
    let old = KEPT.with(|kept| {
        std::mem::replace(
            &mut *kept.borrow_mut(),
            if on { Keeping::On(None) } else { Keeping::Off },
        )
    });
    drop(old);
}

/// The kept connection to the daemon at `paths` of `build`, if there is one.
fn take_kept(paths: &SessionPaths, build: &str) -> Option<Client> {
    let kept = KEPT
        .try_with(|kept| match &mut *kept.borrow_mut() {
            Keeping::On(slot) => slot.take_if(|k| k.paths.sock == paths.sock && k.build == build),
            Keeping::Off => None,
        })
        .ok()
        .flatten()?;
    // A daemon that was started again since (it crashed, or a step closed
    // the session) is another: a fresh hello says whether it has a
    // browser, which a command that starts one needs to know.
    let pid = std::fs::read_to_string(&kept.paths.pid).ok()?;
    if pid.trim().parse::<u32>().ok()? != kept.hello.pid {
        return None;
    }
    let Kept {
        paths,
        build,
        hello,
        conn,
    } = *kept;
    Some(Client {
        conn: Some(conn),
        session: paths.name.clone(),
        paths,
        build,
        hello,
        context: None,
        reused: true,
        broken: false,
    })
}

/// The project root every call carries (see `Request::root`). It is the
/// process's, not a thread's: `test` runs its jobs on threads of their own.
static ROOT: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// The attribute `testid=` locators read, which every call carries (see
/// `Request::testid_attribute`); process-wide like [`ROOT`].
static TESTID_ATTRIBUTE: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// Sets the project root the calls carry, so the daemon prints places in
/// the page's code as paths relative to it.
pub fn set_project_root(root: Option<String>) {
    *ROOT
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = root;
}

/// Sets the configured `testid_attribute` the calls carry, so the stable
/// locators the daemon keeps for the history (and `history --as-scenario`)
/// use it.
pub fn set_testid_attribute(attribute: Option<String>) {
    *TESTID_ATTRIBUTE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = attribute;
}

thread_local! {
    /// Whether this thread's calls leave the session's history alone
    /// (`test`'s workers, plan 22.2: nothing reads a test session's
    /// history, and keeping it costs the daemon the refs' locators).
    static NO_HISTORY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Makes this thread's calls carry no words for the history (`false`),
/// whatever [`set_history_line`] sets, or carry them again (`true`). A
/// call without words is not kept in the history, and the daemon works
/// out no stable locators for it.
pub fn keep_history(on: bool) {
    NO_HISTORY.with(|off| off.set(!on));
}

thread_local! {
    /// How this thread's calls ask navigations and actions to settle
    /// (`test`'s workers, plan 22.3); `None`: the daemon's default, the
    /// full settle.
    static SETTLE: std::cell::Cell<Option<Settle>> = const { std::cell::Cell::new(None) };
}

/// Makes this thread's calls settle as `settle` says (`None`: as the
/// command line does).
pub fn set_settle(settle: Option<Settle>) {
    SETTLE.with(|s| s.set(settle));
}

/// Runs `f` with this thread's calls asking for the full settle, whatever
/// [`set_settle`] set: a step's own `goto --wait settle` asks for more
/// than a test run's settle.
pub fn full_settle<R>(f: impl FnOnce() -> R) -> R {
    let was = SETTLE.with(|s| s.replace(None));
    let out = f();
    SETTLE.with(|s| s.set(was));
    out
}

/// Sets the words of the command about to run on this thread (`None`
/// for one that is not to be kept in the history). Its first call to a
/// daemon carries them, with the password of `--http-credentials` hidden.
pub fn set_history_line(words: Option<Vec<String>>) {
    let words = words.map(redact);
    LINE.with(|line| *line.borrow_mut() = words);
}

/// Changes the words of the command about to run on this thread, if it
/// is kept in the history (`fill-form` puts the values it read from a
/// file or stdin in place of `--file` and `-`, so its scenario line
/// stands alone).
pub fn edit_history_line(edit: impl FnOnce(&mut Vec<String>)) {
    LINE.with(|line| {
        if let Some(words) = line.borrow_mut().as_mut() {
            edit(words);
        }
    });
}

/// The flags whose value is a secret, which `history` must not show.
const SECRET_FLAGS: [&str; 1] = ["--http-credentials"];

/// `words` with each [`SECRET_FLAGS`] value but `-` (stdin) as `***`.
fn redact(mut words: Vec<String>) -> Vec<String> {
    let mut next_is_secret = false;
    for word in &mut words {
        if std::mem::take(&mut next_is_secret) {
            if word != "-" {
                "***".clone_into(word);
            }
            continue;
        }
        for flag in SECRET_FLAGS {
            if word == flag {
                next_is_secret = true;
            } else if word
                .strip_prefix(flag)
                .is_some_and(|rest| rest.starts_with('=') && rest != "=-")
            {
                *word = format!("{flag}=***");
            }
        }
    }
    words
}

/// The socket of a connection.
#[derive(Debug)]
struct Conn {
    reader: Lines<BufReader<ReadHalf>>,
    writer: WriteHalf,
    next_id: u64,
}

/// A connection to a session's daemon.
#[derive(Debug)]
pub struct Client {
    /// Always there but while a dropped client hands it to [`KEPT`].
    conn: Option<Conn>,
    session: String,
    paths: SessionPaths,
    build: String,
    /// What the daemon said to `hello`.
    pub hello: HelloResult,
    /// `--context`: the browser context every call acts in.
    pub context: Option<String>,
    /// A kept connection not used since: its daemon may have gone, and
    /// its first call connects again once if so.
    reused: bool,
    /// The connection may hold a late answer, or the daemon may be gone
    /// or changed (`close`, `open`): it is not kept.
    broken: bool,
}

impl Drop for Client {
    fn drop(&mut self) {
        if self.broken || self.hello.state != protocol::schema::SessionState::Running {
            return;
        }
        let Some(conn) = self.conn.take() else {
            return;
        };
        let kept = Kept {
            paths: self.paths.clone(),
            build: self.build.clone(),
            hello: self.hello.clone(),
            conn,
        };
        // The thread may be ending; then the connection just closes.
        let _ = KEPT.try_with(|slot| {
            if let Keeping::On(slot @ None) = &mut *slot.borrow_mut() {
                *slot = Some(Box::new(kept));
            }
        });
    }
}

/// How a call's exchange of lines went wrong.
enum Failed {
    /// The connection broke before an answer came.
    Lost(String),
    /// No answer in time.
    Late,
}

impl Client {
    /// Connects and says hello.
    async fn open(paths: &SessionPaths, build: &str) -> Result<Self, String> {
        let conn = Self::connect_socket(paths).await?;
        let mut client = Self {
            conn: Some(conn),
            session: paths.name.clone(),
            paths: paths.clone(),
            build: build.to_owned(),
            hello: HelloResult {
                build: String::new(),
                session: paths.name.clone(),
                pid: 0,
                state: protocol::schema::SessionState::Idle,
            },
            context: None,
            reused: false,
            broken: false,
        };
        let hello = Hello {
            build: build.to_owned(),
        };
        let value = client
            .call(rpc::HELLO, &hello, HELLO_WAIT)
            .await
            .map_err(|e| e.message)?;
        client.hello = serde_json::from_value(value).map_err(|e| format!("bad hello: {e}"))?;
        Ok(client)
    }

    async fn connect_socket(paths: &SessionPaths) -> Result<Conn, String> {
        let stream = unix::connect(&paths.sock)
            .await
            .map_err(|e| format!("cannot connect to {}: {e}", paths.sock.display()))?;
        let (read, writer) = stream.into_split();
        Ok(Conn {
            reader: BufReader::new(read).lines(),
            writer,
            next_id: 1,
        })
    }

    /// Makes one call and waits up to `wait` for the answer.
    pub async fn call<P: Serialize>(
        &mut self,
        method: &str,
        params: &P,
        wait: Duration,
    ) -> Result<Value, ErrorInfo> {
        let reused = std::mem::take(&mut self.reused);
        let mut request = self.request(method, params);
        let mut answer = self.exchange(&request, wait).await;
        if reused && matches!(answer, Err(Failed::Lost(_))) {
            // The kept connection's daemon is gone (restarted, say):
            // connect again, once, and send the call there.
            tracing::debug!(
                "the kept connection to session {} broke; connecting again",
                self.session
            );
            if let Ok(fresh) = Box::pin(Self::open(&self.paths, &self.build))
                .await
                .as_mut()
            {
                self.conn = fresh.conn.take();
                self.hello = fresh.hello.clone();
                self.broken = false;
                request.id = self.conn.as_ref().map_or(1, |c| c.next_id);
                answer = self.exchange(&request, wait).await;
            }
        }
        if matches!(
            method,
            rpc::CLOSE | rpc::OPEN | rpc::CONNECT | rpc::TAB_CLOSE
        ) {
            // The daemon may be gone, or its state other than `hello` said.
            self.broken = true;
        }
        let line = match answer {
            Ok(line) => line,
            Err(Failed::Late) => {
                return Err(ErrorInfo::new(
                    ErrorKind::Timeout,
                    format!(
                        "the daemon of session {} did not answer {method} within {}",
                        self.session,
                        format::duration(wait)
                    ),
                )
                .hint(Hint::run(Cmd::new("status").session(&self.session))));
            }
            Err(Failed::Lost(why)) => return Err(self.lost(&why)),
        };
        let mut response = protocol::rpc::decode_response(&line).map_err(|e| self.lost(&e))?;
        if let Some(fingerprint) = response.fingerprint.take() {
            FINGERPRINT.with(|f| f.borrow_mut().1 = Some(*fingerprint));
        }
        response
            .into_result()
            .map_err(protocol::rpc::RpcError::into_error_info)
    }

    fn lost(&self, why: &str) -> ErrorInfo {
        ErrorInfo::new(
            ErrorKind::DaemonUnavailable,
            format!(
                "lost the connection to the daemon of session {}: {why}",
                self.session
            ),
        )
        .hint(Hint::run(Cmd::new("status").session(&self.session)))
    }

    /// Sends `request` and reads its answer's line. A failure leaves the
    /// connection broken: it is not kept.
    async fn exchange(&mut self, request: &Request, wait: Duration) -> Result<String, Failed> {
        let Some(conn) = self.conn.as_mut() else {
            return Err(Failed::Lost("it is closed".into()));
        };
        conn.next_id = request.id + 1;
        let mut line = serde_json::to_vec(request).unwrap_or_default();
        line.push(b'\n');
        let answer = match conn.writer.write_all(&line).await {
            Err(e) => Err(Failed::Lost(e.to_string())),
            Ok(()) => match tokio::time::timeout(wait, conn.reader.next_line()).await {
                Err(_) => Err(Failed::Late),
                Ok(Err(e)) => Err(Failed::Lost(e.to_string())),
                Ok(Ok(None)) => Err(Failed::Lost("it closed the connection".into())),
                Ok(Ok(Some(line))) => Ok(line),
            },
        };
        if answer.is_err() {
            self.broken = true;
        }
        answer
    }

    /// The request of a call, with what this thread's calls carry.
    fn request<P: Serialize>(&self, method: &str, params: &P) -> Request {
        let id = self.conn.as_ref().map_or(1, |c| c.next_id);
        let params = serde_json::to_value(params).unwrap_or(Value::Null);
        let mut request = Request::new(id, method, params).in_context(self.context.clone());
        if method != rpc::HELLO {
            request.line = LINE
                .with(|line| line.borrow_mut().take())
                .filter(|_| !NO_HISTORY.with(std::cell::Cell::get));
            request.root.clone_from(
                &ROOT
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            request.testid_attribute.clone_from(
                &TESTID_ATTRIBUTE
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            request.fingerprint = FINGERPRINT.with(|f| f.borrow().0.clone());
            request.settle = SETTLE.with(std::cell::Cell::get);
        }
        request
    }

    /// A call whose result is a command's [`Output`].
    pub async fn call_output<P: Serialize>(
        &mut self,
        method: &str,
        params: &P,
        wait: Duration,
    ) -> Result<Output, ErrorInfo> {
        let value = self.call(method, params, wait).await?;
        serde_json::from_value(value).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::DaemonUnavailable,
                format!(
                    "the daemon of session {} sent a bad result: {e}",
                    self.session
                ),
            )
        })
    }
}

/// Whether a daemon holds the session's lock.
pub fn daemon_alive(paths: &SessionPaths) -> bool {
    let Ok(file) = File::open(&paths.daemon_lock) else {
        return false;
    };
    matches!(
        file.try_lock_shared(),
        Err(std::fs::TryLockError::WouldBlock)
    )
}

/// Connects to the session's daemon if one runs. Never starts one, and
/// does not check its build (`status` and `close` work across builds).
/// A session dir that is not the user's own and private has no daemon of
/// theirs: its socket could be another user's, listening for commands.
pub async fn connect(paths: &SessionPaths, build: &str) -> Result<Option<Client>, ErrorInfo> {
    if let Some(client) = take_kept(paths, build) {
        return Ok(Some(client));
    }
    if crate::paths::check_private(&paths.dir, browser::launch::effective_uid()).is_err() {
        return Ok(None);
    }
    let deadline = Instant::now() + START_WAIT;
    let mut pause = Duration::from_millis(5);
    loop {
        if !daemon_alive(paths) {
            return Ok(None);
        }
        // Alive: it may still be binding its socket, or be shutting down.
        match Client::open(paths, build).await {
            Ok(client) => return Ok(Some(client)),
            Err(_) if !daemon_alive(paths) => return Ok(None),
            Err(e) if Instant::now() >= deadline => return Err(unresponsive(paths, &e)),
            Err(_) => {
                tokio::time::sleep(pause).await;
                pause = (pause * 2).min(Duration::from_millis(100));
            }
        }
    }
}

fn unresponsive(paths: &SessionPaths, why: &str) -> ErrorInfo {
    let pid = std::fs::read_to_string(&paths.pid).unwrap_or_default();
    let pid = pid.trim();
    ErrorInfo::new(
        ErrorKind::DaemonUnavailable,
        format!(
            "the daemon of session {} does not answer: {why}",
            paths.name
        ),
    )
    .note("log", paths.daemon_log.display().to_string())
    .hint(Hint::text(format!(
        "stop it with kill {}, then run the command again",
        if pid.is_empty() { "<pid>" } else { pid }
    )))
}

/// A daemon ready for calls.
#[derive(Debug)]
pub struct Session {
    pub client: Client,
    /// Whether this call started it.
    pub spawned: bool,
    /// When a daemon of another build was replaced: its `meta.json`.
    pub replaced: Option<Meta>,
}

/// Connects to the session's daemon, starting it (or replacing one of
/// another build) first if needed.
pub async fn connect_or_start(
    env: &Env,
    paths: &SessionPaths,
    build: &str,
) -> Result<Session, ErrorInfo> {
    paths.create(env)?;
    let mut replaced = None;
    if let Some(mut client) = connect(paths, build).await? {
        if client.hello.build == build {
            return Ok(Session {
                client,
                spawned: false,
                replaced: None,
            });
        }
        tracing::info!(
            old = %client.hello.build,
            new = build,
            "replacing the daemon of session {}",
            paths.name
        );
        replaced = Meta::read(&paths.meta);
        let _ = client.call(rpc::CLOSE, &json!({}), EXIT_WAIT).await;
        drop(client);
        wait_for_exit(paths).await?;
    }

    let _start = StartLock::acquire(paths).await?;
    // Another CLI may have started it while we waited for the lock.
    if let Some(client) = connect(paths, build).await? {
        return Ok(Session {
            client,
            spawned: false,
            replaced,
        });
    }
    clean_up(paths);
    let client = spawn(paths, build).await?;
    Ok(Session {
        client,
        spawned: true,
        replaced,
    })
}

/// Waits until the session's daemon has exited.
pub async fn wait_for_exit(paths: &SessionPaths) -> Result<(), ErrorInfo> {
    let deadline = Instant::now() + EXIT_WAIT;
    while daemon_alive(paths) {
        if Instant::now() >= deadline {
            return Err(unresponsive(paths, "it did not exit after close"));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(())
}

/// Removes what a dead daemon left, under the start lock (so no daemon is
/// starting). Kills its Chrome if that survived: on Linux Chrome dies with
/// the daemon, and elsewhere its watchdog kills it, but both can fail.
pub async fn clean_up_dead(paths: &SessionPaths) -> Result<(), ErrorInfo> {
    if !paths.dir.exists() {
        return Ok(());
    }
    let _start = StartLock::acquire(paths).await?;
    if !daemon_alive(paths) {
        clean_up(paths);
    }
    Ok(())
}

fn clean_up(paths: &SessionPaths) {
    if let Some(meta) = Meta::read(&paths.meta) {
        reap_chrome(&meta);
    }
    for file in [&paths.sock, &paths.pid, &paths.meta] {
        let _ = std::fs::remove_file(file);
    }
}

/// Kills the process group of a dead daemon's Chrome, if it still runs and
/// is the Chrome it launched (pids get reused).
fn reap_chrome(meta: &Meta) {
    let Some(browser) = &meta.browser else { return };
    let (Some(pid), Some(profile)) = (browser.pid, &browser.profile) else {
        return;
    };
    if !browser::launch::process_alive(pid) {
        return;
    }
    let Ok(out) = std::process::Command::new("ps")
        .args(["-o", "args=", "-p", &pid.to_string()])
        .output()
    else {
        return;
    };
    let args = String::from_utf8_lossy(&out.stdout);
    if args.contains("--remote-debugging-pipe")
        && args.contains(&format!("--user-data-dir={}", profile.dir))
    {
        tracing::warn!(pid, "killing the Chrome of a dead daemon");
        let _ = browser::launch::kill_process_group(pid);
    }
}

/// `start.lock`, held while a CLI starts the daemon.
#[derive(Debug)]
struct StartLock(#[allow(dead_code)] File);

impl StartLock {
    async fn acquire(paths: &SessionPaths) -> Result<Self, ErrorInfo> {
        let failed = |why: String| {
            ErrorInfo::new(
                ErrorKind::DaemonUnavailable,
                format!("cannot start the daemon of session {}: {why}", paths.name),
            )
        };
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(&paths.start_lock)
            .map_err(|e| failed(e.to_string()))?;
        let deadline = Instant::now() + START_LOCK_WAIT;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self(file)),
                Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(failed("another chrome-cdp is still starting it".into()));
                }
                Err(std::fs::TryLockError::Error(e)) => return Err(failed(e.to_string())),
            }
        }
    }
}

/// Starts `chrome-cdp __daemon` detached (its own process group, stdio to
/// `daemon.log`, cwd `/`) and waits until it answers.
async fn spawn(paths: &SessionPaths, build: &str) -> Result<Client, ErrorInfo> {
    let failed = |why: String| {
        ErrorInfo::new(
            ErrorKind::DaemonUnavailable,
            format!("cannot start the daemon of session {}: {why}", paths.name),
        )
        .note("log", paths.daemon_log.display().to_string())
    };
    let exe = std::env::current_exe().map_err(|e| failed(e.to_string()))?;
    let log = open_log(&paths.daemon_log).map_err(|e| failed(e.to_string()))?;
    let log_err = log.try_clone().map_err(|e| failed(e.to_string()))?;
    let mut child = std::process::Command::new(exe)
        .args(["__daemon", "--session", &paths.name, "--build", build])
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_err)
        .current_dir("/")
        .process_group(0)
        .spawn()
        .map_err(|e| failed(e.to_string()))?;
    tracing::debug!(
        pid = child.id(),
        "started the daemon of session {}",
        paths.name
    );

    let deadline = Instant::now() + START_WAIT;
    let mut pause = Duration::from_millis(5);
    loop {
        if let Ok(client) = Client::open(paths, build).await {
            return Ok(client);
        }
        if let Some(status) = exited(&mut child)
            && !daemon_alive(paths)
        {
            let last = crate::output::log_tail(&paths.daemon_log, 1);
            let mut info = failed(format!("it exited during startup ({status})"));
            if let Some(line) = last.first() {
                info = info.note("last log line", line.clone());
            }
            return Err(info);
        }
        if Instant::now() >= deadline {
            return Err(failed(format!(
                "it did not answer within {}",
                format::duration(START_WAIT)
            )));
        }
        tokio::time::sleep(pause).await;
        pause = (pause * 2).min(Duration::from_millis(100));
    }
}

fn exited(child: &mut Child) -> Option<std::process::ExitStatus> {
    child.try_wait().ok().flatten()
}

/// Opens `daemon.log` for appending, rotating it first when it is large.
fn open_log(path: &Path) -> std::io::Result<File> {
    if path.metadata().is_ok_and(|m| m.len() > DAEMON_LOG_LIMIT) {
        let _ = std::fs::rename(path, path.with_extension("log.1"));
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

/// Every session with a live daemon under `env`'s session dirs, by name.
pub fn live_sessions(env: &Env) -> Vec<SessionPaths> {
    let mut found = Vec::new();
    for base in env.bases() {
        // A base another user made (`/tmp/ccdp-<uid>` before we did) holds
        // no session of ours: `close --all` must not trust its pids.
        if crate::paths::check_private(&base, env.uid).is_err() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            let Some(meta) = Meta::read(&dir.join("meta.json")) else {
                continue;
            };
            let paths = SessionPaths::found(&meta.session, meta.home.clone(), dir);
            // Only sessions of this base: the short base is shared by all.
            let ours = SessionPaths::new(env, &meta.session);
            if paths.dir == ours.dir && daemon_alive(&paths) {
                found.push(ours);
            }
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found.dedup_by(|a, b| a.name == b.name);
    found
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn history_hides_credentials() {
        let words = |s: &str| s.split(' ').map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            redact(words("open --http-credentials ada:s3cret http://x")),
            words("open --http-credentials *** http://x")
        );
        assert_eq!(
            redact(words("open --http-credentials=ada:s3cret")),
            words("open --http-credentials=***")
        );
        assert_eq!(
            redact(words(
                "open --http-credentials - --http-credentials-origin https://x"
            )),
            words("open --http-credentials - --http-credentials-origin https://x")
        );
        assert_eq!(
            redact(words("fill e1 ada:s3cret")),
            words("fill e1 ada:s3cret")
        );
    }
}
