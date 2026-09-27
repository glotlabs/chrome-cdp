//! `chrome-cdp __direct cdp`: a temporary, daemon-less raw CDP call
//! (plan 01, milestone 1.4).
//!
//! It launches Chrome over `--remote-debugging-pipe` with a throwaway
//! profile, makes one call, prints the result, and closes Chrome. Plan 02
//! replaces it with the daemon-backed `chrome-cdp cdp` and moves discovery
//! and the launch flags into the `browser` crate; delete this module then.

use std::collections::VecDeque;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cdp::domains::{browser, target};
use cdp::transport::PipeTransport;
use cdp::{CdpError, Connection, ConnectionOptions};
use protocol::{ErrorInfo, ErrorKind};
use serde_json::{Map, Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::process::{Child, ChildStderr, Command};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::cli::{DirectCdpArgs, GlobalArgs, Target};
use crate::errors;
use crate::output::{self, Success};

/// A minimal flag set for a headless, quiet, isolated Chrome. Plan 02 owns
/// the real list.
const CHROME_FLAGS: &[&str] = &[
    "--headless",
    "--remote-debugging-pipe",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-default-apps",
    "--disable-dev-shm-usage",
    "--disable-sync",
    "--password-store=basic",
    "--use-mock-keychain",
];

/// How long to wait for Chrome to exit after `Browser.close`.
const EXIT_GRACE: Duration = Duration::from_secs(5);

/// Chrome stderr lines kept for launch-failure messages.
const STDERR_TAIL: usize = 20;

/// Runs `__direct cdp`.
pub(crate) fn cdp(args: &DirectCdpArgs, global: &GlobalArgs) -> Result<Success, ErrorInfo> {
    // Check everything that needs no browser before starting one.
    check_method(&args.method)?;
    let params = parse_params(args.params.as_deref())?;
    let chrome = chrome_path(args.chrome.as_deref())?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| {
            ErrorInfo::new(
                ErrorKind::LaunchFailed,
                format!("cannot start the runtime: {e}"),
            )
        })?;
    let value = runtime.block_on(async {
        let chrome = Chrome::launch(&chrome, global.timeout).await?;
        let result = call(&chrome.conn, args, params).await;
        chrome.close().await;
        result
    })?;

    let more = "--max-chars 0 for all of it, or --json";
    let pretty = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    let compact = value.to_string();
    Ok(
        Success::new(value, output::truncate(pretty, args.max_chars, more))
            .quiet(output::truncate(compact, args.max_chars, more)),
    )
}

fn bad_args(message: impl Into<String>) -> ErrorInfo {
    ErrorInfo::new(ErrorKind::BadArgs, message)
}

/// Accepts `Domain.method`, e.g. `Browser.getVersion`.
fn check_method(method: &str) -> Result<(), ErrorInfo> {
    let ok = method.split_once('.').is_some_and(|(domain, name)| {
        domain.starts_with(|c: char| c.is_ascii_uppercase())
            && !name.is_empty()
            && [domain, name]
                .iter()
                .all(|part| part.chars().all(|c| c.is_ascii_alphanumeric()))
    });
    if ok {
        Ok(())
    } else {
        Err(
            bad_args(format!("`{method}` is not a CDP method name")).hint(
                "use Domain.method, e.g. chrome-cdp __direct cdp Browser.getVersion --chrome <path>",
            ),
        )
    }
}

/// Parses the params argument: absent means `{}`, `-` reads stdin.
fn parse_params(arg: Option<&str>) -> Result<Map<String, Value>, ErrorInfo> {
    let text = match arg {
        None => return Ok(Map::new()),
        Some("-") => {
            let mut buf = String::new();
            io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| bad_args(format!("cannot read params from stdin: {e}")))?;
            buf
        }
        Some(text) => text.to_owned(),
    };
    let example = r#"e.g. '{"expression":"1+1"}'"#;
    match serde_json::from_str(&text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(other) => Err(bad_args(format!(
            "params must be a JSON object, not {}",
            json_type(&other)
        ))
        .hint(format!("wrap them in an object, {example}"))),
        Err(e) => Err(bad_args(format!("params are not valid JSON: {e}"))
            .hint(format!("quote the JSON for the shell, {example}"))),
    }
}

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn chrome_path(arg: Option<&Path>) -> Result<PathBuf, ErrorInfo> {
    let Some(path) = arg else {
        return Err(
            ErrorInfo::new(ErrorKind::ChromeNotFound, "no Chrome binary given")
                .hint("pass --chrome /path/to/chrome or set CHROME_CDP_CHROME"),
        );
    };
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    if path.is_file() {
        Ok(path)
    } else {
        Err(ErrorInfo::new(
            ErrorKind::ChromeNotFound,
            format!("no Chrome at {}", path.display()),
        )
        .hint("pass --chrome with the path of a Chrome or Chromium binary"))
    }
}

