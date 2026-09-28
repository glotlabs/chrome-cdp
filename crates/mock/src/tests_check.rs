//! `mock check`: every lint rule, and mocks against an OpenAPI spec.

use serde_json::json;

use crate::Source;
use crate::check::check;
use crate::openapi::{Lookup, OpenApi};
use crate::tests::dir;

const SPEC: &str = r##"
openapi: 3.0.3
info: { title: shop, version: "1" }
servers:
  - url: https://shop.example.com/api
paths:
  /products:
    get:
      responses:
        200:
          description: the products
          content:
            application/json:
              schema:
                type: array
                items: { $ref: "#/components/schemas/Product" }
  /products/{id}:
    get:
      responses:
        "200":
          description: one
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Product" }
        "404": { description: none }
  /orders:
    post:
      responses:
        "201":
          description: made
          content:
            application/json:
              schema:
                type: object
                required: [id]
                properties:
                  id: { type: string }
                  status: { type: string, enum: [new, paid] }
        5XX: { description: busy }
  /todos:
    get: { responses: { "200": { description: list } } }
  /todos/{id}:
    get:
      responses:
        "200":
          description: one
          content:
            application/json:
              schema:
                type: object
                required: [id, title]
                properties:
                  id: { type: integer }
                  title: { type: string }
                  done: { type: boolean, nullable: true }
components:
  schemas:
    Product:
      type: object
      additionalProperties: false
      required: [id, name, price]
      properties:
        id: { type: integer }
        name: { type: string }
        price: { type: number }
        tags: { type: array, items: { type: string } }
"##;

const BASE: &str = r#"
[settings]
scope = ["/api/**"]

[state]
logged_in = false

[[mock]]
name = "products"
match = { method = "GET", path = "/api/products" }
respond = { json_file = "data/products.json" }

[[mock]]
name = "product by id"
match = { method = "GET", path = "/api/products/:id" }
respond = { json_file = "data/products/{{params.id}}.json" }

[[mock]]
name = "product 7"
match = { method = "GET", path = "/api/products/7" }
respond = { json = { id = 7, name = "Seven", price = 7 } }

[[mock]]
name = "teapot"
match = { method = "GET", path = "/api/products/9", headers = { x-teapot = "1" } }
respond = { status = 418 }

[[mock]]
name = "order"
match = { method = "POST", path = "/api/orders" }
sequence = [
  { status = 503 },
  { status = 201, json = { id = "{{uuid}}", status = "shipped" } },
]
when = { logedin = true }

[[mock]]
name = "typo"
match = { method = "GET", path = "/api/prodcts" }
respond = { json = [] }

[[mock]]
name = "delete"
match = { method = "DELETE", path = "/api/products/:id" }
respond = { status = 204 }

[[mock]]
name = "everything under todos"
match = { path = "/api/todos/**" }
respond = { status = 500 }

[[collection]]
name = "todos"
path = "/api/todos"
items = [{ id = 1, title = "Milk" }, { id = 2, title = 3 }]
"#;

const PRODUCTS: &str =
    r#"[{"id": 1, "name": "Keyboard", "price": 120}, {"id": 2, "name": "Mouse", "prize": 40}]"#;

const OTHER: &str = r#"
[[collection]]
name = "todos"
path = "/api/other-todos"
"#;

fn sources(d: &std::path::Path, names: &[&str]) -> Vec<Source> {
    names
        .iter()
        .map(|n| Source::find(d, d, n).unwrap())
        .collect()
}

#[test]
fn lints_find_what_no_request_would() {
    let d = dir(&[
        ("base.toml", BASE),
        ("other.toml", OTHER),
        (
            "broken.toml",
            "[[mock]]\nname = \"x\"\nmatch = { path = \"/a\" }\nrespond = { json_file = \"data/nope.json\" }\n",
        ),
        ("data/products.json", PRODUCTS),
    ]);
    let checked = check(&sources(d.path(), &["base", "broken", "other"]), &[], None);
    let got: Vec<(String, usize, &str)> = checked
        .problems
        .iter()
        .map(|p| {
            let file = std::path::Path::new(&p.file)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            (format!("{} {}", p.severity, file), p.line, p.rule.as_str())
        })
        .collect();
    assert_eq!(
        got,
        vec![
            ("error broken.toml".into(), 4, "load"),
            ("warning base.toml".into(), 18, "unreachable"),
            // `x-teapot` does not help: "product by id" matches first.
            ("warning base.toml".into(), 23, "unreachable"),
            ("warning base.toml".into(), 28, "undeclared-state"),
            ("warning base.toml".into(), 52, "collection-shadowed"),
            ("warning other.toml".into(), 2, "duplicate-collection"),
        ],
        "{:#?}",
        checked.problems
    );
    let text = |rule: &str| {
        let p = checked.problems.iter().find(|p| p.rule == rule).unwrap();
        format!("{} / {}", p.message, p.hint.clone().unwrap_or_default())
    };
    assert_eq!(
        text("unreachable"),
        "mock \"product 7\" never answers: \"product by id\" (line 13) comes first and matches every request it does / move \"product 7\" above \"product by id\", or narrow \"product by id\"'s match"
    );
    assert_eq!(
        text("undeclared-state"),
        "mock \"order\": when names `logedin`, which no mock file's [state] declares / did you mean `logged_in`?"
    );
    assert!(text("collection-shadowed").starts_with(
        "collection \"todos\" never answers: mock \"everything under todos\" (line 47) is tried first"
    ));
    assert!(text("load").contains("data/nope.json"), "{}", text("load"));
    assert_eq!(checked.sets.len(), 2);
    assert_eq!(checked.mocks, 10);
}

