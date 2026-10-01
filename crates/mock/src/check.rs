//! `mock check` (plan 11, milestone 11.8): lints mock files, and with an
//! OpenAPI spec checks each mock against the operation it stands for.
//!
//! Rules (each problem has a file, line, severity and rule id):
//!
//! - `load` (error): what `mock use` would refuse: the format, missing
//!   data files, invalid templates, …;
//! - `unreachable` (warning): a mock an earlier one of its file always
//!   answers for;
//! - `collection-shadowed` (warning): a collection a mock answers for,
//!   whatever the method (in its own file, or a `default_mocks` set
//!   layered after it);
//! - `duplicate-collection` (warning): two files with a collection of one
//!   name (`mock state` and `mock seed` reach only one);
//! - `undeclared-state` (warning): a `when`, `set_state` or `unset_state`
//!   name, or a `{{state.x}}` in an answer (a `[[mock]]`'s, its `.tmpl.`
//!   data files', a `[[websocket]]`'s messages), that no file's `[state]`
//!   declares;
//! - `ws-unreachable` (warning): a `[[websocket]]` an earlier one of its
//!   file takes every socket of;
//! - `ws-rule-unreachable` (warning): an `on_message` rule after one that
//!   answers every message;
//! - `openapi-path`, `openapi-method`, `openapi-status`, `openapi-body`
//!   (errors): the spec has no such path or method, does not list the
//!   status, or its schema does not fit the JSON body;
//! - `openapi-uncovered` (warning): an operation of the spec that no mock
//!   or collection of the sets answers (at most [`UNCOVERED_LISTED`],
//!   then one line with the count), at its line in the spec.
//!
//! A set scoped to full URLs that none of the spec's servers is under
//! (a third party's API) is not checked against the spec.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use protocol::schema::MockProblem;
use serde_json::Value;

use crate::collection::Collection;
use crate::file::AfterSequence;
use crate::glob::PathGlob;
use crate::load::{Mock, MockSet, Source};
use crate::matcher::{Matcher, Target};
use crate::openapi::{Lookup, OpenApi};
use crate::respond::{Body, Respond};
use crate::template::is_template;
use crate::ws::{WsAction, WsMatch, WsSend};

/// How many `openapi-uncovered` operations are listed one by one.
pub const UNCOVERED_LISTED: usize = 20;

/// What [`check`] looked at, and found.
#[derive(Debug, Default)]
pub struct Checked {
    /// The sets that loaded.
    pub sets: Vec<MockSet>,
    /// Mocks and collections.
    pub mocks: usize,
    /// Errors first, then warnings, by file and line.
    pub problems: Vec<MockProblem>,
    /// The spec's operations no mock or collection answers (`GET
    /// /pets/{id}`), all of them.
    pub uncovered: Vec<String>,
}

/// Checks the sets of `sources`. `layers` are the `default_mocks`, in the
/// order they are applied (for shadowed collections across sets).
pub fn check(sources: &[Source], layers: &[String], openapi: Option<&OpenApi>) -> Checked {
    let mut out = Checked::default();
    for source in sources {
        match MockSet::load_located(source) {
            Ok(set) => out.sets.push(set),
            Err(e) => {
                let (line, col) = e.at.unwrap_or((0, 0));
                out.problems.push(problem(
                    &source.file,
                    line,
                    col,
                    Severity::Error,
                    "load",
                    e.message,
                    None,
                ));
            }
        }
    }
    out.mocks = out
        .sets
        .iter()
        .map(|s| s.mocks.len() + s.collections.len())
        .sum();
    for set in &out.sets {
        unreachable(set, &mut out.problems);
        ws_unreachable(set, &mut out.problems);
        shadowed_in_set(set, &mut out.problems);
    }
    shadowed_across(&out.sets, layers, &mut out.problems);
    duplicate_collections(&out.sets, &mut out.problems);
    undeclared(&out.sets, &mut out.problems);
    if let Some(spec) = openapi {
        for set in out.sets.iter().filter(|s| spec.covers_scope(&s.scope_text)) {
            against_spec(set, spec, &mut out.problems);
        }
        let found = uncovered(&out.sets, spec);
        out.uncovered = found.iter().map(|(op, _)| op.clone()).collect();
        let file = spec
            .file()
            .map_or_else(|| Path::new("openapi").to_path_buf(), Path::to_path_buf);
        for (op, line) in found.iter().take(UNCOVERED_LISTED) {
            out.problems.push(problem(
                &file,
                *line,
                usize::from(*line > 0),
                Severity::Warning,
                "openapi-uncovered",
                format!("{op}: no mock or collection answers it"),
                Some(format!(
                    "add a [[mock]] for it (`chrome-cdp mock gen --scope '{}' -o …` writes one)",
                    gen_scope(op)
                )),
            ));
        }
        if out.uncovered.len() > UNCOVERED_LISTED {
            out.problems.push(problem(
                &file,
                0,
                0,
                Severity::Warning,
                "openapi-uncovered",
                format!(
                    "{} more spec operations have no mock (--json lists them all)",
                    out.uncovered.len() - UNCOVERED_LISTED
                ),
                None,
            ));
        }
    }
    out.problems.sort_by(|a, b| {
        (b.severity == "error", &a.file, a.line, a.col).cmp(&(
            a.severity == "error",
            &b.file,
            b.line,
            b.col,
        ))
    });
    out
}