/// Makes the user's call on the requested target.
async fn call(
    conn: &Connection,
    args: &DirectCdpArgs,
    params: Map<String, Value>,
) -> Result<Value, ErrorInfo> {
    let session = match args.target {
        Target::Browser => conn.browser(),
        Target::Page => {
            let created = conn
                .browser()
                .call(target::CreateTargetParams::new("about:blank".into()))
                .await
                .map_err(|e| errors::from_cdp(&e))?;
            conn.attach(created.target_id)
                .await
                .map_err(|e| errors::from_cdp(&e))?
        }
    };
    session
        .call_raw(&args.method, Value::Object(params))
        .await
        .map_err(|e| {
            let info = errors::from_cdp(&e);
            let browser_domain = ["Browser.", "Target."]
                .iter()
                .any(|d| args.method.starts_with(d));
            match e {
                CdpError::Protocol { code: -32601, .. }
                    if args.target == Target::Browser && !browser_domain =>
                {
                    info.hint(format!(
                        "if {m} is a page method, send it to a page: \
                         chrome-cdp __direct cdp --target page {m} …",
                        m = args.method
                    ))
                }
                _ => info,
            }
        })
}

/// A Chrome launched for one call.
struct Chrome {
    conn: Connection,
    child: Child,
    profile: TempDir,
    stderr: StderrTail,
}

impl Chrome {
    /// Starts Chrome and waits until it answers `Browser.getVersion`.
    async fn launch(path: &Path, call_timeout: Duration) -> Result<Self, ErrorInfo> {
        let launch_failed = |message: String| ErrorInfo::new(ErrorKind::LaunchFailed, message);
        let profile = tempfile::Builder::new()
            .prefix("chrome-cdp-profile-")
            .tempdir()
            .map_err(|e| launch_failed(format!("cannot create a profile directory: {e}")))?;

        let mut cmd = Command::new(path);
        cmd.args(CHROME_FLAGS)
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let (transport, mut child) = PipeTransport::spawn(cmd).map_err(|e| {
            let info = launch_failed(format!("cannot start {}: {e}", path.display()));
            match e.kind() {
                io::ErrorKind::PermissionDenied => {
                    info.hint(format!("make it executable: chmod +x {}", path.display()))
                }
                _ => info.hint("check that --chrome points at a Chrome or Chromium binary"),
            }
        })?;
        tracing::debug!(chrome = %path.display(), pid = child.id(), "launched Chrome");
        let stderr = StderrTail::start(child.stderr.take());
        let conn = Connection::with_options(
            transport,
            ConnectionOptions {
                call_timeout,
                ..ConnectionOptions::default()
            },
        );
        let chrome = Self {
            conn,
            child,
            profile,
            stderr,
        };

        match chrome
            .conn
            .browser()
            .call(browser::GetVersionParams::default())
            .await
        {
            Ok(version) => {
                tracing::info!(product = %version.product, "connected to Chrome");
                Ok(chrome)
            }
            Err(e) => Err(chrome.startup_failure(&e).await),
        }
    }

    /// Explains why Chrome did not answer its first call, using the tail of
    /// its stderr, and cleans up.
    async fn startup_failure(mut self, err: &CdpError) -> ErrorInfo {
        let exited = matches!(err, CdpError::Disconnected { .. });
        if !exited {
            let _ = self.child.start_kill();
        }
        // Let Chrome finish exiting and its stderr drain.
        let _ = timeout(EXIT_GRACE, self.child.wait()).await;
        let lines = self.stderr.finish().await;
        let last = lines.iter().rev().find_map(|l| chrome_message(l));

        let mut info = if exited {
            let mut message = "Chrome exited during startup".to_owned();
            if let Some(last) = last {
                message.push_str(": ");
                message.push_str(last);
            }
            ErrorInfo::new(ErrorKind::LaunchFailed, message)
        } else if let CdpError::Timeout { after, .. } = err {
            // Name the wait, not the internal handshake call.
            let mut info = errors::from_cdp(err);
            info.message = format!(
                "Chrome started but did not answer within {}",
                output::duration(*after)
            );
            info
        } else {
            errors::from_cdp(err)
        };
        if lines.iter().any(|l| l.contains("No usable sandbox")) {
            info = info.hint(
                "the kernel blocks Chrome's sandbox for this binary; allow it with an AppArmor \
                 profile granting `userns` (see /etc/apparmor.d/), or use a system Chrome",
            );
        } else if exited {
            info = info.hint("run the command again with -vv to see Chrome's output");
        }
        self.cleanup().await;
        info.details(json!({ "chrome_stderr": lines }))
    }

