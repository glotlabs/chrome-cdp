//! `chrome-cdp doctor` (plan 10, milestone 10.5): checks what chrome-cdp
//! needs from the machine and prints a fix for each problem.
//!
//! The checks, in order: this binary; the Chrome `open` would launch
//! (found, its version); on Linux its shared libraries and Ubuntu's
//! AppArmor rule for its sandbox; launching it for real (headless, a
//! temporary profile; `--no-launch` skips it); the session dir's owner and
//! mode; on macOS the quarantine attribute of both binaries; and running
//! sessions whose daemon is another chrome-cdp version. A failed check
//! makes the command fail with exit code 4, every check in `details`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use browser::launch::{self, UsernsCheck};
use browser::{Chrome, Host, LaunchOptions, Os};
use protocol::schema::{DoctorCheck, DoctorData, DoctorStatus};
use protocol::{Cmd, ErrorInfo, ErrorKind, Output, format};

use crate::cli::DoctorArgs;
use crate::version::{GIT_SHA, VERSION};

/// Runs `doctor`.
pub(crate) fn run(args: &DoctorArgs, timeout: Duration) -> Result<Output, ErrorInfo> {
    let host = Host::current();
    let mut checks = Vec::new();
    let exe = std::env::current_exe().ok();
    checks.push(this_binary(exe.as_deref()));
    let chrome = chrome(args.chrome.as_deref(), &host, &mut checks);
    let runs = libraries(&host, chrome.as_ref(), &mut checks);
    let sandbox_ok = sandbox(&host, chrome.as_ref(), &mut checks);
    checks.push(match (&chrome, runs && sandbox_ok, args.no_launch) {
        (_, _, true) => skip("launch", "--no-launch: Chrome was not started"),
        (None, _, _) => skip("launch", "no Chrome to start"),
        (Some(_), false, _) => skip("launch", "not tried: an earlier check failed"),
        (Some(chrome), true, false) => try_launch(chrome, &host, timeout),
    });
    checks.push(session_dir());
    checks.push(quarantine(&host, exe.as_deref(), chrome.as_ref()));
    checks.push(sessions());
    result(checks)
}

fn check(name: &str, status: DoctorStatus, message: impl Into<String>) -> DoctorCheck {
    DoctorCheck {
        name: name.into(),
        status,
        message: message.into(),
        fix: None,
    }
}

fn ok(name: &str, message: impl Into<String>) -> DoctorCheck {
    check(name, DoctorStatus::Ok, message)
}

fn skip(name: &str, message: impl Into<String>) -> DoctorCheck {
    check(name, DoctorStatus::Skip, message)
}

fn with_fix(mut check: DoctorCheck, fix: impl Into<String>) -> DoctorCheck {
    check.fix = Some(fix.into());
    check
}

/// A check from an error: its message, and its hint as the fix.
fn from_error(name: &str, status: DoctorStatus, info: &ErrorInfo) -> DoctorCheck {
    DoctorCheck {
        name: name.into(),
        status,
        message: info.message.clone(),
        fix: info.hint.clone(),
    }
}

fn this_binary(exe: Option<&Path>) -> DoctorCheck {
    let at = exe.map_or_else(String::new, |p| format!(" at {}", p.display()));
    ok(
        "chrome-cdp",
        format!(
            "chrome-cdp {VERSION} (git {GIT_SHA}, CDP protocol r{}){at}",
            cdp::domains::PROTOCOL_REVISION
        ),
    )
}

/// Finds the Chrome `open` would launch.
fn chrome(explicit: Option<&Path>, host: &Host, checks: &mut Vec<DoctorCheck>) -> Option<Chrome> {
    match browser::discover(explicit, host) {
        Err(e) => {
            checks.push(from_error("chrome", DoctorStatus::Fail, &e.to_error_info()));
            None
        }
        Ok(chrome) => {
            let version = chrome.version.as_ref().map_or_else(
                || "Chrome (version unknown)".to_owned(),
                ToString::to_string,
            );
            let message = format!(
                "{version} at {} ({})",
                chrome.path.display(),
                chrome.source.describe()
            );
            let old = chrome
                .version
                .as_ref()
                .is_some_and(browser::ChromeVersion::is_too_old);
            let entry = if old || chrome.version.is_none() {
                let why = chrome.warnings.first().cloned().unwrap_or(message);
                with_fix(
                    check("chrome", DoctorStatus::Warn, why),
                    format!("{}   (a current Chrome for Testing)", Cmd::new("install")),
                )
            } else {
                ok("chrome", message)
            };
            checks.push(entry);
            Some(chrome)
        }
    }
}

