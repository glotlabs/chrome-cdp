//! `mock gen --openapi` (plan 19, milestone 19.4): a starting mock set
//! from an OpenAPI spec, for a project that has a spec but no mocks yet.
//!
//! One `[[mock]]` per operation answers its success status: the lowest
//! 2xx the spec lists (`default`, as 200, when it lists none). Its JSON
//! body is the media type's `example`, else the first of its `examples`,
//! else built from the schema: a schema's own `example` wins at every
//! level, and the rest is generated (`default`, `enum[0]`, `const`, then
//! by type and format). Generation is deterministic: each operation draws
//! from its own generator, seeded by `--seed` and the operation's name,
//! so the same spec and seed give the same file, and adding an operation
//! does not change the others.
//!
//! With `collections`, a pair of paths `/things` (GET a list) and
//! `/things/{id}` (GET an item) becomes a `[[collection]]` of three
//! generated items instead of mocks, and nested paths
//! (`/users/{user}/todos`) scope its items by `params`.
//!
//! A value the generator cannot make fit the spec (a `pattern`, a body the
//! schema check still refuses) is a `# TODO:` comment above its mock.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use base64::Engine as _;
use serde_json::{Map, Value, json};

use crate::glob::PathGlob;
use crate::openapi::{OpenApi, SpecOperation};
use crate::record::{toml_key, toml_string, toml_value};
use crate::template::{Rng, uuid};

/// The `#:schema` line's URL: editors (Taplo, Even Better TOML) check
/// mock files against it.
pub const SCHEMA_URL: &str =
    "https://raw.githubusercontent.com/glotlabs/chrome-cdp/main/docs/e2e/mocks.schema.json";

/// How deep objects and arrays are generated.
const MAX_DEPTH: usize = 8;
/// JSON bodies up to this many characters (compact) are written inline;
/// TOML's inline tables are one line, so longer ones go to data files.
const INLINE: usize = 100;
/// A collection's items are written inline when each is at most this
/// long (as TOML), else to a seed file.
const INLINE_ITEM: usize = 160;
/// The items a generated collection starts with.
const ITEMS: usize = 3;
/// `date-time` values: this instant plus the seed in hours.
const EPOCH_DAYS: i64 = 20_454; // 2026-01-01

/// What to generate.
#[derive(Debug, Clone, Default)]
pub struct GenOptions {
    /// Seeds the generated values (`--seed`, default 1).
    pub seed: u64,
    /// Only operations whose path a glob matches (the spec's path, or
    /// the mock's with the server's prefix); all when empty.
    pub scope: Vec<String>,
    /// Make `[[collection]]`s of list and item path pairs.
    pub collections: bool,
    /// Where data files go, relative to the mock file's directory:
    /// `data/<set>`.
    pub data_dir: String,
    /// The spec as the header comment names it.
    pub spec_name: String,
}

/// A mock written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenMock {
    pub name: String,
    pub method: String,
    /// The mock's path glob (`/v1/pets/:petId`).
    pub path: String,
    pub status: u16,
    /// Its data file, if the body is in one.
    pub file: Option<String>,
}

/// A collection written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenCollection {
    pub name: String,
    pub path: String,
    /// The spec's operations it answers (`GET /pets`, …).
    pub operations: Vec<String>,
    /// Its seed file, if the items are in one.
    pub file: Option<String>,
}

/// A generated mock set.
#[derive(Debug, Clone, Default)]
pub struct Generated {
    /// The mock file.
    pub toml: String,
    /// Data files: paths relative to the mock file's directory, and bytes.
    pub files: Vec<(String, Vec<u8>)>,
    pub mocks: Vec<GenMock>,
    pub collections: Vec<GenCollection>,
    /// The TODO comments written (`getPet: $.code: pattern …`).
    pub todos: Vec<String>,
    /// The set's `scope`.
    pub scope: Vec<String>,
    /// The spec's operations in `--scope`.
    pub operations: usize,
}

