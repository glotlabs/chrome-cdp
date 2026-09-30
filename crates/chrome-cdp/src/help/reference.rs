//! The command reference in `docs/commands/` (plan 10, milestone 10.6),
//! generated from the clap definitions: an index by task ([`GROUPS`]) with
//! the global options, and a page per command with its subcommands.
//!
//! A test keeps the files in step with the CLI; `cargo xtask docs` (or
//! `UPDATE_DOCS=1` on that test) rewrites them.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use clap::CommandFactory as _;

use super::{GROUPS, TOPICS, grouped};
use crate::cli::Cli;

/// Where the reference lives, from the repository root.
pub(crate) const DIR: &str = "docs/commands";

/// Every file of the reference: its name in [`DIR`] and its contents.
pub(crate) fn files() -> BTreeMap<String, String> {
    let mut root = Cli::command();
    root.build();
    let mut files = BTreeMap::new();
    files.insert("README.md".to_owned(), index(&root));
    for (_, names) in GROUPS {
        for name in *names {
            let Some(cmd) = root.find_subcommand(name) else {
                continue;
            };
            let mut page = String::new();
            let _ = writeln!(page, "{HEADER}");
            command(&mut page, cmd, &format!("chrome-cdp {name}"), 1);
            if *name == "issues" {
                issue_codes(&mut page);
            }
            files.insert(format!("{name}.md"), page);
        }
    }
    files
}

/// The `issues` page's table of Chrome's issue codes (from
/// [`engine::ISSUE_CODES`]): each code's rule ids and severity.
fn issue_codes(page: &mut String) {
    page.push_str(
        "\n## Rules and severities\n\n\
         Each of Chrome's issue codes, the start of its rule ids (the reason\n\
         in Chrome's details follows, in kebab-case: `cookie-` +\n\
         `ExcludeSameSiteUnspecifiedTreatedAsLax` is\n\
         `cookie-exclude-same-site-unspecified-treated-as-lax`), and its\n\
         severity. A code this chrome-cdp does not know (a newer Chrome's)\n\
         is shown as `unknown-<code>`, severity info.\n\n\
         | Code | Rule | Severity | What |\n|---|---|---|---|\n",
    );
    for code in engine::ISSUE_CODES {
        let rule = match (code.rule, code.reasons) {
            ("", _) => "`<reason>`".to_owned(),
            (rule, true) => format!("`{rule}-<reason>`"),
            (rule, false) => format!("`{rule}`"),
        };
        let _ = writeln!(
            page,
            "| `{}` | {rule} | {} | {} |",
            code.name,
            code.severity.as_str(),
            cell(code.about)
        );
    }
}

const HEADER: &str = "<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->";

fn index(root: &clap::Command) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{HEADER}");
    out.push_str(
        "# Command reference\n\n\
         Every chrome-cdp command, by task, with its options and examples: the\n\
         same text as `chrome-cdp help <command>`. The guides in\n\
         [docs/](../README.md) explain how they fit together.\n",
    );
    for (group, commands) in grouped() {
        let _ = write!(
            out,
            "\n## {group}\n\n| Command | What it does |\n|---|---|\n"
        );
        for (name, about) in commands {
            let _ = writeln!(out, "| [`{name}`]({name}.md) | {} |", cell(&about));
        }
    }
    out.push_str(
        "\n## Global options\n\n\
         Every command takes these. Most also have an environment variable, and\n\
         the config files can set some of them (`chrome-cdp help config`).\n\n",
    );
    for arg in root.get_arguments().filter(|a| a.is_global_set()) {
        option(&mut out, arg);
    }
    out.push_str(
        "\n## Help topics\n\n`chrome-cdp help <topic>` explains an idea the commands share:\n\n",
    );
    for topic in TOPICS {
        let _ = writeln!(out, "- `{}`: {}", topic.name, prose(topic.summary));
    }
    out
}

/// `cmd`'s section, and its subcommands' one level deeper.
fn command(out: &mut String, cmd: &clap::Command, path: &str, level: usize) {
    let _ = writeln!(out, "{} `{path}`\n", "#".repeat(level));
    if let Some(about) = cmd.get_long_about().or_else(|| cmd.get_about()) {
        for paragraph in about.to_string().split("\n\n") {
            let _ = writeln!(out, "{}\n", prose(&paragraph.replace('\n', " ")));
        }
    }
    let usage = cmd.clone().render_usage().to_string();
    let usage: Vec<&str> = usage
        .trim_start_matches("Usage: ")
        .lines()
        .map(str::trim)
        .collect();
    let _ = writeln!(out, "```text\n{}\n```\n", usage.join("\n"));
    let visible = |a: &&clap::Arg| !a.is_hide_set() && !a.is_global_set();
    let positionals: Vec<_> = cmd.get_positionals().filter(visible).collect();
    if !positionals.is_empty() {
        out.push_str("Arguments:\n\n");
        for arg in positionals {
            option(out, arg);
        }
        out.push('\n');
    }
    let options: Vec<_> = cmd
        .get_arguments()
        .filter(|a| !a.is_positional())
        .filter(visible)
        .filter(|a| a.get_id() != "help")
        .collect();
    if !options.is_empty() {
        out.push_str("Options:\n\n");
        for arg in options {
            option(out, arg);
        }
        out.push('\n');
    }
    if let Some(after) = cmd.get_after_long_help().or_else(|| cmd.get_after_help()) {
        let _ = writeln!(out, "```text\n{}\n```\n", after.to_string().trim_end());
    }
    for sub in cmd.get_subcommands().filter(|s| !s.is_hide_set()) {
        if sub.get_name() == "help" {
            continue;
        }
        command(out, sub, &format!("{path} {}", sub.get_name()), level + 1);
    }
}

