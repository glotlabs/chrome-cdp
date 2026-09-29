//! Whether a release only adds to the public API (docs/output.md,
//! "Versions and stability"): `release check` compares each golden
//! `--json` schema (`crates/chrome-cdp/tests/golden/schema/`) and the
//! command line (`golden/cli.txt`) with the previous release tag's, and
//! lists what a consumer of the old ones would trip over.
//!
//! Breaking, in a schema: a removed property, a changed type (any change
//! to the set of JSON types a value may have), a new required property or
//! one no longer required, a removed enum value or `oneOf`/`anyOf`
//! variant. On the command line: a removed command, argument, flag, short
//! flag, value of a flag or environment variable; a flag that stops or
//! starts taking a value or stops being repeatable; a new required
//! argument or flag.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::process::Command;

use crate::Result;
use crate::json::{self, Json};

/// Where the golden files live, from the workspace root.
const SCHEMA_DIR: &str = "crates/chrome-cdp/tests/golden/schema";
const SURFACE: &str = "crates/chrome-cdp/tests/golden/cli.txt";

/// What `release check` found.
#[derive(Debug, Default)]
pub(crate) struct Report {
    /// Breaking changes, as `where: what`.
    pub(crate) breaks: Vec<String>,
    /// What could not be compared.
    pub(crate) notes: Vec<String>,
    /// How many schemas were compared.
    pub(crate) schemas: usize,
}

/// Compares the working tree's golden files with `previous`'s.
pub(crate) fn compare_with(root: &Path, previous: &str) -> Result<Report> {
    let mut report = Report::default();
    let listed = git(
        root,
        &[
            "ls-tree",
            "--name-only",
            previous,
            &format!("{SCHEMA_DIR}/"),
        ],
    )
    .ok_or_else(|| format!("cannot list {SCHEMA_DIR} at {previous}"))?;
    let old_files: Vec<&str> = listed
        .lines()
        .filter(|l| Path::new(l).extension().is_some_and(|e| e == "json"))
        .collect();
    if old_files.is_empty() {
        report.notes.push(format!(
            "{previous} has no golden schemas: they are not compared"
        ));
    }
    for path in old_files {
        let name = command_of(path);
        let old = git(root, &["show", &format!("{previous}:{path}")])
            .ok_or_else(|| format!("cannot read {path} at {previous}"))?;
        let old = json::parse(&old).map_err(|e| format!("{path} at {previous}: {e}"))?;
        let Ok(new) = std::fs::read_to_string(root.join(path)) else {
            report.breaks.push(format!(
                "schema {name}: removed (the command has no --json schema)"
            ));
            continue;
        };
        let new = json::parse(&new).map_err(|e| format!("{path}: {e}"))?;
        report.schemas += 1;
        report.breaks.extend(
            schema_breaks(&old, &new)
                .into_iter()
                .map(|b| format!("schema {name}: {b}")),
        );
    }
    match git(root, &["show", &format!("{previous}:{SURFACE}")]) {
        Some(old) => {
            let new = std::fs::read_to_string(root.join(SURFACE))
                .map_err(|e| format!("read {SURFACE}: {e}"))?;
            report.breaks.extend(surface_breaks(&old, &new));
        }
        None => report.notes.push(format!(
            "{previous} has no {SURFACE}: the command line is not compared"
        )),
    }
    Ok(report)
}

/// `…/schema/tab_close.json` → `tab close`.
fn command_of(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.trim_end_matches(".json").replace('_', " ")
}

/// A git command's stdout, or `None` when it fails.
fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Whether going from `previous` to `current` may break the API: a new
/// major version, or before 1.0 a new minor one (Cargo's rule for 0.y.z).
pub(crate) fn breaking_allowed(previous: &str, current: &str) -> bool {
    let parse = |v: &str| -> Option<(u64, u64)> {
        let core = v.trim_start_matches('v').split(['-', '+']).next()?;
        let mut parts = core.split('.');
        Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
    };
    match (parse(previous), parse(current)) {
        (Some((0, old_minor)), Some((0, new_minor))) => new_minor > old_minor,
        (Some((old_major, _)), Some((new_major, _))) => new_major > old_major,
        _ => false,
    }
}

// --- Schemas ---------------------------------------------------------------

