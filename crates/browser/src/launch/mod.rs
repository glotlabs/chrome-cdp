//! Launching Chrome (plan 02, milestone 2.3).
//!
//! [`launch`] starts the Chrome that discovery found over
//! `--remote-debugging-pipe` with the flags in [`flags`], a profile from
//! [`ProfileChoice`], and its sandbox on (see [`sandbox`]), then waits
//! until it answers `Browser.getVersion`. The returned [`Launched`] owns
//! the browser: [`Launched::close`] shuts it down, and dropping it kills
//! Chrome's process group and deletes a temporary profile. If chrome-cdp
//! dies instead, [`process`] makes sure Chrome dies too.

mod apparmor;
pub mod flags;
mod process;
mod profile;
pub mod sandbox;
mod stderr;
mod sys;

use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use cdp::domains::browser as cdp_browser;
use cdp::transport::PipeTransport;
use cdp::{CdpError, Connection, ConnectionOptions};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, format};
use serde_json::json;
use tokio::process::Child;
use tokio::time::timeout;

pub use profile::{ProfileChoice, check_name as check_profile_name, data_dir, named_profile_dir};
pub use sandbox::{AppArmorProfile, UsernsCheck, apparmor_profile, running_as_root, userns_check};
pub use stderr::LOG_LIMIT;

use crate::discover::Chrome;
use crate::host::{Host, Os};
use crate::installed::cache_dir;
use process::Watchdog;
use profile::Profile;
use stderr::{RotatingLog, StderrTail};

/// How long Chrome gets to exit after `Browser.close` (or after its pipe
/// closes) before it is killed.
const EXIT_GRACE: Duration = Duration::from_secs(5);

/// How to launch Chrome.
#[derive(Debug, Clone)]
pub struct LaunchOptions {
    /// Open a window instead of running headless.
    pub headed: bool,
    pub profile: ProfileChoice,
    /// Extra flags (`--chrome-arg`), after chrome-cdp's own.
    pub args: Vec<String>,
    /// Where to keep Chrome's stderr (the session's `chrome.log`).
    pub log_file: Option<PathBuf>,
    /// Bounds the startup handshake and every later CDP call.
    pub timeout: Duration,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            headed: false,
            profile: ProfileChoice::Ephemeral,
            args: Vec::new(),
            log_file: None,
            timeout: Duration::from_secs(30),
        }
    }
}

/// A running Chrome that chrome-cdp launched.
#[derive(Debug)]
pub struct Launched {
    conn: Option<Connection>,
    child: Child,
    pgid: u32,
    profile: Profile,
    stderr: StderrTail,
    watchdog: Option<Watchdog>,
    command_line: Vec<String>,
    /// `Browser.getVersion`'s product, e.g. `HeadlessChrome/154.0.8037.57`.
    product: String,
    /// Non-fatal problems for the command's `warnings`.
    pub warnings: Vec<String>,
    closed: bool,
}

