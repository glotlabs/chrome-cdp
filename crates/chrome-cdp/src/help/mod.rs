//! `chrome-cdp help` and the short `--help` (plan 08, milestone 8.3).
//!
//! `--help` (and `help` alone) prints [`SHORT`]: the top commands by task
//! and the locator grammar. `help commands` lists every command by task
//! ([`GROUPS`]), `help <topic>` prints one of [`TOPICS`], and
//! `help <command>` the command's full `--help`. A topic wins over a
//! command of the same name (`help errors`); its text points at the
//! command.

use std::fmt::Write as _;

use clap::CommandFactory as _;
use protocol::schema::HelpData;
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, suggest};

use crate::cli::{Cli, HelpArgs};

#[cfg(test)]
mod reference;

/// `chrome-cdp --help`: the overview.
pub(crate) const SHORT: &str = include_str!("short.txt");

/// A help topic: `chrome-cdp help <name>`.
#[derive(Debug)]
pub(crate) struct Topic {
    pub(crate) name: &'static str,
    pub(crate) summary: &'static str,
    pub(crate) text: &'static str,
}

/// The help topics, in the order the overview names them.
pub(crate) const TOPICS: &[Topic] = &[
    Topic {
        name: "locators",
        summary: "how commands name elements: refs, CSS, text=, role=, >> chains, frames",
        text: include_str!("locators.txt"),
    },
    Topic {
        name: "refs",
        summary: "refs (e12): where they come from, how long they last, stale refs",
        text: include_str!("refs.txt"),
    },
    Topic {
        name: "waiting",
        summary: "what commands wait for by themselves, and `wait` for the rest",
        text: include_str!("waiting.txt"),
    },
    Topic {
        name: "errors",
        summary: "error kinds, exit codes and hints",
        text: include_str!("errors.txt"),
    },
    Topic {
        name: "config",
        summary: "config files, environment variables and which wins",
        text: include_str!("config.txt"),
    },
    Topic {
        name: "scripts",
        summary: "batch and run: many commands in one call",
        text: include_str!("scripts.txt"),
    },
];

/// Every command, grouped by task, for `help commands`, `--help`'s
/// "commands" pointer and `skill --format llms-txt`. A test checks that
/// each visible command is in exactly one group.
pub(crate) const GROUPS: &[(&str, &[&str])] = &[
    (
        "Browser and sessions",
        &[
            "open",
            "connect",
            "close",
            "status",
            "sessions",
            "extensions",
            "install",
            "uninstall",
        ],
    ),
    (
        "Tabs and navigation",
        &[
            "goto", "back", "forward", "reload", "tabs", "tab", "frames", "context", "wait",
        ],
    ),
    (
        "Reading the page",
        &[
            "snapshot", "find", "locator", "text", "markdown", "html", "attr", "value", "count",
            "box",
        ],
    ),
    (
        "Acting",
        &[
            "click",
            "dblclick",
            "hover",
            "tap",
            "fill",
            "type",
            "press",
            "select",
            "check",
            "uncheck",
            "focus",
            "blur",
            "upload",
            "drag",
            "scroll",
            "mouse",
            "keyboard",
            "dialog",
            "clipboard",
        ],
    ),
    (
        "Verifying",
        &[
            "expect",
            "expect-snapshot",
            "expect-screenshot",
            "mark",
            "init",
            "test",
            "lint",
            "fmt",
            "history",
        ],
    ),
    (
        "Observing and debugging",
        &[
            "screenshot",
            "pdf",
            "console",
            "errors",
            "network",
            "downloads",
            "eval",
            "perf",
            "coverage",
            "har",
            "trace",
            "highlight",
            "inspect",
        ],
    ),
    (
        "Mocking and page scripts",
        &[
            "route",
            "block",
            "routes",
            "unroute",
            "mock",
            "http",
            "init-script",
            "expose",
            "unexpose",
        ],
    ),
    (
        "Emulation",
        &[
            "emulate",
            "viewport",
            "geo",
            "tz",
            "locale",
            "ua",
            "media",
            "offline",
            "throttle",
            "cpu-throttle",
            "time",
            "permissions",
        ],
    ),
    ("Cookies and storage", &["cookies", "storage", "state"]),
    ("Scripts and raw CDP", &["batch", "run", "convert", "cdp"]),
    (
        "Help and tooling",
        &[
            "help",
            "skill",
            "completions",
            "schema",
            "doctor",
            "upgrade",
        ],
    ),
];

pub(crate) fn run(args: &HelpArgs) -> Result<Output, ErrorInfo> {
    let (topic, text) = match args.topic.as_slice() {
        [] => ("chrome-cdp".to_owned(), SHORT.trim_end().to_owned()),
        [one] if one == "commands" => (one.clone(), commands()),
        [one] if topic(one).is_some() => (
            one.clone(),
            topic(one).map_or("", |t| t.text).trim_end().to_owned(),
        ),
        words => (words.join(" "), command_help(words)?),
    };
    Ok(Output::typed(
        &HelpData {
            topic,
            text: text.clone(),
        },
        text.clone(),
    )
    .quiet(text))
}

