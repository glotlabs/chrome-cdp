//! `chrome-cdp load` (plan 23.1) against the load server
//! (`testdata/load/server`): users loop the reference journey in real
//! Chromes and the server sees their requests, `--raw` writes every
//! step's timing, `--keep-failures 3` keeps exactly three failures, and
//! a Chrome killed mid-run is started again while its users go on. Plan
//! 23.2's `[load]` settings: `block = ["third-party"]` stops a
//! third-party request and nothing else, `cache = "returning"` fetches
//! the app's files once per user, and `renderer_memory` stops a page that
//! grows past it.
//!
//! Needs a real Chrome (`CHROME_PATH`; skipped without it).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant};

use common::{Sandbox, chrome_path, stderr, stdout};
use load_server::{LoadServer, Options};
use serde_json::Value;

/// The reference journey, thinking 100 ms between pages.
const JOURNEY: &str = r#"@test "browse, open a product, add it to the cart"
  goto /
  expect 'role=heading[name="Products"]' visible
  think 100ms
  click 'role=link[name="Wireless mouse"]'
  expect role=heading text 'Wireless mouse'
  think 50ms..150ms
  click 'role=button[name="Add to cart"]'
  expect role=status text 'Added to cart'
  click 'role=link[name="Cart (1)"]'
  expect 'role=list[name="Cart items"] >> role=listitem' count 1
"#;

/// A project whose real backend `load` is the load server.
struct Project {
    sandbox: Sandbox,
    root: PathBuf,
    server: LoadServer,
}

impl Project {
    fn new(chrome: &Path, scenarios: &[(&str, &str)]) -> Self {
        Self::with_load(chrome, scenarios, "")
    }

    /// A project whose config ends with `load`, its `[load]` section.
    fn with_load(chrome: &Path, scenarios: &[(&str, &str)], load: &str) -> Self {
        let sandbox = Sandbox::with_chrome(chrome);
        let server = LoadServer::start(Options::default()).unwrap();
        let root = sandbox.dir.path().join("project");
        std::fs::create_dir_all(root.join("e2e")).unwrap();
        std::fs::write(
            root.join("chrome-cdp.toml"),
            format!(
                "[e2e]\ntests = [\"e2e/**/*.cdp\"]\ndefault_backend = \"load\"\n\n\
                 [e2e.backends.load]\nmode = \"real\"\nbase_url = \"{}\"\n\n{load}",
                server.url()
            ),
        )
        .unwrap();
        for (name, text) in scenarios {
            std::fs::write(root.join("e2e").join(name), text).unwrap();
        }
        Self {
            sandbox,
            root,
            server,
        }
    }

    fn load(&self, args: &[&str]) -> Output {
        common::run(
            self.sandbox
                .cli()
                .current_dir(&self.root)
                .arg("load")
                .args(args)
                .arg("--json"),
        )
    }
}

fn data(out: &Output) -> Value {
    let envelope: Value = serde_json::from_str(&stdout(out))
        .unwrap_or_else(|e| panic!("{e}: {}\n{}", stdout(out), stderr(out)));
    if envelope["ok"] == true {
        envelope["data"].clone()
    } else {
        envelope["error"]["details"].clone()
    }
}

#[test]
fn users_loop_the_journey_and_every_step_is_timed() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome, &[("shop.cdp", JOURNEY)]);
    let out = project.load(&[
        "e2e/shop.cdp",
        "--users",
        "4",
        "--users-per-browser",
        "2",
        "--iterations",
        "2",
        "--raw",
    ]);
    assert_eq!(common::code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    let data = data(&out);
    assert_eq!(data["users"], 4);
    assert_eq!(data["browsers"], 2);
    assert_eq!(data["iterations"], 8, "{data}");
    assert_eq!(data["failed"], 0, "{data}");
    assert_eq!(data["steps"]["ok"], 8 * 10, "{data}");
    assert_eq!(data["journeys"][0]["iterations"], 8);
    // Each iteration's fresh context has a cart of its own.
    let seen = project.server.stats();
    assert_eq!(seen["POST /api/cart"].requests, 8, "{seen:?}");
    // steps.jsonl: a line per step, each user's in order.
    let raw = std::fs::read_to_string(data["raw"].as_str().unwrap()).unwrap();
    let lines: Vec<Value> = raw
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 80);
    for line in &lines {
        assert_eq!(line["result"], "ok", "{line}");
        assert!(
            line["end_ms"].as_f64() >= line["start_ms"].as_f64(),
            "{line}"
        );
        assert!(line["at"].as_str().unwrap().starts_with("e2e/shop.cdp:"));
    }
    let think = lines
        .iter()
        .find(|l| l["command"] == "think 100ms")
        .unwrap();
    let waited = think["end_ms"].as_f64().unwrap() - think["start_ms"].as_f64().unwrap();
    assert!((95.0..400.0).contains(&waited), "{think}");
    let mut users: Vec<u64> = lines.iter().map(|l| l["vu"].as_u64().unwrap()).collect();
    users.sort_unstable();
    users.dedup();
    assert_eq!(users, [1, 2, 3, 4]);
}

