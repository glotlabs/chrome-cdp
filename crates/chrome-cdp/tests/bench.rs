//! Plan 02's timing goals: a cold `open` (Chrome installed) under 1.5 s, and
//! a warm no-op command under 30 ms at p50. Plan 08's: a `batch` of 10
//! steps at least 3x faster than 10 separate invocations. Plan 11's: the
//! test runner's own overhead under 100 ms per test, and the E2E example
//! suite under 15 s with `--jobs 4`.
//!
//! Ignored: benchmarks run on a quiet machine after all plans are in
//! (plan 10 moves this into `cargo xtask bench`). Run it with
//! `CHROME_PATH=… cargo test -p chrome-cdp --release --test bench -- --ignored`.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::io::Write as _;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::{Sandbox, chrome_path, code};

const COLD_OPEN: Duration = Duration::from_millis(1500);
const WARM_P50: Duration = Duration::from_millis(30);
const WARM_RUNS: usize = 50;

#[test]
#[ignore = "benchmark; run after all plans are implemented"]
fn cold_open_and_warm_round_trip() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);

    let start = Instant::now();
    let out = sandbox.run(&["open", "about:blank"]);
    let cold = start.elapsed();
    assert_eq!(code(&out), 0);

    // `open` on a running session changes nothing: a round trip to the
    // daemon and back, with no page work.
    let mut warm: Vec<Duration> = (0..WARM_RUNS)
        .map(|_| {
            let start = Instant::now();
            let out = sandbox.run(&["open", "-q"]);
            let took = start.elapsed();
            assert_eq!(code(&out), 0);
            took
        })
        .collect();
    warm.sort();
    let p50 = warm[WARM_RUNS / 2];
    eprintln!("cold open: {cold:?}; warm no-op p50: {p50:?}");
    assert!(cold < COLD_OPEN, "cold open took {cold:?}");
    assert!(p50 < WARM_P50, "warm p50 was {p50:?}");
}

/// How much faster a `batch` of [`BATCH_STEPS`] steps must be than as many
/// separate invocations.
const BATCH_SPEEDUP: f64 = 3.0;
const BATCH_STEPS: usize = 10;

#[test]
#[ignore = "benchmark; run after all plans are implemented"]
fn batch_beats_separate_invocations() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let page = "data:text/html,<h1>Hi</h1><input aria-label=Name>";
    assert_eq!(code(&sandbox.run(&["open", page])), 0);
    // Cheap page work, so the per-call overhead dominates.
    let steps: Vec<[&str; 3]> = (0..BATCH_STEPS)
        .map(|i| {
            if i % 2 == 0 {
                ["text", "css=h1", "-q"]
            } else {
                ["fill", "label=Name", "Ada"]
            }
        })
        .collect();

    let start = Instant::now();
    for step in &steps {
        assert_eq!(code(&sandbox.run(step)), 0);
    }
    let separate = start.elapsed();

    let script = steps
        .iter()
        .map(|s| s.join(" "))
        .collect::<Vec<_>>()
        .join("\n");
    let start = Instant::now();
    let mut child = sandbox
        .cli()
        .arg("batch")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
    let batched = start.elapsed();

    let speedup = separate.as_secs_f64() / batched.as_secs_f64();
    eprintln!("{BATCH_STEPS} invocations: {separate:?}; one batch: {batched:?} ({speedup:.1}x)");
    assert!(
        speedup >= BATCH_SPEEDUP,
        "batch was only {speedup:.1}x faster"
    );
}

/// Plan 11: what the runner adds to a test (a context, mocks, the checks
/// after it, artifacts), beyond the time its steps take.
const RUNNER_OVERHEAD: Duration = Duration::from_millis(100);
/// Plan 11: the whole example suite with `--jobs 4` (planned as 30 tests;
/// examples/e2e-shop has 17).
const SHOP_SUITE: Duration = Duration::from_secs(15);

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
    // its hooks'; the runner's setup and checks are overhead).
    let value = common::json_out(&test(&["--jobs", "1", "--json"]));
    let tests = value["data"]["tests"].as_array().unwrap();
    let mut overhead = Duration::ZERO;
    for t in tests {
        let steps = std::fs::read_to_string(
            Path::new(t["artifacts"].as_str().unwrap()).join("steps.jsonl"),
        )
        .unwrap();
        let page_ms: u64 = steps
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .filter(|s| !matches!(s["hook"].as_str(), Some("setup" | "check")))
            .map(|s| s["duration_ms"].as_u64().unwrap())
            .sum();
        let total_ms = t["duration_ms"].as_u64().unwrap();
        overhead += Duration::from_millis(total_ms.saturating_sub(page_ms));
    }
    let per_test = overhead / u32::try_from(tests.len()).unwrap();

    let start = Instant::now();
    test(&["--jobs", "4", "-q"]);
    let suite = start.elapsed();

    eprintln!(
        "runner overhead: {per_test:?} per test ({} tests); suite with --jobs 4: {suite:?}",
        tests.len()
    );
    assert!(
        per_test < RUNNER_OVERHEAD,
        "runner overhead was {per_test:?} per test"
    );
    assert!(suite < SHOP_SUITE, "the suite took {suite:?}");
}
