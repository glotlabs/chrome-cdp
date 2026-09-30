//! Console arguments as text, formatted like DevTools prints them: the
//! first argument's `%s`/`%d`/`%o`/`%c` directives, then the rest joined
//! by spaces. Objects are shown from the `preview` Chrome sends with each
//! console call (`{a: 1, b: 'x'}`, `[1, 2, 3]`, `Map(1) {'k' => 1}`); they
//! are never fetched, so logging does not cost a round trip per entry.

use std::fmt::Write as _;
use std::sync::LazyLock;

use protocol::schema::StackFrame;
use regex::Regex;
use serde_json::Value;

/// How long a function's source may be in a message before it is cut.
const FUNCTION_LEN: usize = 80;

/// A console call's arguments as one message, and the stack of the first
/// `Error` among them (from its `stack` text), if any.
pub(crate) fn message(args: &[Value]) -> (String, Vec<StackFrame>) {
    let mut text = String::new();
    let mut rest = args;
    if let Some(first) = args.first()
        && first["type"] == "string"
    {
        let format = first["value"].as_str().unwrap_or_default();
        rest = &args[1..];
        text = directives(format, &mut rest);
    }
    for arg in rest {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&arg_text(arg));
    }
    let stack = args
        .iter()
        .find(|a| a["subtype"] == "error")
        .map(|e| parse_stack(e["description"].as_str().unwrap_or_default()))
        .unwrap_or_default();
    (text, stack)
}

/// Applies `format`'s `%` directives, taking their values from `args`.
fn directives(format: &str, args: &mut &[Value]) -> String {
    let mut out = String::with_capacity(format.len());
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let Some(&directive) = chars.peek() else {
            out.push('%');
            break;
        };
        if directive == '%' {
            chars.next();
            out.push('%');
            continue;
        }
        if !matches!(directive, 's' | 'd' | 'i' | 'f' | 'o' | 'O' | 'c') {
            out.push('%');
            continue;
        }
        chars.next();
        let Some((arg, rest)) = args.split_first() else {
            out.push('%');
            out.push(directive);
            continue;
        };
        *args = rest;
        match directive {
            'c' => {} // CSS styling: nothing to show.
            'd' | 'i' => out.push_str(&number(arg, true)),
            'f' => out.push_str(&number(arg, false)),
            _ => out.push_str(&arg_text(arg)),
        }
    }
    out
}

/// `%d`/`%i` (`integer`) and `%f` of a value.
fn number(arg: &Value, integer: bool) -> String {
    let Some(n) = arg["value"].as_f64() else {
        return match arg["unserializableValue"].as_str() {
            Some(v) => v.to_owned(),
            None => "NaN".to_owned(),
        };
    };
    if integer {
        // Truncates toward zero, like DevTools; in range for any log.
        #[allow(clippy::cast_possible_truncation)]
        let n = n.trunc() as i64;
        n.to_string()
    } else {
        n.to_string()
    }
}

/// One `Runtime.RemoteObject` as `console.log` prints it at the top level
/// (strings unquoted).
pub(crate) fn arg_text(object: &Value) -> String {
    match object["type"].as_str().unwrap_or_default() {
        "string" => object["value"].as_str().unwrap_or_default().to_owned(),
        "undefined" => "undefined".to_owned(),
        "function" => function_text(object["description"].as_str().unwrap_or_default()),
        "object" => match object["subtype"].as_str() {
            Some("null") => "null".to_owned(),
            Some("error") => error_message(object["description"].as_str().unwrap_or_default()),
            _ => match &object["preview"] {
                Value::Object(_) => preview_text(&object["preview"]),
                _ => description(object),
            },
        },
        _ => description(object),
    }
}

/// A primitive's or an object's own description.
fn description(object: &Value) -> String {
    if let Some(text) = object["description"].as_str() {
        return text.to_owned();
    }
    if let Some(value) = object["unserializableValue"].as_str() {
        return value.to_owned();
    }
    match &object["value"] {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        value => value.to_string(),
    }
}

/// `ƒ save(a, b)`: a function's first line, cut short.
fn function_text(source: &str) -> String {
    let line = source.lines().next().unwrap_or_default().trim();
    let line = line.strip_prefix("function ").unwrap_or(line);
    let line = line.trim_end_matches('{').trim_end();
    let mut text = format!("ƒ {line}");
    cut(&mut text, FUNCTION_LEN);
    text
}

/// An `Error`'s `stack` text without its `    at …` lines: the message,
/// which may span lines.
pub(crate) fn error_message(stack: &str) -> String {
    let lines: Vec<&str> = stack
        .lines()
        .take_while(|l| !l.trim_start().starts_with("at "))
        .collect();
    lines.join("\n")
}

/// Cuts `text` to `max` characters, ending in `…`.
pub(crate) fn cut(text: &mut String, max: usize) {
    if let Some((at, _)) = text.char_indices().nth(max) {
        text.truncate(at);
        text.push('…');
    }
}

