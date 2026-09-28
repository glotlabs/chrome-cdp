//! The app's web servers during a test run (plan 11, milestone 11.1).
//!
//! [`WebServers::start`] starts each `[e2e.web_server]` in order:
//!
//! - if something already answers on its `url` (any HTTP status below
//!   500), it is reused when `reuse_existing` is set, and nothing is
//!   started or stopped; otherwise that is an error, since the tests would
//!   run against a server the config does not describe;
//! - else its `command` runs under `sh -c` in its own process group, with
//!   stdout and stderr in `<artifacts>/web-server.log` (`web-server-N.log`
//!   when there are several) and `BROWSER=none` unless `env` sets it, so
//!   a create-react-app server does not open a window;
//! - it is ready when a GET of `url` (redirects not followed, TLS
//!   certificates not checked) gets a status below 500. If the command
//!   exits first, or `timeout` passes, the start fails with the last 30
//!   lines of its log.
//!
//! [`WebServers::run`] runs the tests while watching the servers: a
//! server that exits, or an interrupt (Ctrl-C), stops the work.
//! [`WebServers::stop`] sends each group SIGTERM, then SIGKILL after
//! [`STOP_GRACE`]. Dropping [`WebServers`] kills the groups at once, so an
//! early return or a panic leaves nothing behind, and a watchdog per group
//! kills it if chrome-cdp itself is killed.

use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant};

use protocol::{ErrorInfo, ErrorKind, Hint, format};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use crate::config::WebServer;

/// How many lines of a server's log an error shows.
pub const LOG_TAIL_LINES: usize = 30;
/// How long a server has to exit after SIGTERM before it gets SIGKILL.
pub const STOP_GRACE: Duration = Duration::from_secs(5);
/// The longest one readiness check may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// The first and the longest pause between readiness checks.
const FIRST_POLL: Duration = Duration::from_millis(50);
const MAX_POLL: Duration = Duration::from_millis(500);

/// A web server as the run uses it, for reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub command: String,
    pub url: String,
    /// Something already answered on `url`; nothing was started.
    pub reused: bool,
    /// The started command's output.
    pub log: Option<PathBuf>,
    /// The process group of the started command.
    pub pgid: Option<u32>,
    /// From the start until `url` answered.
    pub ready_in: Duration,
}

/// Why [`WebServers::run`] stopped before its work was done.
#[derive(Debug)]
pub enum Stopped {
    /// A server exited; the error has its log's tail.
    Exited(ErrorInfo),
    /// The interrupt future resolved (Ctrl-C).
    Interrupted,
}

/// The web servers of one run.
#[derive(Debug)]
pub struct WebServers {
    servers: Vec<Server>,
    /// Which started server exited, and how.
    exits: mpsc::UnboundedReceiver<(usize, ExitStatus)>,
    exits_tx: mpsc::UnboundedSender<(usize, ExitStatus)>,
}

#[derive(Debug)]
struct Server {
    info: ServerInfo,
    /// Kills the group if chrome-cdp dies before [`WebServers::stop`].
    watchdog: Option<browser::launch::Watchdog>,
    /// `Some` once the started command has exited.
    exited: Option<watch::Receiver<Option<ExitStatus>>>,
    stopped: bool,
}

impl WebServers {
    /// Starts (or reuses) `servers` in order, logging into `log_dir`. On an
    /// error, the ones already started are stopped.
    pub async fn start(servers: &[WebServer], log_dir: &Path) -> Result<Self, ErrorInfo> {
        let (exits_tx, exits) = mpsc::unbounded_channel();
        let mut this = Self {
            servers: Vec::new(),
            exits,
            exits_tx,
        };
        let client = probe_client()?;
        for (i, config) in servers.iter().enumerate() {
            let log = log_dir.join(if servers.len() == 1 {
                "web-server.log".to_owned()
            } else {
                format!("web-server-{}.log", i + 1)
            });
            if let Err(e) = this.start_one(&client, config, log).await {
                this.stop().await;
                return Err(e);
            }
        }
        Ok(this)
    }

    /// The servers, in the config's order.
    pub fn servers(&self) -> Vec<ServerInfo> {
        self.servers.iter().map(|s| s.info.clone()).collect()
    }

