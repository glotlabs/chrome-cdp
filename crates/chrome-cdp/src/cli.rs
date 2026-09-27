//! Command-line definitions (clap derive).
//!
//! Global flags follow plan 08 §4: each has a `CHROME_CDP_*` env var and may
//! appear before or after the subcommand. Plan 02 onwards add the real
//! commands to [`Command`].

use std::path::PathBuf;
use std::time::Duration;

use clap::builder::BoolishValueParser;
use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};

use crate::output::Mode;

/// Drive Chrome over the DevTools Protocol from a shell. Built for AI agents.
#[derive(Debug, Parser)]
#[command(
    name = "chrome-cdp",
    disable_version_flag = true,
    after_help = "Exit codes: 0 ok, 1 action failed, 2 usage, 3 timeout, 4 browser/daemon, 5 dialog."
)]
pub(crate) struct Cli {
    #[command(flatten)]
    pub(crate) global: GlobalArgs,

    /// Print the version, git commit and CDP protocol revision.
    #[arg(short = 'V', long)]
    pub(crate) version: bool,

    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

/// Flags every command accepts.
#[derive(Debug, Args)]
pub(crate) struct GlobalArgs {
    /// Print one JSON object on stdout: {"ok":true,"data":…} or {"ok":false,"error":…}.
    #[arg(
        long,
        global = true,
        env = "CHROME_CDP_JSON",
        action = ArgAction::SetTrue,
        value_parser = BoolishValueParser::new(),
        hide_env_values = true
    )]
    pub(crate) json: bool,

    /// Print only the essential result, for `$(…)` capture.
    #[arg(
        short,
        long,
        global = true,
        env = "CHROME_CDP_QUIET",
        action = ArgAction::SetTrue,
        value_parser = BoolishValueParser::new(),
        hide_env_values = true,
        conflicts_with = "json"
    )]
    pub(crate) quiet: bool,

    /// Browser session to use; each session has its own daemon and Chrome.
    #[arg(
        long,
        global = true,
        env = "CHROME_CDP_SESSION",
        default_value = "default",
        value_name = "NAME"
    )]
    pub(crate) session: String,

    /// Give up after this long, e.g. 500ms, 30s, 2m.
    #[arg(
        long,
        global = true,
        env = "CHROME_CDP_TIMEOUT",
        default_value = "30s",
        value_parser = parse_duration,
        value_name = "DURATION"
    )]
    pub(crate) timeout: Duration,

    /// Log to stderr: -v info, -vv debug, -vvv trace. CHROME_CDP_LOG
    /// (e.g. cdp=trace) overrides it.
    #[arg(short, long, global = true, action = ArgAction::Count)]
    pub(crate) verbose: u8,
}

impl GlobalArgs {
    /// How to render results.
    pub(crate) fn mode(&self) -> Mode {
        if self.json {
            Mode::Json
        } else if self.quiet {
            Mode::Quiet
        } else {
            Mode::Text
        }
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Temporary daemon-less commands (plan 01). Plan 02 replaces them.
    #[command(name = "__direct", hide = true, subcommand)]
    Direct(DirectCommand),
}

#[derive(Debug, Subcommand)]
pub(crate) enum DirectCommand {
    /// Launch Chrome, make one raw CDP call, print the result and exit.
    #[command(after_help = "\
Examples:
  chrome-cdp __direct cdp Browser.getVersion --chrome /path/to/chrome
  chrome-cdp __direct cdp --target page Runtime.evaluate '{\"expression\":\"1+1\"}'
  echo '{\"expression\":\"1+1\"}' | chrome-cdp __direct cdp --target page Runtime.evaluate -")]
    Cdp(DirectCdpArgs),
}

#[derive(Debug, Args)]
pub(crate) struct DirectCdpArgs {
    /// The CDP method, e.g. Browser.getVersion.
    #[arg(value_name = "Domain.method")]
    pub(crate) method: String,

    /// Params as a JSON object; `-` reads them from stdin.
    #[arg(value_name = "JSON")]
    pub(crate) params: Option<String>,

    /// The Chrome or Chromium binary to launch.
    #[arg(long, env = "CHROME_CDP_CHROME", value_name = "PATH")]
    pub(crate) chrome: Option<PathBuf>,

    /// Send the call to the browser, or to a new about:blank page.
    #[arg(long, value_enum, default_value_t = Target::Browser)]
    pub(crate) target: Target,

    /// Truncate text output after this many characters (0 = no limit).
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

/// Where a raw CDP call goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Target {
    /// The browser session (Browser.*, Target.*, …).
    Browser,
    /// A fresh page (Page.*, Runtime.*, DOM.*, …).
    Page,
}

/// Parses `500ms`, `30s`, `1.5s`, `2m`. A unit is required so `5000` is
/// never misread as seconds or milliseconds.
pub(crate) fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let value: f64 = num
        .parse()
        .map_err(|_| format!("`{s}` is not a duration; use e.g. 500ms, 30s or 2m"))?;
    let secs = match unit.trim() {
        "ms" => value / 1000.0,
        "s" => value,
        "m" => value * 60.0,
        "" => return Err(format!("`{s}` needs a unit; use e.g. {s}s or {s}ms")),
        other => return Err(format!("unknown unit `{other}`; use ms, s or m")),
    };
    let duration = Duration::try_from_secs_f64(secs).map_err(|e| format!("`{s}`: {e}"))?;
    if duration.is_zero() {
        return Err("the duration must be greater than zero".into());
    }
    Ok(duration)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use clap::{CommandFactory, Parser};

    use super::{Cli, parse_duration};

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn durations_need_a_unit() {
        assert_eq!(parse_duration("500ms"), Ok(Duration::from_millis(500)));
        assert_eq!(parse_duration("30s"), Ok(Duration::from_secs(30)));
        assert_eq!(parse_duration("1.5s"), Ok(Duration::from_millis(1500)));
        assert_eq!(parse_duration("2m"), Ok(Duration::from_secs(120)));
        assert_eq!(
            parse_duration("5000"),
            Err("`5000` needs a unit; use e.g. 5000s or 5000ms".into())
        );
        assert!(parse_duration("5h").is_err());
        assert!(parse_duration("0s").is_err());
        assert!(parse_duration("fast").is_err());
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn global_flags_work_after_the_subcommand() {
        let cli = Cli::try_parse_from([
            "chrome-cdp",
            "__direct",
            "cdp",
            "Browser.getVersion",
            "--json",
            "--timeout",
            "2s",
            "--session",
            "s1",
            "-vv",
        ])
        .unwrap();
        assert!(cli.global.json);
        assert_eq!(cli.global.timeout, Duration::from_secs(2));
        assert_eq!(cli.global.session, "s1");
        assert_eq!(cli.global.verbose, 2);
    }

    #[test]
    fn json_and_quiet_conflict() {
        assert!(Cli::try_parse_from(["chrome-cdp", "--json", "-q"]).is_err());
    }
}