/// An `ObjectPreview` as DevTools shows it: `{a: 1, b: 'x'}`, `Foo {a: 1}`,
/// `[1, 2]`, `Map(1) {'k' => 1}`, `Set(2) {1, 2}`; other kinds (dates,
/// nodes, regexps) by their description.
fn preview_text(preview: &Value) -> String {
    let description = preview["description"].as_str().unwrap_or_default();
    let overflow = preview["overflow"].as_bool() == Some(true);
    let properties = preview["properties"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let entries = preview["entries"].as_array().map_or(&[][..], Vec::as_slice);
    let mut items: Vec<String> = Vec::new();
    let (open, close, prefix) = match preview["subtype"].as_str() {
        Some("array" | "typedarray") => {
            items.extend(properties.iter().map(property_value));
            ("[", "]", String::new())
        }
        Some("map") => {
            items.extend(entries.iter().map(|e| {
                format!(
                    "{} => {}",
                    nested_preview(&e["key"]),
                    nested_preview(&e["value"])
                )
            }));
            ("{", "}", format!("{description} "))
        }
        Some("set") => {
            items.extend(entries.iter().map(|e| nested_preview(&e["value"])));
            ("{", "}", format!("{description} "))
        }
        None => {
            items.extend(properties.iter().map(|p| {
                format!(
                    "{}: {}",
                    key(p["name"].as_str().unwrap_or_default()),
                    property_value(p)
                )
            }));
            let prefix = if description == "Object" || description.is_empty() {
                String::new()
            } else {
                format!("{description} ")
            };
            ("{", "}", prefix)
        }
        Some(_) => return description.to_owned(),
    };
    if overflow {
        items.push("…".to_owned());
    }
    format!("{prefix}{open}{}{close}", items.join(", "))
}

/// A property name, quoted when it is not a plain identifier.
fn key(name: &str) -> String {
    static IDENT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^[A-Za-z_$][A-Za-z0-9_$]*$|^\d+$").unwrap_or_else(|_| unreachable!())
    });
    if IDENT.is_match(name) {
        name.to_owned()
    } else {
        quote(name)
    }
}

/// A `PropertyPreview`'s value, nested (strings quoted).
fn property_value(property: &Value) -> String {
    let value = property["value"].as_str().unwrap_or_default();
    match property["type"].as_str().unwrap_or_default() {
        "string" => quote(value),
        "function" => "ƒ".to_owned(),
        "accessor" => "(...)".to_owned(),
        "object" if property["subtype"] == "null" => "null".to_owned(),
        "object" => match &property["valuePreview"] {
            Value::Object(_) => preview_text(&property["valuePreview"]),
            _ if value == "Object" => "{…}".to_owned(),
            _ => value.to_owned(),
        },
        _ => value.to_owned(),
    }
}

/// An `EntryPreview`'s key or value (an `ObjectPreview` of anything).
fn nested_preview(preview: &Value) -> String {
    match preview["type"].as_str().unwrap_or_default() {
        "string" => quote(preview["description"].as_str().unwrap_or_default()),
        "object" if preview["subtype"] != "null" => preview_text(preview),
        _ => preview["description"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    }
}

/// `'text'`, as DevTools quotes nested strings.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for c in text.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// The frames of a V8 `stack` text (`    at fn (url:1:2)`), innermost
/// first. Frames without a URL (`<anonymous>`, native) are left out.
pub(crate) fn parse_stack(stack: &str) -> Vec<StackFrame> {
    static FRAME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^\s*at (?:(.+?) \()?(.+):(\d+):(\d+)\)?$").unwrap_or_else(|_| unreachable!())
    });
    stack
        .lines()
        .filter_map(|line| {
            let caps = FRAME.captures(line)?;
            Some(StackFrame {
                function: caps.get(1).map_or("", |m| m.as_str()).to_owned(),
                url: caps[2].to_owned(),
                line: caps[3].parse().ok()?,
                column: caps[4].parse().ok()?,
                original: None,
                file: None,
                async_boundary: None,
                locals: None,
            })
        })
        .collect()
}

