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

/// The binary with a clean `CHROME_CDP_*` environment and no config files
/// (an empty user config dir; run from the temp dir, away from the repo).
pub fn cli() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_chrome-cdp"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CHROME_CDP_") {
            cmd.env_remove(key);
        }
    }
    cmd.env("XDG_CONFIG_HOME", empty_config_home())
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

pub fn direct_cdp(chrome: &Path, args: &[&str]) -> Command {
    let mut cmd = cli();
    cmd.args(["__direct", "cdp", "--chrome"])
        .arg(chrome)
        .args(args);
    cmd
}

/// The version [`fake_chrome`] reports.
pub const FAKE_VERSION: &str = "154.0.0.0";

/// A stand-in "Chrome": a shell script with the given body. It answers
/// `--version` like a current Chrome, as discovery asks for it first.
pub fn fake_chrome(dir: &tempfile::TempDir, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let path = dir.path().join("chrome");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n[ \"$1\" = --version ] && {{ echo 'Chromium {FAKE_VERSION}'; exit 0; }}\n{body}\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// The processes whose command line mentions `marker` (from `ps`, so it
/// works on Linux and macOS).
pub fn processes_mentioning(marker: &Path) -> Vec<String> {
    let out = Command::new("ps")
        .args(["-eo", "pid=,args="])
        .output()
        .unwrap();
    let marker = marker.to_string_lossy();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains(marker.as_ref()))
        .map(str::to_owned)
        .collect()
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
