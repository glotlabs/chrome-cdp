//! The `chrome-cdp` binary: parses the command line, runs the command and
//! renders its result (plan 08 §2-§4).
//!
//! Every command returns `Result<Output, ErrorInfo>`; [`output::print`]
//! renders it as text, quiet or `--json` output (with the shared renderer in
//! `protocol`) and picks the exit code.

use std::process::ExitCode;

use clap::error::ErrorKind as ClapErrorKind;
use clap::{CommandFactory as _, FromArgMatches as _};
use protocol::{ErrorInfo, ErrorKind, Mode, Output};

mod cli;
mod config;
mod direct;
mod errors;
mod input;
mod log;
mod output;
mod schema;
mod version;

use cli::{Cli, Command, DirectCommand};

fn main() -> ExitCode {
    let parsed = Cli::command()
        .try_get_matches()
        .and_then(|m| Cli::from_arg_matches(&m).map(|cli| (cli, m)));
    let (mut cli, matches) = match parsed {
        Ok(parsed) => parsed,
        Err(e) => return ExitCode::from(usage_error(&e)),
    };
    log::init(cli.global.verbose);
    let mode = cli.global.mode();
    let result = config::apply(&mut cli.global, &matches).and_then(|()| run(cli));
    ExitCode::from(output::print(mode, result))
}

fn run(cli: Cli) -> Result<Output, ErrorInfo> {
    if cli.version {
        return Ok(version::run());
    }
    match cli.command {
        None => {
            Err(ErrorInfo::new(ErrorKind::BadArgs, "no command given").hint("chrome-cdp --help"))
        }
        Some(Command::Schema(args)) => schema::run(&args),
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
    output::print(Mode::Json, Err(info))
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