/// Starts `chrome` and waits until it answers.
///
/// Call it from a thread that lives as long as the browser should (the
/// main thread or a runtime worker, not `spawn_blocking`): on Linux,
/// Chrome is killed when the thread that spawned it exits.
pub async fn launch(
    chrome: &Chrome,
    opts: &LaunchOptions,
    host: &Host,
) -> Result<Launched, ErrorInfo> {
    flags::check_extra(&opts.args)?;
    if opts.headed {
        check_can_open_a_window(chrome, host)?;
    }

    let mut warnings = Vec::new();
    let user_no_sandbox = opts.args.iter().any(|a| a == "--no-sandbox");
    let no_sandbox_reason = if user_no_sandbox {
        None
    } else {
        sandbox::decide(&sandbox::SandboxEnv::detect(host))
    };
    if let Some(reason) = &no_sandbox_reason {
        warnings.push(format!(
            "running Chrome without its sandbox (--no-sandbox): {reason}; a malicious page is \
             far more dangerous"
        ));
    } else if user_no_sandbox {
        warnings.push(
            "running Chrome without its sandbox (--chrome-arg=--no-sandbox); a malicious page \
             is far more dangerous"
                .into(),
        );
    } else if sandbox::in_cache(host, &chrome.path)
        && userns_check(host, &chrome.path) == UsernsCheck::Blocked
    {
        return Err(sandbox_blocked(host, chrome, None));
    }

    let profile = profile::prepare(&opts.profile, host, chrome)?;
    warnings.extend(profile.warnings.iter().cloned());
    let log = opts.log_file.as_ref().and_then(|path| {
        RotatingLog::create(path, LOG_LIMIT)
            .map_err(|e| warnings.push(format!("cannot write {}: {e}", path.display())))
            .ok()
    });

    let args = flags::command_line(
        host.os,
        opts.headed,
        no_sandbox_reason.is_some(),
        &profile.dir,
        &opts.args,
    );
    let mut cmd = tokio::process::Command::new(&chrome.path);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(tmp) = profile.tmp_dir() {
        cmd.env("TMPDIR", tmp);
    }
    process::configure(&mut cmd);
    let (transport, mut child) =
        PipeTransport::spawn(cmd).map_err(|e| spawn_failed(&chrome.path, &e))?;
    let pgid = child.id().unwrap_or_default();
    tracing::debug!(chrome = %chrome.path.display(), pid = pgid, ?args, "launched Chrome");

    let watchdog = Watchdog::start(pgid, profile.run.as_ref().map(tempfile::TempDir::path))
        .map_err(|e| tracing::warn!("cannot start the Chrome watchdog: {e}"))
        .ok();
    let stderr = StderrTail::start(child.stderr.take(), log);
    let conn = Connection::with_options(
        transport,
        ConnectionOptions {
            call_timeout: opts.timeout,
            ..ConnectionOptions::default()
        },
    );
    let mut launched = Launched {
        conn: Some(conn.clone()),
        child,
        pgid,
        profile,
        stderr,
        watchdog,
        command_line: args,
        product: String::new(),
        warnings,
        closed: false,
    };
    match conn
        .browser()
        .call(cdp_browser::GetVersionParams::default())
        .await
    {
        Ok(version) => {
            tracing::info!(product = %version.product, "connected to Chrome");
            launched.product = version.product;
            Ok(launched)
        }
        Err(e) => Err(launched.startup_failure(&e, chrome, host).await),
    }
}

/// Whether a process with this pid exists. The daemon's client uses it on
/// pids recorded in a session's files.
pub fn process_alive(pid: u32) -> bool {
    sys::process_alive(pid)
}

/// Kills the process group `pgid` (SIGKILL). A group that is already gone
/// is not an error; pids 0 and 1 are refused.
pub fn kill_process_group(pgid: u32) -> io::Result<()> {
    sys::kill_group(pgid)
}

/// The effective user id, e.g. to name a per-user directory.
pub fn effective_uid() -> u32 {
    sys::euid()
}

/// `--headed` needs full Chrome and (on Linux) a display.
fn check_can_open_a_window(chrome: &Chrome, host: &Host) -> Result<(), ErrorInfo> {
    if chrome.headless_shell {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!(
                "{} is chrome-headless-shell, which cannot open a window",
                chrome.path.display()
            ),
        )
        .hint(
            Hint::text("drop --headed").or(format!("install full Chrome: {}", Cmd::new("install"))),
        ));
    }
    if host.os == Os::Linux
        && host.var("DISPLAY").is_none()
        && host.var("WAYLAND_DISPLAY").is_none()
    {
        return Err(ErrorInfo::new(
            ErrorKind::LaunchFailed,
            "--headed needs a display, and DISPLAY and WAYLAND_DISPLAY are unset",
        )
        .hint(
            Hint::text("drop --headed")
                .or("run it under a virtual display: xvfb-run -a chrome-cdp …"),
        ));
    }
    Ok(())
}

fn spawn_failed(path: &Path, e: &io::Error) -> ErrorInfo {
    let info = ErrorInfo::new(
        ErrorKind::LaunchFailed,
        format!("cannot start {}: {e}", path.display()),
    );
    match e.kind() {
        io::ErrorKind::PermissionDenied => info.hint(format!(
            "make it executable: chmod +x {}",
            format::shell_quote(&path.to_string_lossy())
        )),
        _ => info.hint("check that --chrome points at a Chrome or Chromium binary"),
    }
}

/// The error for a Chrome whose sandbox the kernel blocks: before launch
/// when the userns check predicts it, or after Chrome said "No usable
/// sandbox".
fn sandbox_blocked(host: &Host, chrome: &Chrome, exited: Option<ErrorInfo>) -> ErrorInfo {
    let path = chrome.path.display().to_string();
    let info = exited.unwrap_or_else(|| {
        ErrorInfo::new(
            ErrorKind::LaunchFailed,
            format!("Chrome's sandbox cannot start: AppArmor blocks user namespaces for {path}"),
        )
        .note(
            "why",
            "Ubuntu 23.10+ lets only binaries with an AppArmor profile granting `userns` use them",
        )
    });
    let fix = install_command(host, &chrome.path).zip(apparmor_profile(host));
    match fix {
        Some((install, profile)) if sandbox::in_cache(host, &chrome.path) => info
            .hint(
                Hint::run(install.switch("with-deps"))
                    .why(format!("writes {} with sudo", profile.path.display())),
            )
            .details(json!({ "chrome": path, "apparmor_profile": profile.path })),
        _ => info.hint(
            "the kernel blocks Chrome's sandbox for this binary; allow it with an AppArmor \
             profile granting `userns` (see /etc/apparmor.d/), or use a system Chrome",
        ),
    }
}

