//! `chrome-cdp schema [command]`: the JSON Schema of a command's `--json`
//! output, from the registry in `protocol::schema`.

use std::fmt::Write as _;

use protocol::schema::{self, COMMANDS, SchemaData, SchemaListEntry};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};

use crate::cli::SchemaArgs;

pub(crate) fn run(args: &SchemaArgs) -> Result<Output, ErrorInfo> {
    if args.command.is_empty() {
        return Ok(list());
    }
    let name = args.command.join(" ");
    let Some(entry) = schema::command(&name) else {
        return Err(
            ErrorInfo::new(ErrorKind::BadArgs, format!("no schema for `{name}`"))
                .hint(Hint::run(Cmd::new("schema")).why("lists the commands that have one")),
        );
    };
    let schema = entry.schema();
    let text = serde_json::to_string_pretty(&schema).unwrap_or_else(|_| schema.to_string());
    let data = match schema {
        serde_json::Value::Object(map) => SchemaData::Schema(map),
        _ => SchemaData::Schema(serde_json::Map::new()),
    };
    Ok(Output::typed(&data, text))
}

fn list() -> Output {
    let width = COMMANDS.iter().map(|c| c.name.len()).max().unwrap_or(0);
    let mut text = String::new();
    for c in COMMANDS {
        let _ = writeln!(text, "{:width$}  {}", c.name, c.summary);
    }
    let names: Vec<_> = COMMANDS.iter().map(|c| c.name).collect();
    let data = SchemaData::List {
        commands: COMMANDS
            .iter()
            .map(|c| SchemaListEntry {
                name: c.name.into(),
                summary: c.summary.into(),
            })
            .collect(),
    };
    Output::typed(&data, text.trim_end()).quiet(names.join("\n"))
}