/// What in `new` breaks a reader of `old`, as `path: what`.
pub(crate) fn schema_breaks(old: &Json, new: &Json) -> Vec<String> {
    let mut walk = Walk {
        old_root: old,
        new_root: new,
        seen: HashSet::new(),
        breaks: Vec::new(),
    };
    walk.compare("", old, new);
    walk.breaks
}

struct Walk<'a> {
    old_root: &'a Json,
    new_root: &'a Json,
    /// `$ref` pairs already compared: recursive types end here.
    seen: HashSet<(String, String)>,
    breaks: Vec<String>,
}

impl<'a> Walk<'a> {
    fn report(&mut self, path: &str, what: &str) {
        let at = if path.is_empty() { "(root)" } else { path };
        self.breaks.push(format!("{at}: {what}"));
    }

    fn compare(&mut self, path: &str, old: &'a Json, new: &'a Json) {
        let (old, old_ref) = resolve(self.old_root, old);
        let (new, new_ref) = resolve(self.new_root, new);
        if let (Some(o), Some(n)) = (old_ref, new_ref)
            && !self.seen.insert((o, n))
        {
            return;
        }
        let old_types = types(self.old_root, old, 0);
        let new_types = types(self.new_root, new, 0);
        if old_types != new_types {
            self.report(
                path,
                &format!(
                    "type changed from {} to {}",
                    join(&old_types),
                    join(&new_types)
                ),
            );
            return;
        }
        if let (Some(old_values), Some(new_values)) = (values(old), values(new)) {
            for value in old_values.iter().filter(|v| !new_values.contains(v)) {
                self.report(path, &format!("value {} removed", value.to_text()));
            }
        }
        self.properties(path, old, new);
        if let (Some(o), Some(n)) = (old.get("items"), new.get("items")) {
            self.compare(&format!("{path}[]"), o, n);
        }
        if let (Some(o @ Json::Object(_)), Some(n @ Json::Object(_))) = (
            old.get("additionalProperties"),
            new.get("additionalProperties"),
        ) {
            self.compare(&format!("{path}{{}}"), o, n);
        }
        self.variants(path, old, new);
    }

    fn properties(&mut self, path: &str, old: &'a Json, new: &'a Json) {
        let empty: &'a [(String, Json)] = &[];
        let old_props = old
            .get("properties")
            .and_then(Json::as_object)
            .unwrap_or(empty);
        let new_props = new
            .get("properties")
            .and_then(Json::as_object)
            .unwrap_or(empty);
        let old_required = required(old);
        let new_required = required(new);
        for (name, old_prop) in old_props {
            let at = format!("{path}.{name}");
            match new_props.iter().find(|(n, _)| n == name) {
                None => self.report(&at, "property removed"),
                Some((_, new_prop)) => {
                    if old_required.contains(name.as_str()) && !new_required.contains(name.as_str())
                    {
                        self.report(&at, "no longer required (it may be absent)");
                    }
                    self.compare(&at, old_prop, new_prop);
                }
            }
        }
        for name in new_required.difference(&old_required) {
            let at = format!("{path}.{name}");
            if old_props.iter().any(|(n, _)| n == name) {
                self.report(&at, "now required");
            } else {
                self.report(&at, "new required property");
            }
        }
    }

    /// `oneOf`/`anyOf` variants, paired by their key (`variant_key`);
    /// a node without them counts as its own single variant.
    fn variants(&mut self, path: &str, old: &'a Json, new: &'a Json) {
        let (old_list, new_list) = match (variants(old), variants(new)) {
            (None, None) => return,
            (o, n) => (
                o.unwrap_or_else(|| vec![old]),
                n.unwrap_or_else(|| vec![new]),
            ),
        };
        if let ([o], [n]) = (old_list.as_slice(), new_list.as_slice()) {
            self.compare(path, o, n);
            return;
        }
        let new_keys: Vec<String> = new_list
            .iter()
            .map(|v| variant_key(self.new_root, v))
            .collect();
        let mut used = vec![false; new_list.len()];
        for o in old_list {
            let key = variant_key(self.old_root, o);
            let found = (0..new_list.len()).find(|&i| !used[i] && new_keys[i] == key);
            match found {
                Some(i) => {
                    used[i] = true;
                    self.compare(&format!("{path}({key})"), o, new_list[i]);
                }
                None if key.starts_with('=') => {
                    self.report(path, &format!("value {} removed", &key[1..]));
                }
                None => self.report(path, &format!("variant ({key}) removed")),
            }
        }
    }
}