/// The `install` command for an installed browser, from its path:
/// `<cache>/browsers/<channel>-<version>-<platform>/<dir>/<binary>`.
fn install_command(host: &Host, binary: &Path) -> Option<Cmd> {
    let browsers = cache_dir(host)?.join("browsers");
    let rel = binary.strip_prefix(&browsers).ok()?;
    let id = rel.components().next()?.as_os_str().to_str()?;
    let (channel, rest) = id.split_once('-')?;
    let version = rest.split_once('-').map_or(rest, |(v, _)| v);
    let mut cmd = Cmd::new("install");
    cmd = match channel {
        "stable" => cmd,
        "beta" | "dev" | "canary" => cmd.flag("channel", channel),
        _ => cmd.flag("version", version),
    };
    if binary
        .file_name()
        .is_some_and(|n| n == "chrome-headless-shell")
    {
        cmd = cmd.switch("headless-shell");
    }
    Some(cmd)
}

impl Launched {
    /// The CDP connection to the browser.
    ///
    /// # Panics
    ///
    /// Never: the connection is only taken by `close`, which consumes
    /// `self`.
    #[allow(clippy::missing_panics_doc)]
    pub fn connection(&self) -> &Connection {
        #[allow(clippy::expect_used)]
        self.conn.as_ref().expect("connection taken only by close")
    }

    /// The browser's pid, which is also its process group.
    pub fn pid(&self) -> u32 {
        self.pgid
    }

    /// The profile directory.
    pub fn profile_dir(&self) -> &Path {
        &self.profile.dir
    }

    /// Whether the profile is deleted on close.
    pub fn ephemeral_profile(&self) -> bool {
        self.profile.ephemeral
    }

    /// Chrome's arguments, after the program name.
    pub fn command_line(&self) -> &[String] {
        &self.command_line
    }

    /// What `Browser.getVersion` said, e.g. `HeadlessChrome/154.0.8037.57`.
    pub fn product(&self) -> &str {
        &self.product
    }

    /// The last lines Chrome wrote to stderr.
    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr.lines()
    }

    /// Asks Chrome to exit, waits for it (killing it if it hangs), kills
    /// what is left of its process group, and deletes a temporary profile.
    pub async fn close(mut self) {
        if let Some(conn) = &self.conn {
            let _ = timeout(
                EXIT_GRACE,
                conn.browser().call(cdp_browser::CloseParams::default()),
            )
            .await;
        }
        self.shutdown().await;
    }

    async fn shutdown(&mut self) {
        // Closing the pipe also makes Chrome exit.
        self.conn = None;
        if timeout(EXIT_GRACE, self.child.wait()).await.is_err() {
            tracing::warn!("Chrome did not exit; killing it");
        }
        // Helpers (GPU, renderers, crashpad) that outlived the browser.
        process::kill_group(self.pgid);
        let _ = self.child.wait().await;
        self.stderr.finish().await;
        if let Some(mut dog) = self.watchdog.take() {
            dog.disarm();
        }
        if let Some(temp) = self.profile.run.take() {
            let dir = temp.path().to_owned();
            if let Err(e) = temp.close() {
                tracing::warn!(dir = %dir.display(), "cannot delete the Chrome profile: {e}");
            }
        }
        self.closed = true;
    }

    /// Explains why Chrome did not answer its first call, from the tail of
    /// its stderr, and cleans up.
    async fn startup_failure(mut self, err: &CdpError, chrome: &Chrome, host: &Host) -> ErrorInfo {
        let exited = matches!(err, CdpError::Disconnected { .. });
        if !exited {
            let _ = self.child.start_kill();
        }
        // Let Chrome finish exiting and its stderr drain.
        let _ = timeout(EXIT_GRACE, self.child.wait()).await;
        let lines = self.stderr.finish().await;
        let last = lines.iter().rev().find_map(|l| chrome_message(l));

        let info = match err {
            CdpError::Disconnected { .. } => {
                let mut message = "Chrome exited during startup".to_owned();
                if let Some(last) = last {
                    message.push_str(": ");
                    message.push_str(last);
                }
                ErrorInfo::new(ErrorKind::LaunchFailed, message)
            }
            CdpError::Timeout { after, method } => ErrorInfo::new(
                ErrorKind::Timeout,
                format!(
                    "Chrome started but did not answer within {}",
                    format::duration(*after)
                ),
            )
            .hint(format!(
                "retry with a longer timeout, e.g. --timeout {}",
                format::duration(*after * 2)
            ))
            .details(json!({ "method": method, "timeout_ms": after.as_millis() })),
            other => ErrorInfo::new(
                ErrorKind::LaunchFailed,
                format!("Chrome did not start properly: {other}"),
            ),
        };
        let info = if lines.iter().any(|l| l.contains("No usable sandbox")) {
            sandbox_blocked(host, chrome, Some(info))
        } else if exited {
            info.hint("run the command again with -vv to see Chrome's output")
        } else {
            info
        };
        self.shutdown().await;
        info.details(json!({ "chrome_stderr": lines }))
    }
}

