//! `chrome-cdp load` (plan 23.1) against the load server
//! (`testdata/load/server`): users loop the reference journey in real
//! Chromes and the server sees their requests, `--raw` writes every
//! step's timing, `--keep-failures 3` keeps exactly three failures, and
//! a Chrome killed mid-run is started again while its users go on. Plan
//! 23.2's `[load]` settings: `block = ["third-party"]` stops a
//! third-party request and nothing else, `cache = "returning"` fetches
//! the app's files once per user, and `renderer_memory` stops a page that
//! grows past it. Plan 23.3's profiles: an open model's arrivals keep
//! their rate against a slow server, which shows in the iterations' time
//! from when they were due and in dropped arrivals; a closed model's users
//! take rows of a journey's data until it runs out; `on_failure =
//! "stop-run"` ends a run against a broken site; Ctrl-C lets the running
//! iterations finish and writes the report; `think_between_steps` pauses
//! after each action. Plan 23.4's metrics: the server's latency and error
//! rate per endpoint as the load server sets them, every page view's Web
//! Vitals, the reports, a failed threshold's exit code, `--compare` and
//! `--metrics-listen`.
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
        Self::with_server(chrome, scenarios, load, Options::default())
    }

    /// A project against a load server started with `options`.
    fn with_server(
        chrome: &Path,
        scenarios: &[(&str, &str)],
        load: &str,
        options: Options,
    ) -> Self {
        let sandbox = Sandbox::with_chrome(chrome);
        let server = LoadServer::start(options).unwrap();
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

    /// Writes `e2e/load/<name>.toml`.
    fn profile(&self, name: &str, text: &str) {
        let dir = self.root.join("e2e/load");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.toml")), text).unwrap();
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

/// The browse journey: the products page, whose `GET /api/products` the
/// server can slow down.
const BROWSE: &str =
    "@test \"browses\"\n  goto /\n  expect 'role=heading[name=\"Products\"]' visible\n";

#[test]
fn arrivals_keep_their_rate_when_the_server_is_slow() {
    let Some(chrome) = chrome_path() else { return };
    let options = Options {
        faults: vec![load_server::Fault::latency("/api/products=2s").unwrap()],
        ..Options::default()
    };
    let project = Project::with_server(&chrome, &[("shop.cdp", BROWSE)], "", options);
    // 5 a second for 4 s: 20 arrivals, each taking over 2 s, so about 10
    // would run at once; only 3 may.
    project.profile(
        "slow",
        "[[journeys]]\ntest = \"e2e/shop.cdp:browses\"\n\n\
         [arrivals]\nrate = \"5/s\"\nduration = \"4s\"\nmax_users = 3\n",
    );
    let started = Instant::now();
    let out = project.load(&["slow", "--raw"]);
    assert_eq!(common::code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    let data = data(&out);
    assert_eq!(data["model"], "open");
    assert_eq!(data["profile"], "e2e/load/slow.toml");
    // The rate held: every arrival came, those past max_users dropped.
    assert_eq!(data["arrivals"], 20, "{data}");
    let dropped = data["dropped"].as_u64().unwrap();
    let iterations = data["iterations"].as_u64().unwrap();
    assert_eq!(iterations + dropped, 20, "{data}");
    assert!(dropped >= 10, "{data}");
    assert_eq!(data["failed"], 0, "{data}");
    assert!(data["users"].as_u64() <= Some(3), "{data}");
    // The slowdown is in the iterations' time.
    let p50 = data["iteration_ms"]["p50"].as_f64().unwrap();
    assert!(p50 >= 2000.0, "{data}");
    // Each iteration's time counts from when its arrival was due: 5 a
    // second, from 0.1 s.
    let raw =
        std::fs::read_to_string(Path::new(data["dir"].as_str().unwrap()).join("iterations.jsonl"))
            .unwrap();
    let mut due: Vec<f64> = raw
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["due_ms"]
                .as_f64()
                .unwrap()
        })
        .collect();
    due.sort_by(f64::total_cmp);
    assert_eq!(due.len() as u64, iterations);
    assert!((due[0] - 100.0).abs() < 1.0, "{due:?}");
    for d in &due {
        let slot = (d - 100.0) / 200.0;
        assert!((slot - slot.round()).abs() < 0.01, "{due:?}");
    }
    // The arrivals did not wait for the slow iterations.
    assert!(started.elapsed() < Duration::from_secs(30));
}

#[test]
fn a_closed_models_users_take_rows_until_they_run_out() {
    let Some(chrome) = chrome_path() else { return };
    let journey = "@test \"signs in\"\n  goto /\n  \
                   eval \"if (!'${row.email}'.endsWith('@example.com')) throw new Error('row')\"\n  \
                   eval \"if (!'${vu.id}'.startsWith('${run.id}-u')) throw new Error('vu')\"\n  \
                   eval \"if ('${iteration.id}' !== '${test.id}') throw new Error('iteration')\"\n";
    let project = Project::new(&chrome, &[("signin.cdp", journey), ("shop.cdp", BROWSE)]);
    std::fs::write(
        project.root.join("e2e/users.csv"),
        "email,password\n1@example.com,a\n2@example.com,b\n3@example.com,c\n4@example.com,d\n5@example.com,e\n",
    )
    .unwrap();
    project.profile(
        "accounts",
        "[[journeys]]\ntest = \"e2e/signin.cdp:signs in\"\ndata = \"e2e/users.csv\"\n\
         data_exhausted = \"stop\"\n\n[users]\ncount = 2\nduration = \"1m\"\n",
    );
    let out = project.load(&["accounts"]);
    assert_eq!(common::code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    let data = data(&out);
    assert_eq!(data["model"], "closed");
    assert_eq!(data["iterations"], 5, "{data}");
    assert_eq!(data["failed"], 0, "{data}");
    assert_eq!(data["stopped"]["reason"], "data_exhausted", "{data}");
    assert_eq!(data["journeys"][0]["data"], "e2e/users.csv");
    // A column the data has not is a problem before any browser starts.
    project.profile(
        "typo",
        "[[journeys]]\ntest = \"e2e/signin.cdp:signs in\"\ndata = \"e2e/shop.csv\"\n\n\
         [users]\ncount = 2\nduration = \"1m\"\n",
    );
    std::fs::write(project.root.join("e2e/shop.csv"), "mail\nx\n").unwrap();
    let out = project.load(&["typo"]);
    assert_eq!(common::code(&out), 2, "{}\n{}", stdout(&out), stderr(&out));
    let error: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(
        error["error"]["message"],
        "e2e/signin.cdp:3:3: ${row.email}: data e2e/shop.csv has no column email",
        "{error}"
    );
    assert_eq!(error["error"]["hint"], "its columns are mail");
}

#[test]
fn stop_run_ends_a_run_against_a_broken_site() {
    let Some(chrome) = chrome_path() else { return };
    let failing = "@test \"looks for what is not there\"\n  goto /\n  \
                   expect 'role=heading[name=\"Nope\"]' visible --timeout 200ms\n";
    let project = Project::new(&chrome, &[("fails.cdp", failing)]);
    project.profile(
        "down",
        "on_failure = \"stop-run\"\nmax_failures = 3\n\n\
         [[journeys]]\ntest = \"e2e/fails.cdp:1\"\n\n[users]\ncount = 2\nduration = \"1m\"\n",
    );
    let started = Instant::now();
    let out = project.load(&["down", "--keep-failures", "0"]);
    assert_eq!(common::code(&out), 1, "{}\n{}", stdout(&out), stderr(&out));
    assert!(started.elapsed() < Duration::from_secs(40));
    let envelope: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("the run stopped: 3 iterations failed (max_failures = 3"),
        "{envelope}"
    );
    let data = &envelope["error"]["details"];
    assert_eq!(data["stopped"]["reason"], "max_failures");
    // The two users' iterations running when the third failed finish.
    let failed = data["failed"].as_u64().unwrap();
    assert!((3..=4).contains(&failed), "{data}");
}

#[test]
fn ctrl_c_lets_the_running_iterations_finish_and_writes_the_report() {
    let Some(chrome) = chrome_path() else { return };
    let journey = "@test \"browses slowly\"\n  goto /\n  think 3s\n  \
                   expect 'role=heading[name=\"Products\"]' visible\n";
    let project = Project::new(&chrome, &[("slow.cdp", journey)]);
    project.profile(
        "long",
        "[[journeys]]\ntest = \"e2e/slow.cdp:browses slowly\"\n\n[users]\ncount = 3\nduration = \"5m\"\n",
    );
    let child = project
        .sandbox
        .cli()
        .current_dir(&project.root)
        .args(["load", "long", "--json"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Once the users are on their pages, Ctrl-C.
    let started = Instant::now();
    while project
        .server
        .stats()
        .get("GET /api/products")
        .map_or(0, |s| s.requests)
        < 3
    {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "no user started"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let pid = child.id().to_string();
    assert!(
        std::process::Command::new("kill")
            .args(["-INT", &pid])
            .status()
            .unwrap()
            .success()
    );
    let out = child.wait_with_output().unwrap();
    assert_eq!(common::code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    let data = data(&out);
    assert_eq!(data["stopped"]["reason"], "interrupted", "{data}");
    assert_eq!(data["stopped"]["cut_short"], 0, "{data}");
    // The three iterations that were running finished, and none started.
    assert_eq!(data["iterations"], 3, "{data}");
    assert_eq!(data["failed"], 0, "{data}");
}

#[test]
fn think_between_steps_pauses_after_each_action() {
    let Some(chrome) = chrome_path() else { return };
    let journey = "@test \"browses\"\n  goto /\n  \
                   expect 'role=heading[name=\"Products\"]' visible\n  \
                   click 'role=link[name=\"Wireless mouse\"]'\n  think 50ms\n  \
                   expect role=heading text 'Wireless mouse'\n";
    let project = Project::new(&chrome, &[("shop.cdp", journey)]);
    project.profile(
        "paced",
        "think = \"300ms\"\nthink_between_steps = true\n\n\
         [[journeys]]\ntest = \"e2e/shop.cdp:browses\"\n\n[users]\ncount = 1\niterations = 1\n",
    );
    let out = project.load(&["paced", "--raw"]);
    assert_eq!(common::code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    let data = data(&out);
    assert_eq!(data["failed"], 0, "{data}");
    let raw = std::fs::read_to_string(data["raw"].as_str().unwrap()).unwrap();
    let commands: Vec<String> = raw
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["command"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    // After `goto` the profile's think; after `click` the journey's own.
    assert_eq!(
        commands,
        [
            "goto /",
            "think (think_between_steps)",
            "expect 'role=heading[name=\"Products\"]' visible",
            "click 'role=link[name=\"Wireless mouse\"]'",
            "think 50ms",
            "expect role=heading text 'Wireless mouse'",
        ]
    );
}

/// A server whose `POST /api/cart` takes 200 ms.
fn slow_cart() -> Options {
    Options {
        faults: vec![load_server::Fault::latency("POST /api/cart=200ms").unwrap()],
        ..Options::default()
    }
}

/// The metric named `name` in `list`.
fn metric<'a>(list: &'a Value, name: &str) -> &'a Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == name)
        .unwrap_or_else(|| panic!("no {name} in {list}"))
}

#[test]
fn requests_are_timed_by_endpoint_and_every_page_view_has_its_vitals() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::with_server(&chrome, &[("shop.cdp", JOURNEY)], "", slow_cart());
    let out = project.load(&["e2e/shop.cdp", "--users", "4", "--iterations", "2"]);
    assert_eq!(common::code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    let data = data(&out);
    assert_eq!(data["iterations"], 8, "{data}");
    // The server's 200 ms, within 10%.
    let cart = metric(&data["endpoints"], "POST /api/cart");
    assert_eq!(cart["count"], 8, "{cart}");
    let p50 = cart["p50"].as_f64().unwrap();
    assert!((180.0..=220.0).contains(&p50), "{cart}");
    assert!(
        cart["phases"]["ttfb_ms"].as_f64().unwrap() >= 190.0,
        "{cart}"
    );
    assert_eq!(
        metric(&data["endpoints"], "GET /api/products/:id")["count"],
        8
    );
    assert!(metric(&data["endpoints"], "static js")["count"].as_u64() >= Some(8));
    assert_eq!(data["requests"]["errors"], 0, "{data}");
    // Each iteration loads one page (the shop is a single-page app), and
    // each page view has its Web Vitals.
    for vital in ["ttfb", "fcp", "lcp", "load", "cls"] {
        assert_eq!(
            metric(&data["vitals"], vital)["count"],
            8,
            "{vital}: {data}"
        );
    }
    // Clicks are interactions.
    assert_eq!(metric(&data["vitals"], "inp")["count"], 8, "{data}");
    let goto = metric(&data["step_times"], "step e2e/shop.cdp:2");
    assert_eq!(
        (goto["count"].as_u64(), goto["label"].as_str()),
        (Some(8), Some("goto /"))
    );
    assert_eq!(
        metric(&data["step_times"], "step e2e/shop.cdp:4")["label"],
        "think 100ms"
    );
    // The files.
    let files = &data["files"];
    let summary: Value =
        serde_json::from_str(&std::fs::read_to_string(files["summary"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_eq!(summary["run"], data["run"]);
    assert_eq!(summary["endpoints"], data["endpoints"]);
    let jsonl = std::fs::read_to_string(files["metrics"].as_str().unwrap()).unwrap();
    let lines: Vec<Value> = jsonl
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(lines.iter().any(|l| l["group"] == "run"), "{jsonl}");
    let carts: u64 = lines
        .iter()
        .filter(|l| l["group"] == "POST /api/cart")
        .map(|l| l["count"].as_u64().unwrap())
        .sum();
    assert_eq!(carts, 8, "{jsonl}");
    let md = std::fs::read_to_string(files["report"].as_str().unwrap()).unwrap();
    assert!(md.contains("| `POST /api/cart` | 8 |"), "{md}");
    let html = std::fs::read_to_string(files["html"].as_str().unwrap()).unwrap();
    assert!(html.contains("<figcaption>Request p95</figcaption>"));
    assert!(!html.contains("src=\"http") && !html.contains("<link"));
}

#[test]
fn errors_are_counted_per_endpoint() {
    let Some(chrome) = chrome_path() else { return };
    let options = Options {
        faults: vec![load_server::Fault::errors("GET /api/products=2%").unwrap()],
        ..Options::default()
    };
    // 501 requests to /api/products an iteration (the page's own and 500
    // more, which read their bodies: Chrome ends a fetch only then), 20
    // iterations: over 10,000.
    let journey = "@test \"lists the products\"\n  goto /\n  \
                   eval \"Promise.all(Array.from({ length: 500 }, () => fetch('/api/products').then((r) => r.text())))\"\n";
    let project = Project::with_server(&chrome, &[("list.cdp", journey)], "", options);
    let out = project.load(&["e2e/list.cdp", "--users", "4", "--iterations", "5"]);
    assert_eq!(common::code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    let data = data(&out);
    let products = metric(&data["endpoints"], "GET /api/products");
    let count = products["count"].as_u64().unwrap();
    assert!(count >= 10_000, "{products}");
    let rate = products["error_rate"].as_f64().unwrap();
    assert!((0.015..=0.025).contains(&rate), "{products}");
    assert_eq!(
        products["errors"], products["error_kinds"]["500"],
        "{products}"
    );
    assert_eq!(
        data["request_errors"]["http_5xx"], products["errors"],
        "{data}"
    );
}

/// Waits for the first scrape of `addr` that has the users' gauge.
fn scrape(addr: &str) -> std::thread::JoinHandle<Option<String>> {
    let addr = addr.to_owned();
    std::thread::spawn(move || {
        use std::io::{Read as _, Write as _};
        let until = Instant::now() + Duration::from_secs(60);
        while Instant::now() < until {
            if let Ok(mut stream) = std::net::TcpStream::connect(&addr) {
                let _ = stream.write_all(b"GET /metrics HTTP/1.1\r\nHost: x\r\n\r\n");
                let mut text = String::new();
                let _ = stream.read_to_string(&mut text);
                if text.contains("chrome_cdp_load_requests_total{") {
                    return Some(text);
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        None
    })
}

#[test]
fn a_failed_threshold_exits_6_and_runs_compare_and_serve_their_numbers() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::with_server(&chrome, &[("shop.cdp", JOURNEY)], "", slow_cart());
    project.profile(
        "gated",
        "[[journeys]]\ntest = \"e2e/shop.cdp:1\"\n\n[users]\ncount = 2\niterations = 2\n\n\
         [thresholds]\n\"p95(POST /api/cart)\" = \"< 100ms\"\n\"error_rate\" = \"< 1%\"\n\
         \"p75(lcp)\" = \"< 10s\"\n\"count(dropped)\" = \"== 0\"\n",
    );
    // A free port, unless another test's Chrome takes it first: then
    // another.
    let (addr, scraped, out) = loop {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let addr = format!("127.0.0.1:{port}");
        let scraped = scrape(&addr);
        let out = project.load(&["gated", "--metrics-listen", &addr]);
        if !stdout(&out).contains("cannot listen on") {
            break (addr, scraped, out);
        }
    };
    assert_eq!(common::code(&out), 6, "{}\n{}", stdout(&out), stderr(&out));
    let envelope: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(envelope["error"]["kind"], "ThresholdFailed", "{envelope}");
    assert_eq!(envelope["error"]["message"], "1 of 4 thresholds failed");
    let first = data(&out);
    let thresholds = first["thresholds"].as_array().unwrap();
    assert_eq!(thresholds[0]["passed"], false, "{first}");
    assert!(thresholds[0]["value"].as_f64().unwrap() >= 190.0, "{first}");
    assert!(
        thresholds[1..].iter().all(|t| t["passed"] == true),
        "{first}"
    );
    assert_eq!(first["metrics_listen"], addr);
    // The numbers were served while it ran, and are no longer.
    let text = scraped.join().unwrap().expect("no scrape answered");
    assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{text}");
    assert!(
        text.contains("# TYPE chrome_cdp_load_users gauge"),
        "{text}"
    );
    assert!(std::net::TcpStream::connect(&addr).is_err());
    // The next run sets the first one's numbers beside its own.
    let out = project.load(&["gated", "--compare", "last"]);
    assert_eq!(common::code(&out), 6, "{}\n{}", stdout(&out), stderr(&out));
    let second = data(&out);
    assert_eq!(second["compare"]["run"], first["run"], "{second}");
    let changes = second["compare"]["changes"].as_array().unwrap();
    assert!(
        changes.iter().any(|c| c["name"] == "p95(POST /api/cart)"),
        "{second}"
    );
    // A run that is not there fails before anything starts.
    let out = project.load(&["gated", "--compare", "nope"]);
    assert_eq!(common::code(&out), 1, "{}", stdout(&out));
    assert!(
        stdout(&out).contains("--compare: no run nope"),
        "{}",
        stdout(&out)
    );
}