/// Follows `$ref`s into `$defs`: the schema and the last name followed.
fn resolve<'a>(root: &'a Json, mut node: &'a Json) -> (&'a Json, Option<String>) {
    let mut name = None;
    for _ in 0..32 {
        let Some(target) = node.get("$ref").and_then(Json::as_str) else {
            break;
        };
        let Some(def) = target.strip_prefix("#/$defs/") else {
            break;
        };
        let Some(found) = root.get("$defs").and_then(|d| d.get(def)) else {
            break;
        };
        name = Some(def.to_owned());
        node = found;
    }
    (node, name)
}

/// The JSON types a value of `node` may have (`any` for `true`/`{}`).
fn types(root: &Json, node: &Json, depth: usize) -> BTreeSet<String> {
    let (node, _) = resolve(root, node);
    let mut out = BTreeSet::new();
    if depth > 16 {
        return out;
    }
    match node.get("type") {
        Some(Json::String(t)) => {
            out.insert(t.clone());
        }
        Some(Json::Array(list)) => {
            out.extend(list.iter().filter_map(Json::as_str).map(str::to_owned));
        }
        _ => {
            if let Some(list) = variants(node) {
                for v in list {
                    out.extend(types(root, v, depth + 1));
                }
            } else if let Some(value) = node.get("const") {
                out.insert(type_of(value).to_owned());
            } else if node.get("properties").is_some() {
                out.insert("object".into());
            } else {
                out.insert("any".into());
            }
        }
    }
    // An integer is a number: `number` covers both.
    if out.contains("number") {
        out.remove("integer");
    }
    out
}

fn type_of(value: &Json) -> &'static str {
    match value {
        Json::Null => "null",
        Json::Bool(_) => "boolean",
        Json::Number(n) if n.contains(['.', 'e', 'E']) => "number",
        Json::Number(_) => "integer",
        Json::String(_) => "string",
        Json::Array(_) => "array",
        Json::Object(_) => "object",
    }
}

/// The values `node` allows, when it lists them (`enum`, `const`).
fn values(node: &Json) -> Option<Vec<&Json>> {
    if let Some(list) = node.get("enum").and_then(Json::as_array) {
        return Some(list.iter().collect());
    }
    node.get("const").map(|c| vec![c])
}

fn variants(node: &Json) -> Option<Vec<&Json>> {
    let list: Vec<&Json> = ["oneOf", "anyOf"]
        .iter()
        .filter_map(|k| node.get(k).and_then(Json::as_array))
        .flatten()
        .collect();
    (!list.is_empty()).then_some(list)
}

/// What pairs a variant with its counterpart: `=value` for a `const`,
/// the `const` properties of an object (a tagged union's `type=…`), else
/// its `$ref` name, else its types.
fn variant_key(root: &Json, node: &Json) -> String {
    let (resolved, name) = resolve(root, node);
    if let Some(value) = resolved.get("const") {
        return format!("={}", value.to_text());
    }
    if let Some(props) = resolved.get("properties").and_then(Json::as_object) {
        let tags: Vec<String> = props
            .iter()
            .filter_map(|(k, p)| p.get("const").map(|c| format!("{k}={}", c.to_text())))
            .collect();
        if !tags.is_empty() {
            return tags.join(",");
        }
    }
    name.unwrap_or_else(|| join(&types(root, resolved, 0)))
}

fn required(node: &Json) -> BTreeSet<&str> {
    node.get("required")
        .and_then(Json::as_array)
        .map(|list| list.iter().filter_map(Json::as_str).collect())
        .unwrap_or_default()
}

fn join(set: &BTreeSet<String>) -> String {
    set.iter().cloned().collect::<Vec<_>>().join("|")
}

// --- The command line --------------------------------------------------------

/// A command in `cli.txt`.
#[derive(Debug, Default)]
struct Cmd {
    args: Vec<Arg>,
    flags: Vec<Flag>,
}

#[derive(Debug, Default)]
struct Arg {
    name: String,
    required: bool,
    multiple: bool,
    values: Vec<String>,
}

#[derive(Debug, Default)]
struct Flag {
    long: Option<String>,
    short: Option<String>,
    value: Option<String>,
    values: Vec<String>,
    required: bool,
    repeatable: bool,
    env: Option<String>,
}

impl Flag {
    fn name(&self) -> &str {
        self.long
            .as_deref()
            .or(self.short.as_deref())
            .unwrap_or("?")
    }
}

