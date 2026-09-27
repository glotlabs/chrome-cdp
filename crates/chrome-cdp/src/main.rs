//! The `chrome-cdp` binary.
//!
//! This is a placeholder until milestone 1.4 adds the clap-based CLI.

use std::process::ExitCode;

/// Exit code for usage errors (plan 08 §3).
const EXIT_USAGE: u8 = 2;

#[allow(clippy::print_stdout, clippy::print_stderr)]
fn main() -> ExitCode {
    if let Some("-V" | "--version") = std::env::args().nth(1).as_deref() {
        println!("chrome-cdp {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    eprintln!("error: no commands are implemented yet");
    eprintln!("  hint: chrome-cdp --version");
    ExitCode::from(EXIT_USAGE)
}
