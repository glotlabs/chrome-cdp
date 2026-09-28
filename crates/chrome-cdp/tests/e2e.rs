//! `chrome-cdp test` (plan 11, milestone 11.4) on the fixture project in
//! `testdata/e2e`, against a real Chrome (`CHROME_PATH`; skipped without
//! it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture site.
//! Each test copies the project to a temp dir and adds the site's origin
//! as its `base_url`.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Output;

use common::{Sandbox, chrome_path, stderr, stdout};
use fixtures::Fixtures;
use serde_json::Value;

/// A copy of `testdata/e2e` with `base_url` set, in a sandbox with Chrome.
struct Project {
    sandbox: Sandbox,
    root: PathBuf,
}

impl Project {
    fn new(chrome: &Path) -> Self {
        let sandbox = Sandbox::with_chrome(chrome);
        let root = sandbox.dir.path().join("app");
        copy_dir(&testdata(), &root);
        let config = root.join("chrome-cdp.toml");
        let text = std::fs::read_to_string(&config).unwrap().replace(
            "[e2e]\n",
            &format!("[e2e]\nbase_url = \"{}\"\n", Fixtures::shared().a()),
        );
        std::fs::write(&config, text).unwrap();
        Self { sandbox, root }
    }

    fn test(&self, args: &[&str]) -> Output {
        common::run(
            self.sandbox
                .cli()
                .current_dir(&self.root)
                .arg("test")
                .args(args),
        )
    }

