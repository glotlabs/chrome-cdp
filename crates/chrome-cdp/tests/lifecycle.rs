//! Page lifecycle edge cases (plan 03, milestone 3.5) end to end:
//! downloads (`goto` to a file, `wait --download`, `downloads`), a crashed
//! page and `reload`, and HTTP basic auth (`open --http-credentials`).
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::io::Write as _;
use std::path::Path;
use std::process::Stdio;

use common::{Sandbox, chrome_path, code, json_out, run, stderr, stdout};
use fixtures::{AUTH_PASSWORD, AUTH_USER, Fixtures};

#[test]
fn downloads_land_in_the_session_dir() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let report = site.url("/downloads/report.txt?download=report.txt");
    let expected = std::fs::read(fixtures::site_dir().join("downloads/report.txt")).unwrap();

    // `goto` to a file downloads it; the page stays.
    let out = sandbox.run(&["goto", &site.url("/nav/a.html")]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["goto", &report, "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let data = &json_out(&out)["data"];
    assert_eq!(data["navigated"], false);
    assert_eq!(data["title"], "Page A");
    assert_eq!(data["download"]["state"], "completed");
    assert_eq!(data["download"]["file"], "report.txt");
    let first = data["download"]["path"].as_str().unwrap().to_owned();
    assert!(first.ends_with("/downloads/report.txt"), "{first}");
    assert_eq!(std::fs::read(&first).unwrap(), expected);
    let dir = Path::new(&first).parent().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{mode:o}");
    }

    // The same name again gets a unique one.
    let out = sandbox.run(&["goto", &report, "-q"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let second = stdout(&out).trim_end().to_owned();
    assert_eq!(second, dir.join("report (1).txt").display().to_string());

    // A link clicked (here from raw CDP) downloads asynchronously:
    // `wait --download` waits for it.
    let out = sandbox.run(&["goto", &site.url("/downloads/")]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let click = r#"{"expression":"document.getElementById('slow').click()"}"#;
    let out = sandbox.run(&["cdp", "--target", "page", "Runtime.evaluate", click]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["wait", "--download", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let json = json_out(&out);
    let slow = json["data"]["download"]["path"].as_str().unwrap();
    assert!(slow.ends_with("/downloads/slow-report.txt"), "{slow}");
    assert_eq!(std::fs::read(slow).unwrap(), expected);
    assert!(
        json["data"]["met"]
            .as_str()
            .unwrap()
            .starts_with("downloaded: slow-report.txt (27B) → "),
        "{json}"
    );

    // Only a download since the previous command began counts (so the
    // one `wait --download` just saw starts no second wait).
    let out = sandbox.run(&["wait", "--text", "Downloads"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["wait", "--download", "--timeout", "500ms"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));

    let out = sandbox.run(&["downloads", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let data = &json_out(&out)["data"];
    assert_eq!(data["dir"], dir.display().to_string());
    let files: Vec<&str> = data["downloads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["file"].as_str().unwrap())
        .collect();
    assert_eq!(files, ["report.txt", "report.txt", "slow-report.txt"]);
    let out = sandbox.run(&["downloads", "-q"]);
    assert_eq!(stdout(&out).lines().count(), 3, "{}", stdout(&out));

    // The files outlive the session.
    let out = sandbox.run(&["close"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(std::fs::read(&first).unwrap(), expected);
}

#[test]
fn downloads_go_where_open_says_and_clear_deletes_them() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let report = site.url("/downloads/report.txt?download=report.txt");
    let first_dir = sandbox.dir.path().join("dl/first");
    let second_dir = sandbox.dir.path().join("dl/second");

    // A relative --downloads-dir is the CLI's, and is created.
    let out = run(sandbox.cli().current_dir(sandbox.dir.path()).args([
        "open",
        "--downloads-dir",
        "dl/first",
        &site.url("/nav/a.html"),
    ]));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(first_dir.is_dir());
    let out = sandbox.run(&["goto", &report, "-q"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let first = first_dir.join("report.txt");
    assert_eq!(stdout(&out).trim_end(), first.display().to_string());

    // Given to a running browser, it applies at once.
    let second = second_dir.to_string_lossy().into_owned();
    let out = sandbox.run(&["open", "--downloads-dir", &second]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["goto", &report, "-q"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let second_file = second_dir.join("report.txt");
    assert_eq!(stdout(&out).trim_end(), second_file.display().to_string());
    let out = sandbox.run(&["downloads", "--json"]);
    assert_eq!(json_out(&out)["data"]["dir"], second);

    // --clear deletes what chrome-cdp saved, and nothing else.
    let other = second_dir.join("mine.txt");
    std::fs::write(&other, "not a download").unwrap();
    let out = sandbox.run(&["downloads", "--clear"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "deleted 2 downloaded files; the list is empty\n"
    );
    assert!(!first.exists());
    assert!(!second_file.exists());
    assert!(other.exists());
    let out = sandbox.run(&["downloads", "--json"]);
    assert_eq!(json_out(&out)["data"]["downloads"], serde_json::json!([]));
    let out = sandbox.run(&["downloads", "--clear", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(json_out(&out)["data"]["deleted"], serde_json::Value::Null);
}

#[test]
fn a_replaced_daemon_keeps_the_downloads_dir_and_says_the_list_restarts() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let dir = sandbox.dir.path().join("dl");
    let dir_text = dir.to_string_lossy().into_owned();
    // The browser an older daemon had, as its meta.json says.
    let out = sandbox.run(&["open", "--session", "other", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let browser = json_out(&out)["data"]["browser"].clone();
    assert_eq!(code(&sandbox.run(&["close", "--session", "other"])), 0);

    // A daemon as an older chrome-cdp started it, which opened a browser
    // with --downloads-dir.
    let log = std::fs::File::create(sandbox.dir.path().join("old-daemon.log")).unwrap();
    let mut old = sandbox
        .cli()
        .args(["__daemon", "--session", "default", "--build", "0.0.1+old"])
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let session_dir = sandbox.runtime_dir().join("chrome-cdp/default");
    common::wait_until("the old daemon to listen", || {
        session_dir.join("sock").exists() && session_dir.join("meta.json").exists()
    });
    let meta_path = session_dir.join("meta.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
    meta["browser"] = browser;
    meta["launch"] = serde_json::json!({ "downloads_dir": dir_text, "timeout_ms": 30000 });
    std::fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();

    // The new daemon opens a browser as the old one had, and its list
    // starts now.
    let out = sandbox.run(&["goto", "about:blank"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["downloads", "--json"]);
    assert_eq!(code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    let json = json_out(&out);
    assert_eq!(json["data"]["dir"], dir_text);
    assert!(
        json["data"]["since"].as_str().unwrap().ends_with('Z'),
        "{json}"
    );
    let warning = json["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|w| w.as_str())
        .find(|w| w.starts_with("the list starts at "))
        .unwrap_or_else(|| panic!("{json}"))
        .to_owned();
    assert!(
        warning.ends_with(&format!(
            "(the session's daemon was replaced); earlier downloads are still in {dir_text}"
        )),
        "{warning}"
    );
    let _ = old.wait();
    assert_eq!(code(&sandbox.run(&["close"])), 0);
}

#[test]
fn a_crashed_page_reloads() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let out = sandbox.run(&["goto", &site.url("/nav/a.html")]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    // The call that crashes it never answers; it returns at once.
    let out = sandbox.run(&["cdp", "--target", "page", "Page.crash"]);
    assert_eq!(code(&out), 4, "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        format!(
            "error: the page in tab t1 crashed\n  \
             page: {}\n  \
             hint: chrome-cdp reload   (loads it again)\n",
            site.url("/nav/a.html")
        )
    );
    let out = sandbox.run(&["wait", "--text", "Page A", "--json"]);
    assert_eq!(code(&out), 4, "{}", stderr(&out));
    assert_eq!(json_out(&out)["error"]["kind"], "PageCrashed");
    let out = sandbox.run(&["tabs", "--json"]);
    assert_eq!(json_out(&out)["data"]["tabs"][0]["crashed"], true);

    let out = sandbox.run(&["reload"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["wait", "--text", "Page A"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["tabs", "--json"]);
    assert!(json_out(&out)["data"]["tabs"][0].get("crashed").is_none());

    // `goto` brings a crashed page back too.
    let out = sandbox.run(&["cdp", "--target", "page", "Page.crash"]);
    assert_eq!(code(&out), 4, "{}", stderr(&out));
    let out = sandbox.run(&["goto", &site.url("/nav/b.html")]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["wait", "--text", "Page B"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

#[test]
fn http_basic_auth() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let members = site.url("/auth/");

    let out = sandbox.run(&["goto", &members, "--json"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    let error = &json_out(&out)["error"];
    assert_eq!(error["details"]["reason"], "auth");
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .starts_with("chrome-cdp open --http-credentials USER:PASSWORD"),
        "{error}"
    );

    // Wrong credentials are tried once, then the server's 401 stands.
    let out = sandbox.run(&["open", "--http-credentials", "ada:wrong"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["goto", &members]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));

    // The right ones, from stdin, apply to the running browser.
    let mut child = sandbox
        .cli()
        .args(["open", "--http-credentials", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.take().unwrap(), "{AUTH_USER}:{AUTH_PASSWORD}").unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["goto", &members, "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let data = &json_out(&out)["data"];
    assert_eq!(data["status"], 200);
    assert_eq!(data["title"], "Members");

    // New tabs get them too, and other pages still load.
    let out = sandbox.run(&["tab", "new", &members]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["goto", &site.url("/wait/spa-fetch.html"), "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(json_out(&out)["data"]["title"], "SPA: Inbox");

    let out = sandbox.run(&["open", "--http-credentials", "nocolon"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    let out = sandbox.run(&[
        "open",
        "--http-credentials",
        &format!("{AUTH_USER}:{AUTH_PASSWORD}"),
        "--http-credentials-origin",
        "app.test",
    ]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("is not an http(s) origin"),
        "{}",
        stderr(&out)
    );
}

/// `--http-credentials-origin`: another origin's challenge gets nothing.
/// A browser of its own, since Chrome caches credentials it sent once.
#[test]
fn http_credentials_scoped_to_an_origin() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let members = site.url("/auth/");

    // Scoped to another origin, a challenge gets nothing: the 401 stands.
    let credentials = format!("{AUTH_USER}:{AUTH_PASSWORD}");
    let other = site.url_b("/");
    let out = sandbox.run(&[
        "open",
        "--http-credentials",
        &credentials,
        "--http-credentials-origin",
        &other,
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["goto", &members, "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(json_out(&out)["data"]["status"], 401);
    // Scoped to the page's own origin, it does.
    let out = sandbox.run(&[
        "open",
        "--http-credentials",
        &credentials,
        "--http-credentials-origin",
        &site.url("/auth/"),
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["goto", &members, "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(json_out(&out)["data"]["title"], "Members");
}
