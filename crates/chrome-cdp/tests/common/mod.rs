//! Helpers shared by the binary's integration tests.

// Each test crate uses a different subset.
#![allow(dead_code, unreachable_pub)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

pub fn chrome_path() -> Option<PathBuf> {
    match std::env::var_os("CHROME_PATH") {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => {
            assert!(
                std::env::var_os("CHROME_CDP_REQUIRE_CHROME").is_none(),
                "CHROME_CDP_REQUIRE_CHROME is set but CHROME_PATH is not"
            );
            eprintln!("skipping: set CHROME_PATH to run tests against a real Chrome");
            None
        }
    }
}

/// An empty `XDG_CONFIG_HOME`, so no user config applies. It must exist and
/// be writable: Chrome keeps its crash reports there and dies without it.
pub fn empty_config_home() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("empty-config-home");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A runtime dir no session command should reach: `cli()` points
/// `XDG_RUNTIME_DIR` here, so a stray session never touches the user's.
/// Tests with sessions use a [`Sandbox`].
pub fn no_sessions_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("no-sessions");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The binary with a clean `CHROME_CDP_*` environment and no config files
/// (an empty user config dir; run from the temp dir, away from the repo).
/// The AppArmor userns check is off, so output does not depend on whether
/// the machine has Ubuntu's rule; tests of the check remove the variable.
pub fn cli() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_chrome-cdp"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CHROME_CDP_") {
            cmd.env_remove(key);
        }
    }
    cmd.env("XDG_CONFIG_HOME", empty_config_home())
        .env("CHROME_CDP_NO_USERNS_CHECK", "1")
        .env("XDG_RUNTIME_DIR", no_sessions_dir())
        .env_remove("NO_COLOR")
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null());
    cmd
}

pub fn run(cmd: &mut Command) -> Output {
    let out = cmd.output().unwrap();
    eprintln!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

pub fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}

pub fn code(out: &Output) -> i32 {
    out.status.code().unwrap()
}

/// Parses stdout as exactly one JSON object.
pub fn json_out(out: &Output) -> Value {
    let text = stdout(out);
    assert_eq!(text.lines().count(), 1, "one line of JSON: {text}");
    serde_json::from_str(&text).unwrap()
}

/// A private place for sessions: its own `XDG_RUNTIME_DIR` (session dirs)
/// and `TMPDIR` (Chrome's run dirs), and a stray daemon's idle timeout of
/// two minutes. Dropping it closes every session it started.
pub struct Sandbox {
    pub dir: tempfile::TempDir,
    /// The Chrome sessions launch (`CHROME_CDP_CHROME`), if any.
    pub chrome: Option<PathBuf>,
}

impl Sandbox {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("run")).unwrap();
        std::fs::create_dir_all(dir.path().join("tmp")).unwrap();
        Self { dir, chrome: None }
    }

    /// A sandbox whose sessions launch `chrome`.
    pub fn with_chrome(chrome: &Path) -> Self {
        let mut sandbox = Self::new();
        sandbox.chrome = Some(chrome.to_owned());
        sandbox
    }

    pub fn runtime_dir(&self) -> PathBuf {
        self.dir.path().join("run")
    }

    /// Chrome's `TMPDIR`: its run dirs and temporary profiles.
    pub fn tmp(&self) -> PathBuf {
        self.dir.path().join("tmp")
    }

    /// `cli()` in this sandbox.
    pub fn cli(&self) -> Command {
        let mut cmd = cli();
        cmd.env("XDG_RUNTIME_DIR", self.runtime_dir())
            .env("TMPDIR", self.tmp())
            .env("CHROME_CDP_IDLE_TIMEOUT", "2m");
        if let Some(chrome) = &self.chrome {
            cmd.env("CHROME_CDP_CHROME", chrome);
        }
        cmd
    }

    /// Runs `chrome-cdp <args>` here.
    pub fn run(&self, args: &[&str]) -> Output {
        run(self.cli().args(args))
    }

    /// The pids of this sandbox's live daemons, from their session dirs'
    /// pid files (sessions with shortened dirs are not included).
    pub fn daemons(&self) -> Vec<u32> {
        let Ok(dirs) = std::fs::read_dir(self.runtime_dir().join("chrome-cdp")) else {
            return Vec::new();
        };
        dirs.flatten()
            .filter_map(|d| std::fs::read_to_string(d.path().join("pid")).ok())
            .filter_map(|pid| pid.trim().parse().ok())
            .filter(|&pid| is_daemon(pid))
            .collect()
    }

    /// Chrome's processes started from this sandbox.
    pub fn chromes(&self) -> Vec<String> {
        processes_mentioning(&self.tmp())
    }

    /// Waits until nothing of this sandbox runs and no run dir is left.
    pub fn assert_all_gone(&self) {
        wait_until("the daemons to exit", || self.daemons().is_empty());
        wait_until("Chrome's processes to exit", || self.chromes().is_empty());
        wait_until("Chrome's run dirs to be deleted", || {
            std::fs::read_dir(self.tmp()).unwrap().next().is_none()
        });
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = self.cli().args(["close", "--all"]).output();
    }
}

/// Whether `pid` is a live chrome-cdp daemon.
pub fn is_daemon(pid: u32) -> bool {
    let out = Command::new("ps")
        .args(["-o", "args=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains("__daemon")
}

/// The processes whose command line contains `needle`.
pub fn processes_matching(needle: &str) -> Vec<String> {
    let out = Command::new("ps")
        .args(["-eo", "pid=,args="])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains(needle))
        .map(|l| l.trim().to_owned())
        .collect()
}

/// The version [`fake_chrome`] reports.
pub const FAKE_VERSION: &str = "154.0.0.0";

/// A stand-in "Chrome": a shell script with the given body. It answers
/// `--version` like a current Chrome, as discovery asks for it first.
pub fn fake_chrome(dir: &tempfile::TempDir, body: &str) -> PathBuf {
    use std::io::Write as _;
    let path = dir.path().join("chrome");
    // A child process writes it: a file this process had open for writing
    // could be inherited by a process another test forks meanwhile, and
    // running it would then fail with "Text file busy".
    let mut child = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(&path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    write!(
        stdin,
        "#!/bin/sh\n[ \"$1\" = --version ] && {{ echo 'Chromium {FAKE_VERSION}'; exit 0; }}\n{body}\n"
    )
    .unwrap();
    drop(stdin);
    assert!(child.wait().unwrap().success());
    path
}

/// The processes whose command line mentions `marker` (from `ps`, so it
/// works on Linux and macOS).
pub fn processes_mentioning(marker: &Path) -> Vec<String> {
    processes_matching(&marker.to_string_lossy())
}

/// Polls `done` every 50 ms for up to 15 s.
pub fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let start = std::time::Instant::now();
    while !done() {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(15),
            "timed out waiting for {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
