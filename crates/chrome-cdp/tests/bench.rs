//! Plan 11's timing goals: the test runner's own overhead under 100 ms per
//! test, and the E2E example suite under 15 s with `--jobs 4`. (Plan 02's
//! cold start and warm round trip, and plan 08's `batch` speedup, are in
//! the micro suite of `crates/bench`.) And plan 18.4's: what fingerprints
//! and `test --ui-coverage` add to a step.
//!
//! Ignored: benchmarks run on a quiet machine after all plans are in.
//! `cargo xtask bench micro` runs it, or run it alone with
//! `CHROME_PATH=… cargo test -p chrome-cdp --release --test bench -- --ignored`.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{Sandbox, chrome_path, code};

/// Plan 11: what the runner adds to a test (a context, mocks, the checks
/// after it, artifacts), beyond the time its steps take.
const RUNNER_OVERHEAD: Duration = Duration::from_millis(100);
/// Plan 11: the whole example suite with `--jobs 4` (planned as 30 tests;
/// examples/e2e-shop has 17).
const SHOP_SUITE: Duration = Duration::from_secs(15);

/// Plan 18.4: what `test --ui-coverage`'s snapshots add to a step, on
/// average over the shop's steps (a criterion of this benchmark's own:
/// the plan asks for the measurement).
const UI_COVERAGE_PER_STEP: Duration = Duration::from_millis(50);
/// Plan 18.2: what reading a fingerprint adds to a step, on average (the
/// same; above it, cap the candidates `fingerprint` tries).
const FINGERPRINT_PER_STEP: Duration = Duration::from_millis(10);

/// Copies `from` to `to`, less `node_modules` and earlier results.
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == "node_modules" || name == ".results" {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
#[ignore = "benchmark; run after all plans are implemented"]
fn e2e_runner_overhead_and_the_shop_suite() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let root = sandbox.dir.path().join("e2e-shop");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/e2e-shop"),
        &root,
    );
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = root.join("chrome-cdp.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, text.replace("4173", &port.to_string())).unwrap();
    let test = |args: &[&str]| {
        let out = common::run(sandbox.cli().current_dir(&root).arg("test").args(args));
        assert_eq!(code(&out), 0, "{}", common::stderr(&out));
        out
    };

    // One test at a time: each test's time less its steps' (its own and
    // its hooks'; the runner's setup, checks and teardown are overhead),
    // split by phase from the records steps.jsonl has of them.
    let value = common::json_out(&test(&["--jobs", "1", "--json"]));
    let tests = value["data"]["tests"].as_array().unwrap();
    let mut phases = [Duration::ZERO; 4];
    for t in tests {
        let steps = std::fs::read_to_string(
            Path::new(t["artifacts"].as_str().unwrap()).join("steps.jsonl"),
        )
        .unwrap();
        let mut page = Duration::ZERO;
        let mut runner = Duration::ZERO;
        for s in steps
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        {
            let took = Duration::from_millis(s["duration_ms"].as_u64().unwrap());
            let phase = match s["hook"].as_str() {
                Some("setup") => 0,
                Some("check") => 1,
                Some("teardown") => 2,
                _ => {
                    page += took;
                    continue;
                }
            };
            phases[phase] += took;
            runner += took;
        }
        let total = Duration::from_millis(t["duration_ms"].as_u64().unwrap());
        phases[3] += total.saturating_sub(page + runner);
    }
    let n = u32::try_from(tests.len()).unwrap();
    let [setup, check, teardown, other] = phases.map(|d| d / n);
    let per_test = setup + check + teardown + other;

    let start = Instant::now();
    test(&["--jobs", "4", "-q"]);
    let suite = start.elapsed();

    // Everything first, then the budgets: a failed one still shows the rest.
    eprintln!(
        "runner overhead: {per_test:?} per test ({} tests; setup {setup:?}, check {check:?}, \
         teardown {teardown:?}, other {other:?}); suite with --jobs 4: {suite:?}",
        tests.len()
    );
    let mut over = Vec::new();
    if per_test >= RUNNER_OVERHEAD {
        over.push(format!(
            "runner overhead was {per_test:?} per test (budget {RUNNER_OVERHEAD:?})"
        ));
    }
    if suite >= SHOP_SUITE {
        over.push(format!(
            "the suite took {suite:?} with --jobs 4 (budget {SHOP_SUITE:?})"
        ));
    }
    assert!(over.is_empty(), "{}", over.join("; "));
}

/// The time of the steps of a `test --json` run's tests (their own and
/// their hooks', not the runner's setup, checks and teardown), and how
/// many.
fn step_time(value: &serde_json::Value) -> (Duration, u32) {
    let mut total = Duration::ZERO;
    let mut count = 0;
    for t in value["data"]["tests"].as_array().unwrap() {
        let steps = std::fs::read_to_string(
            Path::new(t["artifacts"].as_str().unwrap()).join("steps.jsonl"),
        )
        .unwrap();
        for step in steps
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .filter(|s| !matches!(s["hook"].as_str(), Some("setup" | "check" | "teardown")))
        {
            total += Duration::from_millis(step["duration_ms"].as_u64().unwrap());
            count += 1;
        }
    }
    (total, count)
}

#[test]
#[ignore = "benchmark; run after all plans are implemented"]
fn fingerprints_and_ui_coverage_cost_per_step() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let root = sandbox.dir.path().join("e2e-shop");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/e2e-shop"),
        &root,
    );
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = root.join("chrome-cdp.toml");
    let text = std::fs::read_to_string(&config)
        .unwrap()
        .replace("4173", &port.to_string());
    std::fs::write(&config, &text).unwrap();
    let fingerprints = |on: bool| {
        let text = if on {
            text.clone()
        } else {
            text.replacen("[e2e]\n", "[e2e]\nfingerprints = \"off\"\n", 1)
        };
        std::fs::write(&config, text).unwrap();
    };
    let test = |args: &[&str]| {
        let out = common::run(
            sandbox
                .cli()
                .current_dir(&root)
                .args(["test", "--jobs", "1", "--json"])
                .args(args),
        );
        assert_eq!(code(&out), 0, "{}", common::stderr(&out));
        step_time(&common::json_out(&out))
    };
    // Warm: the first run pays for Chrome's caches.
    test(&[]);
    fingerprints(false);
    let (bare, steps) = test(&[]);
    fingerprints(true);
    let (fingerprinted, _) = test(&[]);
    let (covered, _) = test(&["--ui-coverage"]);
    let fingerprint = fingerprinted.saturating_sub(bare) / steps;
    let coverage = covered.saturating_sub(fingerprinted) / steps;
    eprintln!(
        "{steps} steps: {bare:?} without fingerprints, {fingerprinted:?} with ({fingerprint:?} more per step), \
         {covered:?} with --ui-coverage too ({coverage:?} more per step)"
    );
    assert!(
        fingerprint < FINGERPRINT_PER_STEP,
        "fingerprints added {fingerprint:?} per step"
    );
    assert!(
        coverage < UI_COVERAGE_PER_STEP,
        "--ui-coverage added {coverage:?} per step"
    );
}