/// Generates a mock set from `spec`.
pub fn generate(spec: &OpenApi, options: &GenOptions) -> Result<Generated, String> {
    let filters = options
        .scope
        .iter()
        .map(|g| PathGlob::path(g).map_err(|e| format!("--scope: {e}")))
        .collect::<Result<Vec<_>, _>>()?;
    let base = spec.base();
    let ops: Vec<SpecOperation<'_>> = spec
        .operations()
        .into_iter()
        .filter(|op| {
            filters.is_empty() || {
                let sample = sample_path(op.path);
                let full = format!("{base}{sample}");
                filters
                    .iter()
                    .any(|g| g.matches(&sample) || g.matches(&full))
            }
        })
        .collect();
    if ops.is_empty() {
        return Err(if options.scope.is_empty() {
            "the spec has no operations".to_owned()
        } else {
            format!(
                "no operation of the spec is in --scope {}",
                options.scope.join(", ")
            )
        });
    }
    let mut out = Generated {
        operations: ops.len(),
        scope: scope(spec, &ops),
        ..Generated::default()
    };
    let mut files = Names::new(&options.data_dir);
    let mut text = String::new();
    let _ = writeln!(text, "#:schema {SCHEMA_URL}");
    let _ = writeln!(
        text,
        "# Generated by `chrome-cdp mock gen` from {} (seed {}).",
        options.spec_name, options.seed
    );
    text.push_str(
        "# One [[mock]] per operation answers its success status with the spec's examples, else\n\
         # generated values; edit them freely. `chrome-cdp mock check --openapi` checks them.\n",
    );
    text.push_str("\n[settings]\n");
    let _ = writeln!(text, "scope = {}", toml_list(&out.scope));

    let pairs = if options.collections {
        collection_pairs(spec, &ops)
    } else {
        Vec::new()
    };
    let taken: HashSet<(String, String)> =
        pairs.iter().flat_map(|p| p.taken.iter().cloned()).collect();
    let mut ordered: Vec<&SpecOperation<'_>> = ops
        .iter()
        .filter(|op| !taken.contains(&(op.method.clone(), op.path.to_owned())))
        .collect();
    // Literal segments before parameters, so `/pets/mine` is tried
    // before `/pets/{id}`.
    ordered.sort_by_cached_key(|op| (path_order(op.path), method_order(&op.method)));
    for op in ordered {
        let mock = mock_toml(spec, op, &base, options.seed, &mut files, &mut out);
        text.push('\n');
        text.push_str(&mock);
    }
    let mut names: HashSet<String> = HashSet::new();
    for pair in &pairs {
        let c = collection_toml(
            spec,
            pair,
            &base,
            options.seed,
            &mut names,
            &mut files,
            &mut out,
        );
        text.push('\n');
        text.push_str(&c);
    }
    out.toml = text;
    Ok(out)
}

/// `/pets/{id}` with its parameters as sample values: `/pets/__param__`.
fn sample_path(path: &str) -> String {
    path.split('/')
        .map(|s| if s.contains('{') { "__param__" } else { s })
        .collect::<Vec<_>>()
        .join("/")
}

/// The spec's path as a mock path glob: `{petId}` is `:petId`; a
/// segment with a parameter inside it (`{name}.json`) is `*`.
pub(crate) fn mock_path(path: &str) -> String {
    let segs: Vec<String> = path
        .split('/')
        .map(|s| {
            if let Some(name) = s.strip_prefix('{').and_then(|r| r.strip_suffix('}'))
                && !name.contains(['{', '}'])
            {
                let name: String = name
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == '_' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                format!(":{name}")
            } else if s.contains('{') {
                "*".to_owned()
            } else {
                s.to_owned()
            }
        })
        .collect();
    segs.join("/")
}

fn path_order(path: &str) -> Vec<(bool, String)> {
    path.split('/')
        .map(|s| (s.contains('{'), s.to_owned()))
        .collect()
}

fn method_order(method: &str) -> usize {
    ["GET", "POST", "PUT", "PATCH", "DELETE"]
        .iter()
        .position(|m| *m == method)
        .unwrap_or(9)
}

/// The set's scope: the servers' paths (`/v1/**`), else the first
/// segments of the operations' paths.
fn scope(spec: &OpenApi, ops: &[SpecOperation<'_>]) -> Vec<String> {
    let servers = spec.server_paths();
    if !servers.is_empty() {
        return servers.iter().map(|p| format!("{p}/**")).collect();
    }
    let mut out: Vec<String> = Vec::new();
    for op in ops {
        let first = op.path.split('/').find(|s| !s.is_empty());
        let glob = match first {
            Some(s) if !s.contains('{') => format!("/{s}/**"),
            _ => "/**".to_owned(),
        };
        if !out.contains(&glob) {
            out.push(glob);
        }
    }
    if out.iter().any(|g| g == "/**") {
        out = vec!["/**".to_owned()];
    }
    out.sort();
    out
}

fn toml_list(items: &[String]) -> String {
    let items: Vec<String> = items.iter().map(|s| toml_string(s)).collect();
    format!("[{}]", items.join(", "))
}

/// A stable 64-bit hash (FNV-1a): seeds an operation's generator.
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// What an operation answers on success.
enum Content {
    /// No body.
    None,
    /// JSON: its schema, and the spec's example of it.
    Json {
        schema: Option<Value>,
        example: Option<Value>,
    },
    /// Another media type, and a text example if the spec gives one.
    Other {
        media: String,
        example: Option<String>,
    },
}

/// The success status of `op` and its response (`$ref` followed).
fn success<'a>(spec: &'a OpenApi, op: &SpecOperation<'a>) -> (u16, Option<&'a Value>) {
    let Some(responses) = op.op.get("responses").and_then(Value::as_object) else {
        return (200, None);
    };
    let follow = |r: &'a Value| spec.follow(r).unwrap_or(r);
    let numbered: Vec<(u16, &Value)> = responses
        .iter()
        .filter_map(|(k, v)| Some((k.parse::<u16>().ok()?, v)))
        .collect();
    if let Some((status, r)) = numbered
        .iter()
        .filter(|(s, _)| (200..300).contains(s))
        .min_by_key(|(s, _)| *s)
    {
        return (*status, Some(follow(r)));
    }
    if let Some((_, r)) = responses
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("2XX"))
    {
        return (200, Some(follow(r)));
    }
    if let Some(r) = responses.get("default") {
        return (200, Some(follow(r)));
    }
    match numbered.iter().min_by_key(|(s, _)| *s) {
        Some((status, r)) => (*status, Some(follow(r))),
        None => (200, None),
    }
}

