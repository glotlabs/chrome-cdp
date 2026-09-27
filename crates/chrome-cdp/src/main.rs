//! The `chrome-cdp` binary: parses the command line, runs the command and
//! renders its result (plan 08 §2-§4).
//!
//! Every command returns `Result<Output, ErrorInfo>`; [`output::print`]
//! renders it as text, quiet or `--json` output (with the shared renderer in
//! `protocol`) and picks the exit code.

use std::process::ExitCode;

use clap::error::ErrorKind as ClapErrorKind;
use clap::{CommandFactory as _, FromArgMatches as _};
use protocol::{Cmd, ErrorInfo, ErrorKind, Mode, Output};

mod cli;
mod config;
mod direct;
mod errors;
mod input;
mod install;
mod log;
mod output;
mod progress;
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
        None => Err(ErrorInfo::new(ErrorKind::BadArgs, "no command given")
            .hint(Cmd::new("").switch("help"))),
        Some(Command::Schema(args)) => schema::run(&args),
        Some(Command::Install(args)) => install::install(&args, cli.global.timeout),
        Some(Command::Uninstall(args)) => install::uninstall(&args),
        Some(Command::Direct(DirectCommand::Cdp(args))) => direct::cdp(&args, &cli.global),
    }
}

/// Reports a clap error (or prints `--help`) and returns the exit code.
/// A usage error is a `BadArgs` error like any other: the §3 text form on
/// stderr, or an envelope on stdout with `--json`.
fn usage_error(e: &clap::Error) -> u8 {
    if matches!(
        e.kind(),
        ClapErrorKind::DisplayHelp | ClapErrorKind::DisplayVersion
    ) {
        let _ = e.print();
        return u8::try_from(e.exit_code()).unwrap_or(0);
    }
    let mode = if json_requested() {
        Mode::Json
    } else {
        Mode::Text
    };
    output::print(mode, Err(usage_info(e)))
}

/// Turns clap's error text into an [`ErrorInfo`]: its first line is the
/// message, its `tip:` lines ("a similar argument exists: '--json'") are
/// notes, and the hint is the failing command's `--help`.
fn usage_info(e: &clap::Error) -> ErrorInfo {
    let rendered = e.render().to_string();
    let command = rendered
        .lines()
        .find_map(|l| l.strip_prefix("Usage: "))
        .map(command_path)
        .unwrap_or_default();
    let missing = [
        ClapErrorKind::DisplayHelpOnMissingArgumentOrSubcommand,
        ClapErrorKind::MissingSubcommand,
    ];
    let message = if missing.contains(&e.kind()) {
        format!("{} needs a subcommand", Cmd::new(&command))
    } else {
        let first = rendered.lines().next().unwrap_or_default();
        first.trim_start_matches("error: ").to_owned()
    };
    let tips = rendered
        .lines()
        .filter_map(|l| l.trim().strip_prefix("tip: "));
    tips.fold(ErrorInfo::new(ErrorKind::BadArgs, message), |info, tip| {
        info.note("tip", tip)
    })
    .hint(Cmd::new(&command).switch("help"))
}

/// `chrome-cdp __direct cdp [OPTIONS] <Domain.method>` → `__direct cdp`.
fn command_path(usage: &str) -> String {
    usage
        .split_whitespace()
        .skip(1)
        .take_while(|w| !w.starts_with(['[', '<', '-']))
        .collect::<Vec<_>>()
        .join(" ")
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

#[cfg(test)]
mod tests {
    use super::command_path;

    #[test]
    fn command_path_from_usage() {
        assert_eq!(
            command_path("chrome-cdp __direct cdp --json <Domain.method> [JSON]"),
            "__direct cdp"
        );
        assert_eq!(command_path("chrome-cdp [OPTIONS] [COMMAND]"), "");
        assert_eq!(
            command_path("chrome-cdp schema [OPTIONS] [COMMAND]..."),
            "schema"
        );
    }
}