#[derive(Clone, Copy)]
enum Severity {
    Error,
    Warning,
}

fn problem(
    file: &Path,
    line: usize,
    col: usize,
    severity: Severity,
    rule: &str,
    message: String,
    hint: Option<String>,
) -> MockProblem {
    MockProblem {
        file: file.display().to_string(),
        line,
        col,
        severity: match severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
        .to_owned(),
        rule: rule.to_owned(),
        message,
        hint,
    }
}

/// A path that stands for every path glob `glob` matches: parameters,
/// `*` and `**` made into sample segments. Another glob that matches it
/// matches (nearly) all the paths `glob` does.
fn sample(glob: &str) -> String {
    let mut out = String::with_capacity(glob.len());
    let mut chars = glob.chars().peekable();
    let mut after_slash = true;
    while let Some(c) = chars.next() {
        match c {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                out.push_str("__any__/__any__");
            }
            '*' => out.push_str("__any__"),
            ':' if after_slash => {
                while chars
                    .peek()
                    .is_some_and(|c| c.is_alphanumeric() || *c == '_')
                {
                    chars.next();
                }
                out.push_str("__param__");
            }
            c => out.push(c),
        }
        after_slash = c == '/';
    }
    out
}

/// Whether glob `a` matches every path glob `b` does.
fn glob_covers(a: &PathGlob, b: &PathGlob) -> bool {
    a.is_path() == b.is_path() && a.matches(&sample(b.text()))
}

/// Whether `a`'s match holds for every request `b`'s does.
fn covers(a: &Matcher, b: &Matcher) -> bool {
    let methods = a.methods.is_empty()
        || (!b.methods.is_empty() && b.methods.iter().all(|m| a.methods.contains(m)));
    let target = match (&a.target, &b.target) {
        (None, _) => true,
        (Some(Target::Path(ga)), Some(Target::Path(gb)))
        | (Some(Target::Url(ga)), Some(Target::Url(gb))) => glob_covers(ga, gb),
        (Some(Target::Regex(ra)), Some(Target::Regex(rb))) => ra.as_str() == rb.as_str(),
        _ => false,
    };
    let pairs = |need: &[(String, crate::glob::Glob)], have: &[(String, crate::glob::Glob)]| {
        need.iter().all(|(k, ga)| {
            have.iter()
                .any(|(hk, gb)| hk == k && ga.matches(gb.as_str()))
        })
    };
    let body = match (&a.body, &b.body) {
        (None, _) => true,
        (Some(x), Some(y)) => format!("{x:?}") == format!("{y:?}"),
        (Some(_), None) => false,
    };
    methods && target && pairs(&a.query, &b.query) && pairs(&a.headers, &b.headers) && body
}

/// Whether mock `m` always matches when its `match` holds.
fn always(m: &Mock) -> bool {
    m.when.is_empty() && m.sequence != Some(AfterSequence::Unmatched)
}

fn unreachable(set: &MockSet, out: &mut Vec<MockProblem>) {
    for (j, b) in set.mocks.iter().enumerate() {
        if let Some(a) = set.mocks[..j]
            .iter()
            .find(|a| always(a) && covers(&a.matcher, &b.matcher))
        {
            out.push(problem(
                &set.source.file,
                b.line,
                1,
                Severity::Warning,
                "unreachable",
                format!(
                    "mock \"{}\" never answers: \"{}\" (line {}) comes first and matches every \
                     request it does",
                    b.name, a.name, a.line
                ),
                Some(format!(
                    "move \"{}\" above \"{}\", or narrow \"{}\"'s match",
                    b.name, a.name, a.name
                )),
            ));
        }
    }
}

