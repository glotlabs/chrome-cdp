//! `chrome-cdp schema [command]`: the JSON Schema of a command's `--json`
//! output, from the registry in `protocol::schema`; `schema mocks`, the
//! mock file format's (plan 11); and `schema scenario`, what a `.cdp`
//! line may hold (plan 11.12), generated from the clap definitions so it
//! cannot drift from the binary.

use protocol::schema::{self, COMMANDS, SchemaData, SchemaListEntry};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format, suggest};

use clap::CommandFactory as _;

use crate::cli::SchemaArgs;

pub(crate) fn run(args: &SchemaArgs) -> Result<Output, ErrorInfo> {
    if args.command.is_empty() {
        return Ok(list());
    }
    let name = args.command.join(" ");
    if name == MOCKS {
        return Ok(typed(mock::schema()));
    }
    if name == SCENARIO {
        return Ok(typed(scenario()));
    }
    if name == AUDIT_BASELINE {
        return Ok(typed(schema::audit_baseline_schema()));
    }
    let Some(entry) = schema::command(&name) else {
        let info = ErrorInfo::new(ErrorKind::BadArgs, format!("no schema for `{name}`"));
        let names = COMMANDS
            .iter()
            .map(|c| c.name)
            .chain([MOCKS, SCENARIO, AUDIT_BASELINE]);
        let close = suggest::similar(&name, names, 3);
        return Err(match close.first() {
            Some(first) => info
                .note("did you mean", close.join(", "))
                .hint(Hint::run(Cmd::new("schema").raw(*first))),
            None => {
                info.hint(Hint::run(Cmd::new("schema")).why("lists the commands that have one"))
            }
        });
    };
    Ok(typed(entry.schema()))
}

/// Not a command: the schema of mock files.
const MOCKS: &str = "mocks";
const MOCKS_SUMMARY: &str = "not a command: the mock file format (plan 11), for editors";

/// Not a command: what a `.cdp` file may hold.
const SCENARIO: &str = "scenario";
const SCENARIO_SUMMARY: &str = "not a command: the .cdp format's directives, commands with their flags, and variables, for editors and agents";

/// Not a command: the audit baseline file format.
const AUDIT_BASELINE: &str = "audit-baseline";
const AUDIT_BASELINE_SUMMARY: &str =
    "not a command: an audit baseline file, the findings `audit --baseline` accepts (plan 17)";

/// Commands a script line cannot run.
const NOT_IN_SCRIPTS: &[&str] = &["batch", "run", "test"];

/// `schema scenario`: the directives, the commands a line may run with
/// their arguments and flags, the global flags, and the variables.
fn scenario() -> serde_json::Value {
    use serde_json::json;
    let mut root = crate::cli::Cli::command();
    root.build();
    let mut commands = Vec::new();
    leaves(&root, "", &mut commands);
    let commands: Vec<serde_json::Value> = commands
        .iter()
        .map(|(name, cmd)| {
            let positionals: Vec<serde_json::Value> = cmd
                .get_positionals()
                .filter(|a| !a.is_hide_set())
                .map(positional)
                .collect();
            let flags: Vec<serde_json::Value> = cmd
                .get_arguments()
                .filter(|a| !a.is_positional() && !a.is_hide_set() && !a.is_global_set())
                .filter(|a| a.get_long() != Some("help"))
                .map(flag)
                .collect();
            json!({
                "name": name,
                "summary": cmd.get_about().map(ToString::to_string).unwrap_or_default(),
                "arguments": positionals,
                "flags": flags,
            })
        })
        .collect();
    let globals: Vec<serde_json::Value> = root
        .get_arguments()
        .filter(|a| a.is_global_set() && !a.is_hide_set())
        .map(flag)
        .collect();
    let directives: Vec<serde_json::Value> = e2e::scenario::DIRECTIVE_DOCS
        .iter()
        .map(|d| json!({"name": d.name, "usage": d.usage, "kind": d.kind, "summary": d.summary}))
        .collect();
    let variables: Vec<serde_json::Value> = protocol::script::VARIABLES
        .iter()
        .map(|(form, summary)| json!({"form": form, "summary": summary}))
        .collect();
    json!({
        "title": "chrome-cdp .cdp scenario format",
        "description": "Each line is a command (the CLI's words without `chrome-cdp`) or a directive (@name args); a directive's body is indented 2 spaces. `#` starts a comment; quoting is shell-like (single quotes literal, double quotes allow \\\" \\\\ \\n and ${…}); a trailing ` \\` continues a line. See docs/scenario-syntax.md and docs/e2e/scenarios.md.",
        "syntax_version": protocol::script::SYNTAX_VERSION,
        "version": env!("CARGO_PKG_VERSION"),
        "directives": directives,
        "variables": variables,
        "global_flags": globals,
        "commands": commands,
        "mock_only": MOCK_ONLY,
    })
}

/// Commands that need the mocks: `test` fails them on a real backend
/// (`crate::test::mock_only`).
const MOCK_ONLY: &[&str] = &[
    "mock state",
    "mock seed",
    "mock ws send",
    "mock sse send",
    "expect mock-state",
];

