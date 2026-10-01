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
/// Its real backends (plan 11.11) are a fixture server of its own, so
/// their API starts from its seed.
struct Project {
    sandbox: Sandbox,
    root: PathBuf,
    real: Fixtures,
}

impl Project {
    fn new(chrome: &Path) -> Self {
        let sandbox = Sandbox::with_chrome(chrome);
        let root = sandbox.dir.path().join("app");
        copy_dir(&testdata(), &root);
        let real = Fixtures::start(0, 0).unwrap();
        let config = root.join("chrome-cdp.toml");
        let text = std::fs::read_to_string(&config)
            .unwrap()
            .replace(
                "[e2e]\n",
                &format!("[e2e]\nbase_url = \"{}\"\n", Fixtures::shared().a()),
            )
            .replace("http://real.invalid", real.a());
        std::fs::write(&config, text).unwrap();
        Self {
            sandbox,
            root,
            real,
        }
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

    // Two shards run every test, each once.
    let mut ran = Vec::new();
    for shard in ["1/2", "2/2"] {
        let (out, value) = project.json(&["--shard", shard]);
        assert_eq!(out.status.code(), Some(0), "{value}");
        for t in value["data"]["tests"].as_array().unwrap() {
            assert_eq!(t["status"], "passed", "{t}");
            ran.push(format!("{}:{}", t["file"], t["line"]));
        }
    }
    let count = ran.len();
    ran.sort();
    ran.dedup();
    assert_eq!((count, ran.len()), (7, 7), "{ran:?}");
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
    // flaky.cdp's test has no expect: `lint`'s warning comes first (11.12).
    assert!(
        err.starts_with(
            "warning: 1 lint warning; `chrome-cdp lint` lists them\nerror: 1 of 4 tests failed\n"
        ),
        "{err}"
    );
    assert!(
        err.contains("hint: chrome-cdp test scenarios/fail/fails.cdp:8"),
        "{err}"
    );

    // Files steps write go in the test's artifacts, not the project.
    assert!(!project.root.join("flaky-marker.json").exists());
    assert!(!project.root.join("hooks-state.json").exists());

    // The same in JSON, with the failed step.
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
    // Its tries disagreed: no rule fits a missing file, so the diagnosis
    // names the first step whose result differs (plan 18.3).
    let flake = &flaky["flake"];
    assert_eq!(flake["cause"], "unknown", "{flake}");
    assert_eq!(
        (flake["runs"].as_u64(), flake["failed_runs"].as_u64()),
        (Some(2), Some(1))
    );
    assert!(
        flake["evidence"][0]
            .as_str()
            .unwrap()
            .starts_with("the first step whose result differs is `state load flaky-marker.json` (scenarios/fail/flaky.cdp:7:3): try 2 (passed) gave \"loaded "),
        "{flake}"
    );

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
    assert!(!stderr(&out).contains("running: "), "{}", stderr(&out));
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
    // The command was shown, with the config it came from, before it ran.
    assert!(
        stderr(&out).contains(&format!(
            "running: echo booting; exit 3 (from {})\nerror: ",
            config.display()
        )),
        "{}",
        stderr(&out)
    );
    assert!(project.sandbox.daemons().is_empty());
}

#[test]
fn layout_baselines_in_the_project() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    // expect --layout baselines go to e2e/snapshots/layout of the project.
    std::fs::write(
        project.root.join("scenarios/pass/layout.cdp"),
        "@test \"small targets\"\n  goto /layout/small.html\n  expect --layout --baseline small\n",
    )
    .unwrap();
    // No baseline yet: it fails, and --update-snapshots writes one.
    let out = project.test(&["scenarios/pass/layout.cdp"]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let out = project.test(&["scenarios/pass/layout.cdp", "--update-snapshots"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let baseline = project.root.join("e2e/snapshots/layout/small.txt");
    let written = std::fs::read_to_string(&baseline).unwrap();
    assert!(
        written.contains("small-target desktop role=button[name=\"Close\"]\n"),
        "{written}"
    );
    // Compared, not written, without the flag.
    let out = project.test(&["scenarios/pass/layout.cdp"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    std::fs::write(&baseline, "# nothing accepted\n").unwrap();
    let out = project.test(&["scenarios/pass/layout.cdp", "--timeout", "1s"]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("the baseline does not list"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn audit_baselines_in_the_project() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    // expect --audit baselines go to e2e/audit of the project.
    std::fs::write(
        project.root.join("scenarios/pass/audit.cdp"),
        "@test \"no new findings\"\n  goto /audit/a11y.html\n  expect --audit --baseline a11y\n",
    )
    .unwrap();
    let run = |ci: bool| {
        let mut cmd = project.sandbox.cli();
        cmd.current_dir(&project.root)
            .args(["test", "scenarios/pass/audit.cdp"]);
        if ci {
            cmd.env("CI", "1");
        } else {
            cmd.env_remove("CI");
        }
        common::run(&mut cmd)
    };
    let baseline = project.root.join("e2e/audit/a11y.json");
    // In CI, a missing baseline fails; a local run writes it.
    let out = run(true);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("there is no baseline yet"),
        "{}",
        stdout(&out)
    );
    assert!(!baseline.exists());
    let out = run(false);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let written = std::fs::read_to_string(&baseline).unwrap();
    assert!(written.contains("\"a11y/image-alt/img\""), "{written}");
    // Compared now, in CI too.
    let out = run(true);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    std::fs::write(&baseline, "{\"version\": 1, \"accepted\": []}\n").unwrap();
    let out = run(true);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("the baseline does not accept"),
        "{}",
        stdout(&out)
    );
    // lint checks baseline files.
    std::fs::write(
        &baseline,
        "{\"version\": 1, \"accepted\": [{\"key\": \"a11y/image-alt/img\", \"why\": \"x\"}]}\n",
    )
    .unwrap();
    let out = common::run(
        project
            .sandbox
            .cli()
            .current_dir(&project.root)
            .args(["lint", "e2e/audit/a11y.json"]),
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("unknown field `why`"),
        "{}",
        stderr(&out)
    );
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
    assert!(text.contains("\n6 tests: 6 passed ("), "{text}");

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
        text.starts_with("running 6 tests 3 times each from 1 file, 4 at once\n"),
        "{text}"
    );
    assert!(
        text.contains("✓ scenarios/determinism/determinism.cdp:11 random values are the same in every run (3 runs, slowest "),
        "{text}"
    );
    assert!(text.contains("\n6 tests: 6 passed ("), "{text}");

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

/// A test's `steps.jsonl` less what timing decides: each step's
/// `start_ms` and `duration_ms`, and the durations outputs give
/// (`goto`'s `(865ms)`).
fn steps_without_timings(path: &Path) -> Vec<Value> {
    let duration = regex::Regex::new(r"\([0-9.]+(ms|s)\)").unwrap();
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| {
            let mut step: Value = serde_json::from_str(line).unwrap();
            let step_map = step.as_object_mut().unwrap();
            step_map.remove("start_ms");
            step_map.remove("duration_ms");
            if let Some(Value::String(output)) = step_map.get_mut("output") {
                *output = duration.replace_all(output, "(…)").into_owned();
            }
            step
        })
        .collect()
}

#[test]
fn twenty_runs_record_the_same_steps_and_screenshots() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let args = ["scenarios/determinism", "--repeat", "20", "--jobs", "4"];
    let out = project.test(&[&args[..], &["--trace"]].concat());
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );

    let results = project
        .root
        .join("e2e/.results/scenarios/determinism/determinism.cdp");
    let tests: Vec<PathBuf> = std::fs::read_dir(&results)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(tests.len(), 6, "{tests:?}");
    for test in tests {
        let first = test.join("run-1");
        let steps = steps_without_timings(&first.join("steps.jsonl"));
        let mut shots: Vec<PathBuf> = std::fs::read_dir(first.join("trace"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        shots.sort();
        // --trace shoots the test's own steps, not the runner's setup.
        assert!(!shots.is_empty(), "{}", test.display());
        for n in 2..=20 {
            let run = test.join(format!("run-{n}"));
            assert_eq!(
                steps_without_timings(&run.join("steps.jsonl")),
                steps,
                "{}",
                run.display()
            );
            for shot in &shots {
                let other = run.join("trace").join(shot.file_name().unwrap());
                assert!(
                    std::fs::read(&other).unwrap() == std::fs::read(shot).unwrap(),
                    "{} differs from {}",
                    other.display(),
                    shot.display()
                );
            }
        }
    }
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
    // --reporter writes files besides stdout.
    let (out, value) = project.json(&[
        "scenarios/fail/fails.cdp",
        "--reporter",
        "text,json,junit=out/junit.xml,html",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let reporters = &value["error"]["details"]["reporters"];
    let junit = project.root.join("out/junit.xml");
    assert_eq!(reporters["junit"], junit.display().to_string(), "{value}");
    let xml = std::fs::read_to_string(&junit).unwrap();
    assert!(
        xml.contains(
            "<testcase name=\"fails on a missing element\" classname=\"scenarios/fail/fails.cdp\""
        ),
        "{xml}"
    );
    assert!(
        xml.contains(
            "<failure message=\"expect failed: text=Nowhere visible\" type=\"ExpectFailed\">"
        ),
        "{xml}"
    );
    let html = project.root.join("e2e/.results/report.html");
    assert_eq!(reporters["html"], html.display().to_string());
    let page = std::fs::read_to_string(&html).unwrap();
    assert!(
        page.contains("✗ scenarios/fail/fails.cdp:8 fails on a missing element"),
        "{page}"
    );
    assert!(
        page.contains("data:image/png;base64,"),
        "failure.png is inlined"
    );
    let json: Value = serde_json::from_str(
        &std::fs::read_to_string(project.root.join("e2e/.results/results.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(json["failed"], 1);
    let text = stdout(&project.test(&["scenarios/fail/fails.cdp:5", "--reporter", "junit"]));
    assert!(
        text.contains(&format!(
            "\njunit report: {}\n",
            project.root.join("e2e/.results/junit.xml").display()
        )),
        "{text}"
    );
}

#[test]
fn debug_keeps_the_failed_tests_context_open() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let (out, value) = project.json(&["scenarios/fail", "--debug", "--jobs", "1"]);
    assert_eq!(out.status.code(), Some(1), "{value}");
    let error = &value["error"];
    let debug = &error["details"]["debug"];
    assert_eq!(debug["session"], "test-debug", "{value}");
    assert_eq!(debug["test"], "scenarios/fail/fails.cdp:8");
    let context = debug["context"].as_str().unwrap().to_owned();
    assert_eq!(
        error["hint"],
        format!(
            "chrome-cdp snapshot --session test-debug --context {context}   (the page as the test left it), or after a fix, chrome-cdp test scenarios/fail/fails.cdp:8 --debug"
        )
    );
    // It stopped at the failure: the flaky test did not run, and the
    // failed step's hint acts on the kept context.
    assert_eq!(statuses(&value)[3].0, "not-run");
    let step_hint = error["details"]["tests"][1]["error"]["error"]["hint"]
        .as_str()
        .unwrap();
    assert!(
        step_hint.starts_with(&format!(
            "chrome-cdp snapshot --session test-debug --context {context}"
        )),
        "{step_hint}"
    );
    let md = std::fs::read_to_string(
        project
            .root
            .join("e2e/.results/scenarios/fail/fails.cdp/fails-on-a-missing-element/report.md"),
    )
    .unwrap();
    assert!(
        md.ends_with(&format!(
            "still open (--debug): `chrome-cdp snapshot --session test-debug --context {context}`\n"
        )),
        "{md}"
    );
    // The page is still there, with its mocks.
    let cli = |args: &[&str]| {
        common::run(
            project
                .sandbox
                .cli()
                .current_dir(&project.root)
                .args(["--session", "test-debug", "--context", &context])
                .args(args),
        )
    };
    let out = cli(&["snapshot"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stdout(&out).contains("\"Shop\""), "{}", stdout(&out));
    assert!(stdout(&cli(&["mock", "list"])).starts_with("shop "));

    // A second --debug run starts over; a passing one leaves nothing open.
    let (out, value) = project.json(&["scenarios/fail/fails.cdp:5", "--debug"]);
    assert_eq!(out.status.code(), Some(0), "{value}");
    assert!(value["data"].get("debug").is_none(), "{value}");
    assert!(project.sandbox.daemons().is_empty());
}

#[test]
fn watch_runs_the_affected_tests_again_until_ctrl_c() {
    use std::io::{BufRead as _, BufReader};
    use std::sync::mpsc;
    use std::time::Duration;

    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let mut child = project
        .sandbox
        .cli()
        .current_dir(&project.root)
        .args(["test", "--watch", "--jobs", "1"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let (tx, rx) = mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let mut seen = String::new();
    let mut until = |text: &str| {
        while !seen.contains(text) {
            let line = rx
                .recv_timeout(Duration::from_secs(60))
                .unwrap_or_else(|_| panic!("no {text:?} in:\n{seen}"));
            seen.push_str(&line);
            seen.push('\n');
        }
        std::mem::take(&mut seen)
    };
    let first = until("watching ");
    assert!(first.contains("\n7 tests: 7 passed ("), "{first}");

    // A mock file: only the tests of its set run again.
    let sale = project.root.join("mocks/sale.toml");
    let text = std::fs::read_to_string(&sale).unwrap();
    std::fs::write(&sale, format!("{text}\n# changed\n")).unwrap();
    let run = until("watching ");
    assert!(
        run.contains("mocks/sale.toml changed\nrunning 1 test from 1 file"),
        "{run}"
    );
    assert!(run.contains("a sale price from a layered set"), "{run}");

    // A step file: the tests of the scenarios that include it.
    let step = project.root.join("steps/add.cdp");
    let text = std::fs::read_to_string(&step).unwrap();
    std::fs::write(&step, format!("{text}# changed\n")).unwrap();
    let run = until("watching ");
    assert!(
        run.contains("steps/add.cdp changed\nrunning 3 tests from 1 file"),
        "{run}"
    );

    // A scenario that no longer checks: its error, then watching on.
    let todos = project.root.join("scenarios/pass/todos.cdp");
    let good = std::fs::read_to_string(&todos).unwrap();
    std::fs::write(&todos, format!("{good}\n@test \"bad\"\n  clickk e1\n")).unwrap();
    let run = until("watching ");
    assert!(!run.contains("running "), "{run}");
    std::fs::write(&todos, &good).unwrap();
    let run = until("watching ");
    assert!(run.contains("running 3 tests from 1 file"), "{run}");

    // A new scenario runs; then its ${data:…} and `state load` files are
    // watched with it.
    std::fs::create_dir_all(project.root.join("data")).unwrap();
    std::fs::write(project.root.join("data/todo.json"), r#"{"title": "Milk"}"#).unwrap();
    std::fs::write(
        project.root.join("data/state.json"),
        r#"{"cookies": [], "origins": []}"#,
    )
    .unwrap();
    std::fs::write(
        project.root.join("scenarios/pass/data.cdp"),
        "@test \"reads data files\"\n  state load data/state.json\n  goto /e2e/shop.html\n  fill 'label=New todo' ${data:../../data/todo.json#/title}\n  expect 'label=New todo' value Milk\n",
    )
    .unwrap();
    let run = until("watching ");
    assert!(run.contains("running 1 test from 1 file"), "{run}");
    assert!(run.contains("reads data files"), "{run}");
    for file in ["data/todo.json", "data/state.json"] {
        let path = project.root.join(file);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("{text}\n")).unwrap();
        let run = until("watching ");
        assert!(
            run.contains(&format!("{file} changed\nrunning 1 test from 1 file")),
            "{run}"
        );
        assert!(run.contains("reads data files"), "{run}");
    }

    // Ctrl-C ends it; the run's session is closed.
    let status = std::process::Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(0));
    assert!(project.sandbox.daemons().is_empty());
}

#[test]
fn mock_check_errors_stop_the_run_before_the_browser_starts() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    std::fs::create_dir_all(project.root.join("api")).unwrap();
    std::fs::write(
        project.root.join("api/openapi.json"),
        r#"{"openapi": "3.0.3", "servers": [{"url": "/e2e/api"}], "paths": {
            "/price": {"get": {"responses": {"200": {"description": "the price",
                "content": {"application/json": {"schema": {"type": "object",
                    "required": ["price_cents"], "properties": {"price_cents": {"type": "integer"}}}}}}}}},
            "/todos": {"get": {"responses": {"200": {"description": "list"}}}},
            "/todos/{id}": {"get": {"responses": {"200": {"description": "one"}}}}
        }}"#,
    )
    .unwrap();
    let config = project.root.join("chrome-cdp.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        text.replace("[e2e]\n", "[e2e]\nopenapi = \"api/openapi.json\"\n"),
    )
    .unwrap();
    let out = project.test(&[]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.contains("mocks/shop.toml:6:1: error [openapi-body] mock \"price\" does not fit the spec's GET /price 200: $.price_cents: the spec requires it, the mock has none"),
        "{err}"
    );
    assert!(err.contains("hint: fix the errors in the mock files (or the spec), then run chrome-cdp mock check shop sale again"), "{err}");
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
    assert!(project.sandbox.daemons().is_empty());

    // The mocks fit; the spec has an operation no mock answers. With
    // mock_check = "warnings" that stops the run too.
    std::fs::write(
        project.root.join("api/openapi.json"),
        r#"{"openapi": "3.0.3", "servers": [{"url": "/e2e/api"}], "paths": {
            "/price": {"get": {"responses": {"200": {"description": "the price",
                "content": {"application/json": {"schema": {"type": "object",
                    "properties": {"price": {"type": "integer"}}}}}}}}},
            "/orders": {"post": {"operationId": "placeOrder", "responses": {"201": {"description": "made"}}}},
            "/todos": {"get": {"responses": {"200": {"description": "list"}}}},
            "/todos/{id}": {"get": {"responses": {"200": {"description": "one"}}}}
        }}"#,
    )
    .unwrap();
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        text.replace("[e2e]\n", "[e2e]\nmock_check = \"warnings\"\n"),
    )
    .unwrap();
    let out = project.test(&[]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.contains("api/openapi.json:5:1: warning [openapi-uncovered] POST /orders (placeOrder): no mock or collection answers it"),
        "{err}"
    );
    assert!(err.contains("hint: fix the warnings (mock_check = \"warnings\" in [e2e] stops on them), then run chrome-cdp mock check shop sale again"), "{err}");
    assert!(project.sandbox.daemons().is_empty());

    // By default the run goes on, and says so in one line.
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, text.replace("mock_check = \"warnings\"\n", "")).unwrap();
    let out = project.test(&[]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(
        err.contains("warning: 1 spec operation has no mock: POST /orders (placeOrder)\n"),
        "{err}"
    );
    assert!(!err.contains("[openapi-uncovered]"), "{err}");

    // mock_check = "off": no check, so a mock that does not fit the spec
    // runs as it is.
    std::fs::write(
        project.root.join("api/openapi.json"),
        r#"{"openapi": "3.0.3", "servers": [{"url": "/e2e/api"}], "paths": {
            "/price": {"get": {"responses": {"200": {"description": "the price",
                "content": {"application/json": {"schema": {"type": "object",
                    "required": ["price_cents"], "properties": {"price_cents": {"type": "integer"}}}}}}}}},
            "/orders": {"post": {"responses": {"201": {"description": "made"}}}}
        }}"#,
    )
    .unwrap();
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        text.replace("[e2e]\n", "[e2e]\nmock_check = \"off\"\n"),
    )
    .unwrap();
    let out = project.test(&[]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(!err.contains("openapi"), "{err}");
    assert!(!err.contains("spec operation"), "{err}");
}

/// Every file below `dir` whose text contains `needle`.
fn files_containing(dir: &Path, needle: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_containing(&path, needle));
        } else if std::fs::read(&path)
            .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(needle))
        {
            found.push(path);
        }
    }
    found
}

