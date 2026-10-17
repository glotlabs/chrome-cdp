//! `chrome-cdp install`, `uninstall` and `--auto-install` against a fake
//! Chrome for Testing server (plan 02, milestone 2.2). The browsers it
//! serves are scripts that answer `--version`.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::path::Path;
use std::process::Command;

use browser::testing::FakeCft;
use common::{Sandbox, cli, code, json_out, run, stderr, stdout};
use serde_json::{Value, json};

/// `cli()` using `server` as the index and `cache` as the cache dir.
fn with_fake(server: &FakeCft, cache: &Path) -> Command {
    let mut cmd = cli();
    cmd.env("CHROME_CDP_CFT_URL", &server.url)
        .env("CHROME_CDP_CACHE", cache);
    cmd
}

/// [`with_fake`] in a session sandbox, for commands that launch Chrome.
fn with_fake_in(sandbox: &Sandbox, server: &FakeCft, cache: &Path) -> Command {
    let mut cmd = sandbox.cli();
    cmd.env("CHROME_CDP_CFT_URL", &server.url)
        .env("CHROME_CDP_CACHE", cache);
    cmd
}

fn zip_downloads(server: &FakeCft) -> usize {
    server
        .requests()
        .iter()
        .filter(|p| p.contains("/linux64/"))
        .count()
}

fn linux_x86_64() -> bool {
    let ok = cfg!(all(target_os = "linux", target_arch = "x86_64"));
    if !ok {
        eprintln!("skipping: the fake index serves linux64 builds only");
    }
    ok
}

#[test]
fn install_list_and_uninstall() {
    if !linux_x86_64() {
        return;
    }
    let server = FakeCft::start("linux64");
    let dir = common::tempdir();
    let cache = dir.path();
    let id = "stable-154.0.8037.57-linux64";
    let binary = cache
        .join("browsers")
        .join(id)
        .join("chrome-linux64/chrome");

    let out = run(with_fake(&server, cache).args(["install", "--json"]));
    assert_eq!(code(&out), 0);
    let data = &json_out(&out)["data"];
    assert_eq!(data["id"], json!(id));
    assert_eq!(data["product"], json!("chrome"));
    assert_eq!(data["downloaded"], json!(true));
    assert_eq!(data["path"], json!(binary.display().to_string()));
    // No terminal: no progress bar.
    assert_eq!(stderr(&out), "");

    let out = run(with_fake(&server, cache).arg("install"));
    assert_eq!(
        stdout(&out),
        format!(
            "Chrome for Testing 154.0.8037.57 (stable) is already installed: {}\n",
            binary.display()
        )
    );
    assert_eq!(zip_downloads(&server), 1);

    let out = run(with_fake(&server, cache).args(["install", "--headless-shell", "-q"]));
    let shell = stdout(&out);
    assert!(
        shell.ends_with("/chrome-headless-shell-linux64/chrome-headless-shell\n"),
        "{shell}"
    );

    let out = run(with_fake(&server, cache).args(["install", "--list", "--json"]));
    let data = &json_out(&out)["data"];
    assert_eq!(data["platform"], json!("linux64"));
    assert_eq!(
        data["installed"][0]["products"],
        json!(["chrome", "chrome-headless-shell"])
    );
    assert_eq!(data["available"][0]["installed"], json!(id));
    assert_eq!(data["available"][1]["installed"], json!(null));

    let out = run(with_fake(&server, cache).args(["uninstall", id, "-q"]));
    assert_eq!(code(&out), 0);
    assert!(!cache.join("browsers").join(id).exists());
    let out = run(with_fake(&server, cache).args(["install", "--list", "-q"]));
    assert_eq!(stdout(&out), "");
}

