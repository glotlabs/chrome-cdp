//! `mock record` and `mock import`: recorded traffic becomes a mock set
//! that loads, and answers the same requests the same way.

use serde_json::{Value, json};

use crate::record::{Exchange, RecordOptions, Recorded, har_exchanges, record};
use crate::tests::{answer, dir, mocks};
use crate::{MockSet, Source};

fn ex(method: &str, url: &str, status: u16, body: &str) -> Exchange {
    Exchange {
        method: method.into(),
        url: url.into(),
        status,
        response_body: Some(body.as_bytes().to_vec()),
        mime: if body.starts_with(['{', '[']) {
            "application/json".into()
        } else {
            "text/plain".into()
        },
        resource_type: "fetch".into(),
        ..Exchange::default()
    }
}

fn options(scope: &[&str]) -> RecordOptions {
    RecordOptions {
        scope: scope.iter().map(|s| (*s).to_owned()).collect(),
        data_dir: "data/recorded".into(),
        origin: "Recorded by a test.".into(),
        match_bodies: false,
    }
}

/// Writes the recording as `recorded.toml` and its data files.
fn write(recorded: &Recorded) -> tempfile::TempDir {
    let d = dir(&[("recorded.toml", &recorded.toml)]);
    for (path, bytes) in &recorded.files {
        let file = d.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, bytes).unwrap();
    }
    d
}

fn big_list() -> String {
    json!([
        {"id": 1, "name": "Mechanical keyboard", "price": 120},
        {"id": 2, "name": "Mouse", "price": 40},
        {"id": 3, "name": "Monitor", "price": 300}
    ])
    .to_string()
}