/// What `response` (of an operation) carries.
fn content(spec: &OpenApi, op: &SpecOperation<'_>, response: Option<&Value>) -> Content {
    let Some(response) = response else {
        return Content::None;
    };
    if spec.is_swagger() {
        let produces: Vec<&str> = op
            .op
            .get("produces")
            .or_else(|| spec.doc().get("produces"))
            .and_then(Value::as_array)
            .map(|p| p.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let example = response
            .get("examples")
            .and_then(Value::as_object)
            .and_then(|e| e.iter().find(|(k, _)| k.contains("json")).map(|(_, v)| v));
        let Some(schema) = response.get("schema") else {
            return Content::None;
        };
        if produces.is_empty() || produces.iter().any(|p| p.contains("json")) {
            return Content::Json {
                schema: Some(schema.clone()),
                example: example.cloned(),
            };
        }
        return Content::Other {
            media: produces[0].to_owned(),
            example: None,
        };
    }
    let Some(media) = response.get("content").and_then(Value::as_object) else {
        return Content::None;
    };
    let json = media
        .iter()
        .find(|(k, _)| k.contains("json"))
        .or_else(|| media.iter().find(|(k, _)| k.as_str() == "*/*"));
    if let Some((_, m)) = json {
        let example = m.get("example").cloned().or_else(|| {
            let (_, first) = m.get("examples")?.as_object()?.iter().next()?;
            let first = spec.follow(first).unwrap_or(first);
            first.get("value").cloned()
        });
        let schema = m.get("schema").cloned();
        if schema.is_none() && example.is_none() {
            return Content::None;
        }
        return Content::Json { schema, example };
    }
    match media.iter().next() {
        Some((k, m)) => Content::Other {
            media: k.clone(),
            example: m.get("example").and_then(Value::as_str).map(str::to_owned),
        },
        None => Content::None,
    }
}

/// Generates values from schemas.
struct Gen<'a> {
    spec: &'a OpenApi,
    rng: Rng,
    seed: u64,
    /// Per property name, the last counter handed out.
    counters: HashMap<String, u64>,
    /// The `$ref`s being expanded (to stop at a cycle).
    refs: Vec<String>,
    todos: Vec<String>,
}

impl<'a> Gen<'a> {
    fn new(spec: &'a OpenApi, seed: u64, name: &str) -> Self {
        Self {
            spec,
            rng: Rng::new(seed ^ fnv(name)),
            seed,
            counters: HashMap::new(),
            refs: Vec::new(),
            todos: Vec::new(),
        }
    }

    fn todo(&mut self, text: String) {
        if !self.todos.contains(&text) {
            self.todos.push(text);
        }
    }

    fn next(&mut self, name: &str) -> u64 {
        let n = self.counters.entry(name.to_owned()).or_insert(0);
        *n += 1;
        *n
    }

