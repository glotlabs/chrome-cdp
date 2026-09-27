//! Command-line definitions (clap derive).
//!
//! Global flags follow plan 08 §4: each has a `CHROME_CDP_*` env var, may
//! appear before or after the subcommand, and (for `--session` and
//! `--timeout`) may default from the config files (see `config`). Plan 02 onwards add the real
//! commands to [`Command`].

use std::path::PathBuf;
use std::time::Duration;

use clap::builder::BoolishValueParser;
use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};

use protocol::Mode;

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

    /// `base_url` from the config files, for relative URLs (`goto /login`).
    #[arg(skip)]
    #[allow(dead_code)] // Read by plan 03's `goto`.
    pub(crate) base_url: Option<String>,
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
    /// Print the JSON Schema of a command's --json output.
    #[command(after_help = "\
Examples:
  chrome-cdp schema                 # list the commands that have one
  chrome-cdp schema version         # the schema of chrome-cdp --version --json")]
    Schema(SchemaArgs),

    /// Download Chrome for Testing into chrome-cdp's cache.
    #[command(after_help = "\
The cache is $CHROME_CDP_CACHE, else ~/.cache/chrome-cdp (Linux) or
~/Library/Caches/chrome-cdp (macOS). --timeout bounds connecting and each
wait for data, not the whole download. CHROME_CDP_CFT_URL points at a
mirror of the Chrome for Testing index.

Examples:
  chrome-cdp install                       # the current stable Chrome
  chrome-cdp install --headless-shell      # smaller, headless only
  chrome-cdp install --channel beta
  chrome-cdp install --version 154         # the newest 154.x
  chrome-cdp install --with-deps           # Linux: also the system libraries
  chrome-cdp install --list")]
    Install(InstallArgs),

    /// Remove a browser installed by `chrome-cdp install`.
    #[command(after_help = "\
Examples:
  chrome-cdp install --list                # shows the ids
  chrome-cdp uninstall stable-154.0.8037.57-linux64")]
    Uninstall(UninstallArgs),

    /// Temporary daemon-less commands (plan 01). Plan 02 replaces them.
    #[command(name = "__direct", hide = true, subcommand)]
    Direct(DirectCommand),
}

#[derive(Debug, Args)]
pub(crate) struct SchemaArgs {
    /// The command, e.g. `version` or `tab close`; omit it to list them.
    #[arg(value_name = "COMMAND")]
    pub(crate) command: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct InstallArgs {
    /// The release channel to install.
    #[arg(long, value_enum, default_value_t = Channel::Stable, conflicts_with = "version")]
    pub(crate) channel: Channel,

    /// A version, or the start of one: 154, 154.0.8037.57.
    #[arg(long, value_name = "VERSION")]
    pub(crate) version: Option<String>,

    /// Install chrome-headless-shell (smaller, headless only) instead of
    /// full Chrome.
    #[arg(long)]
    pub(crate) headless_shell: bool,

    /// Download again even if it is installed.
    #[arg(long)]
    pub(crate) force: bool,

    /// Linux: also install the system libraries Chrome needs (sudo apt-get
    /// or dnf) and, on Ubuntu 23.10+, the AppArmor profile its sandbox
    /// needs. Asks first on a terminal.
    #[arg(long)]
    pub(crate) with_deps: bool,

    /// List the installed browsers and each channel's current version.
    #[arg(
        long,
        conflicts_with_all = ["channel", "version", "headless_shell", "force", "with_deps"]
    )]
    pub(crate) list: bool,
}

/// A Chrome release channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Channel {
    Stable,
    Beta,
    Dev,
    Canary,
}

