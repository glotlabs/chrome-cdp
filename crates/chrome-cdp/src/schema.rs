//! `chrome-cdp schema [command]`: the JSON Schema of a command's `--json`
//! output, from the registry in `protocol::schema`; and `schema mocks`,
//! the mock file format's (plan 11).

use protocol::schema::{self, COMMANDS, SchemaData, SchemaListEntry};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format, suggest};

use crate::cli::SchemaArgs;

pub(crate) fn run(args: &SchemaArgs) -> Result<Output, ErrorInfo> {
    if args.command.is_empty() {
        return Ok(list());
    }
    let name = args.command.join(" ");
    if name == MOCKS {
        return Ok(typed(mock::schema()));
    }
    let Some(entry) = schema::command(&name) else {
        let info = ErrorInfo::new(ErrorKind::BadArgs, format!("no schema for `{name}`"));
        let names = COMMANDS.iter().map(|c| c.name).chain([MOCKS]);
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
        .chain([(MOCKS, MOCKS_SUMMARY)])
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
