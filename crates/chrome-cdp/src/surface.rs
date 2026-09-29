//! The command line's public surface as a golden file,
//! `tests/golden/cli.txt`: every visible command with its arguments,
//! flags, their values and environment variables, and nothing a release
//! may reword (help text, defaults). `cargo xtask release check` compares
//! it with the previous release tag's to catch a removed command or flag
//! (docs/releasing.md).
//!
//! To accept a change: `UPDATE_GOLDEN=1 cargo test -p chrome-cdp --bin
//! chrome-cdp surface`.

use std::fmt::Write as _;
use std::path::Path;

use clap::CommandFactory as _;

use crate::cli::Cli;

const HEADER: &str = "\
# chrome-cdp's command line: each command with its arguments (in order)
# and flags. `cargo xtask release check` compares it with the previous
# release's. Regenerate: UPDATE_GOLDEN=1 cargo test -p chrome-cdp --bin chrome-cdp surface
";

/// The surface file's text.
fn surface() -> String {
    let mut root = Cli::command();
    root.build();
    let mut out = HEADER.to_owned();
    command(&root, "chrome-cdp", &mut out);
    out
}

/// `cmd`'s lines, then its visible subcommands', depth first.
fn command(cmd: &clap::Command, path: &str, out: &mut String) {
    let _ = writeln!(out, "command {path}");
    for arg in cmd.get_positionals().filter(|a| !a.is_hide_set()) {
        let _ = write!(out, "  arg {}", value_name(arg));
        if arg.is_required_set() {
            out.push_str(" required");
        }
        if arg.get_num_args().is_some_and(|n| n.max_values() > 1) {
            out.push_str(" multiple");
        }
        values(arg, out);
        out.push('\n');
    }
    // Global flags are listed once, under `chrome-cdp`.
    let flags = cmd
        .get_arguments()
        .filter(|a| !a.is_positional() && !a.is_hide_set())
        .filter(|a| path == "chrome-cdp" || !a.is_global_set())
        .filter(|a| a.get_long() != Some("help"));
    for arg in flags {
        out.push_str("  flag");
        if let Some(long) = arg.get_long() {
            let _ = write!(out, " --{long}");
        }
        if let Some(short) = arg.get_short() {
            let _ = write!(out, " -{short}");
        }
        if arg.get_action().takes_values() {
            let _ = write!(out, " value={}", value_name(arg));
            values(arg, out);
        }
        if arg.is_required_set() {
            out.push_str(" required");
        }
        if matches!(
            arg.get_action(),
            clap::ArgAction::Append | clap::ArgAction::Count
        ) {
            out.push_str(" repeatable");
        }
        if let Some(env) = arg.get_env() {
            let _ = write!(out, " env={}", env.to_string_lossy());
        }
        out.push('\n');
    }
    for sub in cmd.get_subcommands() {
        // clap's own `help` subcommand of commands with subcommands.
        if sub.is_hide_set() || (sub.get_name() == "help" && path != "chrome-cdp") {
            continue;
        }
        command(sub, &format!("{path} {}", sub.get_name()), out);
    }
}

fn value_name(arg: &clap::Arg) -> String {
    arg.get_value_names()
        .and_then(|names| names.first())
        .map_or_else(|| arg.get_id().as_str().to_uppercase(), ToString::to_string)
        .replace(' ', "_")
}

/// ` values=a|b` when the argument names the values it takes.
fn values(arg: &clap::Arg, out: &mut String) {
    let values: Vec<String> = arg
        .get_possible_values()
        .iter()
        .filter(|v| !v.is_hide_set())
        .map(|v| v.get_name().to_owned())
        .collect();
    if !values.is_empty() {
        let _ = write!(out, " values={}", values.join("|"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_matches_golden_file() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/cli.txt");
        let actual = surface();
        if std::env::var_os("UPDATE_GOLDEN").is_some_and(|v| !v.is_empty() && v != "0") {
            std::fs::write(&path, &actual).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            expected == actual,
            "{} differs from the command line (UPDATE_GOLDEN=1 accepts it); now:\n{actual}",
            path.display()
        );
    }
}
