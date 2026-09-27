//! The `chrome-cdp` binary: parses the command line, runs the command and
//! renders its result (plan 08 §2-§4).
//!
//! Every command returns `Result<Success, ErrorInfo>`; [`output::render`]
//! prints it as text, quiet or `--json` output and picks the exit code.

use std::process::ExitCode;

use clap::Parser as _;
use clap::error::ErrorKind as ClapErrorKind;
use protocol::{ErrorInfo, ErrorKind};

mod cli;
mod direct;
mod errors;
mod log;
mod output;
mod version;

use cli::{Cli, Command, DirectCommand};
use output::Success;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => return ExitCode::from(usage_error(&e)),
    };
    log::init(cli.global.verbose);
    let mode = cli.global.mode();
    ExitCode::from(output::render(mode, run(cli)))
}

fn run(cli: Cli) -> Result<Success, ErrorInfo> {
    if cli.version {
        return Ok(version::run());
    }
    match cli.command {
        None => {
            Err(ErrorInfo::new(ErrorKind::BadArgs, "no command given").hint("chrome-cdp --help"))
        }
        Some(Command::Direct(DirectCommand::Cdp(args))) => direct::cdp(&args, &cli.global),
    }
}

/// Reports a clap error (or prints `--help`) and returns the exit code.
/// With `--json`, a usage error is a `BadArgs` envelope on stdout like any
/// other error.
fn usage_error(e: &clap::Error) -> u8 {
    let help = matches!(
        e.kind(),
        ClapErrorKind::DisplayHelp | ClapErrorKind::DisplayVersion
    );
    if help || !json_requested() {
        let _ = e.print();
        return u8::try_from(e.exit_code()).unwrap_or(2);
    }
    let rendered = e.render().to_string();
    let message = rendered
        .lines()
        .next()
        .unwrap_or_default()
        .trim_start_matches("error: ");
    let info = ErrorInfo::new(ErrorKind::BadArgs, message).hint("chrome-cdp --help");
    output::render(output::Mode::Json, Err(info))
}

/// Whether `--json` was asked for, from the raw arguments (clap failed to
/// parse them) or `CHROME_CDP_JSON`.
fn json_requested() -> bool {
    let flag = std::env::args_os()
        .skip(1)
        .take_while(|a| a != "--")
        .any(|a| a == "--json");
    let env = std::env::var("CHROME_CDP_JSON").is_ok_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on" | "y"
        )
    });
    flag || env
}
