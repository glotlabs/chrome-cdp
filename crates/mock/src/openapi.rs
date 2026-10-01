//! OpenAPI specs for `mock check` (plan 11, milestone 11.8): which
//! operation a mock stands for, and whether its status and JSON body are
//! ones the spec allows.
//!
//! Specs are OpenAPI 3.x or Swagger 2.0, JSON or YAML. Only local `$ref`s
//! (`#/components/…`) are followed. Bodies are checked for their shape:
//! types, required and unknown properties, enums, array items, and the
//! `allOf`/`anyOf`/`oneOf` combinations; formats, patterns and bounds are
//! not, since mock data rarely drifts that way. Strings that are templates
//! (`{{uuid}}`) stand for any value.

use std::path::Path;

use serde_json::{Map, Value};
use yaml_rust2::{Yaml, YamlLoader};

use crate::template::is_template;

/// How many body problems one mock reports.
const MAX_PROBLEMS: usize = 5;
/// How deep `$ref`s and nesting are followed.
const MAX_DEPTH: usize = 64;

/// A loaded spec.
#[derive(Debug, Clone)]
pub struct OpenApi {
    doc: Value,
    /// Path prefixes of the servers (`/v1` of `https://api.example.com/v1`);
    /// `""` when none.
    bases: Vec<String>,
    paths: Vec<SpecPath>,
    swagger: bool,
    /// The servers' URLs as written (`basePath` for Swagger 2), in order.
    servers: Vec<String>,
}

#[derive(Debug, Clone)]
struct SpecPath {
    /// As written: `/products/{id}`.
    template: String,
    /// `None` for a `{param}` segment.
    segments: Vec<Option<String>>,
}

/// The operation a mock's path and method stand for.
#[derive(Debug, Clone)]
pub struct Operation<'a> {
    /// `/products/{id}`.
    pub path: &'a str,
    /// Upper case.
    pub method: String,
    op: &'a Value,
}

/// Where a mock's path lands in the spec.
#[derive(Debug)]
pub enum Lookup<'a> {
    /// The path has an operation for every method asked for.
    Found(Vec<Operation<'a>>),
    /// The path is there, without these methods; it has the others.
    NoMethod {
        path: &'a str,
        missing: Vec<String>,
        has: Vec<String>,
    },
    /// No path of the spec matches; the closest ones.
    NoPath { closest: Vec<&'a str> },
    /// The mock's path is too broad to say (`/api/**`, a regex).
    Unknown,
}

/// An operation of the spec, as `mock gen` and the `openapi-uncovered`
/// rule walk them.
#[derive(Debug, Clone)]
pub struct SpecOperation<'a> {
    /// `/products/{id}`.
    pub path: &'a str,
    /// Upper case.
    pub method: String,
    /// `operationId`, if it has one.
    pub id: Option<&'a str>,
    pub(crate) op: &'a Value,
    /// The path item (for parameters shared by its operations).
    pub(crate) item: &'a Value,
}

impl SpecOperation<'_> {
    /// `operationId`, else `GET /products/{id}`.
    pub fn name(&self) -> String {
        self.id
            .map_or_else(|| format!("{} {}", self.method, self.path), str::to_owned)
    }
}