#[test]
fn parallel_installs_download_once() {
    if !linux_x86_64() {
        return;
    }
    let server = FakeCft::start("linux64");
    let dir = common::tempdir();
    let children: Vec<_> = (0..6)
        .map(|_| {
            with_fake(&server, dir.path())
                .args(["install", "--json"])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut downloaded = 0;
    for child in children {
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        if json_out(&out)["data"]["downloaded"] == json!(true) {
            downloaded += 1;
        }
    }
    assert_eq!(downloaded, 1);
    assert_eq!(zip_downloads(&server), 1);
}

#[test]
fn offline_list_still_shows_the_installs() {
    let dir = common::tempdir();
    let out = run(cli()
        .args(["install", "--list"])
        .env("CHROME_CDP_CFT_URL", "http://127.0.0.1:9")
        .env("CHROME_CDP_CACHE", dir.path()));
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "installed:\n(none)\n");
    assert!(
        stderr(&out).starts_with(
            "warning: cannot list the available versions: cannot download \
             http://127.0.0.1:9/last-known-good-versions-with-downloads.json"
        ),
        "{}",
        stderr(&out)
    );
}

#[test]
fn auto_install_installs_when_no_chrome_is_found() {
    let system = ["/opt/google/chrome/chrome", "/snap/bin/chromium"];
    if !linux_x86_64() || system.iter().any(|p| Path::new(p).exists()) {
        eprintln!("skipping: this machine has a system Chrome");
        return;
    }
    let server = FakeCft::start("linux64");
    let dir = common::tempdir();
    let empty = dir.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let cache = dir.path().join("cache");
    let sandbox = Sandbox::new();
    let open = |auto: &[(&str, &str)], args: &[&str]| {
        let mut cmd = with_fake_in(&sandbox, &server, &cache);
        cmd.args(["open", "--json"])
            .args(args)
            .env("PATH", &empty)
            .envs(auto.iter().copied());
        run(&mut cmd)
    };

    // Without it: the hint says to install.
    let out = open(&[], &[]);
    assert_eq!(json_out(&out)["error"]["kind"], json!("ChromeNotFound"));
    assert_eq!(zip_downloads(&server), 0);

    // With it, Chrome for Testing is installed and launched. The fake one
    // cannot speak CDP, so the launch fails, after the install.
    let out = open(&[("CHROME_CDP_AUTO_INSTALL", "1")], &[]);
    let error = &json_out(&out)["error"];
    assert_eq!(error["kind"], json!("LaunchFailed"), "{error}");
    let installed = cache.join("browsers/stable-154.0.8037.57-linux64");
    assert!(installed.join("INSTALLATION_COMPLETE").is_file());
    let warnings = |error: &Value| -> Vec<String> {
        error["notes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|n| n["label"] == "warning")
            .map(|n| n["text"].as_str().unwrap().to_owned())
            .collect()
    };
    let notes = warnings(error);
    let note = notes.first().expect("a warning note");
    assert!(
        note.starts_with("installed Chrome for Testing 154.0.8037.57 (")
            && note.ends_with("B) because --auto-install is set"),
        "{note}"
    );

    // Next time it is simply found.
    let out = open(&[], &["--auto-install"]);
    assert_eq!(json_out(&out)["error"]["kind"], json!("LaunchFailed"));
    assert!(warnings(&json_out(&out)["error"]).is_empty());
    assert_eq!(zip_downloads(&server), 1);
}

/// Whether this machine has Ubuntu 23.10+'s AppArmor userns rule on.
fn userns_restricted() -> bool {
    std::fs::read_to_string("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
        .is_ok_and(|v| v.trim() == "1")
}

#[test]
fn the_apparmor_rule_is_reported_by_install_and_launch() {
    if !linux_x86_64() {
        return;
    }
    let server = FakeCft::start("linux64");
    let dir = common::tempdir();
    let cache = dir.path().join("cache");
    let out = run(with_fake(&server, &cache)
        .args(["install", "--json"])
        .env_remove("CHROME_CDP_NO_USERNS_CHECK"));
    assert_eq!(code(&out), 0);
    let v = json_out(&out);
    let warnings = v["warnings"].as_array().cloned().unwrap_or_default();
    let blocked = |w: &Value| {
        w.as_str().unwrap().starts_with(
            "AppArmor blocks Chrome's sandbox on this machine until a profile allows it; \
             run chrome-cdp install --with-deps to write /etc/apparmor.d/chrome-cdp-",
        )
    };
    // No profile covers a temp cache, so the rule shows wherever it is on.
    assert_eq!(warnings.iter().any(blocked), userns_restricted(), "{v}");

    // Launching it fails before it starts, with the same fix.
    let sandbox = Sandbox::new();
    let out = run(with_fake_in(&sandbox, &server, &cache)
        .args(["open"])
        .env_remove("CHROME_CDP_NO_USERNS_CHECK"));
    let err = stderr(&out);
    if userns_restricted() {
        assert_eq!(code(&out), 4);
        assert!(
            err.starts_with(
                "error: Chrome's sandbox cannot start: AppArmor blocks user namespaces for "
            ),
            "{err}"
        );
        assert!(
            err.contains(
                "\n  hint: chrome-cdp install --with-deps   (writes /etc/apparmor.d/chrome-cdp-"
            ),
            "{err}"
        );
    }
    // The escape hatch turns the check off: the fake then fails to speak CDP.
    let out = run(with_fake_in(&sandbox, &server, &cache).args(["open"]));
    assert!(!stderr(&out).contains("AppArmor"), "{}", stderr(&out));
}