/// Linux: that Chrome starts (`--version`) and, if not, which shared
/// libraries it lacks. Returns whether it starts.
fn libraries(host: &Host, chrome: Option<&Chrome>, checks: &mut Vec<DoctorCheck>) -> bool {
    let Some(chrome) = chrome else {
        checks.push(skip("libraries", "no Chrome to check"));
        return false;
    };
    if host.os != Os::Linux {
        checks.push(skip("libraries", "only checked on Linux"));
        return true;
    }
    match browser::check_runs(host, &chrome.path) {
        Ok(()) => {
            checks.push(ok("libraries", "Chrome finds all its shared libraries"));
            true
        }
        Err(cannot) => {
            let (message, fix) = match &cannot.fix {
                Some(fix) => (
                    format!("Chrome cannot start: missing {}", fix.missing.join(", ")),
                    fix.sudo_line().map_or_else(
                        || {
                            format!(
                                "install the packages that provide {} with the system package \
                                 manager",
                                fix.missing.join(", ")
                            )
                        },
                        |line| format!("{line}   (or chrome-cdp install --with-deps)"),
                    ),
                ),
                None => (
                    format!("Chrome cannot start: {}", cannot.reason),
                    format!("run {} --version to see why", chrome.path.display()),
                ),
            };
            checks.push(with_fix(
                check("libraries", DoctorStatus::Fail, message),
                fix,
            ));
            false
        }
    }
}

/// Linux: whether Chrome's sandbox can start: not as root, and not
/// blocked by Ubuntu 23.10+'s AppArmor rule. Returns false when it cannot.
fn sandbox(host: &Host, chrome: Option<&Chrome>, checks: &mut Vec<DoctorCheck>) -> bool {
    if host.os != Os::Linux {
        checks.push(ok(
            "sandbox",
            "Chrome's sandbox needs nothing from the system here",
        ));
        return true;
    }
    if launch::running_as_root() {
        checks.push(with_fix(
            check(
                "sandbox",
                DoctorStatus::Warn,
                "running as root: Chrome's sandbox cannot start, so chrome-cdp turns it off",
            ),
            "run chrome-cdp as a normal user",
        ));
        return true;
    }
    let Some(chrome) = chrome else {
        checks.push(skip("sandbox", "no Chrome to check"));
        return true;
    };
    let entry = match launch::userns_check(host, &chrome.path) {
        UsernsCheck::NotRestricted => ok("sandbox", "user namespaces are not restricted"),
        UsernsCheck::Covered { profile } => ok(
            "sandbox",
            format!("the AppArmor profile {profile} lets Chrome's sandbox start"),
        ),
        UsernsCheck::SetuidSandbox => ok("sandbox", "Chrome's setuid sandbox helper is installed"),
        UsernsCheck::Skipped => skip(
            "sandbox",
            format!("{} is set", browser::NO_USERNS_CHECK_ENV),
        ),
        UsernsCheck::Unknown => with_fix(
            check(
                "sandbox",
                DoctorStatus::Warn,
                "AppArmor restricts user namespaces, and its profiles cannot be read to tell \
                 whether Chrome may use them",
            ),
            "see the launch check (or run chrome-cdp open) to find out",
        ),
        UsernsCheck::Blocked => from_error(
            "sandbox",
            DoctorStatus::Fail,
            &browser::sandbox_blocked_error(host, chrome),
        ),
    };
    let blocked = entry.status == DoctorStatus::Fail;
    checks.push(entry);
    !blocked
}

/// Starts Chrome headless with a temporary profile and closes it.
fn try_launch(chrome: &Chrome, host: &Host, timeout: Duration) -> DoctorCheck {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            return check(
                "launch",
                DoctorStatus::Fail,
                format!("cannot start the runtime: {e}"),
            );
        }
    };
    let opts = LaunchOptions {
        timeout,
        ..LaunchOptions::default()
    };
    runtime.block_on(async {
        match browser::launch(chrome, &opts, host).await {
            Ok(launched) => {
                let product = launched.product().to_owned();
                let warning = launched.warnings.first().cloned();
                launched.close().await;
                match warning {
                    Some(w) => check("launch", DoctorStatus::Warn, w),
                    None => ok(
                        "launch",
                        format!("started {product} headless and closed it again"),
                    ),
                }
            }
            Err(e) => from_error("launch", DoctorStatus::Fail, &e),
        }
    })
}