const METHODS: &[&str] = &[
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

impl OpenApi {
    /// Reads and parses `file` (JSON if it ends in `.json`, else YAML).
    pub fn load(file: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(file)
            .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        let json = file
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        Self::parse(&text, json).map_err(|e| format!("{}: {e}", file.display()))
    }

    /// Parses a spec's text.
    pub fn parse(text: &str, json: bool) -> Result<Self, String> {
        let doc = if json {
            serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?
        } else {
            let docs = YamlLoader::load_from_str(text).map_err(|e| {
                let m = e.marker();
                format!("not YAML: line {}: {}", m.line(), e.info())
            })?;
            docs.into_iter().next().map_or(Value::Null, yaml_json)
        };
        let swagger = doc.get("swagger").is_some();
        if doc.get("openapi").is_none() && !swagger {
            return Err("no `openapi` (or `swagger`) version; is it an OpenAPI spec?".into());
        }
        let paths = doc
            .get("paths")
            .and_then(Value::as_object)
            .ok_or("it has no `paths`")?
            .keys()
            .map(|template| SpecPath {
                template: template.clone(),
                segments: segments(template)
                    .into_iter()
                    .map(|s| (!(s.starts_with('{') && s.ends_with('}'))).then(|| s.to_owned()))
                    .collect(),
            })
            .collect();
        let servers: Vec<String> = if swagger {
            doc.get("basePath")
                .and_then(Value::as_str)
                .map(|b| vec![b.to_owned()])
                .unwrap_or_default()
        } else {
            doc.get("servers")
                .and_then(Value::as_array)
                .map(|servers| {
                    servers
                        .iter()
                        .filter_map(|s| Some(s.get("url")?.as_str()?.to_owned()))
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut bases: Vec<String> = servers.iter().map(|s| server_path(s)).collect();
        if bases.is_empty() {
            bases.push(String::new());
        }
        bases.sort_by_key(|b| std::cmp::Reverse(b.len()));
        bases.dedup();
        Ok(Self {
            doc,
            bases,
            paths,
            swagger,
            servers,
        })
    }

    /// The path prefix of the spec's first server (`/v1`), `""` for none:
    /// what `mock gen` puts before each path.
    pub fn base(&self) -> String {
        self.servers
            .first()
            .map(|s| server_path(s))
            .unwrap_or_default()
    }

    /// The path prefixes of all its servers, first server first, without
    /// repeats; empty when they have none.
    pub fn server_paths(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for path in self.servers.iter().map(|s| server_path(s)) {
            if !path.is_empty() && !out.contains(&path) {
                out.push(path);
            }
        }
        out
    }

    /// Whether the spec is about the requests of a set with `scope`: a
    /// set with no scope or a path scope is (the spec's servers are the
    /// app's own), a set scoped to full URLs only if one of them takes a
    /// server of the spec that has a host. A set of a third party's
    /// mocks (`https://api.stripe.com/**`) is not checked against the
    /// app's spec.
    pub fn covers_scope(&self, scope: &[String]) -> bool {
        if scope.is_empty() || scope.iter().any(|g| !g.contains("://")) {
            return true;
        }
        self.servers
            .iter()
            .filter(|s| s.contains("://"))
            .any(|server| {
                let sample = format!(
                    "{}/__any__",
                    server
                        .split('{')
                        .next()
                        .unwrap_or(server)
                        .trim_end_matches('/')
                );
                scope
                    .iter()
                    .any(|g| crate::glob::PathGlob::url(g).is_ok_and(|glob| glob.matches(&sample)))
            })
    }

    /// Every operation, by path (as the spec's keys sort) and method.
    pub fn operations(&self) -> Vec<SpecOperation<'_>> {
        let mut out = Vec::new();
        let Some(paths) = self.doc.get("paths").and_then(Value::as_object) else {
            return out;
        };
        for (path, item) in paths {
            let item = self.resolve(item, 0).unwrap_or(item);
            for method in METHODS {
                if let Some(op) = item.get(*method).filter(|o| o.is_object()) {
                    out.push(SpecOperation {
                        path,
                        method: method.to_ascii_uppercase(),
                        id: op.get("operationId").and_then(Value::as_str),
                        op,
                        item,
                    });
                }
            }
        }
        out
    }

    /// The whole document, for walking what the helpers do not cover.
    pub(crate) fn doc(&self) -> &Value {
        &self.doc
    }

    /// Whether it is a Swagger 2.0 spec.
    pub(crate) fn is_swagger(&self) -> bool {
        self.swagger
    }

    /// `schema` with a local `$ref` followed; `None` when it has none (or
    /// it leads nowhere).
    pub(crate) fn follow<'a>(&'a self, schema: &'a Value) -> Option<&'a Value> {
        self.resolve(schema, 0)
    }

    /// The operations of a mock with path glob `path` and `methods` (upper
    /// case; empty for any).
    pub fn lookup(&self, path: &str, methods: &[String]) -> Lookup<'_> {
        if path.contains("**") {
            return Lookup::Unknown;
        }
        let mut best: Option<(&SpecPath, usize)> = None;
        for base in &self.bases {
            let Some(rest) = strip_base(path, base) else {
                continue;
            };
            let segs = segments(rest);
            for spec in &self.paths {
                if let Some(score) = fits(&segs, &spec.segments)
                    && best.is_none_or(|(_, s)| score > s)
                {
                    best = Some((spec, score));
                }
            }
        }
        let Some((spec, _)) = best else {
            let shown = path;
            let mut closest: Vec<(&str, f64)> = self
                .paths
                .iter()
                .flat_map(|p| {
                    self.bases.iter().map(move |b| {
                        let full = format!("{b}{}", p.template);
                        (
                            p.template.as_str(),
                            strsim::normalized_levenshtein(shown, &full),
                        )
                    })
                })
                .filter(|(_, score)| *score >= 0.6)
                .collect();
            closest.sort_by(|a, b| b.1.total_cmp(&a.1));
            // Only those about as close as the closest.
            let best = closest.first().map_or(0.0, |c| c.1);
            let mut names: Vec<&str> = closest
                .into_iter()
                .filter(|(_, score)| *score >= best - 0.05)
                .map(|(p, _)| p)
                .collect();
            names.dedup();
            names.truncate(3);
            return Lookup::NoPath { closest: names };
        };
        let item = &self.doc["paths"][&spec.template];
        let has: Vec<String> = METHODS
            .iter()
            .filter(|m| item.get(**m).is_some())
            .map(|m| m.to_ascii_uppercase())
            .collect();
        let wanted: Vec<String> = if methods.is_empty() {
            has.clone()
        } else {
            methods.to_vec()
        };
        let missing: Vec<String> = wanted
            .iter()
            .filter(|m| !has.contains(m))
            .cloned()
            .collect();
        if !missing.is_empty() {
            return Lookup::NoMethod {
                path: &spec.template,
                missing,
                has,
            };
        }
        Lookup::Found(
            wanted
                .into_iter()
                .map(|method| Operation {
                    path: &spec.template,
                    op: &item[method.to_ascii_lowercase()],
                    method,
                })
                .collect(),
        )
    }

    /// The statuses operation `op` lists (`200`, `4XX`, `default`).
    pub fn statuses(op: &Operation<'_>) -> Vec<String> {
        op.op
            .get("responses")
            .and_then(Value::as_object)
            .map(|r| r.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// The response of `op` for `status`: exact, then `2XX`, then
    /// `default`. `None` when the spec does not allow the status.
    fn response<'a>(&'a self, op: &Operation<'a>, status: u16) -> Option<&'a Value> {
        let responses = op.op.get("responses")?.as_object()?;
        let exact = status.to_string();
        let range = format!("{}XX", status / 100);
        let found = responses
            .get(&exact)
            .or_else(|| {
                responses
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(&range))
                    .map(|(_, v)| v)
            })
            .or_else(|| responses.get("default"))?;
        Some(self.resolve(found, 0).unwrap_or(found))
    }

    /// Whether `op` allows `status`.
    pub fn allows(&self, op: &Operation<'_>, status: u16) -> bool {
        self.response(op, status).is_some()
    }

    /// The problems of JSON `body` as `op`'s `status` answer (at most a
    /// few), as `$.items[0].price: …`. None when the spec gives no JSON
    /// schema for it.
    pub fn check_body(&self, op: &Operation<'_>, status: u16, body: &Value) -> Vec<String> {
        let Some(response) = self.response(op, status) else {
            return Vec::new();
        };
        let schema = if self.swagger {
            response.get("schema")
        } else {
            response
                .get("content")
                .and_then(Value::as_object)
                .and_then(|content| {
                    content
                        .iter()
                        .find(|(k, _)| k.contains("json"))
                        .or_else(|| content.iter().find(|(k, _)| k.as_str() == "*/*"))
                        .and_then(|(_, v)| v.get("schema"))
                })
        };
        let Some(schema) = schema else {
            return Vec::new();
        };
        self.check(schema, body)
    }

    /// The problems of `value` against `schema` (a schema of this spec).
    pub fn check(&self, schema: &Value, value: &Value) -> Vec<String> {
        let mut out = Vec::new();
        self.validate(schema, value, "$", &mut out, 0);
        out.truncate(MAX_PROBLEMS);
        out
    }

    /// The schema of the items of the collection at `path` (its item
    /// path's GET, 200), if the spec says.
    pub fn item_schema(&self, item_path: &str) -> Option<&Value> {
        let Lookup::Found(ops) = self.lookup(item_path, &["GET".to_owned()]) else {
            return None;
        };
        let op = ops.into_iter().next()?;
        let response = self.response(&op, 200)?;
        if self.swagger {
            return response.get("schema");
        }
        response
            .get("content")?
            .as_object()?
            .iter()
            .find(|(k, _)| k.contains("json"))?
            .1
            .get("schema")
    }

    /// `schema` with a `$ref` followed (local refs only).
    fn resolve<'a>(&'a self, schema: &'a Value, depth: usize) -> Option<&'a Value> {
        let reference = schema.get("$ref")?.as_str()?;
        if depth > MAX_DEPTH {
            return None;
        }
        let pointer = reference.strip_prefix('#')?;
        let target = self.doc.pointer(pointer)?;
        Some(self.resolve(target, depth + 1).unwrap_or(target))
    }

    fn validate(
        &self,
        schema: &Value,
        value: &Value,
        at: &str,
        out: &mut Vec<String>,
        depth: usize,
    ) {
        if out.len() >= MAX_PROBLEMS || depth > MAX_DEPTH {
            return;
        }
        let schema = if schema.get("$ref").is_some() {
            match self.resolve(schema, 0) {
                Some(s) => s,
                None => return,
            }
        } else {
            schema
        };
        let Some(obj) = schema.as_object() else {
            return;
        };
        if let Value::String(s) = value
            && is_template(s)
        {
            return;
        }
        // Before `allOf`: `nullable` next to `allOf: [$ref]` is how 3.0
        // makes a reference nullable.
        let nullable = obj.get("nullable").and_then(Value::as_bool) == Some(true);
        if value.is_null() && nullable {
            return;
        }
        if let Some(all) = obj.get("allOf").and_then(Value::as_array) {
            for s in all {
                self.validate(s, value, at, out, depth + 1);
            }
        }
        for key in ["anyOf", "oneOf"] {
            if let Some(options) = obj.get(key).and_then(Value::as_array) {
                let fits = options.iter().any(|s| {
                    let mut tried = Vec::new();
                    self.validate(s, value, at, &mut tried, depth + 1);
                    tried.is_empty()
                });
                if !fits {
                    out.push(format!("{at}: fits none of the spec's {key} schemas"));
                    return;
                }
            }
        }
        if let Some(types) = obj.get("type") {
            let types: Vec<&str> = match types {
                Value::String(t) => vec![t.as_str()],
                Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            if !types.is_empty() && !types.iter().any(|t| is_type(value, t)) {
                out.push(format!(
                    "{at}: the spec says {}, the mock has {}",
                    types.join(" or "),
                    shown(value)
                ));
                return;
            }
        }
        if let Some(options) = obj.get("enum").and_then(Value::as_array)
            && !options.iter().any(|o| same(o, value))
        {
            let listed: Vec<String> = options.iter().take(6).map(Value::to_string).collect();
            out.push(format!(
                "{at}: the spec allows {}, the mock has {}",
                listed.join(", "),
                shown(value)
            ));
            return;
        }
        if let Some(c) = obj.get("const")
            && !same(c, value)
        {
            out.push(format!(
                "{at}: the spec says {c}, the mock has {}",
                shown(value)
            ));
            return;
        }
        match value {
            Value::Object(map) => self.object(obj, map, at, out, depth),
            Value::Array(items) => {
                if let Some(item) = obj.get("items") {
                    for (i, v) in items.iter().enumerate() {
                        self.validate(item, v, &format!("{at}[{i}]"), out, depth + 1);
                        if out.len() >= MAX_PROBLEMS {
                            return;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn object(
        &self,
        schema: &Map<String, Value>,
        map: &Map<String, Value>,
        at: &str,
        out: &mut Vec<String>,
        depth: usize,
    ) {
        let props = schema.get("properties").and_then(Value::as_object);
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for name in required.iter().filter_map(Value::as_str) {
                if !map.contains_key(name) {
                    out.push(format!(
                        "{}: the spec requires it, the mock has none",
                        key_at(at, name)
                    ));
                }
            }
        }
        for (name, v) in map {
            let here = key_at(at, name);
            match props.and_then(|p| p.get(name)) {
                Some(s) => self.validate(s, v, &here, out, depth + 1),
                None => match schema.get("additionalProperties") {
                    Some(Value::Bool(false)) => {
                        let known: Vec<&str> = props
                            .map(|p| p.keys().map(String::as_str).collect())
                            .unwrap_or_default();
                        let close = protocol::suggest::similar(name, known.iter().copied(), 1)
                            .first()
                            .map(|c| (*c).to_owned());
                        out.push(match close {
                            Some(c) => format!("{here}: not in the spec (did you mean `{c}`?)"),
                            None => format!("{here}: not in the spec"),
                        });
                    }
                    Some(s @ Value::Object(_)) => self.validate(s, v, &here, out, depth + 1),
                    _ => {}
                },
            }
            if out.len() >= MAX_PROBLEMS {
                return;
            }
        }
    }
}

/// `$.items[0].name` style.
fn key_at(at: &str, key: &str) -> String {
    if !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_') {
        format!("{at}.{key}")
    } else {
        format!("{at}[{}]", Value::String(key.to_owned()))
    }
}

fn is_type(value: &Value, t: &str) -> bool {
    match t {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => {
            value.is_i64() || value.is_u64() || value.as_f64().is_some_and(|f| f.fract() == 0.0)
        }
        _ => true,
    }
}

/// Equal, numbers by value.
fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < f64::EPSILON,
        _ => a == b,
    }
}

/// `a string ("12")`, `a number (3)`, `null`.
fn shown(value: &Value) -> String {
    let cut = |s: String| {
        if s.chars().count() > 30 {
            format!("{}…", s.chars().take(29).collect::<String>())
        } else {
            s
        }
    };
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => format!("a boolean ({b})"),
        Value::Number(n) => format!("a number ({n})"),
        Value::String(s) => format!("a string ({})", cut(Value::String(s.clone()).to_string())),
        Value::Array(_) => "an array".into(),
        Value::Object(_) => "an object".into(),
    }
}

/// `/v1` of `https://api.example.com/v1/`, `/api` of `/api`; `""` for none.
fn server_path(url: &str) -> String {
    let path = match url.find("://") {
        Some(i) => {
            let rest = &url[i + 3..];
            rest.find('/').map_or("", |j| &rest[j..])
        }
        None => url,
    };
    // Server variables (`/{version}`) are not known: leave them out.
    let path = path.split('{').next().unwrap_or(path);
    path.trim_end_matches('/').to_owned()
}

/// The part of mock path `path` below `base`, if it is under it.
fn strip_base<'a>(path: &'a str, base: &str) -> Option<&'a str> {
    if base.is_empty() {
        return Some(path);
    }
    let rest = path.strip_prefix(base)?;
    (rest.is_empty() || rest.starts_with('/')).then_some(rest)
}

fn segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

/// How well mock path segments `mock` fit spec segments `spec` (`None`
/// for a literal): the number of literal segments that agree, or `None`
/// if they do not fit. A mock's `:param` and `*` fit only a spec's
/// parameter.
fn fits(mock: &[&str], spec: &[Option<String>]) -> Option<usize> {
    if mock.len() != spec.len() {
        return None;
    }
    let mut score = 0;
    for (m, s) in mock.iter().zip(spec) {
        let wild = m.starts_with(':') || m.contains('*');
        match s {
            None => {}
            Some(lit) if !wild && crate::glob::decode(m) == *lit => score += 1,
            Some(_) => return None,
        }
    }
    Some(score)
}

/// A YAML document as JSON (keys as text).
fn yaml_json(yaml: Yaml) -> Value {
    match yaml {
        Yaml::Real(s) => s
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::String(s), Value::Number),
        Yaml::Integer(i) => Value::from(i),
        Yaml::String(s) => Value::String(s),
        Yaml::Boolean(b) => Value::Bool(b),
        Yaml::Array(items) => Value::Array(items.into_iter().map(yaml_json).collect()),
        Yaml::Hash(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| {
                    let key = match k {
                        Yaml::String(s) | Yaml::Real(s) => s,
                        Yaml::Integer(i) => i.to_string(),
                        Yaml::Boolean(b) => b.to_string(),
                        _ => String::new(),
                    };
                    (key, yaml_json(v))
                })
                .collect(),
        ),
        Yaml::Alias(_) | Yaml::Null | Yaml::BadValue => Value::Null,
    }
}