impl Drop for Launched {
    /// Kills Chrome at once if it was not closed (an error path, or a
    /// signal): its whole process group, then the temporary profile goes
    /// with `self.profile`.
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        process::kill_group(self.pgid);
        let _ = self.child.start_kill();
        if let Some(mut dog) = self.watchdog.take() {
            dog.disarm();
        }
    }
}

/// Strips Chrome's `[pid:tid:date/time:LEVEL:file(line)] ` log prefix.
fn chrome_message(line: &str) -> Option<&str> {
    let msg = match line.strip_prefix('[').and_then(|l| l.split_once("] ")) {
        Some((_, rest)) => rest,
        None => line,
    };
    let msg = msg.trim();
    (!msg.is_empty()).then_some(msg)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use protocol::ErrorKind;

    use super::{check_can_open_a_window, chrome_message, install_command};
    use crate::discover::{Chrome, Source};
    use crate::host::{Arch, Host, Os};

    #[test]
    fn chrome_log_prefix_is_stripped() {
        assert_eq!(
            chrome_message(
                "[1:1:0927/101010.123:FATAL:zygote_host_impl_linux.cc(128)] No usable sandbox!"
            ),
            Some("No usable sandbox!")
        );
        assert_eq!(chrome_message("plain"), Some("plain"));
        assert_eq!(chrome_message("  "), None);
    }

    #[test]
    fn install_commands_for_installed_browsers() {
        let host = Host::fake(Os::Linux, Arch::X86_64, Path::new("/")).with_var("HOME", "/h");
        let b = Path::new("/h/.cache/chrome-cdp/browsers");
        let cmd = |p: &str| install_command(&host, &b.join(p)).map(|c| c.to_string());
        assert_eq!(
            cmd("stable-154.0.1.2-linux64/chrome-linux64/chrome").as_deref(),
            Some("chrome-cdp install")
        );
        assert_eq!(
            cmd("beta-155.0.1.2-linux64/chrome-headless-shell-linux64/chrome-headless-shell")
                .as_deref(),
            Some("chrome-cdp install --channel beta --headless-shell")
        );
        assert_eq!(
            cmd("pinned-150.0.1.2-linux64/chrome-linux64/chrome").as_deref(),
            Some("chrome-cdp install --version 150.0.1.2")
        );
        assert_eq!(
            install_command(&host, Path::new("/opt/google/chrome/chrome")),
            None
        );
    }

    #[test]
    fn headed_needs_full_chrome_and_a_display() {
        let chrome = |shell| Chrome {
            path: PathBuf::from("/c"),
            source: Source::Flag,
            version: None,
            snap: false,
            headless_shell: shell,
            warnings: Vec::new(),
        };
        let linux = Host::fake(Os::Linux, Arch::X86_64, Path::new("/"));
        let err = check_can_open_a_window(&chrome(false), &linux).unwrap_err();
        assert_eq!(err.kind, ErrorKind::LaunchFailed);
        assert!(err.to_text().contains("xvfb-run -a chrome-cdp"), "{err}");
        assert!(
            check_can_open_a_window(&chrome(false), &linux.clone().with_var("DISPLAY", ":0"))
                .is_ok()
        );
        assert!(
            check_can_open_a_window(
                &chrome(false),
                &linux.clone().with_var("WAYLAND_DISPLAY", "wayland-0")
            )
            .is_ok()
        );
        let mac = Host::fake(Os::MacOs, Arch::Aarch64, Path::new("/"));
        assert!(check_can_open_a_window(&chrome(false), &mac).is_ok());
        let err = check_can_open_a_window(&chrome(true), &mac).unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadArgs);
    }
}
