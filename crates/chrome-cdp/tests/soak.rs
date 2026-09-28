//! Plan 06's criterion on bounded buffers: the daemon's RSS stays under
//! 150 MB after a 1-hour soak on a page that logs 100 messages a second.
//!
//! Ignored: benchmarks run on a quiet machine after all plans are in.
//! Run it with
//! `CHROME_PATH=… cargo test -p chrome-cdp --release --test soak -- --ignored --nocapture`;
//! `CHROME_CDP_SOAK_SECS` shortens the hour.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::process::Command;
use std::time::{Duration, Instant};

use common::{Sandbox, chrome_path, code, json_out, stderr};
use fixtures::Fixtures;

const RSS_LIMIT_KB: u64 = 150 * 1024;
const SAMPLE_EVERY: Duration = Duration::from_secs(10);

/// The resident set size of `pid` in KB (`ps` reports it the same way on
/// Linux and macOS).
fn rss_kb(pid: u32) -> u64 {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

#[test]
#[ignore = "benchmark; run after all plans are implemented"]
fn daemon_memory_stays_bounded_under_constant_logging() {
    let Some(chrome) = chrome_path() else { return };
    let soak = std::env::var("CHROME_CDP_SOAK_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map_or(Duration::from_secs(3600), Duration::from_secs);
    let sandbox = Sandbox::with_chrome(&chrome);
    let page = Fixtures::shared().url("/console/soak.html");
    let out = sandbox.run(&["open", &page]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let status = json_out(&sandbox.run(&["status", "--json"]));
    let pid = u32::try_from(status["data"]["daemon_pid"].as_u64().unwrap()).unwrap();

    let start = Instant::now();
    let mut peak = 0;
    while start.elapsed() < soak {
        std::thread::sleep(SAMPLE_EVERY);
        // Read now and then, as an agent would; the buffers must stay
        // bounded whether or not anyone reads them.
        for args in [["console", "--limit", "5"], ["network", "--limit", "5"]] {
            let out = sandbox.run(&args);
            assert_eq!(code(&out), 0, "{}", stderr(&out));
        }
        let rss = rss_kb(pid);
        peak = peak.max(rss);
        eprintln!("{:>5}s: daemon RSS {} KB", start.elapsed().as_secs(), rss);
    }

    // The page kept logging the whole time and the log holds only the
    // newest entries.
    let out = sandbox.run(&["console", "--since", "0", "--limit", "5000", "--json"]);
    let entries = json_out(&out)["data"]["entries"].as_array().unwrap().len();
    assert!(
        entries <= 1000,
        "the console buffer holds {entries} entries"
    );
    let rss = rss_kb(pid);
    eprintln!("daemon RSS at the end {rss} KB, peak {peak} KB");
    assert!(rss < RSS_LIMIT_KB, "the daemon's RSS is {rss} KB");
}