/// The commands a line may run: every leaf of the tree (and commands with
/// subcommands that take arguments too), as the words that name them.
fn leaves(cmd: &clap::Command, path: &str, out: &mut Vec<(String, clap::Command)>) {
    for sub in cmd.get_subcommands() {
        let name = sub.get_name();
        if sub.is_hide_set() || (name == "help" && !path.is_empty()) {
            continue;
        }
        if path.is_empty() && NOT_IN_SCRIPTS.contains(&name) {
            continue;
        }
        let full = format!("{path} {name}").trim().to_owned();
        if !sub.has_subcommands() || sub.get_positionals().next().is_some() {
            out.push((full.clone(), sub.clone()));
        }
        if sub.has_subcommands() {
            leaves(sub, &full, out);
        }
    }
}

fn positional(arg: &clap::Arg) -> serde_json::Value {
    let mut entry = serde_json::json!({
        "name": value_name(arg),
        "required": arg.is_required_set(),
        "multiple": arg.get_num_args().is_some_and(|n| n.max_values() > 1),
        "summary": arg.get_help().map(ToString::to_string).unwrap_or_default(),
    });
    add_values(arg, &mut entry);
    entry
}

fn flag(arg: &clap::Arg) -> serde_json::Value {
    let takes_value = arg.get_action().takes_values();
    let mut entry = serde_json::json!({
        "long": arg.get_long().map(|l| format!("--{l}")),
        "summary": arg.get_help().map(ToString::to_string).unwrap_or_default(),
    });
    if let Some(short) = arg.get_short() {
        entry["short"] = serde_json::json!(format!("-{short}"));
    }
    if takes_value {
        entry["value"] = serde_json::json!(value_name(arg));
        add_values(arg, &mut entry);
    }
    if matches!(
        arg.get_action(),
        clap::ArgAction::Append | clap::ArgAction::Count
    ) {
        entry["repeatable"] = serde_json::json!(true);
    }
    if let Some(env) = arg.get_env() {
        entry["env"] = serde_json::json!(env.to_string_lossy());
    }
    entry
}

fn value_name(arg: &clap::Arg) -> String {
    arg.get_value_names()
        .and_then(|names| names.first())
        .map_or_else(|| arg.get_id().as_str().to_uppercase(), ToString::to_string)
}

/// The values an argument takes, when it names them (`--wait load|…`).
fn add_values(arg: &clap::Arg, entry: &mut serde_json::Value) {
    let values: Vec<String> = arg
        .get_possible_values()
        .iter()
        .filter(|v| !v.is_hide_set())
        .map(|v| v.get_name().to_owned())
        .collect();
    if !values.is_empty() {
        entry["values"] = serde_json::json!(values);
    }
}

fn typed(schema: serde_json::Value) -> Output {
    let text = serde_json::to_string_pretty(&schema).unwrap_or_else(|_| schema.to_string());
    let data = match schema {
        serde_json::Value::Object(map) => SchemaData::Schema(map),
        _ => SchemaData::Schema(serde_json::Map::new()),
    };
    Output::typed(&data, text)
}

fn list() -> Output {
    let entries: Vec<(&str, &str)> = COMMANDS
        .iter()
        .map(|c| (c.name, c.summary))
        .chain([
            (MOCKS, MOCKS_SUMMARY),
            (SCENARIO, SCENARIO_SUMMARY),
            (AUDIT_BASELINE, AUDIT_BASELINE_SUMMARY),
        ])
        .collect();
    let rows: Vec<_> = entries.iter().map(|(n, s)| vec![*n, *s]).collect();
    let names: Vec<_> = entries.iter().map(|(n, _)| *n).collect();
    let data = SchemaData::List {
        commands: entries
            .iter()
            .map(|(name, summary)| SchemaListEntry {
                name: (*name).into(),
                summary: (*summary).into(),
            })
            .collect(),
    };
    Output::typed(&data, format::table(&rows)).quiet(names.join("\n"))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    fn names(list: &Value) -> Vec<&str> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].as_str().unwrap())
            .collect()
    }

    fn command<'a>(spec: &'a Value, name: &str) -> &'a Value {
        spec["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("no {name}"))
    }

    #[test]
    fn scenario_lists_directives_commands_flags_and_variables() {
        let spec = super::scenario();
        assert_eq!(names(&spec["directives"]), e2e::scenario::DIRECTIVES);
        let commands = names(&spec["commands"]);
        for name in [
            "click",
            "goto",
            "expect",
            "mock use",
            "tab",
            "tab close",
            "http",
            "wait",
        ] {
            assert!(commands.contains(&name), "{name} missing: {commands:?}");
        }
        for name in ["batch", "run", "test", "__daemon", "mock help"] {
            assert!(!commands.contains(&name), "{name} listed");
        }
        let screenshot = command(&spec, "screenshot");
        let output = screenshot["flags"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["long"] == "--output")
            .unwrap();
        assert_eq!(output["short"], "-o");
        assert_eq!(output["value"], "PATH");
        let sets = &command(&spec, "mock use")["arguments"][0];
        assert_eq!(sets["multiple"], true, "{sets}");
        let globals: Vec<&str> = spec["global_flags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["long"].as_str().unwrap())
            .collect();
        assert!(
            globals.contains(&"--timeout") && globals.contains(&"--session"),
            "{globals:?}"
        );
        // Globals are listed once, not on every command.
        assert!(
            !command(&spec, "click")["flags"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["long"] == "--timeout")
        );
        let forms: Vec<&str> = spec["variables"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["form"].as_str().unwrap())
            .collect();
        assert!(
            forms.contains(&"${var:name}") && forms.contains(&"${env:NAME}"),
            "{forms:?}"
        );
    }
}