    /// A value for `schema`; `None` to leave it out (too deep, or a
    /// cycle). `name` is the property's (for strings), `at` its place.
    fn value(&mut self, schema: &Value, name: &str, at: &str, depth: usize) -> Option<Value> {
        if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            if self.refs.iter().any(|r| r == reference) {
                return None;
            }
            let Some(target) = self.spec.follow(schema) else {
                self.todo(format!("{at}: the spec's {reference} is not in it"));
                return Some(json!({}));
            };
            self.refs.push(reference.to_owned());
            let v = self.value(target, name, at, depth);
            self.refs.pop();
            return v;
        }
        let Some(obj) = schema.as_object() else {
            return Some(json!({}));
        };
        if let Some(e) = obj.get("example") {
            return Some(e.clone());
        }
        if let Some(e) = obj
            .get("examples")
            .and_then(Value::as_array)
            .and_then(|e| e.first())
        {
            return Some(e.clone());
        }
        if let Some(v) = obj.get("default") {
            return Some(v.clone());
        }
        if let Some(v) = obj
            .get("enum")
            .and_then(Value::as_array)
            .and_then(|e| e.first())
        {
            return Some(v.clone());
        }
        if let Some(v) = obj.get("const") {
            return Some(v.clone());
        }
        if let Some(all) = obj.get("allOf").and_then(Value::as_array) {
            let mut merged = Map::new();
            let mut followed = Vec::new();
            if !self.merge(all, &mut merged, &mut followed, 0) {
                return None;
            }
            for (k, v) in obj {
                if k != "allOf" && !merged.contains_key(k) {
                    merged.insert(k.clone(), v.clone());
                }
            }
            let depth_before = self.refs.len();
            self.refs.extend(followed);
            let v = self.value(&Value::Object(merged), name, at, depth);
            self.refs.truncate(depth_before);
            return v;
        }
        for key in ["oneOf", "anyOf"] {
            if let Some(first) = obj
                .get(key)
                .and_then(Value::as_array)
                .and_then(|o| o.first())
            {
                return self.value(first, name, at, depth);
            }
        }
        match type_of(obj) {
            "object" => {
                if depth >= MAX_DEPTH {
                    return None;
                }
                Some(self.object(obj, at, depth))
            }
            "array" => {
                if depth >= MAX_DEPTH {
                    return None;
                }
                Some(self.array(obj, name, at, depth))
            }
            "string" => Some(Value::String(self.string(obj, name, at))),
            "integer" => Some(self.number(obj, true, at)),
            "number" => Some(self.number(obj, false, at)),
            "boolean" => Some(Value::Bool(true)),
            "null" => Some(Value::Null),
            _ => Some(json!({})),
        }
    }

    /// The `allOf` schemas merged into `into`: their properties and
    /// required names together, other keys from the first that has them.
    /// The `$ref`s it follows go to `followed`; `false` on a cycle.
    fn merge(
        &self,
        all: &[Value],
        into: &mut Map<String, Value>,
        followed: &mut Vec<String>,
        depth: usize,
    ) -> bool {
        for s in all {
            if let Some(reference) = s.get("$ref").and_then(Value::as_str) {
                if self
                    .refs
                    .iter()
                    .chain(followed.iter())
                    .any(|r| r == reference)
                {
                    return false;
                }
                followed.push(reference.to_owned());
            }
            let s = self.spec.follow(s).unwrap_or(s);
            let Some(obj) = s.as_object() else { continue };
            if let Some(inner) = obj.get("allOf").and_then(Value::as_array)
                && depth < MAX_DEPTH
                && !self.merge(inner, into, followed, depth + 1)
            {
                return false;
            }
            for (k, v) in obj {
                match k.as_str() {
                    "allOf" | "example" | "examples" => {}
                    "properties" => {
                        if let (Some(Value::Object(have)), Some(add)) =
                            (into.get_mut("properties"), v.as_object())
                        {
                            for (pk, pv) in add {
                                have.entry(pk.clone()).or_insert_with(|| pv.clone());
                            }
                        } else {
                            into.insert(k.clone(), v.clone());
                        }
                    }
                    "required" => {
                        if let (Some(Value::Array(have)), Some(add)) =
                            (into.get_mut("required"), v.as_array())
                        {
                            for r in add {
                                if !have.contains(r) {
                                    have.push(r.clone());
                                }
                            }
                        } else {
                            into.insert(k.clone(), v.clone());
                        }
                    }
                    _ => {
                        into.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
            }
        }
        true
    }

    fn object(&mut self, obj: &Map<String, Value>, at: &str, depth: usize) -> Value {
        let required: Vec<&str> = obj
            .get("required")
            .and_then(Value::as_array)
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let mut out = Map::new();
        if let Some(props) = obj.get("properties").and_then(Value::as_object) {
            for (k, s) in props {
                let resolved = self.spec.follow(s).unwrap_or(s);
                let needed = required.contains(&k.as_str());
                if resolved.get("writeOnly") == Some(&Value::Bool(true)) && !needed {
                    continue;
                }
                match self.value(s, k, &key_at(at, k), depth + 1) {
                    Some(v) => {
                        out.insert(k.clone(), v);
                    }
                    None if needed => {
                        out.insert(k.clone(), stand_in(resolved));
                    }
                    None => {}
                }
            }
        }
        for name in required {
            if !out.contains_key(name) {
                let n = self.next(name);
                out.insert(name.to_owned(), Value::String(format!("{name}-{n}")));
            }
        }
        Value::Object(out)
    }

    fn array(&mut self, obj: &Map<String, Value>, name: &str, at: &str, depth: usize) -> Value {
        let min = obj.get("minItems").and_then(Value::as_u64).unwrap_or(0);
        let mut n = min.max(2);
        if let Some(max) = obj.get("maxItems").and_then(Value::as_u64) {
            n = n.min(max);
        }
        let Some(items) = obj.get("items") else {
            return Value::Array(Vec::new());
        };
        let mut out = Vec::new();
        for _ in 0..n {
            match self.value(items, name, &format!("{at}[]"), depth + 1) {
                Some(v) => out.push(v),
                None => break,
            }
        }
        Value::Array(out)
    }

    fn string(&mut self, obj: &Map<String, Value>, name: &str, at: &str) -> String {
        let format = obj.get("format").and_then(Value::as_str).unwrap_or("");
        if let Some(pattern) = obj.get("pattern").and_then(Value::as_str) {
            self.todo(format!("{at}: pattern {pattern} is not honored"));
        }
        let label = if name.is_empty() { "value" } else { name };
        let (date, time) = instant(self.seed);

        match format {
            "date-time" => format!("{date}T{time}Z"),
            "date" => date,
            "time" => time,
            "uuid" => uuid(&mut self.rng),
            "email" | "idn-email" => format!("user{}@example.com", self.next("@email")),
            "uri" | "url" | "uri-reference" | "iri" | "iri-reference" => format!(
                "https://example.com/{}/{}",
                url_safe(label),
                self.next(label)
            ),
            "hostname" | "idn-hostname" => format!("host{}.example.com", self.next("@host")),
            "ipv4" => format!("192.0.2.{}", self.next("@ipv4")),
            "ipv6" => format!("2001:db8::{:x}", self.next("@ipv6")),
            "byte" => {
                let n = self.next(label);
                base64::engine::general_purpose::STANDARD.encode(format!("{label}-{n}"))
            }
            _ => {
                let n = self.next(label);
                let mut text = format!("{label}-{n}");
                let min = obj.get("minLength").and_then(Value::as_u64).unwrap_or(0);
                let max = obj.get("maxLength").and_then(Value::as_u64);
                let min = usize::try_from(min).unwrap_or(usize::MAX).min(4096);
                while text.chars().count() < min {
                    text.push('x');
                }
                if let Some(max) = max.and_then(|m| usize::try_from(m).ok())
                    && text.chars().count() > max
                {
                    text = text.chars().take(max).collect();
                }
                text
            }
        }
    }

    fn number(&mut self, obj: &Map<String, Value>, integer: bool, at: &str) -> Value {
        let get = |k: &str| obj.get(k).and_then(Value::as_f64);
        let step = get("multipleOf").filter(|m| *m > 0.0);
        let unit = step.unwrap_or(1.0);
        let mut lo = get("minimum");
        let mut hi = get("maximum");
        // 3.0's booleans, 3.1's numbers.
        match obj.get("exclusiveMinimum") {
            Some(Value::Bool(true)) => lo = lo.map(|l| l + unit),
            Some(v) if v.is_number() => lo = v.as_f64().map(|l| l + unit),
            _ => {}
        }
        match obj.get("exclusiveMaximum") {
            Some(Value::Bool(true)) => hi = hi.map(|h| h - unit),
            Some(v) if v.is_number() => hi = v.as_f64().map(|h| h - unit),
            _ => {}
        }
        let (lo, hi) = match (lo, hi) {
            (Some(l), Some(h)) => (l, h),
            (Some(l), None) => (l, l.max(1.0) + 99.0),
            (None, Some(h)) => ((h - 99.0).min(1.0), h),
            (None, None) => (1.0, 100.0),
        };
        let unit = if integer { unit.max(1.0).ceil() } else { unit };
        let first = (lo / unit).ceil();
        let last = (hi / unit).floor();
        if !integer && step.is_none() && lo <= hi && (last < first || !last.is_finite()) {
            // A fraction in range, to two places.
            #[allow(clippy::cast_precision_loss)]
            let part = (self.rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
            let v = ((lo + (hi - lo) * part) * 100.0).round() / 100.0;
            return number_value(v.clamp(lo, hi), false);
        }
        if !first.is_finite() || !last.is_finite() || last < first {
            self.todo(format!(
                "{at}: no {} fits between {lo} and {hi}",
                if step.is_some() {
                    "multiple"
                } else {
                    "whole number"
                }
            ));
            return number_value(lo, integer);
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let span = ((last - first) as u64).saturating_add(1);
        // The high bits: splitmix's low ones repeat modulo small spans.
        let pick = (u128::from(self.rng.next_u64()) * u128::from(span.max(1))) >> 64;
        #[allow(clippy::cast_precision_loss)]
        let k = first + pick as f64;
        number_value(k * unit, integer)
    }
}

fn number_value(v: f64, integer: bool) -> Value {
    if (integer || v.fract() == 0.0) && v.abs() < 9e15 {
        #[allow(clippy::cast_possible_truncation)]
        return Value::from(v as i64);
    }
    serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number)
}

/// The type a schema stands for: its `type` (the first that is not
/// `null`), else what its keys say.
fn type_of(obj: &Map<String, Value>) -> &str {
    match obj.get("type") {
        Some(Value::String(t)) => return t,
        Some(Value::Array(ts)) => {
            if let Some(t) = ts.iter().filter_map(Value::as_str).find(|t| *t != "null") {
                return t;
            }
            return "null";
        }
        _ => {}
    }
    if obj.contains_key("properties") || obj.contains_key("additionalProperties") {
        "object"
    } else if obj.contains_key("items") {
        "array"
    } else if obj.contains_key("format") || obj.contains_key("pattern") {
        "string"
    } else {
        "any"
    }
}

/// A required property's value when it could not be made (a cycle, too
/// deep): `null` if it may be, else an empty array or object.
fn stand_in(schema: &Value) -> Value {
    let obj = schema.as_object();
    let nullable = obj.is_some_and(|o| {
        o.get("nullable") == Some(&Value::Bool(true))
            || o.get("type")
                .and_then(Value::as_array)
                .is_some_and(|t| t.iter().any(|t| t == "null"))
    });
    if nullable {
        Value::Null
    } else if obj.is_some_and(|o| type_of(o) == "array") {
        json!([])
    } else {
        json!({})
    }
}

/// `$.items[].name` style.
fn key_at(at: &str, key: &str) -> String {
    if !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_') {
        format!("{at}.{key}")
    } else {
        format!("{at}[{}]", Value::String(key.to_owned()))
    }
}

fn url_safe(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// The date and time of `date-time` values: 2026-01-01T00:00:00Z plus
/// `seed` hours (modulo about a century).
fn instant(seed: u64) -> (String, String) {
    let hours = i64::try_from(seed % 876_000).unwrap_or(0);
    let days = EPOCH_DAYS + hours / 24;
    let (y, m, d) = civil(days);
    (
        format!("{y:04}-{m:02}-{d:02}"),
        format!("{:02}:00:00", hours % 24),
    )
}

/// The date of `days` since 1970-01-01 (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// Data file names: `data/<set>/<operation>.json`, unique.
struct Names {
    dir: String,
    used: HashSet<String>,
}

impl Names {
    fn new(dir: &str) -> Self {
        Self {
            dir: dir.trim_end_matches('/').to_owned(),
            used: HashSet::new(),
        }
    }

    fn take(&mut self, stem: &str) -> String {
        let stem: String = stem
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let mut squeezed = String::with_capacity(stem.len());
        for c in stem.chars() {
            if !(c == '-' && squeezed.ends_with('-')) {
                squeezed.push(c);
            }
        }
        let stem = squeezed.trim_matches(['-', '.']);
        let stem = if stem.is_empty() { "body" } else { stem };
        let mut path = format!("{}/{stem}.json", self.dir);
        let mut n = 2;
        while !self.used.insert(path.to_ascii_lowercase()) {
            path = format!("{}/{stem}-{n}.json", self.dir);
            n += 1;
        }
        path
    }
}

/// A JSON value written to a data file.
fn pretty(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).unwrap_or_else(|_| b"null".to_vec());
    bytes.push(b'\n');
    bytes
}

/// One operation's `[[mock]]`.
fn mock_toml(
    spec: &OpenApi,
    op: &SpecOperation<'_>,
    base: &str,
    seed: u64,
    files: &mut Names,
    out: &mut Generated,
) -> String {
    let name = op.name();
    let path = format!("{base}{}", mock_path(op.path));
    let (status, response) = success(spec, op);
    let mut gen_ = Gen::new(spec, seed, &name);
    let mut parts: Vec<String> = Vec::new();
    if status != 200 {
        parts.push(format!("status = {status}"));
    }
    let mut file = None;
    match content(spec, op, response) {
        Content::None => {}
        Content::Json { schema, example } => {
            let given = example.is_some();
            let body = example.or_else(|| schema.as_ref().and_then(|s| gen_.value(s, "", "$", 0)));
            let body = body.unwrap_or_else(|| json!({}));
            if let Some(schema) = &schema {
                let problems = spec.check(schema, &body);
                if !problems.is_empty() {
                    gen_.todo(format!(
                        "{} does not fit the spec: {}",
                        if given {
                            "the spec's example"
                        } else {
                            "the generated body"
                        },
                        problems.join("; ")
                    ));
                }
            }
            let compact = serde_json::to_string(&body).unwrap_or_default();
            if let Some(inline) = toml_value(&body).filter(|_| compact.len() <= INLINE) {
                parts.push(format!("json = {inline}"));
            } else {
                let rel = files.take(&name);
                out.files.push((rel.clone(), pretty(&body)));
                parts.push(format!("json_file = {}", toml_string(&rel)));
                file = Some(rel);
            }
        }
        Content::Other { media, example } => {
            parts.push(format!(
                "headers = {{ content-type = {} }}",
                toml_string(&media)
            ));
            match example {
                Some(text) if !crate::template::is_template(&text) => {
                    parts.push(format!("body = {}", toml_string(&text)));
                }
                _ => {
                    parts.push("body = \"\"".to_owned());
                    gen_.todo(format!(
                        "the answer is {media}, which the spec gives no example of: add a body \
                         (body, body_file or file)"
                    ));
                }
            }
        }
    }
    let mut text = String::new();
    for todo in &gen_.todos {
        let _ = writeln!(text, "# TODO: {todo}");
        out.todos.push(format!("{name}: {todo}"));
    }
    text.push_str("[[mock]]\n");
    let _ = writeln!(text, "name = {}", toml_string(&name));
    let _ = writeln!(
        text,
        "match = {{ method = {}, path = {} }}",
        toml_string(&op.method),
        toml_string(&path)
    );
    if parts.is_empty() {
        text.push_str("respond = {}\n");
    } else {
        let _ = writeln!(text, "respond = {{ {} }}", parts.join(", "));
    }
    out.mocks.push(GenMock {
        name,
        method: op.method.clone(),
        path,
        status,
        file,
    });
    text
}

/// A list path and its item path, made one collection.
struct Pair<'a> {
    list: &'a str,
    item: &'a str,
    /// The operations it answers (method, spec path).
    taken: Vec<(String, String)>,
    /// The list body's array property, when the list is wrapped.
    wrap: Option<String>,
    /// The schema of an item.
    schema: Value,
}

