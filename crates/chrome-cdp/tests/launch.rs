//! Launching Chrome (plan 02, milestone 2.3) through `open`: profiles and
//! launch options. Sessions and processes are in `session.rs`.
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::time::{SystemTime, UNIX_EPOCH};

use common::{Sandbox, chrome_path, code, fake_chrome, json_out, run, stderr, stdout, wait_until};
use serde_json::json;

#[test]
fn a_named_profile_keeps_cookies_between_runs() {
    let Some(chrome) = chrome_path() else { return };
    if !cfg!(target_os = "linux") {
        return; // The data dir is under ~/Library on macOS.
    }
    let sandbox = Sandbox::with_chrome(&chrome);
    let data = sandbox.dir.path().join("data");
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    let cookie = json!({"cookies": [{
        "name": "session", "value": "kept", "domain": "example.test", "path": "/",
        "expires": expires,
    }]});
    let with_data = |args: &[&str]| run(sandbox.cli().args(args).env("XDG_DATA_HOME", &data));
    assert_eq!(code(&with_data(&["open", "--profile", "work"])), 0);
    let out = with_data(&["cdp", "Storage.setCookies", &cookie.to_string()]);
    assert_eq!(code(&out), 0);
    assert_eq!(code(&with_data(&["close"])), 0);
    let profile = data.join("chrome-cdp/profiles/work");
    assert!(profile.join("Default").is_dir(), "{}", profile.display());

    assert_eq!(code(&with_data(&["open", "--profile", "work"])), 0);
    let out = with_data(&["cdp", "Storage.getCookies", "--json"]);
    let cookies = &json_out(&out)["data"]["cookies"];
    assert_eq!(cookies[0]["value"], json!("kept"), "{cookies}");
}

#[test]
fn a_profile_in_use_is_refused() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let profile = sandbox.dir.path().join("profile");
    let dir = profile.to_str().unwrap();
    let out = sandbox.run(&["open", "--session", "first", "--user-data-dir", dir]);
    assert_eq!(code(&out), 0);

    let out = sandbox.run(&["open", "--session", "second", "--user-data-dir", dir]);
    assert_eq!(code(&out), 4);
    assert!(
        stderr(&out).starts_with(&format!(
            "error: the profile {dir} is in use by another Chrome (pid "
        )),
        "{}",
        stderr(&out)
    );
    // The failed session left nothing running.
    assert_eq!(sandbox.daemons().len(), 1);
    assert_eq!(code(&sandbox.run(&["close", "--session", "first"])), 0);
    // Kept profiles stay.
    assert!(profile.is_dir());
}

#[test]
fn chrome_args_reach_chrome() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let out = sandbox.run(&["open", "--chrome-arg=--user-agent=chrome-cdp-test"]);
    assert_eq!(code(&out), 0);
    let out = sandbox.run(&[
        "cdp",
        "--target",
        "page",
        "Runtime.evaluate",
        r#"{"expression":"navigator.userAgent","returnByValue":true}"#,
        "-q",
    ]);
    assert_eq!(code(&out), 0);
    assert!(
        stdout(&out).contains(r#""value":"chrome-cdp-test""#),
        "{}",
        stdout(&out)
    );
    // `status` shows them.
    let out = sandbox.run(&["status", "--json"]);
    assert_eq!(
        json_out(&out)["data"]["browser"]["chrome_args"],
        json!(["--user-agent=chrome-cdp-test"])
    );
}

#[test]
fn launch_options_are_checked_before_launching() {
    let dir = tempfile::tempdir().unwrap();
    // It would hang if launched.
    let chrome = fake_chrome(&dir, "exec sleep 30");
    let sandbox = Sandbox::with_chrome(&chrome);

    let out = sandbox.run(&["open", "--chrome-arg", "https://example.com"]);
    assert_eq!(code(&out), 2);
    assert_eq!(
        stderr(&out),
        "error: --chrome-arg `https://example.com` is not a Chrome flag\n  \
         hint: pass flags in their --name=value form, e.g. --chrome-arg=--window-size=1280,800\n"
    );
    assert_eq!(code(&sandbox.run(&["open", "--profile", "a/b"])), 2);
    let out = sandbox.run(&["open", "--profile", "a", "--user-data-dir", "/x"]);
    assert_eq!(code(&out), 2);
    // None of these started a daemon.
    assert!(!sandbox.runtime_dir().join("chrome-cdp").exists());

    let home = dir.path().join("home");
    std::fs::create_dir_all(home.join(".config/google-chrome")).unwrap();
    let out = run(sandbox
        .cli()
        .args(["open", "--json", "--user-data-dir"])
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
        let out = run(sandbox
            .cli()
            .args(["open", "--headed"])
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY"));
        assert_eq!(code(&out), 4);
        assert_eq!(
            stderr(&out),
            "error: --headed needs a display, and DISPLAY and WAYLAND_DISPLAY are unset\n  \
             hint: drop --headed, or run it under a virtual display: xvfb-run -a chrome-cdp …\n"
        );
    }
    // The daemons these failures started are gone.
    wait_until("the daemons to exit", || sandbox.daemons().is_empty());
}
