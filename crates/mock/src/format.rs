//! `chrome-cdp fmt` for mock files (plan 11, milestone 11.12).
//!
//! [`format`] edits the file with `toml_edit`, so comments and the
//! author's layout of values survive:
//!
//! - the keys of each `[[mock]]` in the order name, match, respond,
//!   sequence (then the rest as [`file`](crate::file) lists them), and
//!   likewise for `[[collection]]` and `[[websocket]]`; a comment above a
//!   key moves with it;
//! - `key = value` with one space around `=`, keys and comments not
//!   indented;
//! - inline tables and arrays kept inline; one written on one line
//!   without comments is spaced as `{ a = 1, b = [1, 2] }`;
//! - one blank line before each table header (and the comments right
//!   above it), none at the start, one line break at the end.
//!
//! The values before and after must be the same, or it fails.

use toml_edit::{DocumentMut, Item, Key, RawString, Table, Value};

/// The key order of each kind of table: known keys in this order, others
/// after them as written.
const ORDERS: &[(&str, &[&str])] = &[
    (
        "mock",
        &[
            "name",
            "match",
            "respond",
            "sequence",
            "after_sequence",
            "when",
            "set_state",
            "unset_state",
        ],
    ),
    (
        "collection",
        &[
            "name", "path", "params", "id", "seed", "items", "new_id", "list",
        ],
    ),
    (
        "websocket",
        &["name", "url", "protocol", "on_open", "on_message"],
    ),
];

/// `src` formatted, or why it is not a TOML file.
pub fn format(src: &str) -> Result<String, String> {
    let mut doc: DocumentMut = src.parse().map_err(|e: toml_edit::TomlError| {
        e.to_string()
            .lines()
            .next()
            .unwrap_or("not TOML")
            .to_owned()
    })?;
    let root = doc.as_table_mut();
    let root_has_values = root.iter().any(|(_, item)| item.is_value());
    tidy_values(root);
    let first = first_position(root);
    for (key, item) in root.iter_mut() {
        let order = ORDERS
            .iter()
            .find(|(name, _)| *name == key.get())
            .map(|(_, order)| *order);
        tidy_tables(item, order, !root_has_values, first);
    }
    // One line break at the end; comments after the last value stay.
    let trailing = doc.trailing().as_str().unwrap_or("").to_owned();
    doc.set_trailing(squeeze(&trailing, Blank::Keep));
    let mut out = doc.to_string();
    let end = out.trim_end().len();
    out.truncate(end);
    if !out.is_empty() {
        out.push('\n');
    }
    let before: toml::Table = toml::from_str(src).map_err(|e| e.to_string())?;
    let after: Result<toml::Table, _> = toml::from_str(&out);
    if after.ok().as_ref() != Some(&before) {
        return Err(
            "formatting would change the file's values; this is a chrome-cdp bug, please report it"
                .into(),
        );
    }
    Ok(out)
}

/// The smallest position among the tables below `table`: the first
/// header of the file.
fn first_position(table: &Table) -> Option<isize> {
    let mut min: Option<isize> = None;
    let mut see = |p: Option<isize>| {
        if let Some(p) = p {
            min = Some(min.map_or(p, |m| m.min(p)));
        }
    };
    for (_, item) in table {
        match item {
            Item::Table(t) => {
                if !t.is_implicit() {
                    see(t.position());
                }
                see(first_position(t));
            }
            Item::ArrayOfTables(array) => {
                for t in array {
                    see(t.position());
                    see(first_position(t));
                }
            }
            _ => {}
        }
    }
    min
}