#[test]
fn the_same_scenarios_pass_against_the_mocks_and_a_real_backend() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let shop = "scenarios/backends/shop.cdp";

    // The mocks: the real-only test is skipped, and says why.
    let (out, value) = project.json(&[shop, "--jobs", "2"]);
    assert_eq!(out.status.code(), Some(0), "{value}");
    let data = &value["data"];
    assert_eq!(
        (data["passed"].as_u64(), data["skipped"].as_u64()),
        (Some(4), Some(1))
    );
    assert_eq!(data["tests"][4]["status"], "skipped");
    assert_eq!(
        data["tests"][4]["reason"],
        "@backend real; this run's backend is mock"
    );
    assert!(data.get("backend").is_none(), "{data}");

    // The real API: the mock-only test is skipped; setup and teardown ran.
    let out = project.test(&[shop, "--backend", "local", "--jobs", "2"]);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    assert!(
        text.starts_with("setup: echo up > setup-ran.txt ("),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "backend local (real, {}): time runs, so determinism is best-effort\n",
            project.real.a()
        )),
        "{text}"
    );
    assert!(
        text.contains("- scenarios/backends/shop.cdp:25 starts from the seed (skipped: @backend mock; this run's backend is local)"),
        "{text}"
    );
    assert!(text.contains("5 tests: 4 passed, 1 skipped ("), "{text}");
    assert!(
        text.contains("teardown: echo down > teardown-ran.txt ("),
        "{text}"
    );
    let config = project.root.join("chrome-cdp.toml");
    for command in ["echo up > setup-ran.txt", "echo down > teardown-ran.txt"] {
        assert!(
            stderr(&out).contains(&format!("running: {command} (from {})\n", config.display())),
            "{}",
            stderr(&out)
        );
    }
    assert!(project.root.join("setup-ran.txt").is_file());
    assert!(project.root.join("teardown-ran.txt").is_file());
    // The todos the tests added through the page went to the real API;
    // the one arranged with http was deleted again.
    let todos = reqwest_get(&project.real.url("/e2e/api/todos"));
    assert!(todos.contains("\"title\":\"Todo "), "{todos}");
    assert!(!todos.contains("From the API"), "{todos}");
    // requests.txt has the page's real requests.
    let requests = std::fs::read_to_string(
        project
            .root
            .join("e2e/.results/scenarios/backends/shop.cdp/adds-a-todo-of-its-own/requests.txt"),
    )
    .unwrap();
    assert!(requests.contains("POST "), "{requests}");

    // Hybrid: the real API, with the price mocked.
    let (out, value) = project.json(&[shop, "--backend", "hybrid"]);
    assert_eq!(out.status.code(), Some(0), "{value}");
    assert_eq!(value["data"]["backend"], "hybrid");
    assert_eq!(value["data"]["passed"], 4);
}

