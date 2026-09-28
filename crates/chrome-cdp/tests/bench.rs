//! Plan 02's timing goals: a cold `open` (Chrome installed) under 1.5 s, and
//! a warm no-op command under 30 ms at p50. Plan 08's: a `batch` of 10
//! steps at least 3x faster than 10 separate invocations.
//!
//! Ignored: benchmarks run on a quiet machine after all plans are in
//! (plan 10 moves this into `cargo xtask bench`). Run it with
//! `CHROME_PATH=… cargo test -p chrome-cdp --release --test bench -- --ignored`.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::io::Write as _;
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