impl Channel {
    pub(crate) fn to_browser(self) -> browser::Channel {
        match self {
            Self::Stable => browser::Channel::Stable,
            Self::Beta => browser::Channel::Beta,
            Self::Dev => browser::Channel::Dev,
            Self::Canary => browser::Channel::Canary,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct UninstallArgs {
    /// The install id, as `chrome-cdp install --list` shows it.
    #[arg(value_name = "ID")]
    pub(crate) id: String,
}

#[derive(Debug, Subcommand)]
pub(crate) enum DirectCommand {
    /// Launch Chrome, make one raw CDP call, print the result and exit.
    #[command(after_help = "\
Examples:
  chrome-cdp __direct cdp Browser.getVersion
  chrome-cdp __direct cdp Browser.getVersion --chrome /path/to/chrome
  chrome-cdp __direct cdp Storage.getCookies --profile work
  chrome-cdp __direct cdp Browser.getVersion --chrome-arg=--window-size=1280,800 --headed
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

    #[command(flatten)]
    pub(crate) launch: LaunchArgs,

    /// Send the call to the browser, or to a new about:blank page.
    #[arg(long, value_enum, default_value_t = Target::Browser)]
    pub(crate) target: Target,

    /// Bound the output to this many characters (0 = no limit). With --json,
    /// long strings and arrays in `data` are shortened instead.
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    pub(crate) max_chars: usize,
}

/// How to find and launch Chrome. Shared by every command that starts a
/// browser (`__direct cdp` now, plan 02's `open` next).
#[derive(Debug, Args)]
pub(crate) struct LaunchArgs {
    /// The Chrome or Chromium binary to launch. Default: $CHROME_CDP_CHROME,
    /// then browsers from `chrome-cdp install`, then a system Chrome.
    #[arg(long, value_name = "PATH")]
    pub(crate) chrome: Option<PathBuf>,

    /// If no Chrome is found, install Chrome for Testing first.
    #[arg(
        long,
        env = "CHROME_CDP_AUTO_INSTALL",
        action = ArgAction::SetTrue,
        value_parser = BoolishValueParser::new(),
        hide_env_values = true
    )]
    pub(crate) auto_install: bool,

    /// Open a browser window instead of running headless.
    #[arg(long)]
    pub(crate) headed: bool,

    /// Keep the profile (logins, cookies, storage) between runs under this
    /// name. Default: a temporary profile, deleted on close.
    #[arg(long, value_name = "NAME", conflicts_with = "user_data_dir")]
    pub(crate) profile: Option<String>,

    /// Use this directory as Chrome's profile (never your everyday one).
    #[arg(long, value_name = "PATH")]
    pub(crate) user_data_dir: Option<PathBuf>,

    /// An extra flag for Chrome, e.g. --chrome-arg=--window-size=1280,800;
    /// repeat it for more. docs/chrome-flags.md lists chrome-cdp's own.
    #[arg(long = "chrome-arg", value_name = "FLAG", allow_hyphen_values = true)]
    pub(crate) chrome_args: Vec<String>,
}

impl LaunchArgs {
    /// The launch options, with `timeout` for startup and each call.
    pub(crate) fn options(&self, timeout: Duration) -> browser::LaunchOptions {
        let profile = match (&self.profile, &self.user_data_dir) {
            (Some(name), _) => browser::ProfileChoice::Named(name.clone()),
            (None, Some(dir)) => browser::ProfileChoice::Dir(dir.clone()),
            (None, None) => browser::ProfileChoice::Ephemeral,
        };
        browser::LaunchOptions {
            headed: self.headed,
            profile,
            args: self.chrome_args.clone(),
            log_file: None,
            timeout,
        }
    }
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

    /// Commands whose output cannot grow with the page, so they need no
    /// `--max-chars`/`--limit`. (`install --list` grows only with the
    /// browsers the user installed.)
    const FIXED_SIZE: &[&str] = &["schema", "install", "uninstall"];

    /// The plan 08 checklist: every command has an example in its help, a
    /// registered `--json` schema, the global flags, and a bounded output.
    #[test]
    fn every_command_follows_the_output_contract() {
        fn leaves(cmd: &clap::Command, path: &str, out: &mut Vec<(String, clap::Command)>) {
            // Skip clap's generated `help` subcommands (plan 08.3 replaces
            // the top-level one with its own, which must then pass too).
            for sub in cmd.get_subcommands().filter(|s| s.get_name() != "help") {
                let name = format!("{path} {}", sub.get_name()).trim().to_owned();
                if sub.has_subcommands() {
                    leaves(sub, &name, out);
                } else {
                    out.push((name, sub.clone()));
                }
            }
        }
        let mut root = Cli::command();
        root.build();
        let mut commands = Vec::new();
        leaves(&root, "", &mut commands);
        assert!(!commands.is_empty());

        let mut problems = Vec::new();
        for (name, cmd) in &commands {
            let help = cmd
                .get_after_help()
                .or(cmd.get_after_long_help())
                .map(ToString::to_string)
                .unwrap_or_default();
            if !help.contains("Examples:\n  chrome-cdp ") {
                problems.push(format!("{name}: no `Examples:` in its help"));
            }
            if protocol::schema::command(name).is_none() {
                problems.push(format!("{name}: no entry in protocol::schema::COMMANDS"));
            }
            for flag in ["json", "quiet", "session", "timeout", "verbose"] {
                if !cmd.get_arguments().any(|a| a.get_long() == Some(flag)) {
                    problems.push(format!("{name}: no global --{flag}"));
                }
            }
            let bounded = cmd
                .get_arguments()
                .any(|a| matches!(a.get_long(), Some("max-chars" | "limit")));
            if !bounded && !FIXED_SIZE.contains(&name.as_str()) {
                problems.push(format!("{name}: no --max-chars or --limit"));
            }
        }
        // `--version` is a flag, not a command, but has a --json form too.
        if protocol::schema::command("version").is_none() {
            problems.push("--version: no schema".into());
        }
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn json_and_quiet_conflict() {
        assert!(Cli::try_parse_from(["chrome-cdp", "--json", "-q"]).is_err());
    }
}