/// Tidies the tables in `item` (a table or an array of tables): their
/// headers, key order (`order`) and values. `first` is the position of
/// the file's first header, which gets no blank line before it when
/// `at_start` (nothing precedes the headers).
fn tidy_tables(item: &mut Item, order: Option<&[&str]>, at_start: bool, first: Option<isize>) {
    let tidy = |table: &mut Table| {
        if let Some(order) = order {
            table.sort_values_by(|a, _, b, _| rank(order, a).cmp(&rank(order, b)));
        }
        if !table.is_implicit() && !table.is_dotted() {
            let is_first = at_start && table.position() == first;
            let prefix = table
                .decor()
                .prefix()
                .and_then(RawString::as_str)
                .unwrap_or("")
                .to_owned();
            let blank = if is_first { Blank::None } else { Blank::One };
            table.decor_mut().set_prefix(squeeze(&prefix, blank));
        }
        tidy_values(table);
        for (_, sub) in table.iter_mut() {
            if matches!(sub, Item::Table(_) | Item::ArrayOfTables(_)) {
                tidy_tables(sub, None, at_start, first);
            }
        }
    };
    match item {
        Item::Table(table) => tidy(table),
        Item::ArrayOfTables(array) => {
            for table in array.iter_mut() {
                tidy(table);
            }
        }
        _ => {}
    }
}

fn rank(order: &[&str], key: &Key) -> usize {
    order
        .iter()
        .position(|k| *k == key.get())
        .unwrap_or(order.len())
}

/// `key = value` spacing for the values of `table`.
fn tidy_values(table: &mut Table) {
    for (i, (mut key, item)) in table.iter_mut().enumerate() {
        let Some(value) = item.as_value_mut() else {
            continue;
        };
        let prefix = key
            .leaf_decor()
            .prefix()
            .and_then(RawString::as_str)
            .unwrap_or("")
            .to_owned();
        let blank = if i == 0 { Blank::None } else { Blank::Keep };
        key.leaf_decor_mut().set_prefix(squeeze(&prefix, blank));
        key.leaf_decor_mut().set_suffix(" ");
        let suffix = value
            .decor()
            .suffix()
            .and_then(RawString::as_str)
            .unwrap_or("")
            .to_owned();
        value.decor_mut().set_prefix(" ");
        value.decor_mut().set_suffix(suffix.trim_end().to_owned());
        tidy_inline(value);
    }
}

/// Spaces an inline table or array written on one line without comments.
fn tidy_inline(value: &mut Value) {
    let one_line = |v: &Value| {
        let text = v.to_string();
        !text.contains(['\n', '#'])
    };
    match value {
        Value::InlineTable(table) => {
            let mut bare = Value::InlineTable(table.clone());
            bare.decor_mut().clear();
            if !one_line(&bare) {
                return;
            }
            table.fmt();
            for (_, v) in table.iter_mut() {
                tidy_inline(v);
            }
        }
        Value::Array(array) => {
            let mut bare = Value::Array(array.clone());
            bare.decor_mut().clear();
            if !one_line(&bare) {
                return;
            }
            array.fmt();
            for v in array.iter_mut() {
                tidy_inline(v);
            }
        }
        _ => {}
    }
}

/// Blank lines before a key or header.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Blank {
    /// None.
    None,
    /// One if there were any.
    Keep,
    /// Exactly one.
    One,
}

/// The text before a key or header (or at the end of the file): its
/// comment lines, not indented, with at most one blank line between
/// them, and `blank` lines first.
fn squeeze(prefix: &str, blank: Blank) -> String {
    let mut out = String::new();
    let mut blank_pending = false;
    let mut leading = false;
    let lines: Vec<&str> = prefix.split('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let line = line.trim();
        let last = i + 1 == lines.len();
        if line.is_empty() {
            // The last piece is the key's own line, before the key.
            if !last {
                if out.is_empty() {
                    leading = true;
                } else {
                    blank_pending = true;
                }
            }
            continue;
        }
        if blank_pending {
            out.push('\n');
            blank_pending = false;
        }
        out.push_str(line);
        out.push('\n');
    }
    // Comments set apart from the key or header stay apart.
    if blank_pending {
        out.push('\n');
    }
    if blank == Blank::One || (blank == Blank::Keep && leading) {
        out.insert(0, '\n');
    }
    out
}

#[cfg(test)]
mod tests;