/// The pairs `/things` (GET a list) and `/things/{id}` (GET an item) of
/// the operations in scope.
fn collection_pairs<'a>(spec: &'a OpenApi, ops: &[SpecOperation<'a>]) -> Vec<Pair<'a>> {
    let has = |method: &str, path: &str| ops.iter().find(|o| o.method == method && o.path == path);
    let mut out = Vec::new();
    for list_op in ops.iter().filter(|o| o.method == "GET") {
        let list = list_op.path;
        let Some(item_op) = ops.iter().find(|o| {
            o.method == "GET"
                && o.path
                    .strip_prefix(list.trim_end_matches('/'))
                    .and_then(|r| r.strip_prefix('/'))
                    .is_some_and(|last| {
                        last.starts_with('{') && last.ends_with('}') && !last[1..].contains('{')
                    })
        }) else {
            continue;
        };
        let item = item_op.path;
        let json_schema = |op: &SpecOperation<'a>| match content(spec, op, success(spec, op).1) {
            Content::Json {
                schema: Some(s), ..
            } => Some(s),
            _ => None,
        };
        let Some(list_schema) = json_schema(list_op) else {
            continue;
        };
        let resolved = spec.follow(&list_schema).unwrap_or(&list_schema).clone();
        let (items, wrap) = if type_of_value(&resolved) == "array" {
            (resolved.get("items").cloned(), None)
        } else if let Some((k, v)) = resolved
            .get("properties")
            .and_then(Value::as_object)
            .and_then(|props| {
                props
                    .iter()
                    .find(|(_, v)| type_of_value(spec.follow(v).unwrap_or(v)) == "array")
            })
        {
            let v = spec.follow(v).unwrap_or(v);
            (v.get("items").cloned(), Some(k.clone()))
        } else {
            continue;
        };
        let Some(schema) = items.or_else(|| json_schema(item_op)) else {
            continue;
        };
        let mut taken = Vec::new();
        for (method, path) in [
            ("GET", list),
            ("POST", list),
            ("GET", item),
            ("PUT", item),
            ("PATCH", item),
            ("DELETE", item),
        ] {
            if has(method, path).is_some() {
                taken.push((method.to_owned(), path.to_owned()));
            }
        }
        out.push(Pair {
            list,
            item,
            taken,
            wrap,
            schema,
        });
    }
    out
}