/// The frames of a `Runtime.StackTrace`, innermost first, 1-based.
pub(crate) fn stack_frames(stack: &Value) -> Vec<StackFrame> {
    let frames = stack["callFrames"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    frames
        .iter()
        .filter_map(|f| {
            let url = f["url"].as_str().filter(|u| !u.is_empty())?;
            Some(StackFrame {
                function: f["functionName"].as_str().unwrap_or_default().to_owned(),
                url: url.to_owned(),
                line: one_based(&f["lineNumber"])?,
                column: one_based(&f["columnNumber"])?,
                original: None,
                file: None,
                async_boundary: None,
                locals: None,
            })
        })
        .collect()
}

/// A 0-based CDP line or column number, 1-based.
pub(crate) fn one_based(value: &Value) -> Option<u32> {
    value.as_u64().and_then(|n| u32::try_from(n + 1).ok())
}

/// `fn (place:12:5)` or `place:12:5`, for a stack line.
pub(crate) fn frame_text(function: &str, place: &str, line: u32, column: u32) -> String {
    let mut text = String::new();
    if function.is_empty() {
        let _ = write!(text, "{place}:{line}:{column}");
    } else {
        let _ = write!(text, "{function} ({place}:{line}:{column})");
    }
    text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{message, parse_stack};

    fn text(args: serde_json::Value) -> String {
        let serde_json::Value::Array(args) = args else {
            panic!("not an array")
        };
        message(&args).0
    }

    #[test]
    fn primitives_and_directives() {
        assert_eq!(
            text(json!([
                { "type": "string", "value": "saved %d of %s%%" },
                { "type": "number", "value": 3.7, "description": "3.7" },
                { "type": "string", "value": "5" },
                { "type": "boolean", "value": true },
                { "type": "undefined" },
                { "type": "object", "subtype": "null", "value": null },
                { "type": "bigint", "unserializableValue": "12n", "description": "12n" },
            ])),
            "saved 3 of 5% true undefined null 12n"
        );
        assert_eq!(
            text(json!([
                { "type": "string", "value": "%cstyled%c %o" },
                { "type": "string", "value": "color: red" },
                { "type": "string", "value": "" },
                { "type": "number", "value": 1, "description": "1" },
            ])),
            "styled 1"
        );
        // Too few values: the directive stays.
        assert_eq!(
            text(json!([{ "type": "string", "value": "%s and %d" }])),
            "%s and %d"
        );
    }

    #[test]
    fn objects_from_their_preview() {
        let object = json!({
            "type": "object", "description": "Object",
            "preview": { "type": "object", "description": "Object", "overflow": false, "properties": [
                { "name": "a", "type": "number", "value": "1" },
                { "name": "b", "type": "string", "value": "x'y" },
                { "name": "not ident", "type": "object", "value": "Object" },
                { "name": "list", "type": "object", "subtype": "array", "value": "Array(2)" },
                { "name": "f", "type": "function", "value": "" },
                { "name": "n", "type": "object", "subtype": "null", "value": "null" },
            ] },
        });
        assert_eq!(
            text(json!([object])),
            "{a: 1, b: 'x\\'y', 'not ident': {…}, list: Array(2), f: ƒ, n: null}"
        );
        let array = json!({
            "type": "object", "subtype": "array", "description": "Array(120)",
            "preview": { "type": "object", "subtype": "array", "description": "Array(120)", "overflow": true, "properties": [
                { "name": "0", "type": "number", "value": "1" },
                { "name": "1", "type": "string", "value": "two" },
            ] },
        });
        assert_eq!(text(json!([array])), "[1, 'two', …]");
        let map = json!({
            "type": "object", "subtype": "map", "description": "Map(1)",
            "preview": { "type": "object", "subtype": "map", "description": "Map(1)", "overflow": false,
                "properties": [],
                "entries": [{
                    "key": { "type": "string", "description": "k", "overflow": false, "properties": [] },
                    "value": { "type": "number", "description": "1", "overflow": false, "properties": [] },
                }],
            },
        });
        assert_eq!(text(json!([map])), "Map(1) {'k' => 1}");
        let instance = json!({
            "type": "object", "className": "Cart", "description": "Cart",
            "preview": { "type": "object", "description": "Cart", "overflow": false, "properties": [
                { "name": "items", "type": "number", "value": "2" },
            ] },
        });
        assert_eq!(text(json!([instance])), "Cart {items: 2}");
        let node = json!({
            "type": "object", "subtype": "node", "description": "div#app",
            "preview": { "type": "object", "subtype": "node", "description": "div#app", "overflow": false, "properties": [] },
        });
        assert_eq!(text(json!([node])), "div#app");
        let function =
            json!({ "type": "function", "description": "function save(a, b) {\n  return a;\n}" });
        assert_eq!(text(json!([function])), "ƒ save(a, b)");
    }

    #[test]
    fn errors_give_their_message_and_stack() {
        let (text, stack) = message(&[
            json!({ "type": "string", "value": "save failed:" }),
            json!({
                "type": "object", "subtype": "error", "description":
                "TypeError: x is undefined\n    at save (http://x/app.js:3:10)\n    at http://x/app.js:9:1\n    at <anonymous>",
            }),
        ]);
        assert_eq!(text, "save failed: TypeError: x is undefined");
        assert_eq!(stack.len(), 2);
        assert_eq!(stack[0].function, "save");
        assert_eq!((stack[0].line, stack[0].column), (3, 10));
        assert_eq!(stack[1].function, "");
        assert_eq!(stack[1].url, "http://x/app.js");
    }

    #[test]
    fn stack_lines() {
        let frames = parse_stack(
            "Error: x\n    at async load (http://h:1/a.js:1:2)\n    at new Foo (http://h:1/b.js:3:4)",
        );
        assert_eq!(frames[0].function, "async load");
        assert_eq!(frames[0].url, "http://h:1/a.js");
        assert_eq!(frames[1].function, "new Foo");
        assert_eq!((frames[1].line, frames[1].column), (3, 4));
    }
}