/// One argument as a list item: its name, help, values, default and env var.
fn option(out: &mut String, arg: &clap::Arg) {
    let takes_values = arg.get_action().takes_values();
    let value = arg
        .get_value_names()
        .filter(|_| takes_values)
        .map(|names| {
            names
                .iter()
                .map(|n| format!("<{n}>"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let name = match (arg.get_short(), arg.get_long()) {
        _ if arg.is_positional() => value.clone(),
        (Some(short), Some(long)) => format!("-{short}, --{long} {value}"),
        (None, Some(long)) => format!("--{long} {value}"),
        (Some(short), None) => format!("-{short} {value}"),
        (None, None) => value.clone(),
    };
    let help = arg
        .get_long_help()
        .or_else(|| arg.get_help())
        .map(|h| prose(&h.to_string().replace("\n\n", " ").replace('\n', " ")))
        .unwrap_or_default();
    let mut notes = Vec::new();
    let values: Vec<String> = arg
        .get_possible_values()
        .iter()
        .filter(|v| !v.is_hide_set())
        .map(|v| format!("`{}`", v.get_name()))
        .collect();
    if !values.is_empty() && takes_values {
        notes.push(format!("one of {}", values.join(", ")));
    }
    let defaults: Vec<String> = arg
        .get_default_values()
        .iter()
        .map(|v| v.to_string_lossy().into_owned())
        .collect();
    if !defaults.is_empty() && takes_values {
        notes.push(format!("default `{}`", defaults.join(",")));
    }
    if let Some(env) = arg.get_env() {
        notes.push(format!("env `{}`", env.to_string_lossy()));
    }
    let notes = if notes.is_empty() {
        String::new()
    } else {
        format!(" ({})", notes.join("; "))
    };
    let _ = writeln!(out, "- `{}`: {help}{notes}", name.trim());
}

/// Help text as Markdown: `<`, `>` and `*` outside code spans are escaped.
fn prose(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_code = false;
    for c in text.chars() {
        match c {
            '`' => {
                in_code = !in_code;
                out.push(c);
            }
            '<' if !in_code => out.push_str("&lt;"),
            '>' if !in_code => out.push_str("&gt;"),
            '*' if !in_code => out.push_str("\\*"),
            _ => out.push(c),
        }
    }
    out
}

/// A table cell: prose with `|` escaped.
fn cell(text: &str) -> String {
    prose(text).replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::{DIR, files};

    /// `docs/commands/` is what the CLI generates. `cargo xtask docs` (which
    /// sets `UPDATE_DOCS=1`) rewrites it and removes pages of commands that
    /// are gone.
    #[test]
    fn the_command_reference_is_up_to_date() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(DIR);
        let files = files();
        let update = std::env::var_os("UPDATE_DOCS").is_some_and(|v| v == "1");
        if update {
            fs::create_dir_all(&dir).unwrap();
        }
        let mut stale = Vec::new();
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let markdown = entry.path().extension().is_some_and(|e| e == "md");
            if markdown && !files.contains_key(&name) {
                if update {
                    fs::remove_file(entry.path()).unwrap();
                } else {
                    stale.push(name);
                }
            }
        }
        for (name, text) in &files {
            let path = dir.join(name);
            let old = fs::read_to_string(&path).unwrap_or_default();
            if &old == text {
                continue;
            }
            if update {
                fs::write(&path, text).unwrap();
            } else {
                stale.push(name.clone());
            }
        }
        assert!(
            stale.is_empty(),
            "{DIR} is out of date ({}); run `cargo xtask docs`",
            stale.join(", ")
        );
    }

    #[test]
    fn every_page_has_usage_and_the_index_links_it() {
        let files = files();
        let index = &files["README.md"];
        for (name, text) in &files {
            if name == "README.md" {
                continue;
            }
            assert!(
                index.contains(&format!("]({name})")),
                "{name} is not linked"
            );
            assert!(text.contains("```text\nchrome-cdp "), "{name}: no usage");
        }
        assert!(index.contains("`--json`"), "{index}");
        assert!(!index.contains("`-h, --help`"));
    }
}