#[test]
fn keep_failures_keeps_that_many() {
    let Some(chrome) = chrome_path() else { return };
    let failing = "@test \"looks for what is not there\"\n  goto /\n  expect 'role=heading[name=\"Nope\"]' visible --timeout 300ms\n";
    let project = Project::new(&chrome, &[("fails.cdp", failing)]);
    let out = project.load(&[
        "e2e/fails.cdp",
        "--users",
        "2",
        "--iterations",
        "3",
        "--keep-failures",
        "3",
    ]);
    // Every iteration failed: the journey is broken.
    assert_eq!(common::code(&out), 1, "{}\n{}", stdout(&out), stderr(&out));
    let data = data(&out);
    assert_eq!(data["iterations"], 6, "{data}");
    assert_eq!(data["failed"], 6);
    assert_eq!(data["errors"]["ExpectFailed"], 6);
    assert_eq!(data["failures_kept"], 3);
    let kept: Vec<PathBuf> = std::fs::read_dir(data["failures_dir"].as_str().unwrap())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(kept.len(), 3, "{kept:?}");
    for dir in kept {
        for file in [
            "failure.png",
            "snapshot.txt",
            "console.txt",
            "requests.txt",
            "report.md",
        ] {
            assert!(dir.join(file).is_file(), "{} has no {file}", dir.display());
        }
        let requests = std::fs::read_to_string(dir.join("requests.txt")).unwrap();
        assert!(
            requests.contains(&format!("GET 200 {}/", project.server.url())),
            "{requests}"
        );
        let report = std::fs::read_to_string(dir.join("report.md")).unwrap();
        assert!(report.contains("expect failed"), "{report}");
    }
}

