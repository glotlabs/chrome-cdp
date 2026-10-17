//! Sessions and their daemons (plan 02, milestone 2.4): open → status →
//! close, several sessions, concurrent starts, idle shutdown, crashes,
//! killed daemons, upgrades and `connect`.
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use common::{Sandbox, chrome_path, code, is_daemon, json_out, run, stderr, stdout, wait_until};
use serde_json::{Value, json};

fn status(sandbox: &Sandbox, session: &str) -> Value {
    let out = sandbox.run(&["status", "--json", "--session", session]);
    assert_eq!(code(&out), 0);
    json_out(&out)["data"].clone()
}

fn kill(signal: &str, target: &str) {
    let status = Command::new("kill")
        .args(["-s", signal, "--", target])
        .status()
        .unwrap();
    assert!(status.success(), "kill -{signal} {target}");
}

#[test]
fn open_status_close_leaves_nothing_behind() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let out = sandbox.run(&["open", "data:text/html,<title>Hello</title>", "--json"]);
    assert_eq!(code(&out), 0);
    let data = &json_out(&out)["data"];
    assert_eq!(data["session"], json!("default"));
    assert_eq!(data["started"], json!(true));
    assert_eq!(data["page"]["title"], json!("Hello"));
    assert_eq!(data["browser"]["kind"], json!("launched"));
    assert_eq!(data["browser"]["headless"], json!(true));
    assert_eq!(data["browser"]["profile"]["kind"], json!("temporary"));

    // A second open is a no-op on the same browser.
    let out = sandbox.run(&["open"]);
    assert_eq!(code(&out), 0);
    assert!(
        stdout(&out).starts_with("session default is already open: "),
        "{}",
        stdout(&out)
    );

    let st = status(&sandbox, "default");
    assert_eq!(st["state"], json!("running"));
    assert_eq!(st["tabs"], json!(1));
    assert_eq!(st["browser"]["pid"], data["browser"]["pid"]);
    let dir = st["dir"].as_str().unwrap();
    assert!(dir.starts_with(&sandbox.runtime_dir().display().to_string()));
    for file in ["sock", "pid", "meta.json", "daemon.log"] {
        assert!(Path::new(dir).join(file).exists(), "{file}");
    }
    assert_eq!(sandbox.daemons().len(), 1);
    assert_eq!(stdout(&sandbox.run(&["status", "-q"])), "running\n");

    let out = sandbox.run(&["close"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "closed session default and its browser\n");
    // Chrome and the daemon have exited when close returns.
    assert!(sandbox.chromes().is_empty(), "{:?}", sandbox.chromes());
    assert!(sandbox.daemons().is_empty());
    sandbox.assert_all_gone();
    assert!(!Path::new(dir).join("sock").exists());
    assert_eq!(stdout(&sandbox.run(&["status", "-q"])), "stopped\n");
    // Closing again says so.
    assert_eq!(
        stdout(&sandbox.run(&["close"])),
        "session default was not running\n"
    );
}

#[test]
fn two_named_sessions_run_at_once() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    for session in ["a", "b"] {
        let url = format!("data:text/html,<title>{session}</title>");
        let out = sandbox.run(&["open", &url, "--session", session]);
        assert_eq!(code(&out), 0);
    }
    let (a, b) = (status(&sandbox, "a"), status(&sandbox, "b"));
    assert_ne!(a["browser"]["pid"], b["browser"]["pid"]);
    assert_ne!(a["daemon_pid"], b["daemon_pid"]);
    let title = |session: &str| {
        let out = sandbox.run(&[
            "cdp",
            "--session",
            session,
            "--target",
            "page",
            "Runtime.evaluate",
            r#"{"expression":"document.title","returnByValue":true}"#,
            "--json",
        ]);
        json_out(&out)["data"]["result"]["value"].clone()
    };
    assert_eq!(title("a"), json!("a"));
    assert_eq!(title("b"), json!("b"));

    let out = sandbox.run(&["sessions", "--json"]);
    let names: Vec<_> = json_out(&out)["data"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["session"].clone())
        .collect();
    assert_eq!(names, [json!("a"), json!("b")]);
    assert_eq!(stdout(&sandbox.run(&["sessions", "-q"])), "a\nb\n");

    let out = sandbox.run(&["close", "--all"]);
    assert_eq!(
        stdout(&out),
        "closed session a and its browser\nclosed session b and its browser\n"
    );
    sandbox.assert_all_gone();
    assert_eq!(
        stdout(&sandbox.run(&["sessions"])),
        "no sessions; start one: chrome-cdp open\n"
    );
}