/// Whether mock `m` answers every request to collection `c`, whatever
/// its method.
fn takes_collection(m: &Mock, c: &Collection) -> bool {
    let bare = m.matcher.methods.is_empty()
        && m.matcher.query.is_empty()
        && m.matcher.headers.is_empty()
        && m.matcher.body.is_none()
        && always(m);
    bare && match &m.matcher.target {
        Some(Target::Path(g)) if g.is_path() => {
            g.matches(&sample(c.path.text())) && g.matches(&sample(c.item.text()))
        }
        _ => false,
    }
}

fn shadow_problem(set: &MockSet, c: &Collection, by: &MockSet, m: &Mock) -> MockProblem {
    let whose = if std::ptr::eq(set, by) {
        format!("mock \"{}\" (line {})", m.name, m.line)
    } else {
        format!("mock \"{}\" ({}:{})", m.name, by.file_name(), m.line)
    };
    problem(
        &set.source.file,
        c.line,
        1,
        Severity::Warning,
        "collection-shadowed",
        format!(
            "collection \"{}\" never answers: {whose} is tried first and answers every request to \
             {}",
            c.name,
            c.path.text()
        ),
        Some(format!(
            "give \"{}\" a method or a narrower path, so the collection answers the rest",
            m.name
        )),
    )
}

fn shadowed_in_set(set: &MockSet, out: &mut Vec<MockProblem>) {
    for c in &set.collections {
        if let Some(m) = set.mocks.iter().find(|m| takes_collection(m, c)) {
            out.push(shadow_problem(set, c, set, m));
        }
    }
}

/// Collections of a `default_mocks` set that a set applied after it
/// takes.
fn shadowed_across(sets: &[MockSet], layers: &[String], out: &mut Vec<MockProblem>) {
    let layered: Vec<&MockSet> = layers
        .iter()
        .filter_map(|name| sets.iter().find(|s| s.name() == name))
        .collect();
    for (i, set) in layered.iter().enumerate() {
        for c in &set.collections {
            let later = layered[i + 1..]
                .iter()
                .rev()
                .find_map(|l| Some((*l, l.mocks.iter().find(|m| takes_collection(m, c))?)));
            if let Some((by, m)) = later {
                out.push(shadow_problem(set, c, by, m));
            }
        }
    }
}

fn duplicate_collections(sets: &[MockSet], out: &mut Vec<MockProblem>) {
    let mut first: HashMap<&str, &MockSet> = HashMap::new();
    for set in sets {
        for c in &set.collections {
            match first.get(c.name.as_str()) {
                Some(other) => out.push(problem(
                    &set.source.file,
                    c.line,
                    1,
                    Severity::Warning,
                    "duplicate-collection",
                    format!(
                        "collection \"{}\" is also in {}: with both sets in use, `mock state` and \
                         `mock seed` reach only the one tried first",
                        c.name,
                        other.file_name()
                    ),
                    Some("rename one of them".to_owned()),
                )),
                None => {
                    first.insert(&c.name, set);
                }
            }
        }
    }
}

fn undeclared(sets: &[MockSet], out: &mut Vec<MockProblem>) {
    let declared: BTreeSet<&str> = sets
        .iter()
        .flat_map(|s| s.state.iter().map(|(n, _)| n.as_str()))
        .collect();
    let report = |set: &MockSet, line: usize, message: String, name: &str| {
        let close = protocol::suggest::similar(name, declared.iter().copied(), 1);
        problem(
            &set.source.file,
            line,
            1,
            Severity::Warning,
            "undeclared-state",
            message,
            Some(match close.first() {
                Some(c) => format!("did you mean `{c}`?"),
                None => format!("declare it: [state] {name} = …"),
            }),
        )
    };
    for set in sets {
        for mock in &set.mocks {
            for (key, names) in [("when", &mock.when), ("set_state", &mock.set_state)] {
                for (name, value) in names {
                    if declared.contains(name.as_str()) {
                        continue;
                    }
                    let key = if value.is_null() && key == "set_state" {
                        "unset_state"
                    } else {
                        key
                    };
                    out.push(report(
                        set,
                        mock.line,
                        format!(
                            "mock \"{}\": {key} names `{name}`, which no mock file's [state] \
                             declares",
                            mock.name
                        ),
                        name,
                    ));
                }
            }
        }
        for mock in &set.mocks {
            let mut texts = Vec::new();
            mock_texts(mock, &set.source.base, &mut texts);
            for name in state_names(&texts)
                .iter()
                .filter(|n| !declared.contains(n.as_str()))
            {
                out.push(report(
                    set,
                    mock.line,
                    format!(
                        "mock \"{}\": its answer reads `state.{name}`, which no mock file's \
                         [state] declares",
                        mock.name
                    ),
                    name,
                ));
            }
        }
        for ws in &set.websockets {
            let mut texts = Vec::new();
            for action in ws
                .on_open
                .iter()
                .chain(ws.on_message.iter().map(|r| &r.action))
            {
                ws_texts(action, &mut texts);
            }
            for name in state_names(&texts)
                .iter()
                .filter(|n| !declared.contains(n.as_str()))
            {
                out.push(report(
                    set,
                    ws.line,
                    format!(
                        "websocket \"{}\": a message reads `state.{name}`, which no mock file's \
                         [state] declares",
                        ws.name
                    ),
                    name,
                ));
            }
        }
    }
}

