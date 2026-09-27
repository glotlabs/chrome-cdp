//! End-to-end tests of the `chrome-cdp` binary: flags, output modes, exit
//! codes, and `__direct cdp` against a real Chrome.
//!
//! The real-Chrome tests need `CHROME_PATH`. Without it they pass as no-ops,
//! unless `CHROME_CDP_REQUIRE_CHROME=1` (set in CI), in which case they fail.

// clippy.toml's test allowances cover `#[test]` fns, not their helpers.
#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

fn chrome_path() -> Option<PathBuf> {
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

/// The binary with a clean `CHROME_CDP_*` environment.
fn cli() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_chrome-cdp"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CHROME_CDP_") {
            cmd.env_remove(key);
        }
    }
    cmd.stdin(Stdio::null());
    cmd
}

fn run(cmd: &mut Command) -> Output {
    let out = cmd.output().unwrap();
    eprintln!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap()
}

/// Parses stdout as exactly one JSON object.
fn json_out(out: &Output) -> Value {
    let text = stdout(out);
    assert_eq!(text.lines().count(), 1, "one line of JSON: {text}");
    serde_json::from_str(&text).unwrap()
}

fn direct_cdp(chrome: &Path, args: &[&str]) -> Command {
    let mut cmd = cli();
    cmd.args(["__direct", "cdp", "--chrome"])
        .arg(chrome)
        .args(args);
    cmd
}

/// A stand-in "Chrome": a shell script with the given body.
fn fake_chrome(dir: &tempfile::TempDir, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let path = dir.path().join("chrome");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn version_names_crate_git_and_protocol() {
    let out = run(cli().arg("--version"));
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    let expected_prefix = format!("chrome-cdp {} (git ", env!("CARGO_PKG_VERSION"));
    assert!(text.starts_with(&expected_prefix), "{text}");
    assert!(
        text.trim_end().ends_with(&format!(
            ", CDP protocol r{})",
            cdp::domains::PROTOCOL_REVISION
        )),
        "{text}"
    );
    assert!(stderr(&out).is_empty());

    let out = run(cli().args(["-V", "--json"]));
    let v = json_out(&out);
    assert_eq!(v["ok"], json!(true));
    assert_eq!(v["data"]["version"], json!(env!("CARGO_PKG_VERSION")));
    assert_eq!(
        v["data"]["protocol_revision"],
        json!(cdp::domains::PROTOCOL_REVISION)
    );
    let sha = v["data"]["git_sha"].as_str().unwrap();
    assert!(!sha.is_empty());

    let out = run(cli().args(["-V", "-q"]));
    assert_eq!(stdout(&out), format!("{}\n", env!("CARGO_PKG_VERSION")));
}

#[test]
fn help_exits_zero_and_hides_direct() {
    let out = run(cli().arg("--help"));
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    for flag in ["--json", "--quiet", "--session", "--timeout", "--verbose"] {
        assert!(text.contains(flag), "{flag} missing from help");
    }
    assert!(!text.contains("__direct"));
}

#[test]
fn no_command_is_a_usage_error() {
    let out = run(&mut cli());
    assert_eq!(code(&out), 2);
    assert!(stdout(&out).is_empty());
    assert_eq!(
        stderr(&out),
        "error: no command given\n  hint: chrome-cdp --help\n"
    );
}

#[test]
fn usage_errors_exit_2_and_honour_json() {
    let out = run(cli().arg("--bogus"));
    assert_eq!(code(&out), 2);
    assert!(stdout(&out).is_empty());
    assert!(stderr(&out).starts_with("error: unexpected argument '--bogus'"));

    let out = run(cli().args(["--bogus", "--json"]));
    assert_eq!(code(&out), 2);
    let v = json_out(&out);
    assert_eq!(v["ok"], json!(false));
    assert_eq!(v["error"]["kind"], json!("BadArgs"));
    assert_eq!(
        v["error"]["message"],
        json!("unexpected argument '--bogus' found")
    );

    let out = run(cli().args(["--timeout", "5000", "-V"]));
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("needs a unit"), "{}", stderr(&out));
}

#[test]
fn global_flags_have_env_vars() {
    let out = run(cli().arg("-V").env("CHROME_CDP_JSON", "1"));
    assert_eq!(json_out(&out)["ok"], json!(true));
    let out = run(cli().arg("-V").env("CHROME_CDP_QUIET", "true"));
    assert_eq!(stdout(&out), format!("{}\n", env!("CARGO_PKG_VERSION")));
    let out = run(cli().arg("-V").env("CHROME_CDP_TIMEOUT", "soon"));
    assert_eq!(code(&out), 2);
    // A usage error found through the env var is still JSON.
    let out = run(cli().arg("--bogus").env("CHROME_CDP_JSON", "1"));
    assert_eq!(json_out(&out)["error"]["kind"], json!("BadArgs"));
}