#[test]
fn parallel_opens_start_one_daemon() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let children: Vec<_> = (0..10)
        .map(|_| {
            sandbox
                .cli()
                .args(["open", "--session", "p", "--json"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut started = 0;
    for child in children {
        let out = child.wait_with_output().unwrap();
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        if json_out(&out)["data"]["started"] == json!(true) {
            started += 1;
        }
    }
    assert_eq!(started, 1, "exactly one open launched Chrome");
    assert_eq!(sandbox.daemons().len(), 1);
    // One browser process (its helpers have a --type).
    let browsers = sandbox
        .chromes()
        .into_iter()
        .filter(|l| l.contains("--remote-debugging-pipe") && !l.contains("--type="))
        .count();
    assert_eq!(browsers, 1);
}

#[test]
fn a_killed_daemon_takes_chrome_with_it() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    assert_eq!(code(&sandbox.run(&["open"])), 0);
    let daemon = status(&sandbox, "default")["daemon_pid"].as_u64().unwrap();
    kill("KILL", &daemon.to_string());
    // Linux: PR_SET_PDEATHSIG; everywhere: the watchdog, which also
    // deletes the run dir with the temporary profile.
    sandbox.assert_all_gone();
    // The next command sees no daemon and cleans up its files.
    let st = status(&sandbox, "default");
    assert_eq!(st["state"], json!("stopped"));
    let dir = Path::new(st["dir"].as_str().unwrap());
    assert!(!dir.join("sock").exists());
    assert!(!dir.join("pid").exists());
    // And a new one starts.
    assert_eq!(code(&sandbox.run(&["open"])), 0);
}

#[test]
fn sigterm_closes_the_session() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    assert_eq!(code(&sandbox.run(&["open"])), 0);
    let daemon = status(&sandbox, "default")["daemon_pid"].as_u64().unwrap();
    kill("TERM", &daemon.to_string());
    sandbox.assert_all_gone();
    let log = std::fs::read_to_string(sandbox.runtime_dir().join("chrome-cdp/default/daemon.log"))
        .unwrap();
    assert!(log.contains("SIGTERM; shutting down"), "{log}");
}

#[test]
fn an_idle_session_shuts_down() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let out = sandbox.run(&["open", "--idle-timeout", "2s"]);
    assert_eq!(code(&out), 0);
    assert_eq!(status(&sandbox, "default")["idle_timeout_ms"], json!(2000));
    // `status` is not activity, so polling it does not keep it alive.
    wait_until("the idle daemon to exit", || {
        stdout(&sandbox.run(&["status", "-q"])) == "stopped\n"
    });
    sandbox.assert_all_gone();
}

#[test]
fn a_connected_client_keeps_a_daemon_without_browser_alive() {
    // As while `open --auto-install` downloads Chrome: a CLI is connected,
    // but has sent nothing yet.
    let sandbox = Sandbox::new();
    let log = std::fs::File::create(sandbox.dir.path().join("daemon.log")).unwrap();
    let mut daemon = sandbox
        .cli()
        .args(["__daemon", "--session", "default", "--build", "test"])
        .args(["--no-browser-grace", "300ms"])
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let sock = sandbox.runtime_dir().join("chrome-cdp/default/sock");
    wait_until("the daemon to listen", || sock.exists());
    let client = std::os::unix::net::UnixStream::connect(&sock).unwrap();

    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(daemon.try_wait().unwrap(), None, "exited despite a client");
    assert_eq!(stdout(&sandbox.run(&["status", "-q"])), "idle\n");

    drop(client);
    wait_until("the daemon to exit", || {
        daemon.try_wait().unwrap().is_some()
    });
    assert!(daemon.wait().unwrap().success());
    assert_eq!(stdout(&sandbox.run(&["status", "-q"])), "stopped\n");
}

#[test]
fn a_crash_is_reported_and_open_starts_a_new_browser() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    assert_eq!(code(&sandbox.run(&["open"])), 0);
    let pid = status(&sandbox, "default")["browser"]["pid"]
        .as_u64()
        .unwrap();
    kill("KILL", &format!("-{pid}"));
    wait_until("the crash to be noticed", || {
        stdout(&sandbox.run(&["status", "-q"])) == "crashed\n"
    });
    let st = status(&sandbox, "default");
    assert!(
        st["crash"]
            .as_str()
            .unwrap()
            .starts_with("Chrome exited unexpectedly"),
        "{st}"
    );

    let out = sandbox.run(&["cdp", "Browser.getVersion"]);
    assert_eq!(code(&out), 4);
    let err = stderr(&out);
    assert!(
        err.starts_with("error: Chrome exited unexpectedly"),
        "{err}"
    );
    assert!(
        err.ends_with(
            "  hint: chrome-cdp open   (starts a new browser), or chrome-cdp status --logs shows why\n"
        ),
        "{err}"
    );
    let out = sandbox.run(&["cdp", "Browser.getVersion", "--json"]);
    assert_eq!(json_out(&out)["error"]["kind"], json!("BrowserCrashed"));

    let out = sandbox.run(&["open"]);
    assert_eq!(code(&out), 0);
    assert!(
        stderr(&out).starts_with("warning: Chrome exited unexpectedly"),
        "{}",
        stderr(&out)
    );
    assert!(stdout(&out).starts_with("opened session default: "));
    assert_eq!(code(&sandbox.run(&["cdp", "Browser.getVersion"])), 0);
}