/// The strings of `value` (keys too), where templates can be.
fn json_texts(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|v| json_texts(v, out)),
        Value::Object(map) => map.iter().for_each(|(k, v)| {
            out.push(k.clone());
            json_texts(v, out);
        }),
        _ => {}
    }
}

/// The texts of a websocket action that are templates.
fn ws_texts(action: &WsAction, out: &mut Vec<String>) {
    match &action.send {
        Some(WsSend::Text(value)) => json_texts(value, out),
        Some(WsSend::Base64(text)) => out.push(text.clone()),
        None => {}
    }
    if let Some((_, reason)) = &action.close {
        out.push(reason.clone());
    }
}

/// The texts of a mock's answers that are templates: inline bodies,
/// headers, `sse` events, data file names, and `.tmpl.` data files
/// (read from `base`).
fn mock_texts(mock: &Mock, base: &Path, out: &mut Vec<String>) {
    for step in &mock.steps {
        let (headers, body) = match &step.respond {
            Respond::Fulfill { headers, body, .. } => (headers, body),
            Respond::Passthrough(Some(patch)) => {
                json_texts(patch, out);
                continue;
            }
            Respond::Passthrough(None) | Respond::Abort(_) => continue,
        };
        out.extend(headers.iter().map(|(_, v)| v.clone()));
        match body {
            Body::Json(value) => json_texts(value, out),
            Body::Text(text) => out.push(text.clone()),
            Body::JsonFile(path) | Body::TextFile(path) | Body::File(path) => {
                out.push(path.clone());
                if path.contains(".tmpl.")
                    && !is_template(path)
                    && let Ok(text) = std::fs::read_to_string(base.join(path))
                {
                    out.push(text);
                }
            }
            Body::Sse(events) => {
                for e in events {
                    out.extend(e.event.iter().chain(&e.id).chain(&e.comment).cloned());
                    if let Some(data) = &e.data {
                        json_texts(data, out);
                    }
                }
            }
            Body::Empty => {}
        }
    }
}

/// The `state.x` names `texts`' templates read.
fn state_names(texts: &[String]) -> BTreeSet<String> {
    static STATE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        #[allow(clippy::expect_used)] // a constant pattern
        regex::Regex::new(
            r#"\bstate\s*(?:\.\s*([A-Za-z_][A-Za-z0-9_]*)|\[\s*["']([^"']+)["']\s*\])"#,
        )
        .expect("valid regex")
    });
    let mut out = BTreeSet::new();
    for text in texts.iter().filter(|t| is_template(t)) {
        for c in STATE.captures_iter(text) {
            if let Some(name) = c.get(1).or_else(|| c.get(2)) {
                out.insert(name.as_str().to_owned());
            }
        }
    }
    out
}

/// Whether a rule answers every message (text and binary).
fn takes_every_message(m: &WsMatch) -> bool {
    match m {
        WsMatch::Any => true,
        WsMatch::Text(_) | WsMatch::Json(_) => false,
    }
}

/// Whether a rule answers every text message: no match, or a text glob
/// of only `*`s.
fn takes_every_text(m: &WsMatch) -> bool {
    match m {
        WsMatch::Any => true,
        WsMatch::Text(glob) => {
            let text = glob.as_str();
            !text.is_empty() && text.chars().all(|c| c == '*')
        }
        WsMatch::Json(_) => false,
    }
}

