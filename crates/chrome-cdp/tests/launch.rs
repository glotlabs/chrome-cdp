//! Launching Chrome (plan 02, milestone 2.3) through `open`: profiles and
//! launch options. Sessions and processes are in `session.rs`.
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::time::{SystemTime, UNIX_EPOCH};

use common::{Sandbox, chrome_path, code, fake_chrome, json_out, run, stderr, stdout, wait_until};
use fixtures::Fixtures;
use serde_json::json;

/// `testdata/extensions/basic`: a content script marks every page with the
/// extension's id, and its popup shows its name and its service worker's
/// answer.
fn fixture_extension() -> std::path::PathBuf {
    fixtures::site_dir()
        .parent()
        .unwrap()
        .join("extensions/basic")
        .canonicalize()
        .unwrap()
}

#[test]
fn an_extension_loads_before_the_first_page() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let dir = fixture_extension();
    let dir_arg = dir.to_str().unwrap();
    let out = sandbox.run(&["open", "--extension", dir_arg, &site.url("/"), "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let extensions = &json_out(&out)["data"]["browser"]["extensions"];
    let id = extensions[0]["id"].as_str().unwrap().to_owned();
    assert_eq!(id.len(), 32, "{extensions}");
    assert_eq!(
        extensions,
        &json!([{"id": id, "name": "chrome-cdp fixture", "version": "1.2.3", "path": dir_arg}])
    );

    // Its content script ran in the first page.
    let out = sandbox.run(&[
        "eval",
        "document.documentElement.dataset.chromeCdpExtension",
    ]);
    assert_eq!(stdout(&out), format!("{id}\n"), "{}", stderr(&out));

    // Its pages open with goto, and its service worker answers them.
    let popup = format!("chrome-extension://{id}/popup.html");
    let out = sandbox.run(&["goto", &popup]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["wait", "--text", "pong from the service worker"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["text", "#name"]);
    assert_eq!(stdout(&out), "chrome-cdp fixture\n");

    // `status` shows it; `open` with the same one is a no-op.
    let out = sandbox.run(&["status"]);
    assert!(
        stdout(&out).contains(&format!(
            "\nextension: chrome-cdp fixture 1.2.3, chrome-extension://{id}/ ({dir_arg})\n"
        )),
        "{}",
        stdout(&out)
    );
    let relative = run(sandbox
        .cli()
        .args(["open", "--extension", "."])
        .current_dir(&dir));
    assert_eq!(code(&relative), 0, "{}", stderr(&relative));

    // Another one needs --restart; without --extension it has none.
    let other = tempfile::tempdir().unwrap();
    std::fs::write(
        other.path().join("manifest.json"),
        r#"{"manifest_version": 3, "name": "other", "version": "1"}"#,
    )
    .unwrap();
    let other_arg = other.path().to_str().unwrap();
    let out = sandbox.run(&["open", "--extension", other_arg, "--json"]);
    assert_eq!(code(&out), 2);
    assert_eq!(
        json_out(&out)["error"]["details"]["differences"],
        json!(["--extension"])
    );
    let out = sandbox.run(&["open", "--restart", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(json_out(&out)["data"]["browser"].get("extensions"), None);
}

#[test]
fn a_broken_extension_fails_the_launch() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("manifest.json"), r#"{"name": "#).unwrap();
    let out = sandbox.run(&["open", "--extension", dir.path().to_str().unwrap()]);
    assert_eq!(code(&out), 2);
    let canonical = dir.path().canonicalize().unwrap();
    assert!(
        stderr(&out).starts_with(&format!(
            "error: cannot load the extension in {}: Manifest is not valid JSON.",
            canonical.display()
        )),
        "{}",
        stderr(&out)
    );
    assert!(
        stderr(&out)
            .ends_with("  hint: fix what Chrome names in the extension's files, then open again\n")
    );
    // Its browser and daemon are gone.
    wait_until("the daemons to exit", || sandbox.daemons().is_empty());
    assert!(sandbox.chromes().is_empty(), "{:?}", sandbox.chromes());
}

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
    let out = sandbox.run(&["open", "--extension", dir.path().to_str().unwrap()]);
    assert_eq!(code(&out), 2);
    assert_eq!(
        stderr(&out),
        format!(
            "error: --extension {0}: no manifest.json in {1}\n  \
             hint: --extension takes an unpacked extension's directory, the one holding its manifest.json\n",
            dir.path().display(),
            dir.path().canonicalize().unwrap().display()
        )
    );
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
