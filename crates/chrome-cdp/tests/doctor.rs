//! `chrome-cdp doctor` (plan 10.5): its checks against stand-in Chromes
//! and session dirs, and a real launch when CHROME_PATH is set.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use common::{Sandbox, chrome_path, code, fake_chrome, json_out, run, stdout};
use serde_json::Value;

/// The checks of a `--json` run, as `name: status`.
fn statuses(data: &Value) -> Vec<String> {
    data["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            format!(
                "{}: {}",
                c["name"].as_str().unwrap(),
                c["status"].as_str().unwrap()
            )
        })
        .collect()
}

fn check<'a>(data: &'a Value, name: &str) -> &'a Value {
    data["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap()
}

#[test]
fn looks_without_launching() {
    let sandbox = Sandbox::new();
    let dir = tempfile::tempdir().unwrap();
    let chrome = fake_chrome(&dir, "exit 1");
    let out = run(sandbox.cli().env("CHROME_CDP_CHROME", &chrome).args([
        "doctor",
        "--no-launch",
        "--json",
    ]));
    assert_eq!(code(&out), 0);
    let json = json_out(&out);
    let data = &json["data"];
    let (libraries, sandboxed, quarantine) = if cfg!(target_os = "macos") {
        ("libraries: skip", "sandbox: ok", "quarantine: ok")
    } else {
        // cli() sets CHROME_CDP_NO_USERNS_CHECK=1.
        ("libraries: ok", "sandbox: skip", "quarantine: skip")
    };
    assert_eq!(
        statuses(data),
        [
            "chrome-cdp: ok",
            "chrome: ok",
            libraries,
            sandboxed,
            "launch: skip",
            "session-dir: ok",
            quarantine,
            "sessions: ok",
        ]
    );
    assert_eq!(
        check(data, "chrome")["message"],
        format!(
            "Chromium {} at {} (given by CHROME_CDP_CHROME)",
            common::FAKE_VERSION,
            chrome.display()
        )
    );
    assert_eq!(
        check(data, "launch")["message"],
        "--no-launch: Chrome was not started"
    );
    assert_eq!(
        check(data, "session-dir")["message"],
        format!(
            "{} will be made on the first open",
            sandbox.runtime_dir().join("chrome-cdp").display()
        )
    );
    assert_eq!(check(data, "sessions")["message"], "no sessions running");

    let quiet =
        run(sandbox
            .cli()
            .env("CHROME_CDP_CHROME", &chrome)
            .args(["doctor", "--no-launch", "-q"]));
    assert_eq!(stdout(&quiet), "ok\n");
}

#[test]
fn fails_without_a_chrome() {
    let sandbox = Sandbox::new();
    let out = run(sandbox
        .cli()
        .env("CHROME_CDP_CHROME", "/nonexistent/chrome")
        .args(["doctor", "--json"]));
    assert_eq!(code(&out), 4);
    let error = &json_out(&out)["error"];
    assert_eq!(error["kind"], "ChromeNotFound");
    assert_eq!(error["message"], "1 of 8 checks failed: chrome");
    assert_eq!(
        error["hint"],
        "point CHROME_CDP_CHROME at a Chrome or Chromium binary, or unset it"
    );
    let details = &error["details"];
    assert_eq!(details["failed"], 1);
    assert_eq!(
        check(details, "chrome")["message"],
        "no Chrome at /nonexistent/chrome (from CHROME_CDP_CHROME)"
    );
    assert_eq!(check(details, "launch")["message"], "no Chrome to start");
}