fn topic(name: &str) -> Option<&'static Topic> {
    TOPICS.iter().find(|t| t.name == name)
}

/// The `about` line of each command, by group.
pub(crate) fn grouped() -> Vec<(&'static str, Vec<(&'static str, String)>)> {
    let root = Cli::command();
    GROUPS
        .iter()
        .map(|(group, names)| {
            let commands = names
                .iter()
                .map(|name| {
                    let about = root
                        .find_subcommand(name)
                        .and_then(clap::Command::get_about)
                        .map(ToString::to_string)
                        .unwrap_or_default();
                    (*name, about)
                })
                .collect();
            (*group, commands)
        })
        .collect()
}

/// `help commands`: every command's one-line description, by task.
fn commands() -> String {
    let groups = grouped();
    let width = groups
        .iter()
        .flat_map(|(_, commands)| commands.iter().map(|(name, _)| name.len()))
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for (group, commands) in groups {
        out.push_str(group);
        out.push('\n');
        for (name, about) in commands {
            let _ = writeln!(out, "  {name:width$}  {about}");
        }
        out.push('\n');
    }
    out.push_str("chrome-cdp help <command> shows one in full, with examples.");
    out
}

/// The full `--help` of the command `words` name (`tab close`).
fn command_help(words: &[String]) -> Result<String, ErrorInfo> {
    let mut cmd = Cli::command();
    cmd.build();
    for (i, word) in words.iter().enumerate() {
        let visible = |c: &clap::Command| !c.is_hide_set();
        let Some(sub) = cmd.find_subcommand(word).filter(|s| visible(s)).cloned() else {
            let mut names: Vec<&str> = cmd
                .get_subcommands()
                .filter(|s| visible(s))
                .map(clap::Command::get_name)
                .collect();
            let path = words[..i].join(" ");
            let message = if i == 0 {
                names.extend(TOPICS.iter().map(|t| t.name));
                names.push("commands");
                format!("no command or help topic is called `{word}`")
            } else {
                format!("`{path}` has no subcommand `{word}`")
            };
            let close = suggest::similar(word, names, 3);
            let info = ErrorInfo::new(ErrorKind::BadArgs, message);
            return Err(match close.first() {
                Some(first) => info
                    .note("did you mean", close.join(", "))
                    .hint(Hint::run(Cmd::new(&format!("help {path}")).arg(first))),
                None if i == 0 => info
                    .hint(Hint::run(Cmd::new("help").arg("commands")).why("lists every command")),
                None => info.hint(Hint::run(Cmd::new(&format!("help {path}")))),
            });
        };
        cmd = sub;
    }
    Ok(cmd.render_long_help().to_string().trim_end().to_owned())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use clap::CommandFactory as _;

    use super::{GROUPS, SHORT, TOPICS, run};
    use crate::cli::{Cli, HelpArgs};

    fn help(words: &[&str]) -> Result<String, protocol::ErrorInfo> {
        let args = HelpArgs {
            topic: words.iter().map(|w| (*w).to_owned()).collect(),
        };
        run(&args).map(|out| out.text)
    }

    #[test]
    fn every_command_is_in_one_group() {
        let root = Cli::command();
        let visible: HashSet<&str> = root
            .get_subcommands()
            .filter(|c| !c.is_hide_set())
            .map(clap::Command::get_name)
            .collect();
        let mut grouped = HashSet::new();
        for (group, names) in GROUPS {
            for name in *names {
                assert!(visible.contains(name), "{group}: no command `{name}`");
                assert!(grouped.insert(*name), "`{name}` is in two groups");
            }
        }
        let missing: Vec<_> = visible.difference(&grouped).collect();
        assert!(missing.is_empty(), "not in help::GROUPS: {missing:?}");
    }

    #[test]
    fn the_overview_is_short_and_names_real_commands() {
        assert!(
            SHORT.lines().count() <= 40,
            "{} lines",
            SHORT.lines().count()
        );
        assert!(SHORT.lines().all(|l| l.chars().count() <= 80));
        let root = Cli::command();
        // `Start      open [url]  …`: a group label (or spaces) in 11
        // columns, then the command.
        let labels = ["Start", "Look", "Act", "Check", "Script", ""];
        let listed: Vec<&str> = SHORT
            .lines()
            .filter(|l| l.get(..11).is_some_and(|p| labels.contains(&p.trim())))
            .filter_map(|l| l[11..].split_whitespace().next())
            .collect();
        assert!(listed.len() >= 15, "{listed:?}");
        for name in listed {
            assert!(root.find_subcommand(name).is_some(), "no command `{name}`");
        }
        for topic in TOPICS {
            assert!(
                SHORT.contains(topic.name),
                "--help does not name {}",
                topic.name
            );
        }
        for flag in [
            "--json",
            "--quiet",
            "--session",
            "--tab",
            "--timeout",
            "--verbose",
        ] {
            assert!(SHORT.contains(flag), "--help does not name {flag}");
        }
    }

    #[test]
    fn topics_have_examples_that_run_real_commands() {
        let root = Cli::command();
        for topic in TOPICS {
            let text = topic.text;
            assert!(
                text.contains("\nExamples:\n  "),
                "{}: no examples",
                topic.name
            );
            assert!(
                text.lines().all(|l| l.chars().count() <= 80),
                "{}",
                topic.name
            );
            for line in text.split("\nExamples:\n").nth(1).unwrap_or("").lines() {
                for (i, _) in line.match_indices("chrome-cdp ") {
                    let word = line[i + "chrome-cdp ".len()..]
                        .split_whitespace()
                        .next()
                        .unwrap_or("");
                    assert!(
                        root.find_subcommand(word).is_some(),
                        "{}: `{line}` runs no command",
                        topic.name
                    );
                }
            }
        }
    }

    #[test]
    fn the_errors_topic_names_every_kind() {
        let text = super::topic("errors").map(|t| t.text).unwrap_or_default();
        for kind in protocol::ErrorKind::ALL {
            let name = format!("{kind:?}");
            assert!(text.contains(&format!("  {name} ")), "no {name}");
        }
    }

    /// The env vars of `cmd`'s flags and its subcommands'.
    fn env_vars(cmd: &clap::Command, out: &mut HashSet<String>) {
        for arg in cmd.get_arguments() {
            if let Some(env) = arg.get_env() {
                out.insert(env.to_string_lossy().into_owned());
            }
        }
        for sub in cmd.get_subcommands() {
            env_vars(sub, out);
        }
    }

    #[test]
    fn the_config_topic_names_every_key_and_env_var() {
        let text = super::topic("config").map(|t| t.text).unwrap_or_default();
        for key in crate::config::KEYS.split(", ") {
            assert!(text.contains(&format!("  {key} = ")), "no key {key}");
        }
        let mut root = Cli::command();
        root.build();
        let mut envs = HashSet::new();
        env_vars(&root, &mut envs);
        assert!(envs.len() > 10, "{envs:?}");
        for env in envs {
            assert!(text.contains(&env), "the config topic does not name {env}");
        }
    }

    /// The topic's `[e2e]` example is a valid config once the notes to the
    /// right of each line are cut.
    #[test]
    fn the_config_topics_e2e_example_parses() {
        let text = super::topic("config").map(|t| t.text).unwrap_or_default();
        let block = text
            .split("\n  [e2e]\n")
            .nth(1)
            .and_then(|rest| rest.split("\n\nA web server").next())
            .unwrap_or_default();
        let mut toml = String::from("[e2e]\n");
        for line in block.lines().map(str::trim_start) {
            let line = line.split_once("  ").map_or(line, |(value, _)| value);
            if line.contains('=') || line.starts_with('[') {
                toml.push_str(line);
                toml.push('\n');
            }
        }
        let file = std::path::Path::new("/p/chrome-cdp.toml");
        let config = e2e::Config::parse(&toml, file)
            .unwrap_or_else(|e| panic!("{}\n{toml}", e.message))
            .unwrap();
        assert_eq!(config.web_servers.len(), 1);
        assert!(config.backends["local"].web_servers.is_some());
        assert_eq!(
            config.determinism.random_seed,
            Some(e2e::config::RandomSeed::Seed(42))
        );
    }

    #[test]
    fn help_shows_topics_commands_and_subcommands() {
        assert_eq!(help(&[]).unwrap(), SHORT.trim_end());
        assert!(help(&["locators"]).unwrap().starts_with("Locators:"));
        // The topic wins over the command; the topic points at it.
        let errors = help(&["errors"]).unwrap();
        assert!(errors.contains("chrome-cdp errors --help"));
        let click = help(&["click"]).unwrap();
        assert!(click.contains("Usage: chrome-cdp click"), "{click}");
        assert!(click.contains("Examples:"));
        let close = help(&["tab", "close"]).unwrap();
        assert!(close.contains("Usage: chrome-cdp tab close"), "{close}");
        let all = help(&["commands"]).unwrap();
        assert!(all.contains("\n  click "), "{all}");
    }

    #[test]
    fn unknown_topics_suggest_the_closest() {
        let err = help(&["locaters"]).unwrap_err();
        assert_eq!(err.message, "no command or help topic is called `locaters`");
        assert_eq!(err.hint.as_deref(), Some("chrome-cdp help locators"));
        let err = help(&["tab", "clse"]).unwrap_err();
        assert_eq!(err.message, "`tab` has no subcommand `clse`");
        assert_eq!(err.hint.as_deref(), Some("chrome-cdp help tab close"));
        let err = help(&["frobnicate"]).unwrap_err();
        assert_eq!(
            err.hint.as_deref(),
            Some("chrome-cdp help commands   (lists every command)")
        );
        // The daemon is not a command to show.
        assert!(help(&["__daemon"]).is_err());
    }
}