/// `cli.txt` → its commands, by their words (`chrome-cdp tab close`).
fn parse_surface(text: &str) -> BTreeMap<String, Cmd> {
    let mut commands: BTreeMap<String, Cmd> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix("command ") {
            commands.entry(name.to_owned()).or_default();
            current = Some(name.to_owned());
            continue;
        }
        let Some(cmd) = current.as_ref().and_then(|c| commands.get_mut(c)) else {
            continue;
        };
        let mut words = line.split_whitespace();
        match words.next() {
            Some("arg") => {
                let mut arg = Arg {
                    name: words.next().unwrap_or_default().to_owned(),
                    ..Arg::default()
                };
                for word in words {
                    match word {
                        "required" => arg.required = true,
                        "multiple" => arg.multiple = true,
                        w => {
                            if let Some(v) = w.strip_prefix("values=") {
                                arg.values = v.split('|').map(str::to_owned).collect();
                            }
                        }
                    }
                }
                cmd.args.push(arg);
            }
            Some("flag") => {
                let mut flag = Flag::default();
                for word in words {
                    match word {
                        "required" => flag.required = true,
                        "repeatable" => flag.repeatable = true,
                        w if w.starts_with("--") => flag.long = Some(w.to_owned()),
                        w if w.starts_with('-') => flag.short = Some(w.to_owned()),
                        w => {
                            if let Some(v) = w.strip_prefix("value=") {
                                flag.value = Some(v.to_owned());
                            } else if let Some(v) = w.strip_prefix("values=") {
                                flag.values = v.split('|').map(str::to_owned).collect();
                            } else if let Some(v) = w.strip_prefix("env=") {
                                flag.env = Some(v.to_owned());
                            }
                        }
                    }
                }
                cmd.flags.push(flag);
            }
            _ => {}
        }
    }
    commands
}