#[test]
fn a_crashed_chrome_is_started_again() {
    let Some(chrome) = chrome_path() else { return };
    let slow = JOURNEY.replace("think 100ms", "think 400ms");
    let project = Project::new(&chrome, &[("shop.cdp", &slow)]);
    let child = project
        .sandbox
        .cli()
        .current_dir(&project.root)
        .args([
            "load",
            "e2e/shop.cdp",
            "--users",
            "2",
            "--duration",
            "8s",
            "--raw",
            "--json",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Once the users run, kill the browser process.
    let started = Instant::now();
    let browser = loop {
        assert!(started.elapsed() < Duration::from_secs(30), "no Chrome ran");
        let found = project
            .sandbox
            .chromes()
            .into_iter()
            .find(|p| !p.contains("--type="));
        if let Some(line) = found
            && project.server.stats().contains_key("GET /api/products")
        {
            break line;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let pid = browser.split_whitespace().next().unwrap();
    assert!(
        std::process::Command::new("kill")
            .args(["-9", pid])
            .status()
            .unwrap()
            .success()
    );
    let carts = || {
        project
            .server
            .stats()
            .get("POST /api/cart")
            .map_or(0, |seen| seen.requests)
    };
    let killed_at = carts();
    let out = child.wait_with_output().unwrap();
    assert_eq!(common::code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    let data = data(&out);
    assert!(data["browser_crashes"].as_u64() >= Some(1), "{data}");
    assert!(
        data["errors"]["browser_crash"].as_u64() >= Some(1),
        "{data}"
    );
    // The users went on in the new Chrome.
    let after = carts();
    assert!(
        after > killed_at + 1,
        "{killed_at} carts before, {after} after"
    );
    let raw = std::fs::read_to_string(data["raw"].as_str().unwrap()).unwrap();
    assert!(raw.contains("\"kind\":\"browser_crash\""), "{raw}");
}

#[test]
fn a_journeys_viewport_is_its_users() {
    let Some(chrome) = chrome_path() else { return };
    let journey = "@test \"on a phone\" @viewport 390x844\n  goto /\n  eval 'if (innerWidth !== 390) throw new Error(\"width \" + innerWidth)'\n";
    let project = Project::new(&chrome, &[("phone.cdp", journey)]);
    let out = project.load(&["e2e/phone.cdp", "--users", "2", "--iterations", "1"]);
    assert_eq!(common::code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    let data = data(&out);
    assert_eq!(data["iterations"], 2, "{data}");
    assert_eq!(data["failed"], 0, "{data}");
}

/// The requests the server saw of `METHOD path`.
fn seen(project: &Project, endpoint: &str) -> u64 {
    project
        .server
        .stats()
        .get(endpoint)
        .map_or(0, |seen| seen.requests)
}

#[test]
fn third_party_requests_are_blocked_and_nothing_else() {
    let Some(chrome) = chrome_path() else { return };
    // The same server as a third party: 127.0.0.1 is another host than
    // the backend's localhost.
    let journey = |port: &str| {
        format!(
            "@test \"fetches its own and a third party's\"\n  goto /\n  \
             eval \"await Promise.allSettled([fetch('/own.js'), \
             fetch('http://127.0.0.1:{port}/third-party.js', {{ mode: 'no-cors' }})])\"\n  \
             expect 'role=heading[name=\"Products\"]' visible\n"
        )
    };
    for (load, third_party) in [("", 2), ("[load]\nblock = [\"third-party\"]\n", 0)] {
        let project = Project::with_load(&chrome, &[], load);
        let port = project.server.url().rsplit(':').next().unwrap().to_owned();
        std::fs::write(project.root.join("e2e/fetch.cdp"), journey(&port)).unwrap();
        let out = project.load(&["e2e/fetch.cdp", "--users", "1", "--iterations", "2"]);
        assert_eq!(common::code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
        let data = data(&out);
        assert_eq!(data["failed"], 0, "{data}");
        assert_eq!(seen(&project, "GET /own.js"), 2, "{load}");
        assert_eq!(seen(&project, "GET /third-party.js"), third_party, "{load}");
        assert_eq!(seen(&project, "GET /api/products"), 2, "{load}");
    }
}

#[test]
fn returning_users_fetch_the_app_once() {
    let Some(chrome) = chrome_path() else { return };
    let journey =
        "@test \"browses\"\n  goto /\n  expect 'role=heading[name=\"Products\"]' visible\n";
    for (cache, files, contexts) in [("new", 3, 3), ("returning", 1, 1)] {
        let project = Project::with_load(
            &chrome,
            &[("browse.cdp", journey)],
            &format!("[load]\ncache = \"{cache}\"\n"),
        );
        let out = project.load(&["e2e/browse.cdp", "--users", "1", "--iterations", "3"]);
        assert_eq!(common::code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
        let data = data(&out);
        assert_eq!(data["cache"], cache);
        assert_eq!(data["iterations"], 3, "{data}");
        assert_eq!(data["contexts"], contexts, "{data}");
        let stats = project.server.stats();
        let script = stats
            .iter()
            .find(|(k, _)| {
                k.starts_with("GET /assets/")
                    && Path::new(k.as_str()).extension().is_some_and(|e| e == "js")
            })
            .unwrap_or_else(|| panic!("no script in {stats:?}"));
        assert_eq!(script.1.requests, files, "{cache}: {stats:?}");
        // The page itself is not cacheable.
        assert_eq!(seen(&project, "GET /"), 3, "{cache}");
    }
}

#[test]
fn a_page_above_renderer_memory_is_a_memory_error() {
    let Some(chrome) = chrome_path() else { return };
    let journey = "@test \"leaks\"\n  goto /\n  \
                   eval \"window.leak = new Array(8e6).fill(0).map((_, i) => i + 0.5); 1\"\n  \
                   think 1100ms\n  expect 'role=heading[name=\"Products\"]' visible\n";
    let project = Project::with_load(
        &chrome,
        &[("leak.cdp", journey)],
        "[load]\ncache = \"returning\"\nrenderer_memory = \"32MB\"\n",
    );
    let out = project.load(&["e2e/leak.cdp", "--users", "1", "--iterations", "2", "--raw"]);
    // Every iteration failed.
    assert_eq!(common::code(&out), 1, "{}\n{}", stdout(&out), stderr(&out));
    let data = data(&out);
    assert_eq!(data["errors"]["memory"], 2, "{data}");
    // The user went on in a fresh context.
    assert_eq!(data["contexts"], 2, "{data}");
    let raw = std::fs::read_to_string(data["raw"].as_str().unwrap()).unwrap();
    let line = raw
        .lines()
        .find(|l| l.contains("\"hook\":\"memory\""))
        .unwrap_or_else(|| panic!("{raw}"));
    assert!(
        line.contains("above [load] renderer_memory (32MB)"),
        "{line}"
    );
}

#[test]
fn a_chrome_cdp_chrome_that_is_no_shell_is_said() {
    let Some(chrome) = chrome_path() else { return };
    if chrome
        .file_name()
        .is_some_and(|n| n == "chrome-headless-shell")
    {
        return;
    }
    let journey = "@test \"browses\"\n  goto /\n";
    let project = Project::new(&chrome, &[("browse.cdp", journey)]);
    let out = common::run(project.sandbox.cli().current_dir(&project.root).args([
        "load",
        "e2e/browse.cdp",
        "--users",
        "1",
        "--iterations",
        "1",
        "--browser",
        "headless-shell",
    ]));
    assert_eq!(common::code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    assert!(
        stderr(&out).contains("which is not chrome-headless-shell: the users run in it"),
        "{}",
        stderr(&out)
    );
    assert!(
        stdout(&out).starts_with("load: 1 user in 1 Chrome (chrome), "),
        "{}",
        stdout(&out)
    );
}