    /// Asks Chrome to exit, waits for it (killing it if it hangs), and deletes
    /// the profile.
    async fn close(self) {
        let _ = timeout(
            EXIT_GRACE,
            self.conn.browser().call(browser::CloseParams::default()),
        )
        .await;
        self.cleanup().await;
    }

    async fn cleanup(self) {
        let Self {
            conn,
            mut child,
            profile,
            mut stderr,
        } = self;
        // Closing the pipe also makes Chrome exit.
        drop(conn);
        if timeout(EXIT_GRACE, child.wait()).await.is_err() {
            tracing::warn!("Chrome did not exit; killing it");
            let _ = child.kill().await;
        }
        stderr.finish().await;
        let dir = profile.path().to_owned();
        if let Err(e) = profile.close() {
            tracing::warn!(dir = %dir.display(), "cannot delete the Chrome profile: {e}");
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

/// Collects the last [`STDERR_TAIL`] lines of Chrome's stderr, and logs every
/// line at debug level (`-vv`).
struct StderrTail {
    lines: Arc<Mutex<VecDeque<String>>>,
    task: Option<JoinHandle<()>>,
}

impl StderrTail {
    fn start(stderr: Option<ChildStderr>) -> Self {
        let lines = Arc::new(Mutex::new(VecDeque::new()));
        let task = stderr.map(|stderr| {
            let lines = lines.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut buf = Vec::new();
                while let Ok(n) = reader.read_until(b'\n', &mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let line = String::from_utf8_lossy(&buf).trim_end().to_owned();
                    buf.clear();
                    tracing::debug!(target: "chrome", "{line}");
                    if let Ok(mut lines) = lines.lock() {
                        if lines.len() == STDERR_TAIL {
                            lines.pop_front();
                        }
                        lines.push_back(line);
                    }
                }
            })
        });
        Self { lines, task }
    }

    /// Waits (briefly) for stderr to close and returns the collected lines.
    async fn finish(&mut self) -> Vec<String> {
        if let Some(task) = self.task.take() {
            // Chrome's helper processes can keep stderr open after the
            // browser exits; do not wait for them long.
            let abort = task.abort_handle();
            if timeout(Duration::from_secs(1), task).await.is_err() {
                abort.abort();
            }
        }
        self.lines
            .lock()
            .map(|l| l.iter().cloned().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use protocol::ErrorKind;
    use serde_json::json;

    use super::{check_method, chrome_message, chrome_path, parse_params};

    #[test]
    fn method_names() {
        assert!(check_method("Browser.getVersion").is_ok());
        assert!(check_method("DOMSnapshot.captureSnapshot").is_ok());
        for bad in [
            "",
            "Browser",
            "Browser.",
            ".getVersion",
            "browser.getVersion",
            "A.b.c",
        ] {
            let err = check_method(bad).unwrap_err();
            assert_eq!(err.kind, ErrorKind::BadArgs, "{bad}");
        }
    }

    #[test]
    fn params() {
        assert_eq!(
            parse_params(None).unwrap(),
            json!({}).as_object().unwrap().clone()
        );
        assert_eq!(
            parse_params(Some(r#"{"expression":"1+1"}"#)).unwrap()["expression"],
            json!("1+1")
        );
        let err = parse_params(Some("[1]")).unwrap_err();
        assert_eq!(err.message, "params must be a JSON object, not an array");
        let err = parse_params(Some("{expression:1}")).unwrap_err();
        assert!(
            err.message.starts_with("params are not valid JSON: "),
            "{err}"
        );
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn chrome_path_must_exist() {
        let err = chrome_path(None).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ChromeNotFound);
        let err = chrome_path(Some(Path::new("/nonexistent/chrome"))).unwrap_err();
        assert_eq!(err.message, "no Chrome at /nonexistent/chrome");
        assert_eq!(err.exit_code(), 4);
    }

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
}
