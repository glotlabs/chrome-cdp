//! Stateful mocks (plan 11.3): sequences, named state and collections.

use serde_json::{Value, json};

use crate::tests::{answer, dir, load_err, mocks, req};
use crate::{Mocks, Outcome, Response, Seed, StateItem};

/// Status, headers and JSON body of an answer.
fn full(
    m: &mut Mocks,
    method: &str,
    url: &str,
    body: Option<&str>,
) -> (u16, Vec<(String, String)>, Value, String) {
    match m.decide(&req(method, url, body.map(str::as_bytes)), 0) {
        Outcome::Answer(a) => match a.response {
            Response::Fulfill {
                status,
                headers,
                body,
                ..
            } => (
                status,
                headers,
                serde_json::from_slice(&body).unwrap_or(Value::Null),
                a.outcome,
            ),
            other => panic!("{other:?}"),
        },
        other => panic!("{method} {url}: {other:?}"),
    }
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

const SEQUENCES: &str = r#"
[settings]
scope = ["/api/**"]

[[mock]]
name = "orders: busy once, then ok"
match = { method = "POST", path = "/api/orders" }
sequence = [
  { status = 503, json = { error = "busy" } },
  { status = 201, json = { qty = "{{json.qty}}", n = "{{seq}}" } },
]

[[mock]]
name = "lights"
match = { path = "/api/light" }
sequence = [{ json = "red" }, { json = "green" }]
after_sequence = "cycle"

[[mock]]
name = "once"
match = { path = "/api/token" }
sequence = [{ json = "first" }]
after_sequence = "unmatched"

[[mock]]
name = "fallback token"
match = { path = "/api/token" }
respond = { json = "later" }
"#;

#[test]
fn sequences_answer_in_turn() {
    let d = dir(&[("base.toml", SEQUENCES)]);
    let mut m = mocks(d.path(), &["base"]);
    let order = Some(r#"{"qty": 2}"#);
    let (status, body, _) = answer(&mut m, "POST", "http://app/api/orders", order);
    assert_eq!((status, body), (503, json!({"error": "busy"})));
    let (status, body, _) = answer(&mut m, "POST", "http://app/api/orders", order);
    assert_eq!((status, body), (201, json!({"qty": 2, "n": 2})));
    // repeat-last is the default.
    let (status, _, _) = answer(&mut m, "POST", "http://app/api/orders", order);
    assert_eq!(status, 201);

    let lights: Vec<Value> = (0..3)
        .map(|_| answer(&mut m, "GET", "http://app/api/light", None).1)
        .collect();
    assert_eq!(lights, [json!("red"), json!("green"), json!("red")]);

    // A used-up sequence stops matching, so the next mock answers.
    let (_, body, name) = answer(&mut m, "GET", "http://app/api/token", None);
    assert_eq!((body, name.as_str()), (json!("first"), "once"));
    let explained = m.explain(&req("GET", "http://app/api/token", None));
    assert_eq!(explained.answer.unwrap().name, "fallback token");
    let reason = explained.checked[2].reason.clone().unwrap();
    assert!(
        reason.contains("sequence of 1 answer is used up"),
        "{reason}"
    );
    let (_, body, name) = answer(&mut m, "GET", "http://app/api/token", None);
    assert_eq!((body, name.as_str()), (json!("later"), "fallback token"));

    // `mock list` says where each sequence is.
    let info = m.info(true);
    let responds: Vec<&str> = info[0].mocks.iter().map(|m| m.responds.as_str()).collect();
    assert_eq!(
        responds[0],
        "sequence 2/2 (then repeat-last): fulfill 201 json {\"n\":\"{{seq}}\",\"qty\":\"{{json.qty}}\"}"
    );
    assert!(
        responds[1].starts_with("sequence 2/2 (then cycle)"),
        "{responds:?}"
    );
    assert_eq!(
        responds[2],
        "sequence of 1 answer used up (after_sequence = \"unmatched\")"
    );

    // A reset starts every sequence over.
    m.reset();
    let (status, _, _) = answer(&mut m, "POST", "http://app/api/orders", order);
    assert_eq!(status, 503);
    let (_, body, _) = answer(&mut m, "GET", "http://app/api/token", None);
    assert_eq!(body, json!("first"));
}

const LOGIN: &str = r#"
[settings]
scope = ["/api/**"]

[state]
logged_in = false
cart = []

[[mock]]
name = "login"
match = { method = "POST", path = "/api/login" }
respond = { json = { ok = true, was = "{{state.logged_in}}" } }
set_state = { logged_in = true, user = "{{json.email}}" }

[[mock]]
name = "logout"
match = { method = "POST", path = "/api/logout" }
respond = { status = 204 }
set_state = { logged_in = false }
unset_state = ["user"]

[[mock]]
name = "me"
match = { method = "GET", path = "/api/me" }
when = { logged_in = true, user = "*" }
respond = { json = { email = "{{state.user}}" } }

[[mock]]
name = "me, logged out"
match = { method = "GET", path = "/api/me" }
respond = { status = 401, json = { error = "log in first" } }
"#;

#[test]
fn named_state_drives_login_and_logout() {
    let d = dir(&[("base.toml", LOGIN)]);
    let mut m = mocks(d.path(), &["base"]);
    assert!(
        m.undeclared().iter().any(|w| w.contains("`user`")),
        "{:?}",
        m.undeclared()
    );
    let (status, _, name) = answer(&mut m, "GET", "http://app/api/me", None);
    assert_eq!((status, name.as_str()), (401, "me, logged out"));
    let why = m.explain(&req("GET", "http://app/api/me", None)).checked[2]
        .reason
        .clone()
        .unwrap();
    assert_eq!(why, "state logged_in: expects true, it is false");

    let login = Some(r#"{"email": "ada@example.com"}"#);
    let (_, body, _) = answer(&mut m, "POST", "http://app/api/login", login);
    // The answer sees the values from before it.
    assert_eq!(body, json!({"ok": true, "was": false}));
    let (status, body, _) = answer(&mut m, "GET", "http://app/api/me", None);
    assert_eq!((status, body), (200, json!({"email": "ada@example.com"})));
    let (values, _) = m.state();
    assert_eq!(
        values,
        [
            ("cart".to_owned(), json!([])),
            ("logged_in".to_owned(), json!(true)),
            ("user".to_owned(), json!("ada@example.com")),
        ]
        .into()
    );

    answer(&mut m, "POST", "http://app/api/logout", None);
    let (status, _, _) = answer(&mut m, "GET", "http://app/api/me", None);
    assert_eq!(status, 401);
    assert_eq!(m.lookup("user"), None);
    assert_eq!(m.lookup("logged_in"), Some(StateItem::Value(json!(false))));

    // Values can be set by hand, but only declared (or set) ones.
    m.set_values(&[
        ("logged_in".into(), json!(true)),
        ("user".into(), json!("x")),
    ])
    .unwrap_err();
    m.set_values(&[("logged_in".into(), json!(true))]).unwrap();
    let why = m.explain(&req("GET", "http://app/api/me", None)).checked[2]
        .reason
        .clone()
        .unwrap();
    assert_eq!(why, "state user: expects \"*\", but it is not set");
    m.reset();
    assert_eq!(m.lookup("logged_in"), Some(StateItem::Value(json!(false))));
}

const TODOS: &str = r#"
[settings]
scope = ["/api/**"]

[[mock]]
name = "todo 99 is special"
match = { method = "GET", path = "/api/todos/99" }
respond = { json = { id = 99, special = true } }

[[collection]]
name = "todos"
path = "/api/todos"
seed = "data/todos.json"
list = { filter_query = true }

[[collection]]
name = "notes"
path = "/api/notes"
items = [{ key = "a", text = "hi" }]
id = "key"
list = { page_param = "page", page_size_param = "limit", page_size = 2, wrap = "items" }
"#;

fn todos() -> tempfile::TempDir {
    dir(&[
        ("base.toml", TODOS),
        (
            "data/todos.json",
            r#"[{"id": 1, "title": "Buy milk", "done": false}, {"id": 2, "title": "Write tests", "done": true}]"#,
        ),
        ("data/other.json", r#"[{"id": 7, "title": "Other"}]"#),
    ])
}

#[test]
fn collections_create_read_update_and_delete() {
    let d = todos();
    let mut m = mocks(d.path(), &["base"]);
    let (status, headers, body, outcome) = full(&mut m, "GET", "http://app/api/todos", None);
    assert_eq!(status, 200);
    assert_eq!(body.as_array().unwrap().len(), 2);
    assert_eq!(header(&headers, "x-total-count"), Some("2"));
    assert_eq!(outcome, "fulfill 200 (todos: list, 2 of 2 items)");

    // A POST appears in the next GET, with the next number as its id.
    let (status, headers, body, outcome) = full(
        &mut m,
        "POST",
        "http://app/api/todos",
        Some(r#"{"title": "Ship it", "done": false}"#),
    );
    assert_eq!((status, body["id"].clone()), (201, json!(3)));
    assert_eq!(header(&headers, "location"), Some("/api/todos/3"));
    assert_eq!(outcome, "fulfill 201 (todos: created 3)");
    let (_, body, _) = answer(&mut m, "GET", "http://app/api/todos/3", None);
    assert_eq!(body, json!({"id": 3, "title": "Ship it", "done": false}));
    let (_, body, _) = answer(&mut m, "GET", "http://app/api/todos", None);
    assert_eq!(body.as_array().unwrap().len(), 3);

    // PATCH merges, PUT replaces; the id stays.
    let (_, body, _) = answer(
        &mut m,
        "PATCH",
        "http://app/api/todos/1",
        Some(r#"{"done": true, "id": 5}"#),
    );
    assert_eq!(body, json!({"id": 1, "title": "Buy milk", "done": true}));
    let (_, body, _) = answer(
        &mut m,
        "PUT",
        "http://app/api/todos/1",
        Some(r#"{"title": "Buy oat milk"}"#),
    );
    assert_eq!(body, json!({"id": 1, "title": "Buy oat milk"}));

    // Filters by query, ignoring parameters no item has.
    let (_, body, _) = answer(&mut m, "GET", "http://app/api/todos?done=true&_t=1", None);
    assert_eq!(
        body,
        json!([{"id": 2, "title": "Write tests", "done": true}])
    );

    // DELETE answers the item it removed.
    let (status, body, _) = answer(&mut m, "DELETE", "http://app/api/todos/2", None);
    assert_eq!((status, body["id"].clone()), (200, json!(2)));
    let (status, body, _) = answer(&mut m, "GET", "http://app/api/todos/2", None);
    assert_eq!(
        (status, body),
        (404, json!({"error": "not found: todos/2"}))
    );

    // Errors: no object body, an id taken, a method it does not take.
    let (status, _, _) = answer(&mut m, "POST", "http://app/api/todos", Some("[1]"));
    assert_eq!(status, 400);
    let (status, _, _) = answer(&mut m, "POST", "http://app/api/todos", Some(r#"{"id": 1}"#));
    assert_eq!(status, 409);
    let (status, headers, _, _) = full(&mut m, "DELETE", "http://app/api/todos", None);
    assert_eq!(
        (status, header(&headers, "allow")),
        (405, Some("GET, POST"))
    );

    // A mock in front of the collection wins.
    let (_, body, name) = answer(&mut m, "GET", "http://app/api/todos/99", None);
    assert_eq!(
        (body["special"].clone(), name.as_str()),
        (json!(true), "todo 99 is special")
    );

    // State, and a reset back to the seed.
    let Some(StateItem::Collection { info, items }) = m.lookup("todos") else {
        panic!()
    };
    assert_eq!(
        (info.count, info.path.as_str(), info.mock.at.as_str()),
        (2, "/api/todos", "base.toml:10")
    );
    assert_eq!(items[1]["title"], "Ship it");
    m.reset();
    let (_, body, _) = answer(&mut m, "GET", "http://app/api/todos", None);
    assert_eq!(body.as_array().unwrap().len(), 2);
}

#[test]
fn a_created_item_can_take_the_real_apis_id() {
    let d = todos();
    let mut m = mocks(d.path(), &["base"]);
    let Outcome::Answer(a) = m.decide(
        &req(
            "POST",
            "http://app/api/todos",
            Some(br#"{"title": "Ship it"}"#),
        ),
        0,
    ) else {
        panic!()
    };
    let Response::Fulfill { body, .. } = &a.response else {
        panic!()
    };
    let created: Value = serde_json::from_slice(body).unwrap();
    assert_eq!(created["id"], 3);
    // The real API called it 41: the mock's item is 41 from now on.
    assert!(m.adopt_id(&a.mock, &created, &json!({"id": 41, "title": "Ship it"})));
    let (status, body, _) = answer(&mut m, "GET", "http://app/api/todos/41", None);
    assert_eq!((status, body["title"].clone()), (200, json!("Ship it")));
    let (status, _, _) = answer(&mut m, "GET", "http://app/api/todos/3", None);
    assert_eq!(status, 404);
    // Not onto an id another item has, and not without an id.
    assert!(!m.adopt_id(&a.mock, &json!({"id": 41}), &json!({"id": 1})));
    assert!(!m.adopt_id(&a.mock, &json!({"id": 41}), &json!({"ok": true})));
}

#[test]
fn collections_page_wrap_and_make_uuids() {
    let d = todos();
    let mut m = mocks(d.path(), &["base"]);
    for text in ["b", "c", "d"] {
        let (status, body, _) = answer(
            &mut m,
            "POST",
            "http://app/api/notes",
            Some(&format!(r#"{{"text": "{text}"}}"#)),
        );
        assert_eq!(status, 201);
        // Seed ids that are not numbers make uuids (seeded).
        assert_eq!(body["key"].as_str().unwrap().len(), 36);
    }
    let (_, headers, body, outcome) = full(&mut m, "GET", "http://app/api/notes?page=2", None);
    assert_eq!(body["total"], json!(4));
    let texts: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["c", "d"]);
    assert_eq!(header(&headers, "x-total-count"), Some("4"));
    assert_eq!(outcome, "fulfill 200 (notes: list page 2, 2 of 4 items)");
    let (_, body, _) = answer(&mut m, "GET", "http://app/api/notes?page=1&limit=3", None);
    assert_eq!(body["items"].as_array().unwrap().len(), 3);
}

#[test]
fn collections_are_seeded_and_explained() {
    let d = todos();
    let mut m = mocks(d.path(), &["base"]);
    let cwd = d.path().to_path_buf();
    let (info, from) = m
        .seed(
            "todos",
            &Seed::File {
                path: "data/other.json".into(),
                cwd: cwd.clone(),
            },
        )
        .unwrap();
    assert_eq!(info.count, 1);
    assert!(from.ends_with("data/other.json"), "{from}");
    let (info, from) = m
        .seed(
            "todos",
            &Seed::Items(json!([{"id": 1}, {"id": 2}, {"id": 3}])),
        )
        .unwrap();
    assert_eq!((info.count, from.as_str()), (3, "stdin"));
    let err = m
        .seed("todos", &Seed::Items(json!([{"title": "x"}])))
        .unwrap_err();
    assert!(err.contains("item 0 has no `id` field"), "{err}");
    let err = m
        .seed(
            "todos",
            &Seed::File {
                path: "nope.json".into(),
                cwd,
            },
        )
        .unwrap_err();
    assert!(
        err.starts_with("no file nope.json in the mocks directory"),
        "{err}"
    );
    assert!(m.seed("nope", &Seed::Start).is_err());
    let (info, _) = m.seed("todos", &Seed::Start).unwrap();
    assert_eq!(info.count, 2);

    let explained = m.explain(&req("GET", "http://app/api/todos/9", None));
    assert_eq!(explained.answer.unwrap().name, "todos");
    assert_eq!(
        explained.outcome,
        "collection todos: get 9 (404, no todos/9)"
    );
    let explained = m.explain(&req("PUT", "http://app/api/other", None));
    let reason = explained.checked[1].reason.clone().unwrap();
    assert_eq!(
        reason,
        "path differs: expects /api/todos or /api/todos/:id, got /api/other"
    );

    // `mock list` lists collections after the mocks, and preflights for
    // them are allowed.
    let info = m.info(true);
    assert_eq!((info[0].count, info[0].collections), (1, 2));
    assert_eq!(info[0].mocks[1].responds, "collection todos: 2 items");
    assert_eq!(info[0].mocks[1].matches, "/api/todos, /api/todos/:id");
    let headers = [
        ("Origin".to_owned(), "http://app".to_owned()),
        (
            "Access-Control-Request-Method".to_owned(),
            "DELETE".to_owned(),
        ),
    ];
    let preflight = m.decide(
        &crate::Request {
            method: "OPTIONS",
            url: "http://api/api/todos/1",
            headers: &headers,
            body: None,
        },
        0,
    );
    assert!(
        matches!(&preflight, Outcome::Preflight { mock: Some(r) } if r.name == "todos"),
        "{preflight:?}"
    );
}

#[test]
fn changed_files_keep_the_state() {
    let d = todos();
    let mut m = mocks(d.path(), &["base"]);
    answer(
        &mut m,
        "POST",
        "http://app/api/todos",
        Some(r#"{"title": "New"}"#),
    );
    let file = d.path().join("base.toml");
    std::fs::write(&file, format!("{TODOS}\n[state]\nflag = 1\n")).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    m.refresh();
    let (_, body, _) = answer(&mut m, "GET", "http://app/api/todos", None);
    assert_eq!(body.as_array().unwrap().len(), 3);
    assert_eq!(m.lookup("flag"), Some(StateItem::Value(json!(1))));
}

#[test]
fn stateful_problems_point_at_the_line() {
    let cases: &[(&str, &str)] = &[
        (
            "[[mock]]\nmatch = { path = \"/a\" }\nrespond = {}\nsequence = [{}]\n",
            "base.toml:1:1: the mock has both `respond` and `sequence`",
        ),
        (
            "[[mock]]\nmatch = { path = \"/a\" }\nsequence = []\n",
            "base.toml:3:12: sequence: it has no answers",
        ),
        (
            "[[mock]]\nmatch = { path = \"/a\" }\nrespond = {}\nafter_sequence = \"cycle\"\n",
            "base.toml:1:1: `after_sequence` needs a `sequence`",
        ),
        (
            "[[mock]]\nmatch = { path = \"/a\" }\nsequence = [{ status = 42 }]\n",
            "base.toml:3:24: sequence[0].status: 42 cannot be answered",
        ),
        (
            "[[mock]]\nmatch = { path = \"/a\" }\nrespond = {}\nset_state = { \"a b\" = 1 }\n",
            "base.toml:4:13: set_state: `a b` is not a name",
        ),
        (
            "[[mock]]\nmatch = { path = \"/a\" }\nrespond = {}\nset_state = { x = \"{{ oops\" }\n",
            "base.toml:4:13: set_state.x: template",
        ),
        (
            "[[mock]]\nmatch = { path = \"/a\" }\nrespond = {}\nset_state = { x = 1 }\nunset_state = [\"x\"]\n",
            "base.toml:5:15: unset_state: `x` is in set_state too",
        ),
        (
            "[[collection]]\nname = \"t\"\npath = \"/t\"\nitems = [{ title = \"x\" }]\n",
            "base.toml:4:9: collection.items: item 0 has no `id` field",
        ),
        (
            "[[collection]]\nname = \"t\"\npath = \"/t\"\nitems = [{ id = 1 }, { id = 1 }]\n",
            "base.toml:4:9: collection.items: item 1: `id` 1 appears twice",
        ),
        (
            "[[collection]]\nname = \"t\"\npath = \"/t\"\nseed = \"data/none.json\"\n",
            "base.toml:4:8: collection.seed: ",
        ),
        (
            "[[collection]]\nname = \"t\"\npath = \"/t\"\nlist = { page_size = 5 }\n",
            "base.toml:4:8: collection.list: page sizes need `page_param`",
        ),
        (
            "[[collection]]\nname = \"t\"\npath = \"/t\"\n[[collection]]\nname = \"t\"\npath = \"/u\"\n",
            "base.toml:4:1: collection: the name `t` is used twice",
        ),
        (
            "[state]\nt = 1\n[[collection]]\nname = \"t\"\npath = \"/t\"\n",
            "base.toml:3:1: collection: `t` is also a [state] value",
        ),
        (
            "[[collection]]\nname = \"t\"\npath = \"t\"\n",
            "base.toml:3:8: collection.path: `t` is no path",
        ),
    ];
    for (text, expected) in cases {
        let err = load_err(&[("base.toml", text)]);
        assert!(
            err.contains(expected),
            "{text}\n→ {err}\nexpected {expected}"
        );
    }
}