#[test]
fn a_later_default_set_can_shadow_a_collection() {
    let d = dir(&[
        (
            "a.toml",
            "[[collection]]\nname = \"todos\"\npath = \"/api/todos\"\n",
        ),
        (
            "b.toml",
            "[[mock]]\nname = \"all\"\nmatch = { path = \"/api/**\" }\nrespond = { status = 503 }\n",
        ),
    ]);
    let layers = vec!["a".to_owned(), "b".to_owned()];
    let checked = check(&sources(d.path(), &["a", "b"]), &layers, None);
    assert_eq!(checked.problems.len(), 1, "{:#?}", checked.problems);
    assert!(
        checked.problems[0]
            .message
            .contains("mock \"all\" (b.toml:1)")
    );
    // Applied the other way round, the collection comes first.
    let layers = vec!["b".to_owned(), "a".to_owned()];
    assert!(
        check(&sources(d.path(), &["a", "b"]), &layers, None)
            .problems
            .is_empty()
    );
}

#[test]
fn mocks_are_checked_against_the_spec() {
    let d = dir(&[("base.toml", BASE), ("data/products.json", PRODUCTS)]);
    let spec = OpenApi::parse(SPEC, false).unwrap();
    let checked = check(&sources(d.path(), &["base"]), &[], Some(&spec));
    let errors: Vec<(usize, &str, &str)> = checked
        .problems
        .iter()
        .filter(|p| p.severity == "error")
        .map(|p| (p.line, p.rule.as_str(), p.message.as_str()))
        .collect();
    assert_eq!(
        errors,
        vec![
            (
                8,
                "openapi-body",
                "mock \"products\" (data/products.json) does not fit the spec's GET /products 200: $[1].price: the spec requires it, the mock has none; $[1].prize: not in the spec (did you mean `price`?)"
            ),
            (
                23,
                "openapi-status",
                "mock \"teapot\" answers 418; the spec's GET /products/{id} has 200, 404"
            ),
            (
                28,
                "openapi-body",
                "answer 2 of mock \"order\" does not fit the spec's POST /orders 201: $.status: the spec allows \"new\", \"paid\", the mock has a string (\"shipped\")"
            ),
            (
                37,
                "openapi-path",
                "mock \"typo\": the spec has no path /api/prodcts"
            ),
            (
                42,
                "openapi-method",
                "mock \"delete\": the spec's /products/{id} has no DELETE"
            ),
            (
                52,
                "openapi-body",
                "collection \"todos\": item 2 does not fit the spec's GET /api/todos/:id: $.title: the spec says string, the mock has a number (3)"
            ),
        ],
        "{:#?}",
        checked.problems
    );
    let typo = checked
        .problems
        .iter()
        .find(|p| p.rule == "openapi-path")
        .unwrap();
    assert_eq!(typo.hint.as_deref(), Some("the spec's closest: /products"));
}

#[test]
fn specs_are_read_as_json_or_yaml_and_swagger() {
    assert!(
        OpenApi::parse("x: 1", false)
            .unwrap_err()
            .contains("no `openapi`")
    );
    assert!(
        OpenApi::parse("{", true)
            .unwrap_err()
            .starts_with("not JSON")
    );
    let swagger = OpenApi::parse(
        &json!({
            "swagger": "2.0",
            "basePath": "/v1",
            "paths": {"/items/{id}": {"get": {"responses": {"200": {"schema": {
                "type": "object", "properties": {"id": {"type": "integer"}}
            }}}}}}
        })
        .to_string(),
        true,
    )
    .unwrap();
    let Lookup::Found(ops) = swagger.lookup("/v1/items/:id", &[]) else {
        panic!("not found");
    };
    assert_eq!(ops[0].method, "GET");
    assert_eq!(
        swagger.check_body(&ops[0], 200, &json!({"id": "x"})),
        vec!["$.id: the spec says integer, the mock has a string (\"x\")".to_owned()]
    );
    assert!(
        swagger
            .check_body(&ops[0], 200, &json!({"id": "{{params.id}}"}))
            .is_empty()
    );
    assert!(matches!(swagger.lookup("/v1/**", &[]), Lookup::Unknown));
    assert!(matches!(
        swagger.lookup("/items/1", &[]),
        Lookup::NoPath { .. }
    ));
}