fn type_of_value(schema: &Value) -> &str {
    schema.as_object().map_or("any", type_of)
}

/// The parameter's schema type (`integer`, `string`, …) of path
/// parameter `name` of the operations at `path`.
fn param_type<'a>(spec: &'a OpenApi, path: &str, name: &str) -> Option<&'a str> {
    let ops = spec.operations();
    let op = ops.iter().find(|o| o.path == path)?;
    let lists = [op.item.get("parameters"), op.op.get("parameters")];
    for p in lists
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
        .flatten()
    {
        let p = spec.follow(p).unwrap_or(p);
        if p.get("name").and_then(Value::as_str) == Some(name)
            && p.get("in").and_then(Value::as_str) == Some("path")
        {
            let schema = p.get("schema").unwrap_or(p);
            let schema = spec.follow(schema).unwrap_or(schema);
            return Some(type_of_value(schema));
        }
    }
    None
}

/// The field a scoping parameter names in an item: itself, else a
/// property spelled alike (`user_id` for `userId`).
fn scope_field(param: &str, props: Option<&Map<String, Value>>) -> String {
    let plain = |s: &str| {
        s.chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase()
    };
    let Some(props) = props else {
        return param.to_owned();
    };
    if props.contains_key(param) {
        return param.to_owned();
    }
    props
        .keys()
        .find(|k| plain(k) == plain(param))
        .cloned()
        .unwrap_or_else(|| param.to_owned())
}

