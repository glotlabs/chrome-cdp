//! `mock gen` (plan 19, milestone 19.4): goldens over the vendored
//! Petstore and a small spec, the sets passing `mock check` against the
//! spec they came from, and each example source, type and format.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::check::check;
use crate::generate::{GenOptions, Generated, generate};
use crate::load::Source;
use crate::openapi::OpenApi;

fn testdata(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/openapi")
        .join(rel)
}

fn options(seed: u64, collections: bool) -> GenOptions {
    GenOptions {
        seed,
        scope: Vec::new(),
        collections,
        data_dir: "data/generated".to_owned(),
        spec_name: "spec.yaml".to_owned(),
    }
}

fn gen_file(spec: &str, seed: u64, collections: bool) -> (OpenApi, Generated) {
    let spec = OpenApi::load(&testdata(spec)).expect("spec loads");
    let out = generate(&spec, &options(seed, collections)).expect("generates");
    (spec, out)
}

fn gen_text(text: &str, collections: bool) -> Generated {
    let spec = OpenApi::parse(text, false).expect("spec parses");
    generate(&spec, &options(1, collections)).expect("generates")
}

/// The file and its data files as one text.
fn shown(out: &Generated) -> String {
    let mut text = format!("== generated.toml ==\n{}", out.toml);
    for (rel, bytes) in &out.files {
        let _ = write!(text, "\n== {rel} ==\n{}", String::from_utf8_lossy(bytes));
    }
    text
}

fn golden(name: &str, actual: &str) {
    let path = testdata(&format!("golden/{name}"));
    let update = std::env::var_os("UPDATE_GOLDEN").is_some_and(|v| !v.is_empty() && v != "0");
    if update {
        std::fs::create_dir_all(path.parent().expect("dir")).expect("mkdir");
        std::fs::write(&path, actual).expect("write golden");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        expected == actual,
        "{} is out of date (UPDATE_GOLDEN=1 cargo test -p chrome-cdp-mock generate writes it)\n{actual}",
        path.display()
    );
}

/// Writes the set to a temporary directory and runs `mock check` on it
/// against `spec`: the problems as lines.
fn checked(spec: &OpenApi, out: &Generated) -> Vec<String> {
    let dir = tempfile::tempdir().expect("tempdir");
    for (rel, bytes) in &out.files {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, bytes).expect("write");
    }
    let file = dir.path().join("generated.toml");
    std::fs::write(&file, &out.toml).expect("write");
    let source = Source {
        name: "generated".to_owned(),
        file,
        base: dir.path().to_path_buf(),
    };
    check(&[source], &[], Some(spec))
        .problems
        .iter()
        .map(|p| {
            format!(
                "{}:{} {} [{}] {}",
                p.line, p.col, p.severity, p.rule, p.message
            )
        })
        .collect()
}

#[test]
fn petstore_golden() {
    let (spec, out) = gen_file("petstore.yaml", 1, false);
    golden("petstore.txt", &shown(&out));
    assert_eq!(out.mocks.len(), 19, "{:?}", out.mocks);
    assert!(out.collections.is_empty());
    assert_eq!(out.scope, vec!["/api/v3/**"]);
    assert_eq!(checked(&spec, &out), Vec::<String>::new());
}

#[test]
fn petstore_collections_golden() {
    // No path pair of the Petstore has a list GET: the same mocks.
    let (spec, out) = gen_file("petstore.yaml", 1, true);
    golden("petstore-collections.txt", &shown(&out));
    assert!(out.collections.is_empty());
    assert_eq!(checked(&spec, &out), Vec::<String>::new());
}

#[test]
fn todos_golden() {
    let (spec, out) = gen_file("todos.yaml", 1, false);
    golden("todos.txt", &shown(&out));
    assert_eq!(checked(&spec, &out), Vec::<String>::new());
}

#[test]
fn todos_collections_golden() {
    let (spec, out) = gen_file("todos.yaml", 1, true);
    golden("todos-collections.txt", &shown(&out));
    let names: Vec<&str> = out.collections.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["users", "todos"]);
    assert_eq!(
        out.collections[1].operations,
        [
            "GET /users/{userId}/todos",
            "POST /users/{userId}/todos",
            "GET /users/{userId}/todos/{todoId}",
            "PATCH /users/{userId}/todos/{todoId}",
            "DELETE /users/{userId}/todos/{todoId}",
        ]
    );
    // The mocks left: the operations no collection answers.
    let mocks: Vec<&str> = out.mocks.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        mocks,
        [
            "getAvatar",
            "health",
            "ping",
            "listShapes",
            "getStats",
            "GET /tree",
            "getMe",
        ]
    );
    assert!(out.toml.contains("params = { userId = \"user_id\" }"));
    assert!(out.toml.contains("list = { wrap = \"todos\" }"));
    assert_eq!(checked(&spec, &out), Vec::<String>::new());
}

