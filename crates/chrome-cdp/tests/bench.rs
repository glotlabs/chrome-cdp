//! Plan 11's timing goals: the test runner's own overhead under 100 ms per
//! test, and the E2E example suite under 15 s with `--jobs 4`. (Plan 02's
//! cold start and warm round trip, and plan 08's `batch` speedup, are in
//! the micro suite of `crates/bench`.)
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