#[test]
fn a_recording_answers_its_requests_again() {
    let exchanges = vec![
        ex("GET", "http://localhost:5173/", 200, "<html></html>"),
        ex(
            "GET",
            "http://localhost:5173/api/products",
            200,
            &big_list(),
        ),
        ex(
            "GET",
            "http://localhost:5173/api/products/1",
            200,
            r#"{"id":1,"name":"Keyboard"}"#,
        ),
        ex(
            "POST",
            "http://localhost:5173/api/orders",
            503,
            r#"{"error":"busy"}"#,
        ),
        ex(
            "POST",
            "http://localhost:5173/api/orders",
            201,
            r#"{"id":"o-1","qty":2}"#,
        ),
        ex(
            "GET",
            "http://localhost:5173/api/search?q=key",
            200,
            r#"{"results":[1]}"#,
        ),
        ex(
            "GET",
            "http://localhost:5173/api/search",
            200,
            r#"{"results":[]}"#,
        ),
        ex("GET", "http://other.example/api/ignored", 200, "{}"),
    ];
    let mut exchanges = exchanges;
    exchanges[0].resource_type = "document".into();
    let recorded = record(&exchanges, &options(&["/api/**"])).unwrap();
    assert_eq!(recorded.requests, 7, "{}", recorded.toml);
    assert_eq!(recorded.mocks, 6, "{}", recorded.toml);
    let d = write(&recorded);
    let mut m = mocks(d.path(), &["recorded"]);
    let base = "http://localhost:5173";
    let (status, body, name) = answer(&mut m, "GET", &format!("{base}/api/products"), None);
    assert_eq!((status, name.as_str()), (200, "GET /api/products"));
    assert_eq!(body, serde_json::from_str::<Value>(&big_list()).unwrap());
    assert_eq!(
        answer(&mut m, "GET", &format!("{base}/api/products/1"), None).1,
        json!({"id": 1, "name": "Keyboard"})
    );
    // A request sent twice with two answers is a sequence.
    assert_eq!(
        answer(&mut m, "POST", &format!("{base}/api/orders"), Some("{}")).0,
        503
    );
    assert_eq!(
        answer(&mut m, "POST", &format!("{base}/api/orders"), Some("{}")).0,
        201
    );
    assert_eq!(
        answer(&mut m, "POST", &format!("{base}/api/orders"), Some("{}")).0,
        201
    );
    // The mock with the query goes first.
    assert_eq!(
        answer(&mut m, "GET", &format!("{base}/api/search?q=key"), None).1,
        json!({"results": [1]})
    );
    assert_eq!(
        answer(&mut m, "GET", &format!("{base}/api/search"), None).1,
        json!({"results": []})
    );
    // Small bodies are inline, big ones in data files.
    assert!(
        recorded
            .toml
            .contains(r#"respond = { json = { id = 1, name = "Keyboard" } }"#),
        "{}",
        recorded.toml
    );
    assert!(
        recorded
            .toml
            .contains(r#"respond = { json_file = "data/recorded/api/products.json" }"#),
        "{}",
        recorded.toml
    );
    assert!(recorded.toml.contains("scope = [\"/api/**\"]"));
    assert!(
        recorded.toml.contains("# ids in the path (1)"),
        "{}",
        recorded.toml
    );
}

#[test]
fn without_a_scope_fetches_are_recorded_and_the_scope_is_guessed() {
    let mut page = ex("GET", "http://localhost/app.js", 200, "x");
    page.resource_type = "script".into();
    let exchanges = vec![
        page,
        ex("GET", "http://localhost/api/a", 200, "{}"),
        ex("GET", "http://localhost/api/b/c", 200, "{}"),
    ];
    let recorded = record(&exchanges, &options(&[])).unwrap();
    assert_eq!(recorded.requests, 2);
    assert_eq!(recorded.scope, vec!["/api/**".to_owned()]);
    let mixed = vec![
        ex("GET", "http://localhost/api/a", 200, "{}"),
        ex("GET", "http://localhost/v2/b", 200, "{}"),
    ];
    let recorded = record(&mixed, &options(&[])).unwrap();
    assert!(recorded.scope.is_empty());
    assert!(recorded.toml.contains("# No scope"));
    write(&recorded);
}

#[test]
fn volatile_headers_go_and_needed_cookies_stay() {
    let mut login = ex("POST", "http://localhost/api/login", 200, r#"{"ok":true}"#);
    login.response_headers = vec![
        ("Date".into(), "Mon, 01 Jan 2026 10:00:00 GMT".into()),
        ("ETag".into(), "\"abc\"".into()),
        (
            "Set-Cookie".into(),
            "sid=1; Path=/\ntracking=9; Path=/".into(),
        ),
        ("X-Total-Count".into(), "3".into()),
        (
            "Content-Type".into(),
            "application/json; charset=utf-8".into(),
        ),
    ];
    let mut me = ex("GET", "http://localhost/api/me", 200, r#"{"name":"Ada"}"#);
    me.request_headers = vec![("Cookie".into(), "sid=1".into())];
    let recorded = record(&[login, me], &options(&["/api/**"])).unwrap();
    let toml = &recorded.toml;
    assert!(
        toml.contains(r#"headers = { set-cookie = "sid=1; Path=/", x-total-count = "3" }"#),
        "{toml}"
    );
    assert!(
        !toml.contains("etag") && !toml.contains("Mon, 01"),
        "{toml}"
    );
    assert!(toml.contains("# set-cookie left out"), "{toml}");
    write(&recorded);
}

#[test]
fn ids_and_timestamps_in_bodies_get_a_comment() {
    let body = json!({
        "id": "4b0f6d0e-6c43-4a8e-9a3f-2d9d6f1c8e01",
        "created_at": "2026-01-01T10:00:00Z",
        "items": [{"updatedAt": 1_767_261_600_000_u64}, {"updatedAt": 1_767_261_600_001_u64}],
        "note": "a note that is long enough to go to a data file, surely"
    });
    let recorded = record(
        &[ex(
            "GET",
            "http://localhost/api/cart",
            200,
            &body.to_string(),
        )],
        &options(&["/api/**"]),
    )
    .unwrap();
    let toml = &recorded.toml;
    assert!(toml.contains("# ids in the body ($.id)"), "{toml}");
    assert!(
        toml.contains("# timestamps in the body ($.created_at, $.items[0].updatedAt)"),
        "{toml}"
    );
}

#[test]
fn bodies_that_look_like_templates_and_binaries_go_to_files() {
    let mut png = ex("GET", "http://localhost/api/avatar", 200, "");
    png.response_body = Some(vec![0x89, b'P', b'N', b'G', 0, 1, 2]);
    png.mime = "image/png".into();
    let mut missing = ex("GET", "http://localhost/api/gone", 200, "");
    missing.response_body = None;
    let exchanges = vec![
        ex(
            "GET",
            "http://localhost/api/tpl",
            200,
            r#"{"x":"{{uuid}}"}"#,
        ),
        ex("GET", "http://localhost/api/text", 200, "hello {{name}}"),
        png,
        missing,
        ex("OPTIONS", "http://localhost/api/tpl", 204, ""),
        ex("GET", "http://localhost/api/failed", 0, ""),
    ];
    let recorded = record(&exchanges, &options(&["/api/**"])).unwrap();
    let toml = &recorded.toml;
    assert!(
        toml.contains(r#"json_file = "data/recorded/api/tpl.json""#),
        "{toml}"
    );
    assert!(
        toml.contains(r#"body_file = "data/recorded/api/text.txt""#),
        "{toml}"
    );
    assert!(
        toml.contains(r#"file = "data/recorded/api/avatar.png""#),
        "{toml}"
    );
    assert!(
        toml.contains("# the response body was not recorded"),
        "{toml}"
    );
    assert_eq!(recorded.skipped.len(), 2, "{:?}", recorded.skipped);
    let d = write(&recorded);
    let mut m = mocks(d.path(), &["recorded"]);
    // A data file is served as it is, not as a template.
    assert_eq!(
        answer(&mut m, "GET", "http://localhost/api/tpl", None).1,
        json!({"x": "{{uuid}}"})
    );
}

#[test]
fn graphql_operations_are_mocks_of_their_own() {
    let mut a = ex(
        "POST",
        "http://localhost/graphql",
        200,
        r#"{"data":{"cart":[]}}"#,
    );
    a.request_body = Some(r#"{"query":"query GetCart { cart { id } }"}"#.into());
    let mut b = ex(
        "POST",
        "http://localhost/graphql",
        200,
        r#"{"data":{"me":null}}"#,
    );
    b.request_body = Some(r#"{"operationName":"Me","query":"query Me { me { id } }"}"#.into());
    let recorded = record(&[a, b], &options(&["/graphql"])).unwrap();
    assert_eq!(recorded.mocks, 2, "{}", recorded.toml);
    assert!(
        recorded
            .toml
            .contains(r#"graphql = { operation = "GetCart" }"#)
    );
    let d = write(&recorded);
    let mut m = mocks(d.path(), &["recorded"]);
    let (_, body, name) = answer(
        &mut m,
        "POST",
        "http://localhost/graphql",
        Some(r#"{"operationName":"Me","query":"query Me { me { id } }"}"#),
    );
    assert_eq!(name, "POST /graphql Me");
    assert_eq!(body, json!({"data": {"me": null}}));
}

#[test]
fn a_har_file_is_read() {
    let har = json!({"log": {"entries": [
        {
            "_resourceType": "xhr",
            "request": {"method": "get", "url": "http://localhost/api/x", "headers": [],
                        "cookies": [{"name": "sid", "value": "1"}]},
            "response": {"status": 200, "headers": [{"name": "Set-Cookie", "value": "sid=1"}],
                         "content": {"size": 11, "mimeType": "application/json",
                                     "text": "eyJhIjogMX0=", "encoding": "base64"}}
        },
        {
            "request": {"method": "POST", "url": "http://localhost/api/y",
                        "postData": {"mimeType": "application/json", "text": "{}"}},
            "response": {"status": 204, "content": {"size": 0, "mimeType": ""}}
        }
    ]}});
    let exchanges = har_exchanges(&har).unwrap();
    assert_eq!(exchanges[0].method, "GET");
    assert_eq!(
        exchanges[0].response_body.as_deref(),
        Some(&b"{\"a\": 1}"[..])
    );
    assert_eq!(
        exchanges[0].request_headers,
        vec![("Cookie".to_owned(), "sid=1".to_owned())]
    );
    assert_eq!(exchanges[1].response_body.as_deref(), Some(&b""[..]));
    assert_eq!(exchanges[1].request_body.as_deref(), Some("{}"));
    assert!(har_exchanges(&json!({"x": 1})).is_err());
    let crafted = json!({"log": {"entries": [{
        "request": {"method": "X/../../../../home/u/.bashrc", "url": "https://x.test/api/a"},
        "response": {"status": 200, "content": {"size": 1, "text": "x"}}
    }]}});
    let err = har_exchanges(&crafted).unwrap_err();
    assert!(err.contains("not an HTTP method"), "{err}");
    let recorded = record(&exchanges, &options(&["/api/**"])).unwrap();
    assert!(
        recorded.toml.contains("respond = { status = 204 }"),
        "{}",
        recorded.toml
    );
    let d = write(&recorded);
    MockSet::load(&Source::find(d.path(), d.path(), "recorded").unwrap()).unwrap();
}

/// Whatever the requests were, data files stay under the mock file's
/// directory.
#[test]
fn data_files_stay_in_their_dir() {
    let exchanges = [
        ex(
            "X/../../..",
            "https://x.test/api/%2e%2e/..%2f/items",
            200,
            &big_list(),
        ),
        ex(
            "GET",
            "https://x.test/api/../../etc?q=../../x",
            200,
            &big_list(),
        ),
    ];
    let recorded = record(&exchanges, &options(&["/**"])).unwrap();
    assert_eq!(recorded.files.len(), 2, "{}", recorded.toml);
    for (rel, _) in &recorded.files {
        let normal = std::path::Path::new(rel)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
        assert!(normal, "{rel}");
    }
}

/// A POST of `body` (with its content type) answered by `answer`.
fn post(url: &str, content_type: &str, body: &str, answer: &str) -> Exchange {
    Exchange {
        request_headers: vec![("Content-Type".into(), content_type.into())],
        request_body: Some(body.into()),
        started_ms: Some(1_790_000_000_000),
        ..ex("POST", url, 200, answer)
    }
}

fn login(email: &str, nonce: &str, answer: &str) -> Exchange {
    post(
        "http://app/api/login",
        "application/json",
        &json!({"email": email, "password": "s3cret", "nonce": nonce, "sentAt": 1_790_000_000_000_i64 + i64::from(nonce.as_bytes()[35])})
            .to_string(),
        answer,
    )
}

#[test]
fn match_bodies_tells_logins_apart_by_their_stable_fields() {
    let exchanges = vec![
        login(
            "ada@example.com",
            "6f1c2a5e-1d2b-4c3d-8e9f-0a1b2c3d4e51",
            r#"{"user":"ada"}"#,
        ),
        login(
            "bob@example.com",
            "6f1c2a5e-1d2b-4c3d-8e9f-0a1b2c3d4e52",
            r#"{"user":"bob"}"#,
        ),
        login(
            "ada@example.com",
            "6f1c2a5e-1d2b-4c3d-8e9f-0a1b2c3d4e53",
            r#"{"user":"ada again"}"#,
        ),
        ex("GET", "http://app/api/me", 200, r#"{"user":"ada"}"#),
    ];
    // Without the flag: one mock, a sequence.
    let plain = record(&exchanges, &options(&["/api/**"])).unwrap();
    assert_eq!((plain.mocks, plain.by_body), (2, 0), "{}", plain.toml);
    assert!(
        plain
            .toml
            .contains("match = { method = \"POST\", path = \"/api/login\" }\nsequence"),
        "{}",
        plain.toml
    );

    let recorded = record(
        &exchanges,
        &RecordOptions {
            match_bodies: true,
            ..options(&["/api/**"])
        },
    )
    .unwrap();
    assert_eq!(
        (recorded.mocks, recorded.by_body),
        (3, 3),
        "{}",
        recorded.toml
    );
    // The nonce and the time differ and look generated: left out.
    assert!(
        recorded.toml.contains(
            r#"match = { method = "POST", path = "/api/login", json = { email = "ada@example.com", password = "s3cret" } }"#
        ),
        "{}",
        recorded.toml
    );
    assert!(
        recorded
            .toml
            .contains(r#"name = "POST /api/login (email=bob@example.com)""#),
        "{}",
        recorded.toml
    );
    let d = write(&recorded);
    let mut m = mocks(d.path(), &["recorded"]);
    let body =
        |email: &str| json!({"email": email, "password": "s3cret", "nonce": "other"}).to_string();
    let (_, got, _) = answer(
        &mut m,
        "POST",
        "http://app/api/login",
        Some(&body("bob@example.com")),
    );
    assert_eq!(got, json!({"user": "bob"}));
    // Equal stable fields still form a sequence.
    let (_, got, _) = answer(
        &mut m,
        "POST",
        "http://app/api/login",
        Some(&body("ada@example.com")),
    );
    assert_eq!(got, json!({"user": "ada"}));
    let (_, got, _) = answer(
        &mut m,
        "POST",
        "http://app/api/login",
        Some(&body("ada@example.com")),
    );
    assert_eq!(got, json!({"user": "ada again"}));
    // The GET has no body: as before.
    let (_, got, name) = answer(&mut m, "GET", "http://app/api/me", None);
    assert_eq!(
        (got, name.as_str()),
        (json!({"user": "ada"}), "GET /api/me")
    );
}

#[test]
fn match_bodies_puts_the_more_specific_body_first_and_skips_equal_ones() {
    let exchanges = vec![
        post(
            "http://app/api/cart",
            "application/json",
            r#"{"sku":"a"}"#,
            r#"{"n":1}"#,
        ),
        post(
            "http://app/api/cart",
            "application/json",
            r#"{"sku":"a","gift":true}"#,
            r#"{"n":2}"#,
        ),
        // One request per path: nothing to tell apart, no body match.
        post(
            "http://app/api/save",
            "application/json",
            r#"{"x":1}"#,
            "{}",
        ),
    ];
    let recorded = record(
        &exchanges,
        &RecordOptions {
            match_bodies: true,
            ..options(&["/api/**"])
        },
    )
    .unwrap();
    assert_eq!(
        (recorded.mocks, recorded.by_body),
        (3, 2),
        "{}",
        recorded.toml
    );
    assert!(
        recorded
            .toml
            .contains(r#"match = { method = "POST", path = "/api/save" }"#),
        "{}",
        recorded.toml
    );
    let d = write(&recorded);
    let mut m = mocks(d.path(), &["recorded"]);
    let (_, got, _) = answer(
        &mut m,
        "POST",
        "http://app/api/cart",
        Some(r#"{"sku":"a"}"#),
    );
    assert_eq!(got, json!({"n": 1}));
    let (_, got, _) = answer(
        &mut m,
        "POST",
        "http://app/api/cart",
        Some(r#"{"sku":"a","gift":true}"#),
    );
    assert_eq!(got, json!({"n": 2}));
}

#[test]
fn match_bodies_reads_forms_and_graphql_variables() {
    let multipart = |user: &str| {
        format!(
            "--b\r\nContent-Disposition: form-data; name=\"user\"\r\n\r\n{user}\r\n\
             --b\r\nContent-Disposition: form-data; name=\"avatar\"; filename=\"{user}.png\"\r\n\
             Content-Type: image/png\r\n\r\nPNG\r\n--b--\r\n"
        )
    };
    let gql = |id: u32| {
        json!({"query": "query GetUser($id: ID!) { user(id: $id) { name } }", "variables": {"id": id}})
            .to_string()
    };
    let exchanges = vec![
        post(
            "http://app/api/login",
            "application/x-www-form-urlencoded",
            "email=ada%40example.com&csrf=6f1c2a5e-1d2b-4c3d-8e9f-0a1b2c3d4e51",
            r#"{"user":"ada"}"#,
        ),
        post(
            "http://app/api/login",
            "application/x-www-form-urlencoded",
            "email=bob%40example.com&csrf=6f1c2a5e-1d2b-4c3d-8e9f-0a1b2c3d4e52",
            r#"{"user":"bob"}"#,
        ),
        post(
            "http://app/api/avatar",
            "multipart/form-data; boundary=b",
            &multipart("ada"),
            r#"{"ok":"ada"}"#,
        ),
        post(
            "http://app/api/avatar",
            "multipart/form-data; boundary=b",
            &multipart("bob"),
            r#"{"ok":"bob"}"#,
        ),
        post(
            "http://app/graphql",
            "application/json",
            &gql(1),
            r#"{"data":1}"#,
        ),
        post(
            "http://app/graphql",
            "application/json",
            &gql(2),
            r#"{"data":2}"#,
        ),
    ];
    let recorded = record(
        &exchanges,
        &RecordOptions {
            match_bodies: true,
            ..options(&[])
        },
    )
    .unwrap();
    assert_eq!(
        (recorded.mocks, recorded.by_body),
        (6, 6),
        "{}",
        recorded.toml
    );
    let toml = &recorded.toml;
    assert!(
        toml.contains(r#"form = { email = "ada@example.com" }"#),
        "{toml}"
    );
    // A multipart body's text fields; its file is left out.
    assert!(toml.contains(r#"form = { user = "bob" }"#), "{toml}");
    assert!(
        toml.contains(r#"graphql = { operation = "GetUser", variables = { id = 2 } }"#),
        "{toml}"
    );
    let d = write(&recorded);
    let mut m = mocks(d.path(), &["recorded"]);
    let (_, got, _) = answer(
        &mut m,
        "POST",
        "http://app/api/login",
        Some("email=bob%40example.com&csrf=x"),
    );
    assert_eq!(got, json!({"user": "bob"}));
    let (_, got, _) = answer(&mut m, "POST", "http://app/graphql", Some(&gql(2)));
    assert_eq!(got, json!({"data": 2}));
    let headers = [(
        "Content-Type".to_owned(),
        "multipart/form-data; boundary=b".to_owned(),
    )];
    let sent = multipart("bob");
    let request = crate::Request {
        method: "POST",
        url: "http://app/api/avatar",
        headers: &headers,
        body: Some(sent.as_bytes()),
    };
    let crate::Outcome::Answer(a) = m.decide(&request, 0) else {
        panic!()
    };
    assert_eq!(a.mock.name, "POST /api/avatar (user=bob)");
}

#[test]
fn a_har_entrys_start_is_read() {
    let har = json!({"log": {"entries": [{
        "startedDateTime": "2026-10-01T00:00:00.250Z",
        "request": {"method": "GET", "url": "http://app/api/x", "headers": []},
        "response": {"status": 200, "headers": [], "content": {"size": 0}},
    }]}});
    let exchanges = har_exchanges(&har).unwrap();
    assert_eq!(exchanges[0].started_ms, Some(1_790_812_800_250));
}
