//! The mock file reference (`docs/e2e/mocks.md`), generated from the
//! file format's JSON Schema ([`crate::schema`]), so the reference says
//! what the parser takes. A test keeps the committed page, and the
//! published schema next to it (`docs/e2e/mocks.schema.json`), up to date.

use std::fmt::Write as _;

use serde_json::Value;

/// The tables of a mock file, in the order the page shows them.
const TABLES: [&str; 5] = ["settings", "state", "mock", "collection", "websocket"];

/// The reference page, in Markdown.
pub fn reference() -> String {
    let schema = crate::schema();
    let defs = &schema["$defs"];
    let mut out = String::from(
        "# Mock file reference\n\n\
         <!-- Generated from `chrome-cdp schema mocks` by the mock crate's tests;\n\
         run `UPDATE_GOLDEN=1 cargo test -p mock reference` after changing the format. -->\n\n",
    );
    let _ = writeln!(out, "{}\n", prose(&schema["description"]));
    out.push_str(
        "The format as JSON Schema: [`mocks.schema.json`](mocks.schema.json) (or\n\
         `chrome-cdp schema mocks`). Start a mock file with\n\n\
         ```toml\n\
         #:schema https://raw.githubusercontent.com/glotlabs/chrome-cdp/main/docs/e2e/mocks.schema.json\n\
         ```\n\n\
         and Taplo (Even Better TOML) checks and completes it in the editor.\n\
         Templates (`{{params.id}}`, `{{json.qty}}`, `{{uuid}}`, …) and how\n\
         matching works are described in plan 11 and\n\
         [scenarios.md](scenarios.md); `chrome-cdp mock explain` shows which\n\
         mock answers a request.\n\n",
    );
    out.push_str("## The file\n\n| Key | Type | What |\n|---|---|---|\n");
    let mut sections = Vec::new();
    for key in TABLES {
        let prop = &schema["properties"][key];
        let _ = writeln!(
            out,
            "| `{}` | {} | {} |",
            table_key(key, prop),
            kind(prop, defs, &mut sections),
            cell(&prop["description"])
        );
    }
    out.push('\n');
    let mut done = Vec::new();
    while !sections.is_empty() {
        let name = sections.remove(0);
        if done.contains(&name) {
            continue;
        }
        done.push(name.clone());
        section(&mut out, &name, defs, &mut sections);
    }
    out.truncate(out.trim_end().len());
    out.push('\n');
    out
}

/// `[settings]` or `[[mock]]`, as the key is written in TOML.
fn table_key(key: &str, prop: &Value) -> String {
    if prop["type"] == "array" {
        format!("[[{key}]]")
    } else {
        format!("[{key}]")
    }
}

/// A table's section: its description and its keys.
fn section(out: &mut String, name: &str, defs: &Value, sections: &mut Vec<String>) {
    let def = &defs[name];
    let description = prose(&def["description"]);
    let (title, rest) = split_title(&description, name);
    let _ = writeln!(out, "<a id=\"{}\"></a>\n\n## {title}\n", anchor(name));
    if !rest.is_empty() {
        let _ = writeln!(out, "{rest}\n");
    }
    let required: Vec<&str> = def["required"]
        .as_array()
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    out.push_str("| Key | Type | What |\n|---|---|---|\n");
    let Some(props) = def["properties"].as_object() else {
        return;
    };
    for (key, prop) in props {
        let need = if required.contains(&key.as_str()) {
            " (required)"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "| `{key}`{need} | {} | {} |",
            kind(prop, defs, sections),
            cell(&prop["description"])
        );
    }
    out.push('\n');
}

/// `` `[[mock]]`: a request… `` → (`` `[[mock]]` ``, `A request…`).
fn split_title(description: &str, name: &str) -> (String, String) {
    if let Some(rest) = description.strip_prefix('`')
        && let Some((code, after)) = rest.split_once('`')
    {
        let after = after.trim_start_matches([':', '.']).trim();
        let mut chars = after.chars();
        let after = chars
            .next()
            .map(|c| c.to_uppercase().chain(chars).collect())
            .unwrap_or_default();
        return (format!("`{code}`"), after);
    }
    (name.to_owned(), description.to_owned())
}

fn anchor(name: &str) -> String {
    name.to_lowercase()
}

