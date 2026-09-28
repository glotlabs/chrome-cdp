//! `[[collection]]`: a fake REST resource kept in memory (plan 11,
//! milestone 11.3). `GET path` lists the items, `GET path/:id` gets one,
//! `POST path` adds one, `PUT`/`PATCH path/:id` replace or change one and
//! `DELETE path/:id` removes it, so create-then-list flows work without
//! code. Validation is minimal on purpose: a `[[mock]]` in front of the
//! collection handles anything special.

use std::path::Path;

use serde_json::{Map, Value, json};

use crate::file::NewId;
use crate::glob::PathGlob;
use crate::matcher::Parsed;
use crate::respond::Response;
use crate::template::{Rng, uuid};

/// The parameter of the item path that holds the id.
const ITEM: &str = "chrome_cdp_item";

/// The page size of a paged list when neither the request nor the file
/// gives one.
pub(crate) const PAGE_SIZE: u64 = 20;

/// How `GET path` lists the items.
#[derive(Debug, Clone, Default)]
pub(crate) struct List {
    pub(crate) filter_query: bool,
    pub(crate) page_param: Option<String>,
    pub(crate) page_size_param: Option<String>,
    pub(crate) page_size: u64,
    pub(crate) wrap: Option<String>,
}

/// A collection, checked, with its items.
#[derive(Debug, Clone)]
pub(crate) struct Collection {
    pub(crate) name: String,
    pub(crate) line: usize,
    pub(crate) path: PathGlob,
    pub(crate) item: PathGlob,
    /// The id field.
    pub(crate) id: String,
    pub(crate) new_id: NewId,
    /// The data file the items start from (relative to the mocks
    /// directory), if they come from one.
    pub(crate) seed_file: Option<String>,
    /// The items it starts with (and starts over with on `mock reset`).
    pub(crate) seed: Vec<Value>,
    pub(crate) list: List,
    /// The items now.
    pub(crate) items: Vec<Value>,
    pub(crate) hits: u64,
}

/// What a request asks of a collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Op {
    List,
    Get(String),
    Create,
    Replace(String),
    Update(String),
    Delete(String),
    /// The path is the collection's, the method is not one it takes.
    NotAllowed(&'static str),
}

impl Op {
    /// `list`, `get 7`, `create`, …
    pub(crate) fn text(&self) -> String {
        match self {
            Self::List => "list".into(),
            Self::Get(id) => format!("get {id}"),
            Self::Create => "create".into(),
            Self::Replace(id) => format!("replace {id}"),
            Self::Update(id) => format!("update {id}"),
            Self::Delete(id) => format!("delete {id}"),
            Self::NotAllowed(_) => "method not allowed".into(),
        }
    }
}

/// The text of an id: strings as they are, numbers and others as JSON.
pub(crate) fn id_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Checks items a collection starts with (or is seeded with): objects,
/// each with a unique `id` field.
pub(crate) fn check_items(value: Value, id: &str) -> Result<Vec<Value>, String> {
    let Value::Array(items) = value else {
        return Err("the items must be a JSON array of objects".into());
    };
    let mut seen = std::collections::HashSet::new();
    for (i, item) in items.iter().enumerate() {
        let Some(object) = item.as_object() else {
            return Err(format!("item {i} is not an object: {}", shown(item)));
        };
        let Some(value) = object.get(id).filter(|v| !v.is_null()) else {
            return Err(format!("item {i} has no `{id}` field: {}", shown(item)));
        };
        if !seen.insert(id_text(value)) {
            return Err(format!("item {i}: `{id}` {value} appears twice"));
        }
    }
    Ok(items)
}

/// Reads data file `rel` of mocks directory `base` as a collection's
/// items.
pub(crate) fn read_items(base: &Path, rel: &str, id: &str) -> Result<Vec<Value>, String> {
    let file = base.join(rel);
    let bytes = std::fs::read(&file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("{} is not JSON: {e}", file.display()))?;
    check_items(value, id).map_err(|e| format!("{}: {e}", file.display()))
}

/// The id kind the items suggest: numbers count up, anything else gets
/// uuids.
pub(crate) fn default_new_id(items: &[Value], id: &str) -> NewId {
    if items
        .iter()
        .all(|item| item[id].is_i64() || item[id].is_u64())
    {
        NewId::Increment
    } else {
        NewId::Uuid
    }
}

