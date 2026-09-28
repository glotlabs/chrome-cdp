//! `chrome-cdp schema [command]`: the JSON Schema of a command's `--json`
//! output, from the registry in `protocol::schema`.

use protocol::schema::{self, COMMANDS, SchemaData, SchemaListEntry};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format, suggest};

use crate::cli::SchemaArgs;

pub(crate) fn run(args: &SchemaArgs) -> Result<Output, ErrorInfo> {
    if args.command.is_empty() {
        return Ok(list());
    }
    let name = args.command.join(" ");
    let Some(entry) = schema::command(&name) else {
        let info = ErrorInfo::new(ErrorKind::BadArgs, format!("no schema for `{name}`"));
        let close = suggest::similar(&name, COMMANDS.iter().map(|c| c.name), 3);
        return Err(match close.first() {
            Some(first) => info
                .note("did you mean", close.join(", "))
                .hint(Hint::run(Cmd::new("schema").raw(*first))),
            None => {
                info.hint(Hint::run(Cmd::new("schema")).why("lists the commands that have one"))
            }
        });
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
    let rows: Vec<_> = COMMANDS.iter().map(|c| vec![c.name, c.summary]).collect();
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
    Output::typed(&data, format::table(&rows)).quiet(names.join("\n"))
}
