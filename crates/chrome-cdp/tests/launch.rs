//! Launching Chrome (plan 02, milestone 2.3), through `__direct cdp`:
//! nothing outlives chrome-cdp, profiles, and launch options.
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::path::Path;
use std::process::{Child, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use common::{
    chrome_path, code, direct_cdp, fake_chrome, json_out, processes_mentioning, run, stderr,
    stdout, wait_until,
};
use serde_json::json;

/// A call that never returns: the CLI sits mid-call until killed.
const HANG: &[&str] = &[
    "--target",
    "page",
    "Runtime.evaluate",
    r#"{"expression":"new Promise(() => {})","awaitPromise":true}"#,
    "--timeout",
    "60s",
];

/// Starts a hanging call whose temporary profile lives in `tmp`, and waits
/// until its Chrome runs.
fn hanging_call(chrome: &Path, tmp: &Path, extra: &[&str]) -> Child {
    let child = direct_cdp(chrome, HANG)
        .args(extra)
        .env("TMPDIR", tmp)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_until("Chrome to start", || !processes_mentioning(tmp).is_empty());
    child
}

/// Nothing runs from `dir`, and a temporary profile there is gone.
fn assert_all_gone(dir: &Path) {
    wait_until("Chrome's processes to exit", || {
        processes_mentioning(dir).is_empty()
    });
    wait_until("the temporary profile to be deleted", || {
        std::fs::read_dir(dir).unwrap().next().is_none()
    });
}

#[test]
fn closing_leaves_no_process_or_profile_behind() {
    let Some(chrome) = chrome_path() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let out = run(direct_cdp(&chrome, &["Browser.getVersion", "-q"]).env("TMPDIR", tmp.path()));
    assert_eq!(code(&out), 0);
    assert_all_gone(tmp.path());
}

#[test]
fn chrome_dies_with_a_killed_cli() {
    let Some(chrome) = chrome_path() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let mut child = hanging_call(&chrome, tmp.path(), &[]);
    child.kill().unwrap();
    child.wait().unwrap();
    // PR_SET_PDEATHSIG (Linux) and the watchdog kill the process group;
    // the watchdog also deletes the temporary profile.
    assert_all_gone(tmp.path());
}

#[test]
fn sigterm_closes_chrome_and_deletes_the_profile() {
    let Some(chrome) = chrome_path() else { return };
    for (signal, name, exit) in [("TERM", "SIGTERM", 143), ("INT", "SIGINT", 130)] {
        let tmp = tempfile::tempdir().unwrap();
        let child = hanging_call(&chrome, tmp.path(), &[]);
        let status = std::process::Command::new("kill")
            .args([format!("-{signal}"), child.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
        let out = child.wait_with_output().unwrap();
        assert_eq!(code(&out), exit, "{name}");
        assert_eq!(
            stderr(&out),
            format!("error: interrupted by {name}; Chrome was closed\n")
        );
        assert_all_gone(tmp.path());
    }
}

#[test]
fn a_named_profile_keeps_cookies_between_runs() {
    let Some(chrome) = chrome_path() else { return };
    if !cfg!(target_os = "linux") {
        return; // The data dir is under ~/Library on macOS.
    }
    let data = tempfile::tempdir().unwrap();
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    let cookie = json!({"cookies": [{
        "name": "session", "value": "kept", "domain": "example.test", "path": "/",
        "expires": expires,
    }]});
    let with_profile = |args: &[&str]| {
        let mut cmd = direct_cdp(&chrome, args);
        cmd.args(["--profile", "work"])
            .env("XDG_DATA_HOME", data.path());
        run(&mut cmd)
    };
    let out = with_profile(&["Storage.setCookies", &cookie.to_string()]);
    assert_eq!(code(&out), 0);
    let profile = data.path().join("chrome-cdp/profiles/work");
    assert!(profile.join("Default").is_dir(), "{}", profile.display());

    let out = with_profile(&["Storage.getCookies", "--json"]);
    let cookies = &json_out(&out)["data"]["cookies"];
    assert_eq!(cookies[0]["value"], json!("kept"), "{cookies}");
}

#[test]
fn a_profile_in_use_is_refused() {
    let Some(chrome) = chrome_path() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let profile = tmp.path().join("profile");
    let dir = profile.to_str().unwrap();
    let mut first = hanging_call(&chrome, tmp.path(), &["--user-data-dir", dir]);
    wait_until("the profile lock", || {
        profile.join("SingletonLock").symlink_metadata().is_ok()
    });

    let out = run(&mut direct_cdp(
        &chrome,
        &["Browser.getVersion", "--user-data-dir", dir],
    ));
    assert_eq!(code(&out), 4);
    assert!(
        stderr(&out).starts_with(&format!(
            "error: the profile {dir} is in use by another Chrome (pid "
        )),
        "{}",
        stderr(&out)
    );
    first.kill().unwrap();
    first.wait().unwrap();
    // Kept profiles stay.
    wait_until("Chrome to exit", || {
        processes_mentioning(&profile).is_empty()
    });
    assert!(profile.is_dir());
}

#[test]
fn chrome_args_reach_chrome() {
    let Some(chrome) = chrome_path() else { return };
    let out = run(&mut direct_cdp(
        &chrome,
        &[
            "--target",
            "page",
            "Runtime.evaluate",
            r#"{"expression":"navigator.userAgent","returnByValue":true}"#,
            "--chrome-arg=--user-agent=chrome-cdp-test",
            "-q",
        ],
    ));
    assert_eq!(code(&out), 0);
    assert!(
        stdout(&out).contains(r#""value":"chrome-cdp-test""#),
        "{}",
        stdout(&out)
    );
}

#[test]
fn launch_options_are_checked_before_launching() {
    let dir = tempfile::tempdir().unwrap();
    // It would hang if launched.
    let chrome = fake_chrome(&dir, "exec sleep 30");

    let out = run(&mut direct_cdp(
        &chrome,
        &["Browser.getVersion", "--chrome-arg", "https://example.com"],
    ));
    assert_eq!(code(&out), 2);
    assert_eq!(
        stderr(&out),
        "error: --chrome-arg `https://example.com` is not a Chrome flag\n  \
         hint: pass flags in their --name=value form, e.g. --chrome-arg=--window-size=1280,800\n"
    );

    let out = run(&mut direct_cdp(
        &chrome,
        &["Browser.getVersion", "--profile", "a/b"],
    ));
    assert_eq!(code(&out), 2);

    let out = run(&mut direct_cdp(
        &chrome,
        &[
            "Browser.getVersion",
            "--profile",
            "a",
            "--user-data-dir",
            "/x",
        ],
    ));
    assert_eq!(code(&out), 2);

    let home = dir.path().join("home");
    std::fs::create_dir_all(home.join(".config/google-chrome")).unwrap();
    let out = run(direct_cdp(
        &chrome,
        &["Browser.getVersion", "--json", "--user-data-dir"],
    )
    .arg(home.join(".config/google-chrome"))
    .env("HOME", &home));
    let error = &json_out(&out)["error"];
    if cfg!(target_os = "linux") {
        assert_eq!(error["kind"], json!("BadArgs"), "{error}");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .ends_with("is your everyday Chrome profile")
        );
    }

    if cfg!(target_os = "linux") {
        let out = run(direct_cdp(&chrome, &["Browser.getVersion", "--headed"])
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY"));
        assert_eq!(code(&out), 4);
        assert_eq!(
            stderr(&out),
            "error: --headed needs a display, and DISPLAY and WAYLAND_DISPLAY are unset\n  \
             hint: drop --headed, or run it under a virtual display: xvfb-run -a chrome-cdp …\n"
        );
    }
}