/// What in the `new` command line breaks a user of the `old` one.
pub(crate) fn surface_breaks(old: &str, new: &str) -> Vec<String> {
    let old = parse_surface(old);
    let new = parse_surface(new);
    let root = new.get("chrome-cdp");
    let mut breaks = Vec::new();
    for (name, old_cmd) in &old {
        let Some(new_cmd) = new.get(name) else {
            breaks.push(format!("`{name}`: command removed"));
            continue;
        };
        let mut report = |what: String| breaks.push(format!("`{name}`: {what}"));
        for (i, old_arg) in old_cmd.args.iter().enumerate() {
            let arg = &old_arg.name;
            let Some(new_arg) = new_cmd.args.get(i) else {
                report(format!("argument {arg} removed"));
                continue;
            };
            if new_arg.required && !old_arg.required {
                report(format!("argument {arg} is now required"));
            }
            if old_arg.multiple && !new_arg.multiple {
                report(format!("argument {arg} no longer takes several values"));
            }
            if !new_arg.values.is_empty() {
                for value in old_arg
                    .values
                    .iter()
                    .filter(|v| !new_arg.values.contains(v))
                {
                    report(format!("argument {arg} no longer takes the value {value}"));
                }
            }
        }
        for new_arg in new_cmd.args.iter().skip(old_cmd.args.len()) {
            if new_arg.required {
                report(format!("new required argument {}", new_arg.name));
            }
        }
        for old_flag in &old_cmd.flags {
            let flag = old_flag.name();
            let same = |f: &&Flag| match &old_flag.long {
                Some(long) => f.long.as_ref() == Some(long),
                None => f.short == old_flag.short,
            };
            // A flag that became global is listed under `chrome-cdp`.
            let found = new_cmd
                .flags
                .iter()
                .find(same)
                .or_else(|| root.and_then(|r| r.flags.iter().find(same)));
            let Some(new_flag) = found else {
                report(format!("flag {flag} removed"));
                continue;
            };
            if let Some(short) = &old_flag.short
                && new_flag.short.as_ref() != Some(short)
            {
                report(format!("flag {flag}: its short form {short} removed"));
            }
            match (&old_flag.value, &new_flag.value) {
                (Some(_), None) => report(format!("flag {flag} no longer takes a value")),
                (None, Some(_)) => report(format!("flag {flag} now takes a value")),
                _ => {}
            }
            if !new_flag.values.is_empty() {
                for value in old_flag
                    .values
                    .iter()
                    .filter(|v| !new_flag.values.contains(v))
                {
                    report(format!("flag {flag} no longer takes the value {value}"));
                }
            }
            if new_flag.required && !old_flag.required {
                report(format!("flag {flag} is now required"));
            }
            if old_flag.repeatable && !new_flag.repeatable {
                report(format!("flag {flag} is no longer repeatable"));
            }
            if let Some(env) = &old_flag.env
                && new_flag.env.as_ref() != Some(env)
            {
                report(format!(
                    "flag {flag}: its environment variable {env} removed"
                ));
            }
        }
        for new_flag in &new_cmd.flags {
            let existed = old_cmd.flags.iter().any(|f| {
                f.long.is_some() && f.long == new_flag.long
                    || f.long.is_none() && f.short == new_flag.short
            });
            if new_flag.required && !existed {
                report(format!("new required flag {}", new_flag.name()));
            }
        }
    }
    breaks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn breaks(old: &str, new: &str) -> Vec<String> {
        schema_breaks(&json::parse(old).unwrap(), &json::parse(new).unwrap())
    }

    /// A schema like the golden ones: the envelope, `data`'s type in `$defs`.
    fn envelope(data: &str, defs: &str) -> String {
        format!(
            r##"{{"anyOf":[{{"$ref":"#/$defs/Ok"}},{{"$ref":"#/$defs/Err"}}],
               "$defs":{{"Ok":{{"type":"object","required":["ok","data"],"properties":{{
                   "ok":{{"type":"boolean"}},"data":{data}}}}},
                 "Err":{{"type":"object","required":["ok"],"properties":{{"ok":{{"type":"boolean"}}}}}}
                 {defs}}}}}"##
        )
    }

    #[test]
    fn same_schema_has_no_breaks() {
        let schema = envelope(
            r##"{"$ref":"#/$defs/D"}"##,
            r#","D":{"type":"object","properties":{"a":{"type":"string"}},"required":["a"]}"#,
        );
        assert!(breaks(&schema, &schema).is_empty());
    }

    #[test]
    fn additions_are_not_breaks() {
        let old = r#"{"type":"object","properties":{"a":{"type":"string"},"k":{"enum":["x"]}},"required":["a"]}"#;
        let new = r#"{"type":"object","description":"new words","properties":{"a":{"type":"string","description":"doc"},
            "k":{"enum":["x","y"]},"b":{"type":["integer","null"]}},"required":["a"]}"#;
        assert_eq!(breaks(old, new), Vec::<String>::new());
    }

    #[test]
    fn removed_or_renamed_property_breaks() {
        let old = r#"{"type":"object","properties":{"url":{"type":"string"}}}"#;
        let new = r#"{"type":"object","properties":{"href":{"type":"string"}}}"#;
        assert_eq!(breaks(old, new), [".url: property removed"]);
    }

    #[test]
    fn changed_type_breaks() {
        let old =
            r#"{"type":"object","properties":{"n":{"type":"integer"},"s":{"type":"string"}}}"#;
        let new = r#"{"type":"object","properties":{"n":{"type":"string"},"s":{"type":["string","null"]}}}"#;
        assert_eq!(
            breaks(old, new),
            [
                ".n: type changed from integer to string",
                ".s: type changed from string to null|string"
            ]
        );
        // integer → number widens within JSON's numbers, still a change;
        // number → number with integer alongside is the same set.
        assert_eq!(
            breaks(r#"{"type":"number"}"#, r#"{"type":["number","integer"]}"#),
            Vec::<String>::new()
        );
    }

    #[test]
    fn type_changes_are_found_through_refs() {
        let old = envelope(
            r##"{"$ref":"#/$defs/D"}"##,
            r#","D":{"type":"object","properties":{"at":{"type":"integer"}}}"#,
        );
        let new = envelope(
            r##"{"$ref":"#/$defs/D2"}"##,
            r#","D2":{"type":"object","properties":{"at":{"type":"string"}}}"#,
        );
        assert_eq!(
            breaks(&old, &new),
            ["(Ok).data.at: type changed from integer to string"]
        );
    }

    #[test]
    fn required_changes_break() {
        let old = r#"{"type":"object","properties":{"a":{"type":"string"},"b":{"type":"string"}},"required":["a"]}"#;
        let new = r#"{"type":"object","properties":{"a":{"type":"string"},"b":{"type":"string"},"c":{"type":"string"}},"required":["b","c"]}"#;
        assert_eq!(
            breaks(old, new),
            [
                ".a: no longer required (it may be absent)",
                ".b: now required",
                ".c: new required property"
            ]
        );
    }

    #[test]
    fn removed_enum_value_breaks() {
        let old = r#"{"type":"string","enum":["error","warn","info"]}"#;
        let new = r#"{"type":"string","enum":["error","info","debug"]}"#;
        assert_eq!(breaks(old, new), [r#"(root): value "warn" removed"#]);
    }

    #[test]
    fn removed_const_variant_breaks() {
        let old = r#"{"oneOf":[{"type":"string","const":"NotFound"},{"type":"string","const":"Timeout"}]}"#;
        let new = r#"{"oneOf":[{"type":"string","const":"Timeout"},{"type":"string","const":"Obscured"}]}"#;
        assert_eq!(breaks(old, new), [r#"(root): value "NotFound" removed"#]);
    }

    #[test]
    fn tagged_variants_pair_by_their_tag() {
        let old = r#"{"oneOf":[
            {"type":"object","properties":{"type":{"type":"string","const":"tab_opened"},"tab":{"type":"string"}},"required":["type","tab"]},
            {"type":"object","properties":{"type":{"type":"string","const":"dialog"},"message":{"type":"string"}}}]}"#;
        let new = r#"{"oneOf":[
            {"type":"object","properties":{"type":{"type":"string","const":"download"}}},
            {"type":"object","properties":{"type":{"type":"string","const":"dialog"},"text":{"type":"string"}}},
            {"type":"object","properties":{"type":{"type":"string","const":"tab_opened"},"tab":{"type":"integer"}},"required":["type","tab"]}]}"#;
        assert_eq!(
            breaks(old, new),
            [
                r#"(type="tab_opened").tab: type changed from string to integer"#,
                r#"(type="dialog").message: property removed"#
            ]
        );
        let fewer = r#"{"oneOf":[{"type":"object","properties":{"type":{"type":"string","const":"dialog"},"message":{"type":"string"}}},
            {"type":"object","properties":{"type":{"type":"string","const":"x"}}}]}"#;
        assert_eq!(
            breaks(old, fewer),
            [r#"(root): variant (type="tab_opened") removed"#]
        );
    }

    #[test]
    fn nullable_ref_becoming_required_ref_is_a_type_change() {
        let old = r##"{"type":"object","properties":{"p":{"anyOf":[{"$ref":"#/$defs/P"},{"type":"null"}]}},
            "$defs":{"P":{"type":"object","properties":{"x":{"type":"number"}}}}}"##;
        let new = r##"{"type":"object","properties":{"p":{"$ref":"#/$defs/P"}},
            "$defs":{"P":{"type":"object","properties":{"x":{"type":"number"}}}}}"##;
        assert_eq!(
            breaks(old, new),
            [".p: type changed from null|object to object"]
        );
        // Same types: the object inside is still compared.
        let new = r##"{"type":"object","properties":{"p":{"anyOf":[{"$ref":"#/$defs/P"},{"type":"null"}]}},
            "$defs":{"P":{"type":"object","properties":{}}}}"##;
        assert_eq!(breaks(old, new), [".p(P).x: property removed"]);
    }

    #[test]
    fn items_and_maps_are_compared() {
        let old = r#"{"type":"object","properties":{
            "list":{"type":"array","items":{"type":"object","properties":{"a":{"type":"string"}}}},
            "map":{"type":"object","additionalProperties":{"type":"integer"}}}}"#;
        let new = r#"{"type":"object","properties":{
            "list":{"type":"array","items":{"type":"object","properties":{}}},
            "map":{"type":"object","additionalProperties":{"type":"string"}}}}"#;
        assert_eq!(
            breaks(old, new),
            [
                ".list[].a: property removed",
                ".map{}: type changed from integer to string"
            ]
        );
    }

    #[test]
    fn recursive_types_end() {
        let tree = r##"{"$ref":"#/$defs/Node","$defs":{"Node":{"type":"object","properties":{
            "children":{"type":"array","items":{"$ref":"#/$defs/Node"}}}}}}"##;
        assert!(breaks(tree, tree).is_empty());
    }

    #[test]
    fn the_golden_schemas_have_no_breaks_against_themselves() {
        let dir = crate::workspace_root().join(SCHEMA_DIR);
        for entry in std::fs::read_dir(dir).unwrap() {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            let schema = json::parse(&text).unwrap();
            assert!(schema_breaks(&schema, &schema).is_empty());
        }
    }

    const SURFACE_OLD: &str = "\
# comment
command chrome-cdp
  flag --json env=CHROME_CDP_JSON
  flag --quiet -q env=CHROME_CDP_QUIET