fn ws_unreachable(set: &MockSet, out: &mut Vec<MockProblem>) {
    for (j, b) in set.websockets.iter().enumerate() {
        if let Some(a) = set.websockets[..j]
            .iter()
            .find(|a| glob_covers(&a.url, &b.url))
        {
            out.push(problem(
                &set.source.file,
                b.line,
                1,
                Severity::Warning,
                "ws-unreachable",
                format!(
                    "websocket \"{}\" never opens: \"{}\" (line {}) comes first and takes every \
                     socket it does",
                    b.name, a.name, a.line
                ),
                Some(format!(
                    "move \"{}\" above \"{}\", or narrow \"{}\"'s url",
                    b.name, a.name, a.name
                )),
            ));
        }
        for (k, rule) in b.on_message.iter().enumerate() {
            let Some(first) = b.on_message[..k].iter().find(|r| {
                takes_every_message(&r.matcher)
                    || (takes_every_text(&r.matcher) && !matches!(rule.matcher, WsMatch::Any))
            }) else {
                continue;
            };
            out.push(problem(
                &set.source.file,
                rule.line,
                1,
                Severity::Warning,
                "ws-rule-unreachable",
                format!(
                    "websocket \"{}\": on_message rule {} never answers: rule {} (line {}) comes \
                     first and answers every message",
                    b.name,
                    k + 1,
                    b.on_message
                        .iter()
                        .position(|r| std::ptr::eq(r, first))
                        .unwrap_or(0)
                        + 1,
                    first.line
                ),
                Some("move the rule above it, or give that rule a `match`".to_owned()),
            ));
        }
    }
}

/// The path of a mock's target as the spec sees it: a path glob, or a
/// full-URL glob's path. `None` for a regex or no target.
fn spec_path(m: &Matcher) -> Option<String> {
    match &m.target {
        Some(Target::Path(g)) => Some(g.text().to_owned()),
        Some(Target::Url(g)) => {
            let text = g.text();
            let rest = &text[text.find("://")? + 3..];
            Some(
                rest.find('/')
                    .map_or_else(|| "/".to_owned(), |i| rest[i..].to_owned()),
            )
        }
        _ => None,
    }
}

/// The JSON a body answers with, when it can be known without a
/// request: inline JSON, or a data file named without a template.
fn body_json(body: &Body, base: &Path) -> Option<(Value, Option<String>)> {
    match body {
        Body::Json(v) => Some((v.clone(), None)),
        Body::JsonFile(path) if !is_template(path) => {
            let text = std::fs::read_to_string(base.join(path)).ok()?;
            Some((serde_json::from_str(&text).ok()?, Some(path.clone())))
        }
        _ => None,
    }
}