#[test]
fn direct_checks_arguments_before_launching() {
    // `/nonexistent` would fail with ChromeNotFound if it were launched.
    let chrome = Path::new("/nonexistent/chrome");
    let out = run(&mut direct_cdp(chrome, &["getVersion", "--json"]));
    assert_eq!(code(&out), 2);
    assert_eq!(json_out(&out)["error"]["kind"], json!("BadArgs"));

    let out = run(&mut direct_cdp(
        chrome,
        &["Runtime.evaluate", "{nope", "--json"],
    ));
    assert_eq!(code(&out), 2);
    let err = &json_out(&out)["error"];
    assert!(
        err["message"]
            .as_str()
            .unwrap()
            .starts_with("params are not valid JSON")
    );
    assert!(
        err["hint"]
            .as_str()
            .unwrap()
            .contains(r#"'{"expression":"1+1"}'"#)
    );

    let out = run(&mut direct_cdp(chrome, &["Browser.getVersion"]));
    assert_eq!(code(&out), 4);
    assert_eq!(
        stderr(&out),
        "error: no Chrome at /nonexistent/chrome\n  \
         hint: pass --chrome with the path of a Chrome or Chromium binary\n"
    );

    let out = run(cli().args(["__direct", "cdp", "Browser.getVersion", "--json"]));
    assert_eq!(code(&out), 4);
    assert_eq!(json_out(&out)["error"]["kind"], json!("ChromeNotFound"));
}

#[test]
fn direct_reports_why_chrome_failed_to_start() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = fake_chrome(
        &dir,
        "echo '[1:1:0927/101010.1:FATAL:zygote_host_impl_linux.cc(128)] No usable sandbox! \
         If you are running on Ubuntu 23.10+ ...' >&2\nexit 1",
    );
    let out = run(&mut direct_cdp(&chrome, &["Browser.getVersion"]));
    assert_eq!(code(&out), 4);
    let err = stderr(&out);
    assert!(
        err.starts_with(
            "error: Chrome exited during startup: No usable sandbox! If you are running on Ubuntu"
        ),
        "{err}"
    );
    assert!(
        err.contains("hint: the kernel blocks Chrome's sandbox"),
        "{err}"
    );

    let out = run(&mut direct_cdp(&chrome, &["Browser.getVersion", "--json"]));
    let v = json_out(&out);
    assert_eq!(v["error"]["kind"], json!("LaunchFailed"));
    let lines = v["error"]["details"]["chrome_stderr"].as_array().unwrap();
    assert_eq!(lines.len(), 1);
}

#[test]
fn direct_times_out_on_a_silent_chrome() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = fake_chrome(&dir, "exec sleep 30");
    let started = std::time::Instant::now();
    let out = run(&mut direct_cdp(
        &chrome,
        &["Browser.getVersion", "--timeout", "300ms", "--json"],
    ));
    assert_eq!(code(&out), 3);
    let v = json_out(&out);
    assert_eq!(v["error"]["kind"], json!("Timeout"));
    assert_eq!(
        v["error"]["message"],
        json!("Browser.getVersion: no response from Chrome after 300ms")
    );
    // The hung child is killed, not waited for.
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn direct_browser_get_version() {
    let Some(chrome) = chrome_path() else { return };
    let out = run(&mut direct_cdp(&chrome, &["Browser.getVersion"]));
    assert_eq!(code(&out), 0);
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert!(v["product"].as_str().unwrap().contains("Chrome"), "{v}");
    assert_eq!(v["protocolVersion"], json!("1.3"));
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));

    let out = run(&mut direct_cdp(&chrome, &["Browser.getVersion", "--json"]));
    let v = json_out(&out);
    assert_eq!(v["ok"], json!(true));
    assert_eq!(v["data"]["protocolVersion"], json!("1.3"));
}

#[test]
fn direct_page_evaluate_from_stdin() {
    let Some(chrome) = chrome_path() else { return };
    let mut child = direct_cdp(
        &chrome,
        &["--target", "page", "Runtime.evaluate", "-", "-q"],
    )
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"expression":"6*7","returnByValue":true}"#)
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "{\"result\":{\"description\":\"42\",\"type\":\"number\",\"value\":42}}\n"
    );
}

#[test]
fn direct_protocol_errors_and_timeouts() {
    let Some(chrome) = chrome_path() else { return };
    // A page method sent to the browser.
    let out = run(&mut direct_cdp(
        &chrome,
        &["Runtime.evaluate", r#"{"expression":"1"}"#],
    ));
    assert_eq!(code(&out), 4);
    assert_eq!(
        stderr(&out),
        "error: Runtime.evaluate: 'Runtime.evaluate' wasn't found\n  \
         hint: if Runtime.evaluate is a page method, send it to a page: \
         chrome-cdp __direct cdp --target page Runtime.evaluate …\n"
    );

    // Chrome answers, but only after the timeout.
    let out = run(&mut direct_cdp(
        &chrome,
        &[
            "--target",
            "page",
            "Runtime.evaluate",
            r#"{"expression":"new Promise(() => {})","awaitPromise":true}"#,
            "--timeout",
            "500ms",
            "--json",
        ],
    ));
    assert_eq!(code(&out), 3);
    let err = &json_out(&out)["error"];
    assert_eq!(err["kind"], json!("Timeout"));
    assert_eq!(
        err["hint"],
        json!("retry with a longer timeout, e.g. --timeout 1s")
    );
}

#[test]
fn direct_output_is_bounded() {
    let Some(chrome) = chrome_path() else { return };
    let out = run(&mut direct_cdp(
        &chrome,
        &[
            "--target",
            "page",
            "Runtime.evaluate",
            r#"{"expression":"'x'.repeat(100000)","returnByValue":true}"#,
        ],
    ));
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    let last = text.lines().last().unwrap();
    assert!(
        last.starts_with("… truncated ")
            && last.ends_with("use --max-chars 0 for all of it, or --json"),
        "{last}"
    );
    assert!(text.chars().count() < 20_200);

    // --json is never truncated: it must stay valid JSON.
    let out = run(&mut direct_cdp(
        &chrome,
        &[
            "--target",
            "page",
            "Runtime.evaluate",
            r#"{"expression":"'x'.repeat(100000)","returnByValue":true}"#,
            "--json",
        ],
    ));
    let v = json_out(&out);
    assert_eq!(
        v["data"]["result"]["value"].as_str().unwrap().len(),
        100_000
    );
}