    fn json(&self, args: &[&str]) -> (Output, Value) {
        let out = self.test(&[args, &["--json"]].concat());
        let value = serde_json::from_slice(&out.stdout).unwrap();
        (out, value)
    }
}

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/e2e")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// `[(status, name)]` of a `--json` run's tests.
fn statuses(value: &Value) -> Vec<(String, String)> {
    let data = if value["ok"] == Value::Bool(true) {
        &value["data"]
    } else {
        &value["error"]["details"]
    };
    data["tests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["status"].as_str().unwrap().to_owned(),
                t["name"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn the_passing_suite_passes_in_parallel_contexts() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let out = project.test(&["--jobs", "4"]);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    assert!(
        text.starts_with("running 7 tests from 2 files, 4 at once\n"),
        "{text}"
    );
    assert!(
        text.contains("✓ scenarios/pass/todos.cdp:9 adds a todo ("),
        "{text}"
    );
    assert!(text.contains("\n7 tests: 7 passed ("), "{text}");
    // The run's session is closed and gone: no daemon, no session dir.
    assert!(project.sandbox.daemons().is_empty());
    let sessions = project.sandbox.runtime_dir().join("chrome-cdp");
    let left: Vec<_> = std::fs::read_dir(&sessions)
        .map(|d| d.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn tests_do_not_see_each_others_writes() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    // Eight tests at once, each adding to the same collection, each
    // expecting only its own item on top of the seed.
    let mut scenario =
        String::from("@before\n  goto /e2e/shop.html\n  expect 'role=listitem' count 2\n\n");
    for i in 1..=8 {
        let _ = write!(
            scenario,
            "@test \"writer {i}\"\n  @include ../../steps/add.cdp title=Item-{i}\n  expect 'role=listitem' count 3\n  expect mock-state todos count 3\n  eval \"document.cookie = 'n={i}'\"\n  eval \"if (document.cookie !== 'n={i}') throw new Error(document.cookie)\"\n\n"
        );
    }
    std::fs::write(project.root.join("scenarios/pass/writers.cdp"), scenario).unwrap();
    let (out, value) = project.json(&["scenarios/pass/writers.cdp", "--jobs", "8"]);
    assert_eq!(out.status.code(), Some(0), "{value}");
    assert_eq!(value["data"]["passed"], 8);
}

#[test]
fn failures_skips_and_flaky_tests_are_reported() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let out = project.test(&["scenarios/fail"]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(
        text.contains("✗ scenarios/fail/fails.cdp:8 fails on a missing element ("),
        "{text}"
    );
    assert!(
        text.contains("    failed at scenarios/fail/fails.cdp:9:3: expect text=Nowhere visible --timeout 500ms\n"),
        "{text}"
    );
    assert!(
        text.contains("- scenarios/fail/fails.cdp:11 is skipped (skipped: not ready)"),
        "{text}"
    );
    assert!(
        text.contains("~ scenarios/fail/flaky.cdp:6 passes on the second try ("),
        "{text}"
    );
    assert!(
        text.contains("4 tests: 1 passed, 1 failed, 1 flaky, 1 skipped ("),
        "{text}"
    );
    // Hints do not name the run's session, which is gone.
    assert!(!text.contains("--session test-"), "{text}");
    let err = stderr(&out);
    assert!(err.starts_with("error: 1 of 4 tests failed\n"), "{err}");
    assert!(
        err.contains("hint: chrome-cdp test scenarios/fail/fails.cdp:8"),
        "{err}"
    );

    // The same in JSON, with the failed step.
    std::fs::remove_file(project.root.join("flaky-marker.json")).unwrap();
    let (out, value) = project.json(&["scenarios/fail", "--jobs", "1"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(value["error"]["kind"], "ExpectFailed");
    let details = &value["error"]["details"];
    assert_eq!(
        statuses(&value),
        [
            ("passed".into(), "passes".into()),
            ("failed".into(), "fails on a missing element".into()),
            ("skipped".into(), "is skipped".into()),
            ("flaky".into(), "passes on the second try".into()),
        ]
    );
    let failed = &details["tests"][1];
    assert_eq!(failed["error"]["at"], "scenarios/fail/fails.cdp:9:3");
    assert_eq!(failed["error"]["error"]["kind"], "ExpectFailed");
    let flaky = &details["tests"][3];
    assert_eq!(flaky["tries"], 2);
    assert_eq!(flaky["error"]["at"], "scenarios/fail/flaky.cdp:7:3");

    // --bail stops after the first failure.
    let (_, value) = project.json(&["scenarios/fail/fails.cdp", "--bail", "--jobs", "1"]);
    let details = &value["error"]["details"];
    assert_eq!(details["not_run"], 0, "{value}");
    assert_eq!(statuses(&value)[2].0, "skipped");
}

#[test]
fn only_lines_and_bail() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    // @only leaves the other tests out.
    let (out, value) = project.json(&["scenarios/only"]);
    assert_eq!(out.status.code(), Some(0), "{value}");
    assert_eq!(statuses(&value), [("passed".into(), "runs".into())]);
    // file:line runs the test at that line (with no @only among them);
    // --bail leaves the rest unrun.
    let (out, value) = project.json(&[
        "scenarios/fail/fails.cdp:9",
        "scenarios/only/only.cdp:4",
        "--bail",
        "--jobs",
        "1",
    ]);
    assert_eq!(out.status.code(), Some(1), "{value}");
    assert_eq!(
        statuses(&value),
        [
            ("failed".into(), "fails on a missing element".into()),
            ("not-run".into(), "does not run".into())
        ]
    );
    let mut writers = String::new();
    for i in 1..=3 {
        let _ = write!(
            writers,
            "@test \"t{i}\"\n  expect text=Nowhere visible --timeout 100ms\n"
        );
    }
    std::fs::write(project.root.join("scenarios/fail/three.cdp"), writers).unwrap();
    let (_, value) = project.json(&["scenarios/fail/three.cdp", "--bail", "--jobs", "1"]);
    let details = &value["error"]["details"];
    assert_eq!(
        (details["failed"].clone(), details["not_run"].clone()),
        (1.into(), 2.into())
    );
}

#[test]
fn a_bad_scenario_fails_before_the_browser_starts() {
    // No Chrome needed: nothing runs.
    let sandbox = Sandbox::new();
    let root = sandbox.dir.path().join("app");
    copy_dir(&testdata(), &root);
    let out = common::run(sandbox.cli().current_dir(&root).args([
        "test",
        "scenarios/bad",
        "scenarios/pass",
    ]));
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(
        err.starts_with("error: scenarios/bad/typo.cdp:3:3: unrecognized subcommand 'clik'\n"),
        "{err}"
    );
    assert!(err.contains("hint: chrome-cdp click text=Add"), "{err}");
    assert!(sandbox.daemons().is_empty());
}

#[test]
fn list_and_shards_need_no_browser() {
    let sandbox = Sandbox::new();
    let root = sandbox.dir.path().join("app");
    copy_dir(&testdata(), &root);
    let run = |args: &[&str]| common::run(sandbox.cli().current_dir(&root).arg("test").args(args));
    let out = run(&["--list"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        stdout(&out),
        "scenarios/pass/layers.cdp:11 the regular price [smoke]\n\
         scenarios/pass/layers.cdp:14 a sale price from a layered set\n\
         scenarios/pass/layers.cdp:18 a phone's viewport\n\
         scenarios/pass/layers.cdp:22 the state the @before-all saved\n\
         scenarios/pass/todos.cdp:9 adds a todo [todos]\n\
         scenarios/pass/todos.cdp:15 adds a todo through a step file [todos]\n\
         scenarios/pass/todos.cdp:19 starts from the seed again [todos smoke]\n\
         7 tests\n"
    );
    // Every test is in exactly one shard.
    let mut all = Vec::new();
    for shard in ["1/3", "2/3", "3/3"] {
        let out = run(&["--list", "-q", "--shard", shard]);
        all.extend(stdout(&out).lines().map(str::to_owned).collect::<Vec<_>>());
    }
    let out = run(&["--list", "-q"]);
    assert_eq!(all, stdout(&out).lines().collect::<Vec<_>>());
    // --tags and --grep.
    let out = run(&["--list", "-q", "--tags", "smoke,!todos"]);
    assert_eq!(stdout(&out), "scenarios/pass/layers.cdp:11\n");
    let out = run(&["--list", "-q", "--grep", "seed|sale"]);
    assert_eq!(
        stdout(&out),
        "scenarios/pass/layers.cdp:14\nscenarios/pass/todos.cdp:19\n"
    );
    // A line outside a test.
    let out = run(&["--list", "scenarios/pass/todos.cdp:3"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).starts_with("error: scenarios/pass/todos.cdp:3 is not in a test\n"));
    assert!(sandbox.daemons().is_empty());
}

#[test]
fn web_servers_are_reused_or_fail_the_run() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let config = project.root.join("chrome-cdp.toml");
    let base = std::fs::read_to_string(&config).unwrap();
    // Something answers on the URL already: it is used as is.
    std::fs::write(
        &config,
        format!(
            "{base}\n[e2e.web_server]\ncommand = \"exit 3\"\nurl = \"{}\"\n",
            Fixtures::shared().a()
        ),
    )
    .unwrap();
    let out = project.test(&["scenarios/only"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(stdout(&out).contains(" (already running)\n"));
    // A server that exits at once fails the run with its log.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    std::fs::write(
        &config,
        format!(
            "{base}\n[e2e.web_server]\ncommand = \"echo booting; exit 3\"\nurl = \"http://127.0.0.1:{port}\"\ntimeout = \"10s\"\n"
        ),
    )
    .unwrap();
    let out = project.test(&["scenarios/only"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert!(stderr(&out).contains("booting"), "{}", stderr(&out));
    assert!(project.sandbox.daemons().is_empty());
}

#[test]
fn a_chrome_per_job_and_snapshot_baselines_in_the_project() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let (out, value) = project.json(&[
        "scenarios/pass/layers.cdp",
        "--jobs",
        "2",
        "--jobs-isolation",
        "browser",
    ]);
    assert_eq!(out.status.code(), Some(0), "{value}");
    assert_eq!(value["data"]["passed"], 4);
    assert!(project.sandbox.daemons().is_empty());

    // expect-snapshot baselines go to e2e/snapshots of the project.
    std::fs::write(
        project.root.join("scenarios/pass/snap.cdp"),
        "@test \"the price\"\n  goto /e2e/shop.html\n  expect text='Price: 49' visible\n  expect-snapshot price --scope '#price'\n",
    )
    .unwrap();
    let out = project.test(&["scenarios/pass/snap.cdp", "--update-snapshots"]);
    assert_eq!(out.status.code(), Some(0));
    let baseline = project.root.join("e2e/snapshots/price.txt");
    assert!(baseline.is_file(), "{}", stdout(&out));
    // Compared, not written, without the flag.
    let out = project.test(&["scenarios/pass/snap.cdp"]);
    assert_eq!(out.status.code(), Some(0));
    std::fs::write(&baseline, "- paragraph: Price: 1\n").unwrap();
    let out = project.test(&["scenarios/pass/snap.cdp"]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));

    // A screenshot baseline made on another OS only: this OS gets its
    // own, with a warning (plan 11.6); in CI, a failure that says why.
    let other = if std::env::consts::OS == "macos" {
        "linux"
    } else {
        "macos"
    };
    let snapshots = project.root.join("e2e/snapshots");
    for name in ["price", "price-ci"] {
        std::fs::copy(
            testdata().join("../site/har/dot.png"),
            snapshots.join(format!("{name}-{other}.png")),
        )
        .unwrap();
    }
    let shot = |name: &str, ci: bool| {
        std::fs::write(
            project.root.join("scenarios/pass/shot.cdp"),
            format!(
                "@test \"the price\"\n  goto /e2e/shop.html\n  expect-screenshot {name} '#price'\n"
            ),
        )
        .unwrap();
        let mut cmd = project.sandbox.cli();
        cmd.current_dir(&project.root)
            .args(["test", "scenarios/pass/shot.cdp"]);
        if ci {
            cmd.env("CI", "1");
        } else {
            cmd.env_remove("CI");
        }
        common::run(&mut cmd)
    };
    let out = shot("price", false);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains(&format!(
            "\n    warning: the baseline price was made on {other} only; fonts render differently on each OS"
        )),
        "{text}"
    );
    let out = shot("price-ci", true);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(
        text.contains(&format!(
            "other OS: the baseline price-ci was made on {other} only;"
        )),
        "{text}"
    );
}

#[test]
fn requests_unmatched_requests_and_console_errors() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let out = project.test(&["scenarios/checks", "--jobs", "1"]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    eprintln!("{text}");
    assert!(
        text.contains("✓ scenarios/checks/checks.cdp:6 posts the todo it adds ("),
        "{text}"
    );
    // An unmatched request fails the step after which it was seen, with
    // the mock that comes closest.
    assert!(
        text.contains("✗ scenarios/checks/checks.cdp:14 fails on an unmatched request ("),
        "{text}"
    );
    assert!(
        text.contains(
            "    failed at scenarios/checks/checks.cdp:15:3: eval \"fetch('/e2e/api/prices?user=7').then((r) => r.status)\"\n    error: unmatched request: GET http://"
        ),
        "{text}"
    );
    assert!(
        text.contains("/e2e/api/prices?user=7\n      scope: mock set shop, whose unmatched = \"fail\" answered it 501\n      closest mock: shop.toml:6 \"price\": path differs: expects /e2e/api/price, got /e2e/api/prices\n"),
        "{text}"
    );
    assert!(
        text.contains("      hint: add a mock for it, or run chrome-cdp mock explain GET 'http://"),
        "{text}"
    );
    assert!(
        text.contains("/e2e/api/prices?user=7' --mocks shop\n"),
        "{text}"
    );
    // A step that failed on its own names what went unmatched meanwhile.
    assert!(
        text.contains(
            "failed at scenarios/checks/checks.cdp:20:3: expect --title 'status 200' --timeout 1s"
        ),
        "{text}"
    );
    assert!(
        text.contains("/e2e/api/prices (no mock answered it; closest mock: shop.toml:6 \"price\": path differs"),
        "{text}"
    );
    // Console errors fail a test unless it allows them.
    assert!(
        text.contains("    failed at scenarios/checks/checks.cdp:22: expect --no-errors --all-tabs (checked after the test's steps)\n"),
        "{text}"
    );
    assert!(text.contains(": boom: the cart is gone (eval:"), "{text}");
    assert!(text.contains("@allow-console-errors"), "{text}");
    assert!(
        text.contains("✓ scenarios/checks/checks.cdp:25 may log a console error ("),
        "{text}"
    );
    assert!(text.contains("5 tests: 2 passed, 3 failed ("), "{text}");

    // allow_console_errors in the config allows them everywhere.
    let config = project.root.join("chrome-cdp.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        text.replace("[e2e]\n", "[e2e]\nallow_console_errors = true\n"),
    )
    .unwrap();
    let (_, value) = project.json(&["scenarios/checks/checks.cdp:22"]);
    assert_eq!(value["ok"], true, "{value}");
}

#[test]
fn tests_see_a_frozen_clock_seeded_random_values_and_no_animations() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let out = project.test(&["scenarios/determinism", "--jobs", "2"]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(0), "{text}{}", stderr(&out));
    assert!(text.contains("\n4 tests: 4 passed ("), "{text}");

    // [e2e.determinism] turns each of them off.
    let config = project.root.join("chrome-cdp.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        format!(
            "{text}\n[e2e.determinism]\nfreeze_time = false\nrandom_seed = false\n\
             disable_animations = false\nlocale = false\ntimezone = false\n"
        ),
    )
    .unwrap();
    let out = project.test(&["scenarios/determinism-off"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
}

#[test]
fn repeat_runs_each_test_and_fails_a_flaky_one() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let out = project.test(&["scenarios/determinism", "--repeat", "3", "--jobs", "4"]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(0), "{text}{}", stderr(&out));
    assert!(
        text.starts_with("running 4 tests 3 times each from 1 file, 4 at once\n"),
        "{text}"
    );
    assert!(
        text.contains("✓ scenarios/determinism/determinism.cdp:11 random values are the same in every run (3 runs, slowest "),
        "{text}"
    );
    assert!(text.contains("\n4 tests: 4 passed ("), "{text}");

    // The flaky test fails its first run (retries are off) and passes
    // the others: the run fails.
    let (out, value) = project.json(&["scenarios/fail/flaky.cdp", "--repeat", "3", "--jobs", "1"]);
    assert_eq!(out.status.code(), Some(1), "{value}");
    let details = &value["error"]["details"];
    assert_eq!(details["repeat"], 3);
    let test = &details["tests"][0];
    assert_eq!(test["status"], "failed", "{test}");
    assert_eq!(test["failed_runs"], 1, "{test}");
    assert_eq!(test["tries"], 3, "{test}");
    assert!(
        value["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("chrome-cdp test scenarios/fail/flaky.cdp:6 --repeat 3"),
        "{value}"
    );
    std::fs::remove_file(project.root.join("flaky-marker.json")).unwrap();
    let text = stdout(&project.test(&["scenarios/fail/flaky.cdp", "--repeat", "3", "--jobs", "1"]));
    assert!(
        text.contains(
            "✗ scenarios/fail/flaky.cdp:6 passes on the second try (failed 1 of 3 runs, slowest "
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "\n    first failure at scenarios/fail/flaky.cdp:7:3: state load flaky-marker.json\n"
        ),
        "{text}"
    );
    let out = project.test(&[
        "scenarios/fail/flaky.cdp",
        "--repeat",
        "2",
        "--retries",
        "1",
    ]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}

#[test]
fn every_test_leaves_artifacts_and_a_failure_a_report() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let results = project.root.join("e2e/.results/scenarios");
    // A leftover of an earlier run is removed.
    let stale = results.join("fail/fails.cdp/passes/report.md");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, "old").unwrap();
    let out = project.test(&[
        "scenarios/fail",
        "scenarios/checks/checks.cdp:18",
        "--jobs",
        "1",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);

    // A passing test: the journals, no failure files.
    let passes = results.join("fail/fails.cdp/passes");
    for file in ["steps.jsonl", "requests.txt", "console.txt"] {
        assert!(passes.join(file).is_file(), "{file}");
    }
    assert!(!stale.exists());
    assert!(!passes.join("failure.png").exists());
    let steps: Vec<Value> = std::fs::read_to_string(passes.join("steps.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let own: Vec<&Value> = steps.iter().filter(|s| s.get("hook").is_none()).collect();
    assert_eq!(own.len(), 1, "{steps:?}");
    assert_eq!(own[0]["command"], "expect text='Price: 49' visible");
    assert_eq!(own[0]["at"], "scenarios/fail/fails.cdp:6:3");
    assert_eq!(own[0]["ok"], true);
    assert!(
        steps
            .iter()
            .any(|s| s["hook"] == "before" && s["command"] == "goto /e2e/shop.html")
    );
    assert!(
        steps
            .iter()
            .any(|s| s["hook"] == "setup" && s["command"] == "mock use")
    );
    assert!(steps.iter().any(|s| s["hook"] == "check"));
    // No run's own session name in the outputs.
    assert!(
        !steps
            .iter()
            .any(|s| s.to_string().contains("--session test-"))
    );
    let requests = std::fs::read_to_string(passes.join("requests.txt")).unwrap();
    assert!(
        requests.contains(" GET 200 ") && requests.contains("/e2e/api/price fetch "),
        "{requests}"
    );
    assert!(
        requests.contains(" ← mock \"price\" (shop.toml:6)\n"),
        "{requests}"
    );

    // A failed test: its report, screenshot and snapshot, named under
    // its line and at the end.
    let failed = results.join("fail/fails.cdp/fails-on-a-missing-element");
    let report = failed.join("report.md");
    assert!(
        text.contains(&format!("    report: {}\n", report.display())),
        "{text}"
    );
    let list = text
        .split("\nfailure reports:\n")
        .nth(1)
        .unwrap_or_default();
    assert!(
        list.contains(&format!("  {}\n", report.display())),
        "{text}"
    );
    assert!(failed.join("failure.png").is_file());
    assert!(
        std::fs::read_to_string(failed.join("snapshot.txt"))
            .unwrap()
            .starts_with("page t")
    );
    let md = std::fs::read_to_string(&report).unwrap();
    assert!(
        md.starts_with("# Failed: fails on a missing element\n\n`scenarios/fail/fails.cdp:8`\n"),
        "{md}"
    );
    assert!(
        md.contains(
            "`scenarios/fail/fails.cdp:9:3`:\n\n    expect text=Nowhere visible --timeout 500ms\n"
        ),
        "{md}"
    );
    assert!(md.contains("  actual: no element matches"), "{md}");
    // The hint to look at the page points at the snapshot: the page is gone.
    assert!(
        md.contains(&format!(
            "  hint: see {}   (the page when the test failed",
            failed.join("snapshot.txt").display()
        )),
        "{md}"
    );
    assert!(
        md.contains("✓ scenarios/fail/fails.cdp:3:3 goto /e2e/shop.html ("),
        "{md}"
    );
    assert!(md.contains("`snapshot -i` when it failed"), "{md}");
    assert!(
        md.contains(&format!(
            "- {}: the page when it failed",
            failed.join("failure.png").display()
        )),
        "{md}"
    );
    assert!(
        md.ends_with("Run it again: `chrome-cdp test scenarios/fail/fails.cdp:8`\n"),
        "{md}"
    );

    // A flaky test: the failed first try's report, the second try below.
    let flaky = results.join("fail/flaky.cdp/passes-on-the-second-try");
    assert!(flaky.join("report.md").is_file());
    assert!(flaky.join("try-2/steps.jsonl").is_file());
    assert!(!flaky.join("try-2/report.md").exists());

    // The request no mock answered, and its console error.
    let unmatched = results.join("checks/checks.cdp/names-the-unmatched-request-a-failed-step-saw");
    let md = std::fs::read_to_string(unmatched.join("report.md")).unwrap();
    assert!(
        md.contains("## Failed and unmatched requests\n\n- t"),
        "{md}"
    );
    assert!(md.contains("/e2e/api/prices fetch "), "{md}");
    assert!(
        md.contains(" ← unmatched in mock set shop (fail); closest mock: shop.toml:6 \"price\""),
        "{md}"
    );
    assert!(md.contains("## Console errors\n\n- t"), "{md}");
    assert!(md.contains("status of 501"), "{md}");

    // The same in JSON.
    let (_, value) = project.json(&["scenarios/fail/fails.cdp", "--jobs", "1"]);
    let tests = &value["error"]["details"]["tests"];
    assert_eq!(tests[0]["artifacts"], passes.display().to_string());
    assert_eq!(tests[1]["error"]["report"], report.display().to_string());
    assert!(tests[2].get("artifacts").is_none(), "{}", tests[2]);

    // --trace: a screenshot after every step.
    let out = project.test(&["scenarios/fail/fails.cdp:5", "--trace"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let steps: Vec<Value> = std::fs::read_to_string(passes.join("steps.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let shots: Vec<&Value> = steps.iter().filter_map(|s| s.get("screenshot")).collect();
    assert_eq!(shots.len(), 2, "{steps:?}");
    for shot in shots {
        assert!(passes.join(shot.as_str().unwrap()).is_file(), "{shot}");
    }
}