#[test]
fn warns_about_an_old_chrome() {
    let sandbox = Sandbox::new();
    let dir = tempfile::tempdir().unwrap();
    let chrome = dir.path().join("chrome");
    fs::write(&chrome, "#!/bin/sh\necho 'Chromium 100.0.0.0'\n").unwrap();
    fs::set_permissions(&chrome, fs::Permissions::from_mode(0o755)).unwrap();
    let out = run(sandbox.cli().env("CHROME_CDP_CHROME", &chrome).args([
        "doctor",
        "--no-launch",
        "--json",
    ]));
    assert_eq!(code(&out), 0);
    let json = json_out(&out);
    let chrome_check = check(&json["data"], "chrome");
    assert_eq!(chrome_check["status"], "warn");
    assert!(
        chrome_check["message"]
            .as_str()
            .unwrap()
            .contains("is older than Chrome"),
        "{chrome_check}"
    );
    assert_eq!(
        chrome_check["fix"],
        "chrome-cdp install   (a current Chrome for Testing)"
    );
    assert_eq!(json["data"]["warnings"], 1);
}

#[test]
fn fails_on_a_session_dir_others_can_enter() {
    let sandbox = Sandbox::new();
    let base = sandbox.runtime_dir().join("chrome-cdp");
    fs::create_dir_all(&base).unwrap();
    fs::set_permissions(&base, fs::Permissions::from_mode(0o755)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let chrome = fake_chrome(&dir, "exit 1");
    let out = run(sandbox
        .cli()
        .env("CHROME_CDP_CHROME", &chrome)
        .args(["doctor", "--no-launch"]));
    assert_eq!(code(&out), 4);
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        err.starts_with("error: 1 of 8 checks failed: session-dir\n"),
        "{err}"
    );
    assert!(
        err.contains(&format!(
            "  session-dir: FAIL {}: mode 755 lets other users in\n",
            base.display()
        )),
        "{err}"
    );
    assert!(
        err.ends_with(&format!(
            "  hint: chmod 700 {0}, or remove {0} if another user made it; or set {1} to a \
             private directory\n",
            base.display(),
            if cfg!(target_os = "macos") {
                "TMPDIR"
            } else {
                "XDG_RUNTIME_DIR"
            }
        )),
        "{err}"
    );
}

/// The real thing: the sandbox check on, Chrome started and closed, and a
/// session whose daemon claims an older version.
#[test]
fn launches_chrome_and_sees_a_session_on_another_version() {
    let Some(chrome) = chrome_path() else {
        return;
    };
    let sandbox = Sandbox::with_chrome(&chrome);
    let doctor = |sandbox: &Sandbox| {
        let out = run(sandbox
            .cli()
            .env_remove("CHROME_CDP_NO_USERNS_CHECK")
            .args(["doctor", "--json"]));
        assert_eq!(code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
        json_out(&out)["data"].clone()
    };
    let data = doctor(&sandbox);
    let launch = check(&data, "launch");
    assert_eq!(launch["status"], "ok", "{launch}");
    assert!(
        launch["message"].as_str().unwrap().starts_with("started "),
        "{launch}"
    );
    assert_eq!(check(&data, "sandbox")["status"], "ok");
    assert!(sandbox.chromes().is_empty(), "doctor closed its Chrome");

    let session = sandbox.session();
    session.ok(&["open"]);
    let meta_path = sandbox
        .runtime_dir()
        .join("chrome-cdp")
        .join(&session.name)
        .join("meta.json");
    let data = doctor(&sandbox);
    assert_eq!(
        check(&data, "sessions")["message"],
        format!(
            "1 session running, all on chrome-cdp {}",
            env!("CARGO_PKG_VERSION")
        )
    );
    rewrite_build(&meta_path, "0.0.9+abc+1");
    let data = doctor(&sandbox);
    let sessions = check(&data, "sessions");
    assert_eq!(sessions["status"], "warn");
    assert_eq!(
        sessions["message"],
        format!(
            "session {} runs chrome-cdp 0.0.9 (this is {}); the next command restarts it",
            session.name,
            env!("CARGO_PKG_VERSION")
        )
    );
    assert_eq!(
        sessions["fix"],
        format!(
            "chrome-cdp close --session {}   (to restart now)",
            session.name
        )
    );
}

fn rewrite_build(meta: &Path, build: &str) {
    let mut value: Value = serde_json::from_str(&fs::read_to_string(meta).unwrap()).unwrap();
    value["build"] = Value::String(build.into());
    fs::write(meta, value.to_string()).unwrap();
}