fn against_spec(set: &MockSet, spec: &OpenApi, out: &mut Vec<MockProblem>) {
    let file = &set.source.file;
    for mock in &set.mocks {
        let Some(path) = spec_path(&mock.matcher) else {
            continue;
        };
        let err = |rule: &str, message: String, hint: Option<String>| {
            problem(file, mock.line, 1, Severity::Error, rule, message, hint)
        };
        let ops = match spec.lookup(&path, &mock.matcher.methods) {
            Lookup::Unknown => continue,
            Lookup::NoPath { closest } => {
                out.push(err(
                    "openapi-path",
                    format!("mock \"{}\": the spec has no path {path}", mock.name),
                    Some(if closest.is_empty() {
                        "fix the mock's path, or add the path to the spec".to_owned()
                    } else {
                        format!("the spec's closest: {}", closest.join(", "))
                    }),
                ));
                continue;
            }
            Lookup::NoMethod { path, missing, has } => {
                out.push(err(
                    "openapi-method",
                    format!(
                        "mock \"{}\": the spec's {path} has no {}",
                        mock.name,
                        missing.join(", ")
                    ),
                    Some(format!("it has {}", has.join(", "))),
                ));
                continue;
            }
            Lookup::Found(ops) => ops,
        };
        for (i, step) in mock.steps.iter().enumerate() {
            let Respond::Fulfill {
                status,
                headers,
                body,
            } = &step.respond
            else {
                continue;
            };
            let which = if mock.steps.len() > 1 {
                format!("answer {} of mock \"{}\"", i + 1, mock.name)
            } else {
                format!("mock \"{}\"", mock.name)
            };
            let json_typed = headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
                .is_none_or(|(_, v)| v.contains("json"));
            for op in &ops {
                if !spec.allows(op, *status) {
                    out.push(err(
                        "openapi-status",
                        format!(
                            "{which} answers {status}; the spec's {} {} has {}",
                            op.method,
                            op.path,
                            OpenApi::statuses(op).join(", ")
                        ),
                        None,
                    ));
                    continue;
                }
                if !json_typed {
                    continue;
                }
                let Some((value, data)) = body_json(body, &set.source.base) else {
                    continue;
                };
                let found = spec.check_body(op, *status, &value);
                if found.is_empty() {
                    continue;
                }
                let whose = data.map_or_else(String::new, |d| format!(" ({d})"));
                out.push(err(
                    "openapi-body",
                    format!(
                        "{which}{whose} does not fit the spec's {} {} {status}: {}",
                        op.method,
                        op.path,
                        found.join("; ")
                    ),
                    None,
                ));
            }
        }
    }
    for c in &set.collections {
        let item_path = format!("{}/:{}", c.path.text().trim_end_matches('/'), c.id);
        for (path, shown) in [
            (c.path.text(), c.path.text()),
            (c.item.text(), item_path.as_str()),
        ] {
            // Any method will do: a collection answers them all, and the
            // real API need not (a cart with no GET of one item).
            if let Lookup::NoPath { closest } = spec.lookup(path, &[]) {
                out.push(problem(
                    file,
                    c.line,
                    1,
                    Severity::Error,
                    "openapi-path",
                    format!("collection \"{}\": the spec has no path {shown}", c.name),
                    (!closest.is_empty())
                        .then(|| format!("the spec's closest: {}", closest.join(", "))),
                ));
            }
        }
        let item_path = format!("{}/:{}", c.path.text().trim_end_matches('/'), c.id);
        if let Some(schema) = spec.item_schema(c.item.text()) {
            for (i, item) in c.seed.iter().enumerate() {
                let found = spec.check(schema, item);
                if !found.is_empty() {
                    out.push(problem(
                        file,
                        c.line,
                        1,
                        Severity::Error,
                        "openapi-body",
                        format!(
                            "collection \"{}\": item {} does not fit the spec's GET {}: {}",
                            c.name,
                            i + 1,
                            item_path,
                            found.join("; ")
                        ),
                        None,
                    ));
                    break;
                }
            }
        }
    }
}

/// `--scope` of `mock gen` for an uncovered operation: its path with the
/// parameters as `*`.
fn gen_scope(op: &str) -> String {
    let path = op.split(' ').nth(1).unwrap_or(op);
    path.split('/')
        .map(|s| if s.contains('{') { "*" } else { s })
        .collect::<Vec<_>>()
        .join("/")
}

/// The spec's operations (`GET /pets/{id}`, with ` (operationId)`) that
/// no mock or collection of `sets` stands for. A mock stands for the
/// operations its path and methods land on in the spec (`/pets/:id`,
/// `/pets/7`, `/pets/*`), as the spec checks see it; a catch-all
/// (`/api/**`, a regex, no path) answers for many and stands for none,
/// so an `errors/api-down` set does not hide the operations no other
/// mock has.
fn uncovered(sets: &[MockSet], spec: &OpenApi) -> Vec<(String, usize)> {
    let takes = |glob: &str, method: &str, path: &str| {
        matches!(spec.lookup(glob, &[method.to_owned()]),
            Lookup::Found(ops) if ops.iter().any(|o| o.path == path))
    };
    let mut out = Vec::new();
    for op in spec.operations() {
        let method = op.method.as_str();
        let covered = sets.iter().any(|set| {
            set.mocks.iter().any(|m| {
                let methods =
                    m.matcher.methods.is_empty() || m.matcher.methods.iter().any(|x| x == method);
                methods && spec_path(&m.matcher).is_some_and(|p| takes(&p, method, op.path))
            }) || set.collections.iter().any(|c| {
                (matches!(method, "GET" | "POST") && takes(c.path.text(), method, op.path))
                    || (matches!(method, "GET" | "PUT" | "PATCH" | "DELETE")
                        && takes(c.item.text(), method, op.path))
            })
        });
        if !covered {
            let mut name = format!("{} {}", op.method, op.path);
            if let Some(id) = op.id {
                let _ = std::fmt::Write::write_fmt(&mut name, format_args!(" ({id})"));
            }
            out.push((name, spec.line_of(op.path, &op.method.to_ascii_lowercase())));
        }
    }
    out
}