fn shown(value: &Value) -> String {
    let text = value.to_string();
    let cut: String = text.chars().take(60).collect();
    if cut.len() < text.len() {
        format!("{cut}…")
    } else {
        cut
    }
}

impl Collection {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        name: String,
        line: usize,
        path: PathGlob,
        id: String,
        new_id: NewId,
        seed_file: Option<String>,
        seed: Vec<Value>,
        list: List,
    ) -> Result<Self, String> {
        let text = path.to_string();
        let item = PathGlob::path(&format!("{}/:{ITEM}", text.trim_end_matches('/')))?;
        Ok(Self {
            name,
            line,
            path,
            item,
            id,
            new_id,
            seed_file,
            items: seed.clone(),
            seed,
            list,
            hits: 0,
        })
    }

    /// What a request for `path` with `method` asks, or why the
    /// collection does not take it.
    pub(crate) fn op(&self, path: &str, method: &str) -> Result<Op, String> {
        if self.path.matches(path) {
            return Ok(match method {
                "GET" => Op::List,
                "POST" => Op::Create,
                _ => Op::NotAllowed("GET, POST"),
            });
        }
        if let Some(params) = self.item.captures(path) {
            let id = params.get(ITEM).cloned().unwrap_or_default();
            return Ok(match method {
                "GET" => Op::Get(id),
                "PUT" => Op::Replace(id),
                "PATCH" => Op::Update(id),
                "DELETE" => Op::Delete(id),
                _ => Op::NotAllowed("GET, PUT, PATCH, DELETE"),
            });
        }
        let cut: String = path.chars().take(60).collect();
        Err(format!(
            "path differs: expects {} or {}/:id, got {cut}",
            self.path,
            self.path.to_string().trim_end_matches('/')
        ))
    }

    /// `/api/todos, /api/todos/:id`.
    pub(crate) fn summary(&self) -> String {
        let path = self.path.to_string();
        format!("{path}, {}/:id", path.trim_end_matches('/'))
    }

    fn position(&self, id: &str) -> Option<usize> {
        self.items
            .iter()
            .position(|item| item.get(&self.id).is_some_and(|value| id_text(value) == id))
    }

    /// Whether `id` names an item.
    pub(crate) fn has(&self, id: &str) -> bool {
        self.position(id).is_some()
    }

    /// Starts the items over from the seed.
    pub(crate) fn reset(&mut self) {
        self.items = self.seed.clone();
        self.hits = 0;
    }

    /// Carries out `op` for request `req` and answers it; the `String` is
    /// what the journal says (`todos: created 4`).
    pub(crate) fn handle(
        &mut self,
        op: &Op,
        req: &Parsed,
        rng: &mut Rng,
        cors: bool,
    ) -> (Response, String) {
        self.hits += 1;
        let name = self.name.clone();
        let error = |status: u16, message: String| {
            (
                Response::json(status, &json!({ "error": message }), cors),
                format!("{name}: {message}"),
            )
        };
        let not_found = |id: &str| error(404, format!("not found: {name}/{id}"));
        match op {
            Op::NotAllowed(allow) => {
                let (mut response, note) = error(
                    405,
                    format!("{name} takes {allow} here, not {}", req.method),
                );
                push_header(&mut response, "Allow", allow);
                (response, note)
            }
            Op::List => self.list(req, cors),
            Op::Get(id) => match self.position(id) {
                Some(i) => (
                    Response::json(200, &self.items[i], cors),
                    format!("{name}: get {id}"),
                ),
                None => not_found(id),
            },
            Op::Create => {
                let Some(Value::Object(mut item)) = req.json.clone() else {
                    return error(400, format!("{name}: POST needs a JSON object body"));
                };
                match item.get(&self.id).filter(|v| !v.is_null()) {
                    Some(id) if self.has(&id_text(id)) => {
                        let id = id_text(id);
                        return error(409, format!("{name}/{id} exists already"));
                    }
                    Some(_) => {}
                    None => {
                        let id = self.next_id(rng);
                        item.insert(self.id.clone(), id);
                    }
                }
                let id = id_text(&item[&self.id]);
                let item = Value::Object(item);
                self.items.push(item.clone());
                let mut response = Response::json(201, &item, cors);
                let location = format!("{}/{id}", req.path.trim_end_matches('/'));
                push_header(&mut response, "Location", &location);
                (response, format!("{name}: created {id}"))
            }
            Op::Replace(id) | Op::Update(id) => {
                let Some(i) = self.position(id) else {
                    return not_found(id);
                };
                let Some(Value::Object(body)) = req.json.clone() else {
                    return error(
                        400,
                        format!("{name}: {} needs a JSON object body", req.method),
                    );
                };
                let key = self.items[i][&self.id].clone();
                let item = if matches!(op, Op::Replace(_)) {
                    let mut item = body;
                    item.insert(self.id.clone(), key);
                    Value::Object(item)
                } else {
                    let mut item = self.items[i].clone();
                    merge(&mut item, &Value::Object(body));
                    if let Value::Object(map) = &mut item {
                        map.insert(self.id.clone(), key);
                    }
                    item
                };
                self.items[i] = item.clone();
                let what = if matches!(op, Op::Replace(_)) {
                    "replaced"
                } else {
                    "updated"
                };
                (
                    Response::json(200, &item, cors),
                    format!("{name}: {what} {id}"),
                )
            }
            Op::Delete(id) => match self.position(id) {
                Some(i) => {
                    let item = self.items.remove(i);
                    (
                        Response::json(200, &item, cors),
                        format!("{name}: deleted {id}"),
                    )
                }
                None => not_found(id),
            },
        }
    }

    /// `GET path`: the items the query filters, a page of them if the
    /// list pages, wrapped if it wraps.
    fn list(&self, req: &Parsed, cors: bool) -> (Response, String) {
        let list = &self.list;
        let paging = |key: &Option<String>| {
            key.as_deref().and_then(|key| {
                req.query
                    .iter()
                    .find(|(k, _)| k == key)
                    .and_then(|(_, v)| v.trim().parse::<u64>().ok())
            })
        };
        let mut items: Vec<&Value> = self.items.iter().collect();
        if list.filter_query {
            for (key, value) in &req.query {
                let is_paging = [&list.page_param, &list.page_size_param]
                    .iter()
                    .any(|p| p.as_deref() == Some(key));
                if is_paging || !self.items.iter().any(|item| item.get(key).is_some()) {
                    continue;
                }
                items.retain(|item| item.get(key).is_some_and(|v| id_text(v) == *value));
            }
        }
        let total = items.len();
        let mut note = format!("{}: list, {total} of {} items", self.name, self.items.len());
        if list.page_param.is_some() {
            let page = paging(&list.page_param).unwrap_or(1).max(1);
            let size = paging(&list.page_size_param)
                .unwrap_or(list.page_size)
                .max(1);
            let start = usize::try_from((page - 1).saturating_mul(size)).unwrap_or(usize::MAX);
            let size = usize::try_from(size).unwrap_or(usize::MAX);
            items = items.into_iter().skip(start).take(size).collect();
            note = format!(
                "{}: list page {page}, {} of {total} items",
                self.name,
                items.len()
            );
        }
        let items: Vec<Value> = items.into_iter().cloned().collect();
        let body = match &list.wrap {
            Some(key) => json!({ key.as_str(): items, "total": total }),
            None => Value::Array(items),
        };
        let mut response = Response::json(200, &body, cors);
        push_header(&mut response, "X-Total-Count", &total.to_string());
        (response, note)
    }

    /// A new item's id.
    fn next_id(&self, rng: &mut Rng) -> Value {
        match self.new_id {
            NewId::Uuid => Value::String(uuid(rng)),
            NewId::Increment => {
                let max = self
                    .items
                    .iter()
                    .filter_map(|item| item.get(&self.id)?.as_i64())
                    .max()
                    .unwrap_or(0);
                json!(max.saturating_add(1))
            }
        }
    }
}

fn push_header(response: &mut Response, name: &str, value: &str) {
    if let Response::Fulfill { headers, .. } = response {
        headers.push((name.to_owned(), value.to_owned()));
    }
}

/// JSON merge patch (RFC 7386): objects merge, `null` removes a key,
/// anything else replaces.
pub(crate) fn merge(target: &mut Value, patch: &Value) {
    let Value::Object(patch) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(Map::new());
    }
    let Value::Object(map) = target else {
        return;
    };
    for (key, value) in patch {
        if value.is_null() {
            map.remove(key);
        } else {
            merge(map.entry(key.clone()).or_insert(Value::Null), value);
        }
    }
}
