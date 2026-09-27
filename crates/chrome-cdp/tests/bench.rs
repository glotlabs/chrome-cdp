//! Plan 02's timing goals: a cold `open` (Chrome installed) under 1.5 s, and
//! a warm no-op command under 30 ms at p50.
//!
//! Ignored: benchmarks run on a quiet machine after all plans are in
//! (plan 10 moves this into `cargo xtask bench`). Run it with
//! `CHROME_PATH=… cargo test -p chrome-cdp --release --test bench -- --ignored`.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

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