/// One `[[collection]]`.
#[allow(clippy::too_many_arguments)]
fn collection_toml(
    spec: &OpenApi,
    pair: &Pair<'_>,
    base: &str,
    seed: u64,
    names: &mut HashSet<String>,
    files: &mut Names,
    out: &mut Generated,
) -> String {
    let literals: Vec<&str> = pair
        .list
        .split('/')
        .filter(|s| !s.is_empty() && !s.contains('{'))
        .collect();
    let mut name = literals.last().map_or("items", |s| *s).to_owned();
    if names.contains(&name) && literals.len() > 1 {
        name = format!("{}-{name}", literals[literals.len() - 2]);
    }
    let stem = name.clone();
    let mut n = 2;
    while !names.insert(name.clone()) {
        name = format!("{stem}-{n}");
        n += 1;
    }
    let path = format!("{base}{}", mock_path(pair.list));
    let mut gen_ = Gen::new(spec, seed, &format!("collection {name}"));
    let resolved = {
        let mut s = spec.follow(&pair.schema).unwrap_or(&pair.schema).clone();
        if let Some(all) = s.get("allOf").and_then(Value::as_array).cloned() {
            let mut merged = Map::new();
            gen_.merge(&all, &mut merged, &mut Vec::new(), 0);
            s = Value::Object(merged);
        }
        s
    };
    let props = resolved.get("properties").and_then(Value::as_object);
    let item_param = pair
        .item
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim_matches(['{', '}'])
        .to_owned();
    // `id`, unless only the path parameter's name is a property.
    let id = match props {
        Some(p) if !p.contains_key("id") && p.contains_key(&item_param) => item_param,
        _ => "id".to_owned(),
    };
    // Parameters of the list path scope the items.
    let mut params: Vec<(String, String)> = Vec::new();
    let mut scoped: Vec<(String, Value)> = Vec::new();
    for seg in pair.list.split('/') {
        let Some(param) = seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) else {
            continue;
        };
        let glob_name = mock_path(seg);
        let glob_name = glob_name.trim_start_matches(':').to_owned();
        let field = scope_field(&glob_name, props);
        let field_type = props
            .and_then(|p| p.get(&field))
            .map(|s| type_of_value(spec.follow(s).unwrap_or(s)).to_owned())
            .or_else(|| param_type(spec, pair.list, param).map(str::to_owned));
        let value = match field_type.as_deref() {
            Some("integer" | "number") => json!(1),
            _ => json!("1"),
        };
        if field != glob_name {
            params.push((glob_name, field.clone()));
        }
        scoped.push((field, value));
    }
    let mut items = Vec::new();
    for i in 0..ITEMS {
        let mut item = gen_
            .value(&pair.schema, "", "$[]", 1)
            .unwrap_or_else(|| json!({}));
        if let Some(obj) = item.as_object_mut() {
            let id_type = props
                .and_then(|p| p.get(&id))
                .map_or("integer", |s| type_of_value(spec.follow(s).unwrap_or(s)));
            let uuid_format = props
                .and_then(|p| p.get(&id))
                .map(|s| spec.follow(s).unwrap_or(s))
                .and_then(|s| s.get("format"))
                .and_then(Value::as_str)
                == Some("uuid");
            let value = match id_type {
                "string" if uuid_format => json!(uuid(&mut gen_.rng)),
                "string" => json!((i + 1).to_string()),
                _ => json!(i + 1),
            };
            obj.insert(id.clone(), value);
            for (field, value) in &scoped {
                obj.insert(field.clone(), value.clone());
            }
        }
        items.push(item);
    }
    if let Some(item_schema) = spec.item_schema(&format!("{base}{}", mock_path(pair.item))) {
        for item in &items {
            let problems = spec.check(item_schema, item);
            if !problems.is_empty() {
                gen_.todo(format!(
                    "a generated item does not fit the spec: {}",
                    problems.join("; ")
                ));
                break;
            }
        }
    }
    let mut text = String::new();
    for todo in &gen_.todos {
        let _ = writeln!(text, "# TODO: {todo}");
        out.todos.push(format!("collection {name}: {todo}"));
    }
    text.push_str("[[collection]]\n");
    let _ = writeln!(text, "name = {}", toml_string(&name));
    let _ = writeln!(text, "path = {}", toml_string(&path));
    if !params.is_empty() {
        let pairs: Vec<String> = params
            .iter()
            .map(|(p, f)| format!("{} = {}", toml_key(p), toml_string(f)))
            .collect();
        let _ = writeln!(text, "params = {{ {} }}", pairs.join(", "));
    }
    if id != "id" {
        let _ = writeln!(text, "id = {}", toml_string(&id));
    }
    let inline: Option<Vec<String>> = items
        .iter()
        .map(|item| toml_value(item).filter(|t| t.len() <= INLINE_ITEM))
        .collect();
    let mut file = None;
    if let Some(lines) = inline {
        text.push_str("items = [\n");
        for line in lines {
            let _ = writeln!(text, "  {line},");
        }
        text.push_str("]\n");
    } else {
        let rel = files.take(&name);
        out.files.push((rel.clone(), pretty(&Value::Array(items))));
        let _ = writeln!(text, "seed = {}", toml_string(&rel));
        file = Some(rel);
    }
    if let Some(wrap) = &pair.wrap {
        let _ = writeln!(text, "list = {{ wrap = {} }}", toml_string(wrap));
    }
    out.collections.push(GenCollection {
        name,
        path,
        operations: pair.taken.iter().map(|(m, p)| format!("{m} {p}")).collect(),
        file,
    });
    text
}