/// A property's type, in words; tables it names are queued as sections.
fn kind(prop: &Value, defs: &Value, sections: &mut Vec<String>) -> String {
    if let Some(name) = reference_name(prop) {
        let def = &defs[name];
        if let Some(values) = def["enum"].as_array() {
            return values
                .iter()
                .filter_map(Value::as_str)
                .map(|v| format!("`\"{v}\"`"))
                .collect::<Vec<_>>()
                .join(" \\| ");
        }
        if def.get("properties").is_some() {
            sections.push(name.to_owned());
            return format!("table ([{}](#{}))", title_of(name, defs), anchor(name));
        }
        return kind(def, defs, sections);
    }
    if let Some(any) = prop["anyOf"].as_array() {
        let parts: Vec<String> = any
            .iter()
            .filter(|p| p["type"] != "null")
            .map(|p| {
                let k = kind(p, defs, sections);
                match p["description"].as_str() {
                    Some(d) if p.get("$ref").is_none() => format!("{k}: {}", prose_str(d)),
                    _ => k,
                }
            })
            .collect();
        return parts.join("; or ");
    }
    let types: Vec<&str> = match &prop["type"] {
        Value::String(t) => vec![t.as_str()],
        Value::Array(ts) => ts
            .iter()
            .filter_map(Value::as_str)
            .filter(|t| *t != "null")
            .collect(),
        _ => Vec::new(),
    };
    match types.as_slice() {
        [] => "any TOML value".into(),
        ["string"] => "text".into(),
        ["integer"] => "integer".into(),
        ["number"] => "number".into(),
        ["boolean"] => "boolean".into(),
        ["array"] => {
            let items = kind(&prop["items"], defs, sections);
            match items.strip_prefix("table ") {
                Some(link) => format!("list of tables {link}"),
                None => format!("list of {items}"),
            }
        }
        ["object"] => {
            if let Some(props) = prop["properties"].as_object() {
                let keys: Vec<String> = props.keys().map(|k| format!("{k} = …")).collect();
                format!("`{{ {} }}`", keys.join(", "))
            } else if prop["additionalProperties"].is_object() {
                format!(
                    "table of {}",
                    kind(&prop["additionalProperties"], defs, sections)
                )
            } else {
                "table of any values".into()
            }
        }
        other => other.join(" or "),
    }
}

fn reference_name(prop: &Value) -> Option<&str> {
    prop["$ref"].as_str()?.strip_prefix("#/$defs/")
}

fn title_of(name: &str, defs: &Value) -> String {
    split_title(&prose(&defs[name]["description"]), name).0
}

/// A description as one paragraph per blank-line-separated block.
fn prose(value: &Value) -> String {
    prose_str(value.as_str().unwrap_or_default())
}

fn prose_str(text: &str) -> String {
    text.split("\n\n")
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A description inside a table cell: one line, `|` escaped.
fn cell(value: &Value) -> String {
    prose(value).replace("\n\n", " ").replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    fn docs() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/e2e")
    }

    /// The committed reference and schema are what the code makes;
    /// `UPDATE_GOLDEN=1` writes them.
    #[test]
    fn the_reference_and_the_published_schema_are_up_to_date() {
        let schema = serde_json::to_string_pretty(&crate::schema()).unwrap_or_default() + "\n";
        let files = [
            ("mocks.md", super::reference()),
            ("mocks.schema.json", schema),
        ];
        let update = std::env::var_os("UPDATE_GOLDEN").is_some_and(|v| !v.is_empty() && v != "0");
        for (name, text) in files {
            let path = docs().join(name);
            if update {
                std::fs::write(&path, &text).unwrap_or_default();
            }
            let committed = std::fs::read_to_string(&path).unwrap_or_default();
            assert!(
                committed == text,
                "docs/e2e/{name} is out of date (UPDATE_GOLDEN=1 cargo test -p mock reference writes it)"
            );
        }
    }

    #[test]
    fn every_table_has_a_section() {
        let page = super::reference();
        for title in [
            "## `[settings]`",
            "## `[[mock]]`",
            "## `match`",
            "## `respond`",
            "## `[[collection]]`",
            "## `[[websocket]]`",
        ] {
            assert!(page.contains(title), "{title} missing:\n{page}");
        }
        assert!(page.contains("`\"fail\"` \\| `\"passthrough\"` \\| `\"abort\"`"));
    }
}
