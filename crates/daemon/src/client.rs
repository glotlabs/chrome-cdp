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

use protocol::rpc::{Request, Response};
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

/// A connection to a session's daemon.
#[derive(Debug)]
pub struct Client {
    reader: Lines<BufReader<ReadHalf>>,
    writer: WriteHalf,
    next_id: u64,
    session: String,
    /// What the daemon said to `hello`.
    pub hello: HelloResult,
}

impl Client {
    /// Connects and says hello.
    async fn open(paths: &SessionPaths, build: &str) -> Result<Self, String> {
        let stream = unix::connect(&paths.sock)
            .await
            .map_err(|e| format!("cannot connect to {}: {e}", paths.sock.display()))?;
        let (read, writer) = stream.into_split();
        let mut client = Self {
            reader: BufReader::new(read).lines(),
            writer,
            next_id: 1,
            session: paths.name.clone(),
            hello: HelloResult {
                build: String::new(),
                session: paths.name.clone(),
                pid: 0,
                state: protocol::schema::SessionState::Idle,
            },
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

    /// Makes one call and waits up to `wait` for the answer.
    pub async fn call<P: Serialize>(
        &mut self,
        method: &str,
        params: &P,
        wait: Duration,
    ) -> Result<Value, ErrorInfo> {
        let id = self.next_id;
        self.next_id += 1;
        let params = serde_json::to_value(params).unwrap_or(Value::Null);
        let mut line = serde_json::to_vec(&Request::new(id, method, params)).unwrap_or_default();
        line.push(b'\n');
        let lost = |why: String| {
            ErrorInfo::new(
                ErrorKind::DaemonUnavailable,
                format!(
                    "lost the connection to the daemon of session {}: {why}",
                    self.session
                ),
            )
            .hint(Hint::run(Cmd::new("status").session(&self.session)))
        };
        self.writer
            .write_all(&line)
            .await
            .map_err(|e| lost(e.to_string()))?;
        let answer = tokio::time::timeout(wait, self.reader.next_line()).await;
        let line = match answer {
            Err(_) => {
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
            Ok(Err(e)) => return Err(lost(e.to_string())),
            Ok(Ok(None)) => return Err(lost("it closed the connection".into())),
            Ok(Ok(Some(line))) => line,
        };
        let response: Response =
            serde_json::from_str(&line).map_err(|e| lost(format!("bad response: {e}")))?;
        response
            .into_result()
            .map_err(protocol::rpc::RpcError::into_error_info)
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
pub async fn connect(paths: &SessionPaths, build: &str) -> Result<Option<Client>, ErrorInfo> {
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