/// The dir that holds the session dirs: private and ours, or creatable.
fn session_dir() -> DoctorCheck {
    let env = daemon::paths::Env::current();
    let base = env.base();
    if !base.exists() {
        let parent = base.parent().map(Path::to_path_buf).unwrap_or_default();
        return match parent.metadata() {
            Ok(m) if m.is_dir() && !m.permissions().readonly() => ok(
                "session-dir",
                format!("{} will be made on the first open", base.display()),
            ),
            _ => with_fix(
                check(
                    "session-dir",
                    DoctorStatus::Fail,
                    format!(
                        "{} cannot be made: {} is missing or read-only",
                        base.display(),
                        parent.display()
                    ),
                ),
                set_runtime_dir(&env),
            ),
        };
    }
    match daemon::paths::check_private(&base, env.uid) {
        Ok(()) => ok(
            "session-dir",
            format!("{} is private (0700) and ours", base.display()),
        ),
        Err(e) => with_fix(
            check(
                "session-dir",
                DoctorStatus::Fail,
                format!("{}: {e}", base.display()),
            ),
            format!(
                "chmod 700 {0}, or remove {0} if another user made it; or {1}",
                format::shell_quote(&base.to_string_lossy()),
                set_runtime_dir(&env)
            ),
        ),
    }
}

fn set_runtime_dir(env: &daemon::paths::Env) -> String {
    format!(
        "set {} to a private directory",
        if env.macos {
            "TMPDIR"
        } else {
            "XDG_RUNTIME_DIR"
        }
    )
}

/// macOS: Gatekeeper's quarantine attribute on chrome-cdp or Chrome.
fn quarantine(host: &Host, exe: Option<&Path>, chrome: Option<&Chrome>) -> DoctorCheck {
    if host.os != Os::MacOs {
        return skip("quarantine", "only macOS quarantines downloads");
    }
    let mut found: Vec<PathBuf> = Vec::new();
    if let Some(exe) = exe.filter(|p| browser::quarantined(p)) {
        found.push(exe.to_path_buf());
    }
    if let Some(chrome) = chrome {
        let top = app_bundle(&chrome.path).unwrap_or_else(|| chrome.path.clone());
        if browser::quarantined(&top) || browser::quarantined(&chrome.path) {
            found.push(top);
        }
    }
    if found.is_empty() {
        return ok(
            "quarantine",
            "no quarantine attribute on chrome-cdp or Chrome",
        );
    }
    let paths: Vec<String> = found
        .iter()
        .map(|p| format::shell_quote(&p.to_string_lossy()).into_owned())
        .collect();
    with_fix(
        check(
            "quarantine",
            DoctorStatus::Warn,
            format!(
                "Gatekeeper may block {}: it has the com.apple.quarantine attribute",
                paths.join(" and ")
            ),
        ),
        format!("xattr -dr com.apple.quarantine {}", paths.join(" ")),
    )
}

/// `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome` →
/// `/Applications/Google Chrome.app`.
fn app_bundle(binary: &Path) -> Option<PathBuf> {
    binary
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
}

/// Running sessions whose daemon is another chrome-cdp version.
fn sessions() -> DoctorCheck {
    let env = daemon::paths::Env::current();
    let live = daemon::client::live_sessions(&env);
    if live.is_empty() {
        return ok("sessions", "no sessions running");
    }
    let mut skewed = Vec::new();
    for paths in &live {
        let build = daemon::meta::Meta::read(&paths.meta).map(|m| m.build);
        let version = build
            .as_deref()
            .and_then(|b| b.split('+').next())
            .unwrap_or("an unknown version");
        if version != VERSION {
            skewed.push((paths.name.clone(), version.to_owned()));
        }
    }
    if skewed.is_empty() {
        return ok(
            "sessions",
            format!(
                "{} running, all on chrome-cdp {VERSION}",
                plural(live.len(), "session")
            ),
        );
    }
    let mut message = String::new();
    for (i, (name, version)) in skewed.iter().enumerate() {
        if i > 0 {
            message.push_str("; ");
        }
        let _ = write!(message, "session {name} runs chrome-cdp {version}");
    }
    let _ = write!(
        message,
        " (this is {VERSION}); the next command restarts {}",
        if skewed.len() == 1 { "it" } else { "them" }
    );
    let closes: Vec<String> = skewed
        .iter()
        .map(|(name, _)| Cmd::new("close").session(name).to_string())
        .collect();
    with_fix(
        check("sessions", DoctorStatus::Warn, message),
        format!("{}   (to restart now)", closes.join("; ")),
    )
}