    async fn start_one(
        &mut self,
        client: &reqwest::Client,
        config: &WebServer,
        log: PathBuf,
    ) -> Result<(), ErrorInfo> {
        let started = Instant::now();
        let mut info = ServerInfo {
            command: config.command.clone(),
            url: config.url.clone(),
            reused: false,
            log: None,
            pgid: None,
            ready_in: Duration::ZERO,
        };
        if let Ok(status) = probe(client, &config.url).await {
            if !config.reuse_existing {
                return Err(ErrorInfo::new(
                    ErrorKind::LaunchFailed,
                    format!(
                        "{} already answers (HTTP {status}), and reuse_existing is false",
                        config.url
                    ),
                )
                .note("command", &config.command)
                .hint(
                    Hint::text(format!("stop what serves {}", config.url))
                        .or("set reuse_existing = true in [e2e.web_server]"),
                ));
            }
            tracing::info!(url = %config.url, "reusing a running web server");
            info.reused = true;
            info.ready_in = started.elapsed();
            self.servers.push(Server {
                info,
                watchdog: None,
                exited: None,
                stopped: false,
            });
            return Ok(());
        }

        let (pgid, exited) = self.spawn(config, &log)?;
        info.log = Some(log.clone());
        info.pgid = Some(pgid);
        let watchdog = browser::launch::Watchdog::start(pgid, None)
            .inspect_err(|e| tracing::warn!(pgid, "no watchdog for the web server: {e}"))
            .ok();
        self.servers.push(Server {
            info,
            watchdog,
            exited: Some(exited.clone()),
            stopped: false,
        });
        let index = self.servers.len() - 1;
        let ready = wait_ready(client, config, &log, exited, started).await?;
        self.servers[index].info.ready_in = ready;
        tracing::info!(url = %config.url, ?ready, "web server is up");
        Ok(())
    }

    /// Starts the command in its own process group; a task waits for it
    /// and reports its exit.
    fn spawn(
        &self,
        config: &WebServer,
        log: &Path,
    ) -> Result<(u32, watch::Receiver<Option<ExitStatus>>), ErrorInfo> {
        let cannot = |what: &str, e: &std::io::Error| {
            ErrorInfo::new(
                ErrorKind::LaunchFailed,
                format!("cannot start web server `{}`: {what}: {e}", config.command),
            )
        };
        if let Some(dir) = log.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| cannot(&format!("cannot create {}", dir.display()), &e))?;
        }
        let mut out = File::create(log)
            .map_err(|e| cannot(&format!("cannot write {}", log.display()), &e))?;
        let _ = writeln!(
            out,
            "# chrome-cdp: {} (in {})",
            config.command,
            config.cwd.display()
        );
        let err = out
            .try_clone()
            .map_err(|e| cannot(&format!("cannot write {}", log.display()), &e))?;
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&config.command)
            .current_dir(&config.cwd)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .process_group(0);
        if !config.env.contains_key("BROWSER") {
            cmd.env("BROWSER", "none");
        }
        cmd.envs(&config.env);
        let mut child = cmd.spawn().map_err(|e| {
            cannot(&format!("in {}", config.cwd.display()), &e).hint(
                "check that the directory exists (the web server's cwd, relative to the config)",
            )
        })?;
        let pgid = child
            .id()
            .ok_or_else(|| cannot("it exited at once", &std::io::Error::other("no pid")))?;
        tracing::info!(command = %config.command, pgid, "started a web server");
        let (tx, rx) = watch::channel(None);
        let exits = self.exits_tx.clone();
        let index = self.servers.len();
        tokio::spawn(async move {
            if let Ok(status) = child.wait().await {
                let _ = tx.send(Some(status));
                let _ = exits.send((index, status));
            }
        });
        Ok((pgid, rx))
    }

    /// Runs `work` unless a started server exits or `interrupt` resolves
    /// first. The servers keep running either way; call [`Self::stop`].
    pub async fn run<T>(
        &mut self,
        work: impl Future<Output = T>,
        interrupt: impl Future<Output = ()>,
    ) -> Result<T, Stopped> {
        tokio::select! {
            biased;
            () = interrupt => Err(Stopped::Interrupted),
            exited = self.exited() => Err(Stopped::Exited(exited)),
            done = work => Ok(done),
        }
    }

    /// Resolves when a started server exits on its own, with an error that
    /// says so; never if none was started.
    pub async fn exited(&mut self) -> ErrorInfo {
        loop {
            let Some((index, status)) = self.exits.recv().await else {
                return std::future::pending().await;
            };
            let Some(server) = self.servers.get(index) else {
                continue;
            };
            if server.stopped {
                continue;
            }
            let info = &server.info;
            return with_log(
                ErrorInfo::new(
                    ErrorKind::LaunchFailed,
                    format!(
                        "web server `{}` exited ({}) while the tests ran",
                        info.command,
                        describe(status)
                    ),
                ),
                info.log.as_deref(),
            )
            .hint("run the command by hand to see why, then run the tests again");
        }
    }

    /// Stops the started servers, last first: SIGTERM to each process
    /// group, then SIGKILL to what is left after [`STOP_GRACE`].
    pub async fn stop(&mut self) {
        for server in self.servers.iter_mut().rev() {
            let Some(pgid) = server.info.pgid else {
                continue;
            };
            if server.stopped {
                continue;
            }
            server.stopped = true;
            if let Err(e) = browser::launch::terminate_process_group(pgid) {
                tracing::warn!(pgid, "cannot stop the web server's process group: {e}");
            }
            let deadline = Instant::now() + STOP_GRACE;
            if let Some(exited) = &mut server.exited {
                let _ = tokio::time::timeout(STOP_GRACE, exited.wait_for(Option::is_some)).await;
            }
            // Children of the command may outlive it.
            while browser::launch::process_group_alive(pgid) && Instant::now() < deadline {
                tokio::time::sleep(FIRST_POLL).await;
            }
            if browser::launch::process_group_alive(pgid) {
                tracing::warn!(pgid, "the web server ignored SIGTERM; killing it");
                let _ = browser::launch::kill_process_group(pgid);
            }
            if let Some(mut watchdog) = server.watchdog.take() {
                watchdog.disarm();
            }
        }
    }
}