/// A GET's body, with no client library: curl is on every CI runner.
fn reqwest_get(url: &str) -> String {
    let out = std::process::Command::new("curl")
        .args(["-s", url])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn backend_all_runs_every_backend_and_groups_the_results() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let (out, value) = project.json(&[
        "scenarios/backends/shop.cdp",
        "--backend",
        "all",
        "--allow-writes",
        "--jobs",
        "3",
    ]);
    assert_eq!(out.status.code(), Some(0), "{value}");
    let data = &value["data"];
    assert_eq!(data["backend"], "all");
    let backends: Vec<(&str, &str, u64, u64)> = data["backends"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            (
                b["name"].as_str().unwrap(),
                b["mode"].as_str().unwrap(),
                b["passed"].as_u64().unwrap(),
                b["skipped"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        backends,
        [
            ("mock", "mock", 4, 1),
            ("hybrid", "real", 4, 1),
            ("local", "real", 4, 1),
            ("prod", "real", 4, 1)
        ]
    );
    assert_eq!(data["tests"].as_array().unwrap().len(), 20);
    assert_eq!(data["tests"][5]["backend"], "hybrid");
    // Each real backend's artifacts are its own.
    assert!(
        project
            .root
            .join(
                "e2e/.results/local/scenarios/backends/shop.cdp/adds-a-todo-of-its-own/steps.jsonl"
            )
            .is_file()
    );
    assert!(
        project
            .root
            .join("e2e/.results/scenarios/backends/shop.cdp/adds-a-todo-of-its-own/steps.jsonl")
            .is_file()
    );

    // An unknown backend is a usage error with a suggestion.
    let out = project.test(&["--backend", "locl"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("hint: did you mean --backend local?"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn mock_only_commands_fail_against_a_real_backend() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let (out, value) = project.json(&["scenarios/backends-bad", "--backend", "local"]);
    assert_eq!(out.status.code(), Some(1), "{value}");
    let error = &value["error"]["details"]["tests"][0]["error"];
    assert_eq!(error["at"], "scenarios/backends-bad/unmarked.cdp:4:3");
    assert_eq!(
        error["error"]["message"],
        "expect mock-state needs the mocks, and backend local is real"
    );
    assert_eq!(
        error["error"]["hint"],
        "mark the test @backend mock, so real backends skip it"
    );
    let error = &value["error"]["details"]["tests"][1]["error"];
    assert_eq!(error["at"], "scenarios/backends-bad/unmarked.cdp:8:3");
    assert_eq!(
        error["error"]["message"],
        "mock ws send needs the mocks, and backend local is real"
    );
}

/// A free port on 127.0.0.1.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn a_mock_server_answers_server_side_fetches_per_test() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let config = project.root.join("chrome-cdp.toml");
    let base = std::fs::read_to_string(&config).unwrap();
    let port = free_port();
    std::fs::write(
        &config,
        format!(
            "{base}\n[e2e.web_server]\ncommand = \"exit 3\"\nurl = \"{}\"\nmock_server = {{ port = {port} }}\n",
            Fixtures::shared().a()
        ),
    )
    .unwrap();
    let dir = project.root.join("scenarios/ssr");
    std::fs::create_dir_all(&dir).unwrap();
    // The SSR page's server fetches the mock server with the test's
    // header: each test has state of its own, so each sees `n` = 1.
    let page = format!("/e2e/ssr?api=http://127.0.0.1:{port}");
    let mut scenario = String::from("@mocks ssr\n");
    for name in ["first", "second"] {
        let _ = write!(
            scenario,
            "\n@test \"{name}\"\n  goto {page}\n  expect text='The server got: 200 {{\"greeting\":\"hello from the mock server\",\"n\":1}}' visible\n  expect '#test' text 'Test header: none' --not\n"
        );
    }
    std::fs::write(dir.join("ssr.cdp"), scenario).unwrap();
    // Without @mocks ssr, the greeting is in shop's scope but not mocked:
    // the server's unmatched request fails the test.
    std::fs::write(
        dir.join("other.cdp"),
        format!(
            "@test \"unmatched\"\n  goto {page}\n  expect text='The server got: 501' visible\n"
        ),
    )
    .unwrap();

    let (out, value) = project.json(&["scenarios/ssr/ssr.cdp"]);
    assert_eq!(out.status.code(), Some(0), "{value}");
    assert_eq!(
        statuses(&value),
        [
            ("passed".into(), "first".into()),
            ("passed".into(), "second".into()),
        ]
    );
    let first = value["data"]["tests"][0]["artifacts"].as_str().unwrap();
    let requests = std::fs::read_to_string(Path::new(first).join("requests.txt")).unwrap();
    assert!(
        requests.contains(&format!("== mock server http://127.0.0.1:{port}")),
        "{requests}"
    );
    assert!(
        requests.contains(r#"server → ssr.toml:6 "greeting" fulfill 200"#),
        "{requests}"
    );
    // A second run starts the server on the port again.
    let (out, value) = project.json(&["scenarios/ssr/other.cdp"]);
    assert_eq!(out.status.code(), Some(1), "{value}");
    let failure = &value["error"]["details"]["tests"][0]["error"]["error"];
    assert_eq!(
        failure["message"],
        format!("unmatched request: GET http://127.0.0.1:{port}/e2e/api/greeting")
    );
}

#[test]
fn a_hybrid_backends_mock_server_forwards_to_its_upstream() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let config = project.root.join("chrome-cdp.toml");
    let base = std::fs::read_to_string(&config).unwrap();
    let port = free_port();
    let real = project.real.a();
    std::fs::write(
        &config,
        format!(
            "{base}\n[e2e.backends.ssr]\nmode = \"real\"\nbase_url = \"{real}\"\nmocks = [\"ssr-hybrid\"]\n\
             web_server = {{ command = \"exit 3\", url = \"{real}\", mock_server = {{ port = {port}, upstream = \"{real}\" }} }}\n"
        ),
    )
    .unwrap();
    std::fs::write(
        project.root.join("mocks/ssr-hybrid.toml"),
        r#"[settings]
scope = ["/e2e/api/**"]
unmatched = "passthrough"

[[mock]]
name = "greeting"
match = { method = "GET", path = "/e2e/api/greeting" }
respond = { json = { greeting = "mocked" } }

[[mock]]
name = "price"
match = { method = "GET", path = "/e2e/api/price" }
respond = { passthrough = true, patch = { via = "mock server" } }
"#,
    )
    .unwrap();
    let dir = project.root.join("scenarios/hybrid-ssr");
    std::fs::create_dir_all(&dir).unwrap();
    // The page's server fetches the mock server: a mock answers the
    // greeting, the real API (patched) the price and (unmatched) the
    // todo.
    let page = format!("/e2e/ssr?api=http://127.0.0.1:{port}");
    std::fs::write(
        dir.join("ssr.cdp"),
        format!(
            "@test \"hybrid\"\n  goto {page}\n  expect text='The server got: 200 {{\"greeting\":\"mocked\"}}' visible\n\
             \x20 goto {page}&path=/e2e/api/price\n  expect text='The server got: 200 {{\"price\":49,\"via\":\"mock server\"}}' visible\n\
             \x20 goto {page}&path=/e2e/api/todos/1\n  expect '#answer' text 'Buy milk'\n"
        ),
    )
    .unwrap();
    let (out, value) = project.json(&["scenarios/hybrid-ssr/ssr.cdp", "--backend", "ssr"]);
    assert_eq!(out.status.code(), Some(0), "{value}");
    let artifacts = value["data"]["tests"][0]["artifacts"].as_str().unwrap();
    let requests = std::fs::read_to_string(Path::new(artifacts).join("requests.txt")).unwrap();
    assert!(requests.contains(r#""greeting" fulfill 200"#), "{requests}");
    assert!(requests.contains("→ upstream 200, patched"), "{requests}");
    assert!(
        requests.contains("unmatched, passthrough → upstream 200"),
        "{requests}"
    );
}

#[test]
fn pushes_in_a_test_go_to_the_browser_and_the_mock_server() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let config = project.root.join("chrome-cdp.toml");
    let base = std::fs::read_to_string(&config).unwrap();
    let port = free_port();
    std::fs::write(
        &config,
        format!(
            "{base}\n[e2e.web_server]\ncommand = \"exit 3\"\nurl = \"{}\"\nmock_server = {{ port = {port} }}\n",
            Fixtures::shared().a()
        ),
    )
    .unwrap();
    let live =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/site/mock/mocks/live.toml");
    std::fs::copy(live, project.root.join("mocks/live.toml")).unwrap();
    let dir = project.root.join("scenarios/push");
    std::fs::create_dir_all(&dir).unwrap();
    // The page's socket and stream are the browser's; the mock server
    // has none open, and says so.
    std::fs::write(
        dir.join("push.cdp"),
        "@mocks live\n\n@test \"pushes\"\n  goto /mock/live.html\n  wait --fn 'window.seen && seen.length >= 4'\n  mock ws send '/mock/ws/**' '{\"type\":\"price\",\"value\":42}'\n  mock sse send /mock/api/news pushed\n  expect '#price' text 42\n  wait --fn 'seen.includes(\"sse pushed\")'\n",
    )
    .unwrap();
    let (out, value) = project.json(&["scenarios/push/push.cdp"]);
    assert_eq!(out.status.code(), Some(0), "{value}");
    let artifacts = value["data"]["tests"][0]["artifacts"].as_str().unwrap();
    let steps: Vec<Value> = std::fs::read_to_string(Path::new(artifacts).join("steps.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let output = |prefix: &str| {
        steps
            .iter()
            .find(|s| s["command"].as_str().unwrap().starts_with(prefix))
            .map(|s| s["output"].as_str().unwrap_or_default().to_owned())
            .unwrap()
    };
    let ws = output("mock ws send");
    assert!(ws.starts_with("sent 27B to 1 socket:"), "{ws}");
    assert!(
        ws.ends_with(&format!(
            "mock server http://127.0.0.1:{port}: sent to 0 sockets"
        )),
        "{ws}"
    );
    let sse = output("mock sse send");
    assert!(
        sse.starts_with("sent to 1 stream (/mock/api/news)"),
        "{sse}"
    );
    assert!(
        sse.ends_with(&format!(
            "mock server http://127.0.0.1:{port}: sent to 0 streams"
        )),
        "{sse}"
    );
}

#[test]
fn protected_backends_run_only_safe_tests_and_refuse_writes() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    // Tests not marked @safe: the run does not start.
    let out = project.test(&["scenarios/backends/shop.cdp", "--backend", "prod"]);
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(
        err.starts_with(
            "error: backend prod is protected, and 4 of the run tests are not marked @safe"
        ),
        "{err}"
    );
    assert!(err.contains("--allow-writes runs the rest too"), "{err}");
    assert!(project.sandbox.daemons().is_empty());

    // @safe tests run; an http write in one fails.
    let (out, value) = project.json(&["scenarios/backends/safe.cdp", "--backend", "prod"]);
    assert_eq!(out.status.code(), Some(1), "{value}");
    assert_eq!(
        statuses(&value),
        [
            ("passed".into(), "reads the todos".into()),
            ("failed".into(), "writes in its steps".into()),
        ]
    );
    assert_eq!(
        value["error"]["details"]["tests"][1]["error"]["error"]["message"],
        "http POST on protected backend prod: a @safe test only reads"
    );
    let todos = reqwest_get(&project.real.url("/e2e/api/todos"));
    assert!(!todos.contains("\"no\""), "{todos}");

    // --allow-writes lets it write.
    let (out, value) = project.json(&[
        "scenarios/backends/safe.cdp",
        "--backend",
        "prod",
        "--allow-writes",
    ]);
    assert_eq!(out.status.code(), Some(0), "{value}");
}

#[test]
fn secrets_never_reach_the_output_or_the_artifacts() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let secret = "hunter-2-correct-horse";
    for json in [false, true] {
        let mut cmd = project.sandbox.cli();
        cmd.current_dir(&project.root)
            .env("E2E_TODO_SECRET", secret)
            .env_remove("CI")
            .args(["test", "scenarios/backends/secret.cdp"]);
        if json {
            cmd.args(["--json", "--trace"]);
        }
        let out = common::run(&mut cmd);
        assert_eq!(out.status.code(), Some(1));
        let (text, err) = (stdout(&out), stderr(&out));
        assert!(
            !text.contains(secret) && !err.contains(secret),
            "{text}{err}"
        );
        assert!(
            text.contains("<secret:E2E_TODO_SECRET>") || err.contains("<secret:E2E_TODO_SECRET>"),
            "{text}{err}"
        );
    }
    let results = project.root.join("e2e/.results");
    let dir = results.join("scenarios/backends/secret.cdp/types-a-secret");
    // The text field holding the secret is painted over in the
    // screenshots, and the report says so.
    let report = std::fs::read_to_string(dir.join("report.md")).unwrap();
    assert!(
        report.contains(
            "\nPainted over because they hold a `${secret:…}` value: 1 field in failure.png, and in "
        ),
        "{report}"
    );
    assert!(dir.join("failure.png").is_file());
    let steps = std::fs::read_to_string(dir.join("steps.jsonl")).unwrap();
    assert!(steps.contains("\"masked\":1"), "{steps}");
    assert_eq!(files_containing(&results, secret), Vec::<PathBuf>::new());
    // The test that passed left fingerprints, without the value.
    let fingerprints = project.root.join("e2e/fingerprints");
    let written =
        std::fs::read_to_string(fingerprints.join("scenarios/backends/secret.json")).unwrap();
    assert!(written.contains("\"shows a secret\""), "{written}");
    assert_eq!(
        files_containing(&fingerprints, secret),
        Vec::<PathBuf>::new()
    );

    // Without the variable, the run does not start.
    let out = project.test(&["scenarios/backends/secret.cdp"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("environment variable E2E_TODO_SECRET is not set"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn verify_mocks_reports_drift_and_update_fixes_data_files() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let scenario = "scenarios/verify/shop.cdp";
    let args = [scenario, "--backend", "local", "--verify-mocks"];

    // Honest mocks: nothing to report but the request no mock has.
    let out = project.test(&args);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(
        text.contains("mocks against backend local: 3 real answers compared with 2 mocks\n"),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "  no mock: GET {}/e2e/api/me (real answer 401; in the scope of mock set shop)\n",
            project.real.a()
        )),
        "{text}"
    );
    assert!(
        stderr(&out).contains(
            "\nerror: the mocks do not match backend local: 1 real request has no mock\n"
        ),
        "{}",
        stderr(&out)
    );

    // Drift: the price's data file has a field the API lacks (and the
    // right one with an old value); the todos' ids became strings and
    // their title is called name.
    let shop = project.root.join("mocks/shop.toml");
    let text = std::fs::read_to_string(&shop).unwrap();
    std::fs::write(
        &shop,
        text.replace(
            "respond = { json = { price = 49 } }",
            "respond = { json_file = \"data/price.json\" }",
        ) + "\n[[mock]]\nname = \"stock\"\nmatch = { method = \"GET\", path = \"/e2e/api/stock\" }\nrespond = { json = { count = 3 } }\n",
    )
    .unwrap();
    // …and a mocked endpoint the real API does not have.
    let page = project.root.join(scenario);
    let text = std::fs::read_to_string(&page).unwrap();
    std::fs::write(
        &page,
        format!("{text}  eval \"fetch('/e2e/api/stock').then((r) => r.status)\"\n"),
    )
    .unwrap();
    let price = project.root.join("mocks/data/price.json");
    std::fs::write(&price, "{\n  \"cost\": 49,\n  \"price\": 39\n}\n").unwrap();
    let todos = project.root.join("mocks/data/todos.json");
    let text = std::fs::read_to_string(&todos).unwrap();
    std::fs::write(
        &todos,
        text.replace("\"id\": 1", "\"id\": \"1\"")
            .replace("\"id\": 2", "\"id\": \"2\"")
            .replace("\"title\"", "\"name\""),
    )
    .unwrap();

    let (out, value) = project.json(&args);
    assert_eq!(out.status.code(), Some(1), "{value}");
    let verify = &value["error"]["details"]["verify"];
    assert_eq!(verify["backend"], "local");
    let drifted: Vec<(&str, Vec<&str>)> = verify["drifted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["mock"]["at"].as_str().unwrap(),
                d["problems"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| p.as_str().unwrap())
                    .collect(),
            )
        })
        .collect();
    assert!(
        drifted.contains(&(
            "shop.toml:6",
            vec![r#"$: the mock has field "cost" (number); the real answer has none"#]
        )),
        "{drifted:?}"
    );
    assert!(
        drifted.contains(&(
            "shop.toml:11",
            vec![
                r#"$[]: the real answer has field "title" (string); the mock has "name" (string)"#,
                "$[].id: the real answer has a number; the mock has a string",
            ]
        )),
        "{drifted:?}"
    );
    assert!(
        drifted.contains(&(
            "shop.toml:16",
            vec!["status: the real answer is 404; the mock's is 200"]
        )),
        "{drifted:?}"
    );
    assert_eq!(verify["unmocked"][0]["status"], 401);
    assert!(
        value["error"]["hint"]
            .as_str()
            .unwrap()
            .ends_with("chrome-cdp test --backend local --verify-mocks --update   (rewrites the drifted data files from the real answers; review the diff)"),
        "{value}"
    );

    // --update rewrites the price's data file, keeping the value that
    // still fits; the collection's seed is left for a person.
    let (out, value) = project.json(&[&args[..], &["--update"]].concat());
    assert_eq!(out.status.code(), Some(1), "{value}");
    assert_eq!(
        std::fs::read_to_string(&price).unwrap(),
        "{\n  \"price\": 39\n}\n"
    );
    let verify = &value["error"]["details"]["verify"];
    assert_eq!(
        verify["updated"][0].as_str().unwrap(),
        price.display().to_string()
    );
    assert!(
        std::fs::read_to_string(&todos)
            .unwrap()
            .contains("\"id\": \"1\"")
    );
}

#[test]
fn record_mocks_writes_a_set_per_passed_test() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let (out, value) = project.json(&[
        "scenarios/backends/shop.cdp:7",
        "--backend",
        "local",
        "--record-mocks",
        "--match-bodies",
    ]);
    assert_eq!(out.status.code(), Some(0), "{value}");
    let recorded = &value["data"]["recorded"][0];
    let set = "recorded/scenarios/backends/shop/shows-the-first-todo-and-the-price";
    assert_eq!(recorded["set"], set);
    assert_eq!(recorded["requests"], 2, "{recorded}");
    let file = project.root.join(format!("mocks/{set}.toml"));
    let toml = std::fs::read_to_string(&file).unwrap();
    assert!(toml.contains("path = \"/e2e/api/price\""), "{toml}");
    assert!(
        toml.contains("from backend local, test \"shows the first todo and the price\""),
        "{toml}"
    );

    // The recording answers the test in mock mode, without the project's
    // own mocks.
    let config = project.root.join("chrome-cdp.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        text.replace("default_mocks = [\"shop\"]", "default_mocks = []"),
    )
    .unwrap();
    let scenario = project.root.join("scenarios/backends/replay.cdp");
    std::fs::write(
        &scenario,
        format!(
            "@mocks {set}\n\n@test \"replays\"\n  goto /e2e/shop.html\n  expect 'text=Buy milk' visible\n  expect 'text=Price: 49' visible\n"
        ),
    )
    .unwrap();
    let (out, value) = project.json(&["scenarios/backends/replay.cdp"]);
    assert_eq!(out.status.code(), Some(0), "{value}");

    // Recording needs a real backend.
    let out = project.test(&["scenarios/backends/shop.cdp", "--record-mocks"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out)
            .starts_with("error: --record-mocks records a real backend, and mock is the mock one"),
        "{}",
        stderr(&out)
    );
    // --match-bodies only goes with --record-mocks.
    let out = project.test(&["scenarios/backends/shop.cdp", "--match-bodies"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("--record-mocks"), "{}", stderr(&out));
}

/// Compares `actual` with golden file `heal/<name>`, or writes it with
/// UPDATE_GOLDEN=1.
fn golden(name: &str, actual: &str) {
    golden_in("heal", name, actual);
}

/// Compares `actual` with golden file `<dir>/<name>`, or writes it with
/// UPDATE_GOLDEN=1.
fn golden_in(dir: &str, name: &str, actual: &str) {
    let name = format!("{dir}/{name}");
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(&name);
    if std::env::var_os("UPDATE_GOLDEN").is_some_and(|v| !v.is_empty() && v != "0") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        expected, actual,
        "golden/{name} differs (UPDATE_GOLDEN=1 accepts it)"
    );
}

/// `text` without what changes from run to run: the project's path,
/// durations, retry counts, tab ids and the fixture site's origin.
fn steady(text: &str, root: &Path) -> String {
    let text = text
        .replace(&root.display().to_string(), "<root>")
        .replace(Fixtures::shared().a(), "<site>");
    let durations = regex::Regex::new(r"\b\d+(\.\d+)?(ms|s)\b").unwrap();
    let tabs = regex::Regex::new(r"\bt\d+\b").unwrap();
    let retries = regex::Regex::new(r"\b\d+ retries\b").unwrap();
    let text = durations.replace_all(&text, "<t>");
    let text = retries.replace_all(&text, "<n> retries");
    tabs.replace_all(&text, "<tab>").into_owned()
}

/// Plan 18.2: a passed run leaves fingerprints; on the redesigned page
/// the failure names the element each broken locator's fingerprint
/// matches, and --heal runs the steps with them and writes the patch.
#[test]
fn a_redesigned_page_is_healed() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let run = |args: &[&str]| {
        common::run(
            project
                .sandbox
                .cli()
                .current_dir(&project.root)
                .env_remove("CI")
                .args(["test", "scenarios/heal", "--timeout", "2s"])
                .args(args),
        )
    };
    assert_eq!(run(&[]).status.code(), Some(0));
    let fingerprints = project
        .root
        .join("e2e/fingerprints/scenarios/heal/tickets.json");
    let taken = std::fs::read_to_string(&fingerprints).unwrap();
    golden("fingerprints.json", &taken);

    let vars = project.root.join("vars/mock.toml");
    let text = std::fs::read_to_string(&vars).unwrap();
    std::fs::write(&vars, text.replace("heal_version = 1", "heal_version = 2")).unwrap();
    let out = run(&[]);
    assert_eq!(out.status.code(), Some(1));
    golden("without-heal.txt", &steady(&stdout(&out), &project.root));
    let report = project
        .root
        .join("e2e/.results/scenarios/heal/tickets.cdp/buys-a-ticket/report.md");
    let report = std::fs::read_to_string(report).unwrap();
    let from = report.find("# Failed").unwrap();
    let to = report.find("\n## Page").unwrap();
    golden("report.md", &steady(&report[from..to], &project.root));

    let out = run(&["--heal"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    golden("with-heal.txt", &steady(&stdout(&out), &project.root));
    let patch = std::fs::read_to_string(project.root.join("e2e/.results/heal.patch")).unwrap();
    golden("heal.patch", &patch);
    let steps = std::fs::read_to_string(
        project
            .root
            .join("e2e/.results/scenarios/heal/tickets.cdp/buys-a-ticket/steps.jsonl"),
    )
    .unwrap();
    assert_eq!(steps.matches("\"healed\":{").count(), 2, "{steps}");
    // Healed, the test passed: its fingerprints are the new elements', under
    // the old keys until the file changes.
    assert_ne!(std::fs::read_to_string(&fingerprints).unwrap(), taken);
}

/// Heal with another `testid_attribute`: when only the test id tells the
/// redesigned element apart, the healed locator is `testid=…` (not
/// `css=[data-test=…]`), and after `--write` it replays without `--heal`.
#[test]
fn a_heal_by_a_configured_test_id_writes_testid() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let config = project.root.join("chrome-cdp.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, format!("testid_attribute = \"data-test\"\n{text}")).unwrap();
    let run = |args: &[&str]| {
        common::run(
            project
                .sandbox
                .cli()
                .current_dir(&project.root)
                .env_remove("CI")
                .args(["test", "scenarios/heal-testid", "--timeout", "2s"])
                .args(args),
        )
    };
    let out = run(&[]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));

    let vars = project.root.join("vars/mock.toml");
    let text = std::fs::read_to_string(&vars).unwrap();
    std::fs::write(&vars, text.replace("heal_version = 1", "heal_version = 2")).unwrap();
    let out = run(&["--heal", "--write"]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains(": role=button[name=\"Buy ticket\"] -> testid=buy-adult ("),
        "{text}"
    );
    let scenario =
        std::fs::read_to_string(project.root.join("scenarios/heal-testid/tickets.cdp")).unwrap();
    assert!(
        scenario.contains("\n  click testid=buy-adult\n"),
        "{scenario}"
    );
    let out = run(&[]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
}

/// Plan 18.3: each planted flake of `scenarios/flaky` fails some of its
/// 20 runs (the page picks them with a PRNG seeded with the run's number,
/// so they are the same every time) and is diagnosed as its own cause,
/// first in rank, with evidence naming the request or element.
#[test]
fn planted_flakes_are_diagnosed() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    // The moving target and the clock need animations and real time.
    let config = project.root.join("chrome-cdp.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        format!("{text}\n[e2e.determinism]\nfreeze_time = false\ndisable_animations = false\n"),
    )
    .unwrap();
    let (out, value) = project.json(&["scenarios/flaky", "--repeat", "20", "--jobs", "4"]);
    assert_eq!(out.status.code(), Some(1), "{value}");
    let tests = value["error"]["details"]["tests"].as_array().unwrap();
    let expected = [
        (
            "different.cdp",
            "different-requests",
            4,
            "`GET /e2e/api/flaky/promo` was sent in all 4 failing runs, and in none of the 16 passing runs",
        ),
        (
            "moving.cdp",
            "moving-target",
            3,
            "in all 3 failing runs the step could not act: #pay did not stop moving",
        ),
        (
            "race.cdp",
            "race",
            3,
            "`GET /e2e/api/flaky/offer` was answered before `GET /e2e/api/flaky/stock`, in all 3 failing runs",
        ),
        (
            "settle.cdp",
            "unsettled",
            3,
            "`GET /e2e/api/flaky/price` was still open when `click '#pay'` (scenarios/flaky/settle.cdp:8:3) acted, in all 3 failing runs",
        ),
        (
            "time.cdp",
            "time-or-randomness",
            4,
            "the failing step saw another value in each failing run: \"Updated 20",
        ),
    ];
    assert_eq!(tests.len(), expected.len(), "{value}");
    for (test, (file, cause, failed, evidence)) in tests.iter().zip(expected) {
        assert_eq!(test["file"], format!("scenarios/flaky/{file}"), "{test}");
        assert_eq!(test["status"], "failed", "{test}");
        assert_eq!(test["failed_runs"], failed, "{test}");
        let flake = &test["flake"];
        assert_eq!(flake["cause"], cause, "{flake}");
        assert_eq!(
            (flake["runs"].as_u64(), flake["failed_runs"].as_u64()),
            (Some(20), Some(failed)),
            "{flake}"
        );
        assert!(flake["confidence"].as_f64().unwrap() >= 0.8, "{flake}");
        assert!(
            flake["evidence"][0].as_str().unwrap().starts_with(evidence),
            "{flake}"
        );
        assert!(flake["hint"].is_string(), "{flake}");
    }

    // The text reporter and report.md of one of them.
    let out = project.test(&[
        "scenarios/flaky/settle.cdp",
        "--repeat",
        "20",
        "--jobs",
        "4",
    ]);
    assert_eq!(out.status.code(), Some(1));
    golden_in("flaky", "settle.txt", &steady(&stdout(&out), &project.root));
    let report = project
        .root
        .join("e2e/.results/scenarios/flaky/settle.cdp/pays-the-price/run-1/report.md");
    let report = std::fs::read_to_string(report).unwrap();
    let from = report.find("\n## Flaky").unwrap();
    golden_in(
        "flaky",
        "report.md",
        &steady(&report[from..], &project.root),
    );
    // Each run keeps its own folder, with its requests' times.
    let requests = std::fs::read_to_string(
        project
            .root
            .join("e2e/.results/scenarios/flaky/settle.cdp/pays-the-price/run-2/requests.txt"),
    )
    .unwrap();
    assert!(
        regex::Regex::new(
            r#"/e2e/api/flaky/price fetch \S+ \(sent \+\S+, answered \+\S+\) ← mock "slow price""#
        )
        .unwrap()
        .is_match(&requests),
        "{requests}"
    );
}
