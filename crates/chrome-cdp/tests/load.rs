//! `chrome-cdp load` (plan 23.1) against the load server
//! (`testdata/load/server`): users loop the reference journey in real
//! Chromes and the server sees their requests, `--raw` writes every
//! step's timing, `--keep-failures 3` keeps exactly three failures, and
//! a Chrome killed mid-run is started again while its users go on.
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
        let sandbox = Sandbox::with_chrome(chrome);
        let server = LoadServer::start(Options::default()).unwrap();
        let root = sandbox.dir.path().join("project");
        std::fs::create_dir_all(root.join("e2e")).unwrap();
        std::fs::write(
            root.join("chrome-cdp.toml"),
            format!(
                "[e2e]\ntests = [\"e2e/**/*.cdp\"]\ndefault_backend = \"load\"\n\n\
                 [e2e.backends.load]\nmode = \"real\"\nbase_url = \"{}\"\n",
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
