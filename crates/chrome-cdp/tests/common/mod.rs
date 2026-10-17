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

/// A temp dir under its canonical path. macOS's temp dir is behind a
/// symlink (`/var` is `/private/var`), and chrome-cdp prints canonical
/// paths, so a test that compares them needs the same form.
pub fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir_in(temp_dir()).unwrap()
}

/// [`std::env::temp_dir`], canonical.
pub fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir();
    dir.canonicalize().unwrap_or(dir)
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
        .current_dir(temp_dir())
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
        let dir = tempdir();
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

    /// Where sessions live (under `chrome-cdp/`): `XDG_RUNTIME_DIR` on
    /// Linux, `TMPDIR` on macOS, as `daemon::paths` has it.
    pub fn runtime_dir(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.tmp()
        } else {
            self.dir.path().join("run")
        }
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
    /// pid files, shortened dirs included.
    pub fn daemons(&self) -> Vec<u32> {
        let homes = std::fs::read_dir(self.runtime_dir().join("chrome-cdp"));
        homes
            .into_iter()
            .flatten()
            .flatten()
            .map(|d| d.path())
            .chain(self.short_dirs())
            .filter_map(|d| std::fs::read_to_string(d.join("pid")).ok())
            .filter_map(|pid| pid.trim().parse().ok())
            .filter(|&pid| is_daemon(pid))
            .collect()
    }

    /// The dirs of this sandbox's sessions whose socket path was too long
    /// for their home (`/tmp/ccdp-<uid>/<hash>/`, as `daemon::paths` makes
    /// them), found by the home their `meta.json` names.
    pub fn short_dirs(&self) -> Vec<PathBuf> {
        let base = daemon::paths::Env::current().short_base();
        let Ok(dirs) = std::fs::read_dir(base) else {
            return Vec::new();
        };
        let run = self.runtime_dir();
        dirs.flatten()
            .map(|d| d.path())
            .filter(|dir| {
                std::fs::read(dir.join("meta.json"))
                    .ok()
                    .and_then(|m| serde_json::from_slice::<daemon::meta::Meta>(&m).ok())
                    .is_some_and(|m| m.home.starts_with(&run))
            })
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
        // On macOS the sessions' own dirs share TMPDIR with Chrome's.
        wait_until("Chrome's run dirs to be deleted", || {
            std::fs::read_dir(self.tmp())
                .unwrap()
                .flatten()
                .all(|e| self.tmp() == self.runtime_dir() && e.file_name() == "chrome-cdp")
        });
    }
}

impl Drop for Sandbox {
    /// Closes every session and then checks for leaks (plan 10.1): no
    /// daemon or Chrome of this sandbox may still run, and no socket file
    /// may be left in its session dirs, since `close` waits for both to go.
    /// A leak fails the test that caused it (unless it failed already),
    /// and the sandbox's dir is kept for a look.
    fn drop(&mut self) {
        let _ = self.cli().args(["close", "--all"]).output();
        let leaks = self.leaks();
        if leaks.is_empty() || std::thread::panicking() {
            return;
        }
        let kept = std::mem::replace(&mut self.dir, tempdir()).keep();
        panic!(
            "the sandbox leaked after `close --all` (kept in {}):\n  {}",
            kept.display(),
            leaks.join("\n  ")
        );
    }
}

impl Sandbox {
    /// What is left of this sandbox's sessions: live daemons and Chrome
    /// processes (after a grace period of 5 s, as a browser's helpers exit
    /// shortly after it) and socket files in its session dirs.
    pub fn leaks(&self) -> Vec<String> {
        let start = std::time::Instant::now();
        while !(self.daemons().is_empty() && self.chromes().is_empty())
            && start.elapsed() < std::time::Duration::from_secs(5)
        {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let mut leaks: Vec<String> = self
            .daemons()
            .into_iter()
            .map(|pid| format!("daemon {pid}"))
            .collect();
        leaks.extend(self.chromes().into_iter().map(|p| format!("process {p}")));
        let dirs = std::iter::once(self.runtime_dir()).chain(self.short_dirs());
        leaks.extend(
            dirs.flat_map(|d| sockets_in(&d))
                .map(|s| format!("socket {}", s.display())),
        );
        leaks
    }

    /// A session with a name no other test uses, closed when dropped.
    pub fn session(&self) -> Session<'_> {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Session {
            sandbox: self,
            name: format!("t{}-{n}", std::process::id()),
        }
    }
}

/// The socket files under `dir`, at any depth.
pub fn sockets_in(dir: &Path) -> Vec<PathBuf> {
    use std::os::unix::fs::FileTypeExt as _;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_socket() {
            found.push(entry.path());
        } else if kind.is_dir() {
            found.extend(sockets_in(&entry.path()));
        }
    }
    found
}

/// One `--session` of a [`Sandbox`] (plan 10.1's E2E harness): every
/// command runs with its name, and dropping it runs `close` for it, even
/// when the test failed.
pub struct Session<'a> {
    pub sandbox: &'a Sandbox,
    pub name: String,
}

impl Session<'_> {
    /// The sandbox's `cli()` with `--session <name>`.
    pub fn cli(&self) -> Command {
        let mut cmd = self.sandbox.cli();
        cmd.env("CHROME_CDP_SESSION", &self.name);
        cmd
    }

    /// Runs `chrome-cdp <args>` in this session.
    pub fn run(&self, args: &[&str]) -> Output {
        run(self.cli().args(args))
    }

    /// Runs a command that must succeed and write nothing to stderr, and
    /// returns its stdout.
    pub fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert_eq!(
            code(&out),
            0,
            "chrome-cdp {}: {}",
            args.join(" "),
            stderr(&out)
        );
        assert_eq!(stderr(&out), "", "chrome-cdp {}", args.join(" "));
        stdout(&out)
    }

    /// Runs a command with `--json` that must succeed, and returns `data`.
    pub fn json(&self, args: &[&str]) -> Value {
        let out = self.run(&[args, &["--json"]].concat());
        assert_eq!(
            code(&out),
            0,
            "chrome-cdp {}: {}",
            args.join(" "),
            stderr(&out)
        );
        let envelope = json_out(&out);
        assert_eq!(envelope["ok"], true, "{envelope}");
        envelope["data"].clone()
    }

    /// Runs a command with `--json` that must fail with exit code `exit`
    /// and error kind `kind`, and returns the error object.
    pub fn fails(&self, args: &[&str], exit: i32, kind: &str) -> Value {
        let out = self.run(&[args, &["--json"]].concat());
        assert_eq!(code(&out), exit, "chrome-cdp {}", args.join(" "));
        let error = json_out(&out)["error"].clone();
        assert_eq!(error["kind"], kind, "{error}");
        error
    }
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        let _ = self.cli().arg("close").output();
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
