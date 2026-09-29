//! The benchmark harness behind `cargo xtask bench` (plan 10.4). Three
//! suites: `micro` (per-command timings next to the Playwright library's),
//! `tokens` (snapshot sizes next to Playwright MCP's) and `agent` (Claude
//! Code on scripted tasks, with chrome-cdp or with Playwright MCP).
//! docs/benchmarks.md says how to run each, and what they measure.

pub mod agent;
pub mod micro;
pub mod report;
pub mod sandbox;
pub mod stats;
pub mod tokens;

/// A progress line on stderr.
#[allow(clippy::print_stderr)]
pub fn note(message: &str) {
    eprintln!("{message}");
}

/// A warning, as an annotation in GitHub Actions.
#[allow(clippy::print_stderr, clippy::print_stdout)]
pub fn warn(message: &str) {
    if std::env::var_os("GITHUB_ACTIONS").is_some() {
        println!("::warning::{message}");
    } else {
        eprintln!("warning: {message}");
    }
}