#[test]
fn the_same_spec_and_seed_give_the_same_bytes() {
    for collections in [false, true] {
        let (_, a) = gen_file("todos.yaml", 7, collections);
        let (_, b) = gen_file("todos.yaml", 7, collections);
        assert_eq!(shown(&a), shown(&b));
    }
    let (_, a) = gen_file("todos.yaml", 1, true);
    let (_, b) = gen_file("todos.yaml", 2, true);
    assert_ne!(shown(&a), shown(&b), "the seed changes the values");
}

/// A spec of one `GET /x` answering `media` (YAML, indented under the
/// media type).
fn one(media: &str) -> String {
    format!(
        "openapi: 3.0.0\ninfo: {{title: t, version: '1'}}\npaths:\n  /x:\n    get:\n      \
         operationId: getX\n      responses:\n        '200':\n          content:\n            \
         application/json:\n{media}"
    )
}

/// The body `mock gen` writes for `GET /x` (inline or in a data file).
fn body(out: &Generated) -> Value {
    if let Some((_, bytes)) = out.files.first() {
        return serde_json::from_slice(bytes).expect("JSON data file");
    }
    let line = out
        .toml
        .lines()
        .find(|l| l.starts_with("respond = "))
        .expect("respond");
    let doc: toml::Table = toml::from_str(line).expect("TOML");
    serde_json::to_value(&doc["respond"]["json"]).expect("json")
}

#[test]
fn examples_come_first_in_order() {
    let schema = "              schema:\n                type: object\n                \
                  example: {from: schema}\n                properties:\n                  \
                  from: {type: string}\n";
    let media = format!(
        "              example: {{from: media}}\n              examples:\n                \
         a: {{value: {{from: examples}}}}\n{schema}"
    );
    assert_eq!(
        body(&gen_text(&one(&media), false)),
        json!({"from": "media"})
    );
    let media = format!(
        "              examples:\n                a: {{value: {{from: examples}}}}\n{schema}"
    );
    assert_eq!(
        body(&gen_text(&one(&media), false)),
        json!({"from": "examples"})
    );
    assert_eq!(
        body(&gen_text(&one(schema), false)),
        json!({"from": "schema"})
    );
    // A property's own example, inside a generated object.
    let media = "              schema:\n                type: object\n                \
                 properties:\n                  name: {type: string, example: Rex}\n                  \
                 other: {type: string}\n";
    assert_eq!(
        body(&gen_text(&one(media), false)),
        json!({"name": "Rex", "other": "other-1"})
    );
}

/// The value generated for one property schema (YAML flow syntax).
fn generated(prop: &str) -> Value {
    let media = format!(
        "              schema:\n                type: object\n                properties:\n                  \
         v: {prop}\n"
    );
    body(&gen_text(&one(&media), false))["v"].clone()
}

#[test]
fn each_type_and_format() {
    assert_eq!(
        generated("{type: string, default: d, enum: [a, b]}"),
        json!("d")
    );
    assert_eq!(generated("{type: string, enum: [a, b]}"), json!("a"));
    assert_eq!(generated("{const: 3}"), json!(3));
    assert_eq!(generated("{type: boolean}"), json!(true));
    assert_eq!(generated("{type: string}"), json!("v-1"));
    assert_eq!(generated("{type: string, minLength: 6}"), json!("v-1xxx"));
    assert_eq!(generated("{type: string, maxLength: 2}"), json!("v-"));
    assert_eq!(
        generated("{type: string, format: date-time}"),
        json!("2026-01-01T01:00:00Z"),
        "seed 1 is one hour past the epoch"
    );
    assert_eq!(
        generated("{type: string, format: date}"),
        json!("2026-01-01")
    );
    assert_eq!(
        generated("{type: string, format: email}"),
        json!("user1@example.com")
    );
    assert_eq!(
        generated("{type: string, format: uri}"),
        json!("https://example.com/v/1")
    );
    assert_eq!(
        generated("{type: string, format: ipv4}"),
        json!("192.0.2.1")
    );
    assert_eq!(generated("{type: string, format: byte}"), json!("di0x"));
    let uuid = generated("{type: string, format: uuid}");
    let uuid = uuid.as_str().expect("text");
    assert_eq!(uuid.len(), 36);
    assert_eq!(&uuid[14..15], "4");
    let n = generated("{type: integer, minimum: 10, maximum: 20, multipleOf: 5}");
    assert!([10, 15, 20].contains(&n.as_i64().expect("integer")), "{n}");
    let n = generated("{type: integer}").as_i64().expect("integer");
    assert!((1..=100).contains(&n), "{n}");
    let n = generated("{type: number, minimum: 0.25, maximum: 0.75}");
    assert!(
        n.as_f64().is_some_and(|f| (0.25..=0.75).contains(&f)),
        "{n}"
    );
    assert_eq!(
        generated("{type: array, items: {type: string}}"),
        json!(["v-1", "v-2"])
    );
    assert_eq!(
        generated("{type: array, minItems: 3, items: {type: integer, enum: [4]}}"),
        json!([4, 4, 4])
    );
    assert_eq!(
        generated("{type: array, maxItems: 1, items: {type: boolean}}"),
        json!([true])
    );
    assert_eq!(
        generated("{oneOf: [{type: boolean}, {type: string}]}"),
        json!(true)
    );
    assert_eq!(
        generated(
            "{allOf: [{type: object, properties: {a: {type: boolean}}}, {properties: {b: {const: 1}}}]}"
        ),
        json!({"a": true, "b": 1})
    );
    assert_eq!(
        generated("{type: object, additionalProperties: {type: string}}"),
        json!({})
    );
}