command chrome-cdp click
  arg LOCATOR required
  flag --button value=BUTTON values=left|right|middle
  flag --count value=N
  flag --force
  flag --header value=H repeatable
command chrome-cdp tab
command chrome-cdp tab close
  arg ID multiple
";

    #[test]
    fn same_command_line_has_no_breaks() {
        assert!(surface_breaks(SURFACE_OLD, SURFACE_OLD).is_empty());
    }

    #[test]
    fn additions_to_the_command_line_are_not_breaks() {
        let new = "\
command chrome-cdp
  flag --json env=CHROME_CDP_JSON
  flag --quiet -q env=CHROME_CDP_QUIET
  flag --force
command chrome-cdp click
  arg LOCATOR required
  arg MORE
  flag --button value=BUTTON values=left|right|middle|back
  flag --count -c value=N
  flag --header value=H repeatable env=CHROME_CDP_HEADER
  flag --new value=X
command chrome-cdp tab
command chrome-cdp tab close
  arg IDS multiple
command chrome-cdp tab new
  arg URL required
";
        // `--force` moved to the global flags: still there.
        assert_eq!(surface_breaks(SURFACE_OLD, new), Vec::<String>::new());
    }

    #[test]
    fn removals_from_the_command_line_break() {
        let new = "\
command chrome-cdp
  flag --json
  flag --quiet
command chrome-cdp click
  flag --button value=BUTTON values=left|right
  flag --count
  flag --force value=WHEN
  flag --header value=H
  flag --must value=X required
command chrome-cdp tab
";
        assert_eq!(
            surface_breaks(SURFACE_OLD, new),
            [
                "`chrome-cdp`: flag --json: its environment variable CHROME_CDP_JSON removed",
                "`chrome-cdp`: flag --quiet: its short form -q removed",
                "`chrome-cdp`: flag --quiet: its environment variable CHROME_CDP_QUIET removed",
                "`chrome-cdp click`: argument LOCATOR removed",
                "`chrome-cdp click`: flag --button no longer takes the value middle",
                "`chrome-cdp click`: flag --count no longer takes a value",
                "`chrome-cdp click`: flag --force now takes a value",
                "`chrome-cdp click`: flag --header is no longer repeatable",
                "`chrome-cdp click`: new required flag --must",
                "`chrome-cdp tab close`: command removed",
            ]
        );
    }

    #[test]
    fn new_required_arguments_break() {
        let old = "command chrome-cdp go\n  arg URL\n";
        let new = "command chrome-cdp go\n  arg URL required\n  arg WHEN required\n";
        assert_eq!(
            surface_breaks(old, new),
            [
                "`chrome-cdp go`: argument URL is now required",
                "`chrome-cdp go`: new required argument WHEN"
            ]
        );
    }

    #[test]
    fn the_golden_command_line_parses() {
        let text = std::fs::read_to_string(crate::workspace_root().join(SURFACE)).unwrap();
        let commands = parse_surface(&text);
        assert!(commands.contains_key("chrome-cdp click"));
        assert!(
            commands["chrome-cdp"]
                .flags
                .iter()
                .any(|f| f.long.as_deref() == Some("--json"))
        );
        assert!(surface_breaks(&text, &text).is_empty());
    }

    #[test]
    fn a_major_version_allows_breaking_changes() {
        assert!(breaking_allowed("v1.4.2", "2.0.0"));
        assert!(breaking_allowed("v1.4.2", "2.0.0-rc.1"));
        assert!(!breaking_allowed("v1.4.2", "1.5.0"));
        assert!(!breaking_allowed("v2.0.0-rc.1", "2.0.0"));
        // Before 1.0 the minor version is the breaking one (Cargo's rule).
        assert!(breaking_allowed("v0.1.3", "0.2.0"));
        assert!(!breaking_allowed("v0.1.3", "0.1.4"));
        assert!(breaking_allowed("v0.9.0", "1.0.0"));
        assert!(!breaking_allowed("vnope", "1.0.0"));
    }
}
