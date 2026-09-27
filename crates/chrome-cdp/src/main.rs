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
mod input;
mod install;
mod log;
mod output;
mod progress;
mod schema;
mod session;
mod version;

use cli::{Cli, Command};

fn main() -> ExitCode {
    let parsed = Cli::command()
        .try_get_matches()
        .and_then(|m| Cli::from_arg_matches(&m).map(|cli| (cli, m)));
    let (mut cli, matches) = match parsed {
        Ok(parsed) => parsed,
        Err(e) => return ExitCode::from(usage_error(&e)),
    };
    if let Some(Command::Daemon(args)) = &cli.command {
        // The daemon logs to daemon.log (its stderr) at info level, and has
        // no output to render.
        log::init(cli.global.verbose.max(1));
        return ExitCode::from(daemon::server::run(&daemon::server::ServeOptions {
            session: cli.global.session.clone(),
            build: args.build.clone(),
            no_browser_grace: args
                .no_browser_grace
                .unwrap_or(daemon::server::NO_BROWSER_GRACE),
        }));
    }
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
        Some(Command::Install(args)) => {
            install::install(&args, cli.global.timeout_or(cli::DEFAULT_TIMEOUT))
        }
        Some(Command::Uninstall(args)) => install::uninstall(&args),
        Some(Command::Open(args)) => session::open(&args, &cli.global),
        Some(Command::Connect(args)) => session::connect(&args, &cli.global),
        Some(Command::Close(args)) => session::close(&args, &cli.global),
        Some(Command::Status(args)) => session::status(&args, &cli.global),
        Some(Command::Sessions(args)) => session::sessions(&args, &cli.global),
        Some(Command::Cdp(args)) => session::cdp(&args, &cli.global),
        Some(Command::Tabs(args)) => session::tabs(&args, &cli.global),
        Some(Command::Tab(args)) => session::tab(&args, &cli.global),
        Some(Command::Goto(args)) => session::goto(&args, &cli.global),
        Some(Command::Back(args)) => session::history(engine::Navigation::Back, &args, &cli.global),
        Some(Command::Forward(args)) => {
            session::history(engine::Navigation::Forward, &args, &cli.global)
        }
        Some(Command::Reload(args)) => session::reload(&args, &cli.global),
        Some(Command::Wait(args)) => session::wait(&args, &cli.global),
        Some(Command::Dialog(args)) => session::dialog(&args, &cli.global),
        Some(Command::Downloads(args)) => session::downloads(&args, &cli.global),
        Some(Command::Snapshot(args)) => session::snapshot(&args, &cli.global),
        // Handled in `main`.
        Some(Command::Daemon(_)) => Ok(Output::new(serde_json::Value::Null, "")),
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
        let mut lines = rendered.lines();
        let first = lines
            .next()
            .unwrap_or_default()
            .trim_start_matches("error: ");
        if first.ends_with(':') {
            // "the following required arguments were not provided:" lists
            // them on the lines below.
            let listed: Vec<&str> = lines
                .take_while(|l| !l.trim().is_empty())
                .map(str::trim)
                .collect();
            format!("{first} {}", listed.join(", "))
        } else {
            first.to_owned()
        }
    };
    let tips = rendered
        .lines()
        .filter_map(|l| l.trim().strip_prefix("tip: "));
    tips.fold(ErrorInfo::new(ErrorKind::BadArgs, message), |info, tip| {
        info.note("tip", tip)
    })
    .hint(Cmd::new(&command).switch("help"))
}

/// `chrome-cdp tab close [OPTIONS] <N>` → `tab close`.
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
            command_path("chrome-cdp cdp --json <Domain.method> [JSON]"),
            "cdp"
        );
        assert_eq!(command_path("chrome-cdp [OPTIONS] [COMMAND]"), "");
        assert_eq!(
            command_path("chrome-cdp schema [OPTIONS] [COMMAND]..."),
            "schema"
        );
    }
}