#[test]
fn a_pattern_is_a_todo() {
    let out = gen_text(
        &one(
            "              schema: {type: object, properties: {code: {type: string, pattern: '^[A-Z]+$'}}}\n",
        ),
        false,
    );
    assert_eq!(out.todos, ["getX: $.code: pattern ^[A-Z]+$ is not honored"]);
    assert!(
        out.toml
            .contains("# TODO: $.code: pattern ^[A-Z]+$ is not honored\n[[mock]]")
    );
}

#[test]
fn an_example_that_does_not_fit_is_a_todo() {
    let out = gen_text(
        &one(
            "              example: {n: text}\n              schema: {type: object, properties: {n: {type: integer}}}\n",
        ),
        false,
    );
    assert_eq!(out.todos.len(), 1, "{:?}", out.todos);
    assert!(
        out.todos[0].contains("the spec's example does not fit the spec"),
        "{:?}",
        out.todos
    );
}

#[test]
fn cycles_and_depth_stop() {
    let (_, out) = gen_file("todos.yaml", 1, false);
    let tree = out
        .files
        .iter()
        .find(|(rel, _)| rel.ends_with("/GET-tree.json"))
        .map(|(_, b)| serde_json::from_slice::<Value>(b).expect("JSON"));
    let tree = tree.unwrap_or_else(|| {
        panic!(
            "no tree file: {:?}",
            out.files.iter().map(|f| &f.0).collect::<Vec<_>>()
        )
    });
    // Node's parent is a Node: required and nullable, so null; its
    // children are Nodes too: left out, so an empty list.
    assert_eq!(
        tree,
        json!({"children": [], "name": "name-1", "parent": null})
    );
}

#[test]
fn any_of_takes_the_first_branch_and_deep_objects_stop() {
    assert_eq!(
        generated("{anyOf: [{type: boolean}, {type: string}]}"),
        json!(true)
    );
    // Ten objects nested without a `$ref`: those past a depth of 8 (the
    // body is 0, `v` 1) are left out.
    let mut prop = "{type: string}".to_owned();
    for _ in 0..10 {
        prop = format!("{{type: object, properties: {{a: {prop}}}}}");
    }
    let mut v = generated(&prop);
    let mut levels = 1;
    while let Some(inner) = v.get("a") {
        v = inner.clone();
        levels += 1;
    }
    assert_eq!((levels, v), (7, json!({})));
}

#[test]
fn scope_limits_operations() {
    let spec = OpenApi::load(&testdata("petstore.yaml")).expect("spec");
    let mut opts = options(1, false);
    opts.scope = vec!["/store/**".to_owned()];
    let out = generate(&spec, &opts).expect("generates");
    let names: Vec<&str> = out.mocks.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        names,
        ["getInventory", "placeOrder", "getOrderById", "deleteOrder"]
    );
    opts.scope = vec!["/api/v3/user/*".to_owned()];
    let out = generate(&spec, &opts).expect("generates");
    assert_eq!(out.mocks.len(), 6, "{:?}", out.mocks);
    opts.scope = vec!["/nothing/**".to_owned()];
    let err = generate(&spec, &opts).expect_err("nothing in scope");
    assert!(
        err.contains("no operation of the spec is in --scope /nothing/**"),
        "{err}"
    );
}

#[test]
fn swagger_2_specs() {
    let text = r##"{"swagger": "2.0", "info": {"title": "t", "version": "1"}, "basePath": "/v2",
      "produces": ["application/json"],
      "paths": {"/pets/{id}": {"get": {"operationId": "getPet",
        "responses": {"200": {"description": "ok", "schema": {"$ref": "#/definitions/Pet"},
          "examples": {"application/json": {"id": 3, "name": "Rex"}}}}},
        "delete": {"responses": {"204": {"description": "gone"}}}}},
      "definitions": {"Pet": {"type": "object", "properties": {"id": {"type": "integer"}, "name": {"type": "string"}}}}}"##;
    let spec = OpenApi::parse(text, true).expect("parses");
    let out = generate(&spec, &options(1, false)).expect("generates");
    assert!(out.toml.contains("scope = [\"/v2/**\"]"), "{}", out.toml);
    assert!(
        out.toml.contains(
            "name = \"getPet\"\nmatch = { method = \"GET\", path = \"/v2/pets/:id\" }\nrespond = { json = { id = 3, name = \"Rex\" } }"
        ),
        "{}",
        out.toml
    );
    assert!(out.toml.contains("name = \"DELETE /pets/{id}\"\nmatch = { method = \"DELETE\", path = \"/v2/pets/:id\" }\nrespond = { status = 204 }"), "{}", out.toml);
}