impl Drop for WebServers {
    fn drop(&mut self) {
        for server in &self.servers {
            if let (Some(pgid), false) = (server.info.pgid, server.stopped) {
                let _ = browser::launch::kill_process_group(pgid);
            }
        }
    }
}

/// Polls `url` until it answers below 500, the command exits, or the
/// timeout passes. Returns how long it took from `started`.
async fn wait_ready(
    client: &reqwest::Client,
    config: &WebServer,
    log: &Path,
    mut exited: watch::Receiver<Option<ExitStatus>>,
    started: Instant,
) -> Result<Duration, ErrorInfo> {
    let deadline = started + config.timeout;
    let mut pause = FIRST_POLL;
    loop {
        let Err(last) = probe(client, &config.url).await else {
            return Ok(started.elapsed());
        };
        if let Some(status) = *exited.borrow() {
            return Err(early_exit(config, log, status));
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(with_log(
                ErrorInfo::new(
                    ErrorKind::Timeout,
                    format!(
                        "web server `{}` did not answer on {} within {}",
                        config.command,
                        config.url,
                        format::duration(config.timeout)
                    ),
                )
                .note("last try", last),
                Some(log),
            )
            .hint(
                Hint::text("check that url is where the command serves")
                    .or("raise the web server's timeout in chrome-cdp.toml"),
            ));
        }
        tokio::select! {
            status = exited.wait_for(Option::is_some) => {
                let status = status.ok().and_then(|s| *s).unwrap_or_default();
                return Err(early_exit(config, log, status));
            }
            () = tokio::time::sleep(pause.min(deadline - now)) => {}
        }
        pause = (pause * 2).min(MAX_POLL);
    }
}

fn early_exit(config: &WebServer, log: &Path, status: ExitStatus) -> ErrorInfo {
    with_log(
        ErrorInfo::new(
            ErrorKind::LaunchFailed,
            format!(
                "web server `{}` exited ({}) before {} answered",
                config.command,
                describe(status),
                config.url
            ),
        ),
        Some(log),
    )
    .hint(format!(
        "run it by hand to see why: cd {} && {}",
        protocol::format::shell_quote(&config.cwd.display().to_string()),
        config.command
    ))
}

/// Adds the log's path and its last lines to `info`.
fn with_log(info: ErrorInfo, log: Option<&Path>) -> ErrorInfo {
    let Some(log) = log else {
        return info;
    };
    let info = info.note("log", log.display().to_string());
    let tail = log_tail(log, LOG_TAIL_LINES);
    if tail.is_empty() {
        info
    } else {
        info.block("output", tail)
    }
}

/// The last `n` lines of the file at `path` ("" if it cannot be read).
pub fn log_tail(path: &Path, n: usize) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// `status 1` or `signal 9`.
fn describe(status: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt as _;
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("status {code}"),
        (None, Some(signal)) => format!("signal {signal}"),
        (None, None) => status.to_string(),
    }
}

fn probe_client() -> Result<reqwest::Client, ErrorInfo> {
    // reqwest is built without a default provider (no aws-lc).
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        // A dev server's certificate is often self-signed.
        .tls_danger_accept_invalid_certs(true)
        .no_proxy()
        .timeout(PROBE_TIMEOUT)
        .user_agent(concat!("chrome-cdp/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| {
            ErrorInfo::new(
                ErrorKind::LaunchFailed,
                format!("cannot make an HTTP client: {e}"),
            )
        })
}

/// The status of a GET of `url` if it is below 500; else what happened,
/// in a few words.
async fn probe(client: &reqwest::Client, url: &str) -> Result<u16, String> {
    match client.get(url).send().await {
        Ok(response) if response.status().as_u16() < 500 => Ok(response.status().as_u16()),
        Ok(response) => Err(format!("HTTP {}", response.status().as_u16())),
        Err(e) if e.is_timeout() => Err(format!(
            "no answer within {}",
            format::duration(PROBE_TIMEOUT)
        )),
        Err(e) if e.is_connect() => Err("connection refused".into()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::log_tail;

    #[test]
    fn the_log_tail_is_the_last_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        let text = (1..=40).fold(String::new(), |text, i| text + &format!("line {i}\n"));
        std::fs::write(&path, text).unwrap();
        let tail = log_tail(&path, 30);
        assert!(tail.starts_with("line 11\n") && tail.ends_with("line 40"));
        assert_eq!(log_tail(&dir.path().join("missing"), 30), "");
    }
}