#[test]
fn a_daemon_of_another_build_is_replaced() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    // A daemon as an older chrome-cdp would have started it.
    let log = std::fs::File::create(sandbox.dir.path().join("old-daemon.log")).unwrap();
    let mut old = sandbox
        .cli()
        .args(["__daemon", "--session", "default", "--build", "0.0.1+old"])
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let sock = sandbox.runtime_dir().join("chrome-cdp/default/sock");
    wait_until("the old daemon to listen", || sock.exists());
    assert_eq!(
        status(&sandbox, "default")["daemon_pid"],
        json!(old.id()),
        "status talks to any build"
    );

    let out = sandbox.run(&["open", "--json"]);
    assert_eq!(code(&out), 0);
    assert_eq!(json_out(&out)["data"]["started"], json!(true));
    // The old one was asked to close and exited.
    let exit = old.wait().unwrap();
    assert!(exit.success(), "{exit}");
    let new = status(&sandbox, "default")["daemon_pid"].as_u64().unwrap();
    assert_ne!(new, u64::from(old.id()));
    assert!(is_daemon(u32::try_from(new).unwrap()));
}

#[test]
fn connect_attaches_and_close_only_detaches() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let profile = common::tempdir();
    let mut browser = Command::new(&chrome)
        .args([
            "--headless",
            "--remote-debugging-port=0",
            "--no-first-run",
            "--no-default-browser-check",
        ])
        .arg(format!("--user-data-dir={}", profile.path().display()))
        .arg("about:blank")
        .env("TMPDIR", profile.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let active = profile.path().join("DevToolsActivePort");
    wait_until("Chrome's DevTools port", || {
        std::fs::read_to_string(&active).is_ok_and(|t| t.lines().count() >= 2)
    });
    let text = std::fs::read_to_string(&active).unwrap();
    let port = text.lines().next().unwrap().to_owned();

    let out = sandbox.run(&["connect", &port, "--session", "c", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let browser_info = &json_out(&out)["data"]["browser"];
    assert_eq!(browser_info["kind"], json!("connected"));
    assert_eq!(browser_info["endpoint"], json!(port));
    let out = sandbox.run(&["cdp", "Browser.getVersion", "--session", "c", "-q"]);
    assert_eq!(code(&out), 0);
    // The browser's page is the session's first tab.
    let out = sandbox.run(&["tabs", "--session", "c", "-q"]);
    assert_eq!(stdout(&out), "t1\n");
    // Connecting again is a no-op; launching into it is refused.
    assert_eq!(code(&sandbox.run(&["connect", &port, "--session", "c"])), 0);
    let out = sandbox.run(&["open", "--headed", "--session", "c"]);
    assert_eq!(code(&out), 2);

    let out = sandbox.run(&["close", "--session", "c"]);
    assert_eq!(
        stdout(&out),
        "closed session c; the browser it was connected to keeps running\n"
    );
    assert!(browser.try_wait().unwrap().is_none(), "Chrome kept running");
    browser.kill().unwrap();
    browser.wait().unwrap();
}

#[test]
fn long_session_names_get_a_short_socket_path() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    // A runtime dir as long as macOS's $TMPDIR, and the longest name.
    let long = sandbox
        .runtime_dir()
        .join("var-folders-xy-0123456789abcdefghijklmn0000gn-T");
    std::fs::create_dir_all(&long).unwrap();
    let name = "n".repeat(64);
    let cli = |args: &[&str]| run(sandbox.cli().args(args).env("XDG_RUNTIME_DIR", &long));
    let out = cli(&["open", "--session", &name]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = cli(&["status", "--session", &name, "--json"]);
    let dir = json_out(&out)["data"]["dir"].as_str().unwrap().to_owned();
    assert!(dir.starts_with("/tmp/ccdp-"), "{dir}");
    assert!(Path::new(&dir).join("sock").exists());
    let out = cli(&["sessions", "-q"]);
    assert_eq!(stdout(&out), format!("{name}\n"));
    let out = cli(&["close", "--all"]);
    assert_eq!(code(&out), 0);
    wait_until("Chrome to exit", || sandbox.chromes().is_empty());
    assert!(!Path::new(&dir).join("sock").exists());
    // Keep the dir from outliving the test in the shared base.
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_interrupted_command_leaves_the_session_running() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    assert_eq!(code(&sandbox.run(&["open"])), 0);
    let mut hanging = sandbox
        .cli()
        .args([
            "cdp",
            "--target",
            "page",
            "Runtime.evaluate",
            r#"{"expression":"new Promise(() => {})","awaitPromise":true}"#,
            "--timeout",
            "60s",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    hanging.kill().unwrap();
    hanging.wait().unwrap();
    // The page is locked until the call times out; the browser answers.
    assert_eq!(code(&sandbox.run(&["cdp", "Browser.getVersion"])), 0);
    assert_eq!(status(&sandbox, "default")["state"], json!("running"));
}

#[test]
fn uninstall_refuses_a_browser_in_use() {
    let Some(chrome) = chrome_path() else { return };
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return; // The install id below is linux64's.
    }
    // An install cache holding (a link to) the test Chrome.
    let sandbox = Sandbox::new();
    let cache = sandbox.dir.path().join("cache");
    let id = "stable-154.0.0.0-linux64";
    let install = cache.join("browsers").join(id);
    std::fs::create_dir_all(install.join("chrome-linux64")).unwrap();
    std::os::unix::fs::symlink(&chrome, install.join("chrome-linux64/chrome")).unwrap();
    std::fs::write(install.join("INSTALLATION_COMPLETE"), "").unwrap();
    let cli = |args: &[&str]| run(sandbox.cli().args(args).env("CHROME_CDP_CACHE", &cache));

    assert_eq!(code(&cli(&["open", "--session", "shop"])), 0);
    let out = cli(&["uninstall", id]);
    assert_eq!(code(&out), 2);
    assert_eq!(
        stderr(&out),
        format!(
            "error: session shop is using {id}\n  \
             hint: chrome-cdp close --session shop   (then uninstall it)\n"
        )
    );
    assert!(install.is_dir());
    assert_eq!(code(&cli(&["close", "--session", "shop"])), 0);
    assert_eq!(code(&cli(&["uninstall", id])), 0);
    assert!(!install.exists());
}

#[test]
fn open_init_scripts_run_on_the_first_page_and_outlive_the_daemon() {
    let Some(chrome) = chrome_path() else { return };
    let site = fixtures::Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let url = site.url("/nav/a.html");
    let stub = sandbox.dir.path().join("stub.js");
    std::fs::write(&stub, "window.__stub = 'file';").unwrap();
    let stub = stub.to_string_lossy().into_owned();

    // A missing file fails before a browser starts.
    let out = sandbox.run(&["open", "--init-script", "missing.js", &url]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(stderr(&out).contains("cannot read "), "{}", stderr(&out));
    assert!(sandbox.chromes().is_empty(), "{:?}", sandbox.chromes());

    let open = [
        "open",
        "--init-script",
        &stub,
        "--init-script-code",
        "window.__code = 1",
        &url,
    ];
    let out = sandbox.run(&open);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains(&format!("\ninit script: s1 main {stub} 23B\n")),
        "{text}"
    );
    assert!(
        text.contains("\ninit script: s2 main 'window.__code = 1' 17B\n"),
        "{text}"
    );
    let out = sandbox.run(&["eval", "window.__stub + window.__code", "-q"]);
    assert_eq!(stdout(&out).trim(), "file1", "{}", stderr(&out));

    // Open again: the session has them, so nothing is added.
    let out = sandbox.run(&[&open[..], &["--json"]].concat());
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let ids: Vec<Value> = json_out(&out)["data"]["init_scripts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].clone())
        .collect();
    assert_eq!(ids, [json!("s1"), json!("s2")]);
    let out = sandbox.run(&["init-script", "list", "-q"]);
    assert_eq!(stdout(&out).trim(), "s1\ns2");
    let browser = json_out(&sandbox.run(&["status", "--json"]))["data"]["browser"].clone();
    assert_eq!(code(&sandbox.run(&["close"])), 0);

    // A daemon as an older chrome-cdp started it, whose browser was
    // opened with an init script: the new daemon's browser has it too.
    let log = std::fs::File::create(sandbox.dir.path().join("old-daemon.log")).unwrap();
    let mut old = sandbox
        .cli()
        .args(["__daemon", "--session", "default", "--build", "0.0.1+old"])
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let session_dir = sandbox.runtime_dir().join("chrome-cdp/default");
    wait_until("the old daemon to listen", || {
        session_dir.join("sock").exists() && session_dir.join("meta.json").exists()
    });
    let meta_path = session_dir.join("meta.json");
    let mut meta: Value = serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
    meta["browser"] = browser;
    meta["launch"] = json!({
        "init_scripts": [{ "source": "window.__kept = 'yes'" }],
        "timeout_ms": 30000,
    });
    std::fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let out = sandbox.run(&["goto", &url]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["eval", "window.__kept", "-q"]);
    assert_eq!(stdout(&out).trim(), "yes", "{}", stderr(&out));
    let _ = old.wait();
    assert_eq!(code(&sandbox.run(&["close"])), 0);
}