/// `1 warning`, `2 warnings`.
fn plural(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// The output, or with a failed check the error (exit 4).
fn result(checks: Vec<DoctorCheck>) -> Result<Output, ErrorInfo> {
    let count = |s| checks.iter().filter(|c| c.status == s).count();
    let data = DoctorData {
        ok: count(DoctorStatus::Ok),
        warnings: count(DoctorStatus::Warn),
        failed: count(DoctorStatus::Fail),
        skipped: count(DoctorStatus::Skip),
        checks,
    };
    let summary = format!(
        "{} ok, {}, {} failed, {} skipped",
        data.ok,
        plural(data.warnings, "warning"),
        data.failed,
        data.skipped
    );
    let failed: Vec<&DoctorCheck> = data
        .checks
        .iter()
        .filter(|c| c.status == DoctorStatus::Fail)
        .collect();
    if let Some(first) = failed.first() {
        let names: Vec<&str> = failed.iter().map(|c| c.name.as_str()).collect();
        let kind = match first.name.as_str() {
            "chrome" => ErrorKind::ChromeNotFound,
            "session-dir" => ErrorKind::DaemonUnavailable,
            _ => ErrorKind::LaunchFailed,
        };
        let mut info = ErrorInfo::new(
            kind,
            format!(
                "{} of {} checks failed: {}",
                failed.len(),
                data.checks.len(),
                names.join(", ")
            ),
        );
        for c in &data.checks {
            info = info.note(
                c.name.clone(),
                format!("{} {}", c.status.label(), c.message),
            );
        }
        for c in failed.iter().skip(1) {
            if let Some(fix) = &c.fix {
                info = info.note(format!("fix {}", c.name), fix.clone());
            }
        }
        if let Some(fix) = &first.fix {
            info = info.hint(fix.clone());
        }
        return Err(info
            .note("checked", summary)
            .details(serde_json::to_value(&data).unwrap_or_default()));
    }
    let width = data.checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
    let mut text = String::new();
    for c in &data.checks {
        let _ = writeln!(
            text,
            "{:<4}  {:<width$}  {}",
            c.status.label(),
            c.name,
            c.message
        );
        if let Some(fix) = &c.fix {
            let _ = writeln!(text, "      {:<width$}  fix: {fix}", "");
        }
    }
    text.push_str(&summary);
    let quiet = if data.warnings == 0 { "ok" } else { "warn" };
    Ok(Output::typed(&data, text).quiet(quiet))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use protocol::ErrorKind;
    use protocol::schema::DoctorStatus;

    use super::{app_bundle, check, ok, result, skip, with_fix};

    #[test]
    fn finds_the_app_bundle() {
        assert_eq!(
            app_bundle(Path::new(
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
            ))
            .as_deref(),
            Some(Path::new("/Applications/Google Chrome.app"))
        );
        assert_eq!(app_bundle(Path::new("/usr/bin/chromium")), None);
    }

    #[test]
    fn lists_the_checks_with_their_fixes() {
        let out = result(vec![
            ok("chrome", "Chrome 154 at /c"),
            with_fix(
                check("sessions", DoctorStatus::Warn, "session x runs 0.0.9"),
                "chrome-cdp close --session x",
            ),
            skip("quarantine", "only macOS"),
        ])
        .unwrap();
        assert_eq!(
            out.text,
            "ok    chrome      Chrome 154 at /c\n\
             warn  sessions    session x runs 0.0.9\n\
             \x20                 fix: chrome-cdp close --session x\n\
             skip  quarantine  only macOS\n\
             1 ok, 1 warning, 0 failed, 1 skipped"
        );
    }

    #[test]
    fn a_failed_check_fails_with_its_fix_as_the_hint() {
        let err = result(vec![
            ok("chrome-cdp", "chrome-cdp 0.1.0"),
            with_fix(
                check("libraries", DoctorStatus::Fail, "missing libnss3.so"),
                "sudo apt-get install -y libnss3",
            ),
            with_fix(
                check("session-dir", DoctorStatus::Fail, "mode 755"),
                "chmod 700 /run/user/1/chrome-cdp",
            ),
        ])
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::LaunchFailed);
        assert_eq!(err.message, "2 of 3 checks failed: libraries, session-dir");
        assert_eq!(err.hint.as_deref(), Some("sudo apt-get install -y libnss3"));
        let notes: Vec<String> = err
            .notes
            .iter()
            .map(|n| format!("{}: {}", n.label, n.text))
            .collect();
        assert_eq!(
            notes,
            [
                "chrome-cdp: ok chrome-cdp 0.1.0",
                "libraries: FAIL missing libnss3.so",
                "session-dir: FAIL mode 755",
                "fix session-dir: chmod 700 /run/user/1/chrome-cdp",
                "checked: 1 ok, 0 warnings, 2 failed, 0 skipped",
            ]
        );
        assert_eq!(err.details.unwrap()["failed"], 2);
    }
}
