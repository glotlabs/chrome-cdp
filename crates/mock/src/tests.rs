//! Mock sets end to end: files on disk, loaded, layered and asked about
//! requests.

use std::path::Path;

use serde_json::{Value, json};

use crate::{MockSet, Mocks, Outcome, Request, Response, Source};

/// A mocks directory with `files` (path → text) in it.
pub(crate) fn dir(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in files {
        let file = dir.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    dir
}

pub(crate) fn set(dir: &Path, name: &str) -> MockSet {
    MockSet::load(&Source::find(dir, dir, name).unwrap()).unwrap()
}

pub(crate) fn load_err(files: &[(&str, &str)]) -> String {
    let d = dir(files);
    MockSet::load(&Source::find(d.path(), d.path(), "base").unwrap())
        .unwrap_err()
        .message
}

pub(crate) fn mocks(dir: &Path, names: &[&str]) -> Mocks {
    let mut mocks = Mocks::new(7);
    mocks.apply(names.iter().map(|n| set(dir, n)).collect(), false);
    mocks
}

pub(crate) fn req<'a>(method: &'a str, url: &'a str, body: Option<&'a [u8]>) -> Request<'a> {
    Request {
        method,
        url,
        headers: &[],
        body,
    }
}

/// The status and JSON body a request gets, and the mock's name.
pub(crate) fn answer(
    mocks: &mut Mocks,
    method: &str,
    url: &str,
    body: Option<&str>,
) -> (u16, Value, String) {
    match mocks.decide(&req(method, url, body.map(str::as_bytes)), 0) {
        Outcome::Answer(a) => match a.response {
            Response::Fulfill { status, body, .. } => (
                status,
                serde_json::from_slice(&body).unwrap_or(Value::Null),
                a.mock.name,
            ),
            other => panic!("{other:?}"),
        },
        other => panic!("{url}: {other:?}"),
    }
}

const BASE: &str = r#"
[settings]
scope = ["/api/**"]

[[mock]]
name = "list products"
match = { method = "GET", path = "/api/products" }
respond = { json_file = "data/products.json" }

[[mock]]
name = "product by id"
match = { method = "GET", path = "/api/products/:id" }
respond = { json_file = "data/products/{{params.id}}.json" }

[[mock]]
name = "search"
match = { method = "GET", path = "/api/search", query = { q = "*" } }
respond = { json = { results = [], query = "{{query.q}}" } }

[[mock]]
name = "login"
match = { method = "POST", path = "/api/login", json = { email = "*@example.com" } }
respond = { status = 200, headers = { "set-cookie" = "sid=test; Path=/" }, json = { ok = true } }

[[mock]]
name = "orders"
match = { method = "POST", path = "/api/orders" }
respond = { status = 201, json = { id = "{{uuid}}", n = "{{seq}}", qty = "{{json.qty}}" } }
"#;

fn shop() -> tempfile::TempDir {
    dir(&[
        ("base.toml", BASE),
        ("data/products.json", r#"[{"id":1},{"id":2}]"#),
        ("data/products/1.json", r#"{"id":1,"name":"Keyboard"}"#),
        (
            "checkout.toml",
            r#"
[[mock]]
name = "one product only"
match = { method = "GET", path = "/api/products" }
respond = { json = [{ id = 1 }] }
"#,
        ),
    ])
}

#[test]
fn mocks_answer_what_they_match() {
    let d = shop();
    let mut m = mocks(d.path(), &["base"]);
    let (status, body, name) = answer(&mut m, "GET", "http://app/api/products", None);
    assert_eq!((status, name.as_str()), (200, "list products"));
    assert_eq!(body, json!([{"id": 1}, {"id": 2}]));
    let (_, body, _) = answer(&mut m, "GET", "http://app/api/products/1", None);
    assert_eq!(body["name"], "Keyboard");
    let (_, body, _) = answer(&mut m, "GET", "http://app/api/search?q=keyboard", None);
    assert_eq!(body, json!({"results": [], "query": "keyboard"}));
    let (status, body, _) = answer(
        &mut m,
        "POST",
        "http://app/api/login",
        Some(r#"{"email":"ada@example.com"}"#),
    );
    assert_eq!((status, body), (200, json!({"ok": true})));
}

#[test]
fn templates_keep_types_and_count() {
    let d = shop();
    let mut m = mocks(d.path(), &["base"]);
    let order = Some(r#"{"qty": 3}"#);
    let (status, first, _) = answer(&mut m, "POST", "http://app/api/orders", order);
    let (_, second, _) = answer(&mut m, "POST", "http://app/api/orders", order);
    assert_eq!(status, 201);
    assert_eq!(
        (first["n"].clone(), second["n"].clone()),
        (json!(1), json!(2))
    );
    assert_eq!(first["qty"], json!(3));
    assert_ne!(first["id"], second["id"]);
    // The same seed gives the same ids after a reset.
    m.reset();
    let (_, again, _) = answer(&mut m, "POST", "http://app/api/orders", order);
    assert_eq!(
        (again["id"].clone(), again["n"].clone()),
        (first["id"].clone(), json!(1))
    );
}

#[test]
fn a_missing_data_file_answers_404() {
    let d = shop();
    let mut m = mocks(d.path(), &["base"]);
    let Outcome::Answer(a) = m.decide(&req("GET", "http://app/api/products/9", None), 0) else {
        panic!()
    };
    assert_eq!(a.outcome, "fulfill 404 (no data/products/9.json)");
    // A parameter cannot climb out of the mocks directory.
    let Outcome::Answer(a) = m.decide(&req("GET", "http://app/api/products/..%2Fsecret", None), 0)
    else {
        panic!()
    };
    assert!(
        a.outcome.starts_with("fulfill 500 (mock error:"),
        "{}",
        a.outcome
    );
}

#[test]
fn later_sets_win_and_unmatched_requests_in_scope_fail() {
    let d = shop();
    let mut m = mocks(d.path(), &["base", "checkout"]);
    let (_, body, name) = answer(&mut m, "GET", "http://app/api/products", None);
    assert_eq!(
        (body, name.as_str()),
        (json!([{"id": 1}]), "one product only")
    );
    // What checkout does not mock, base still does.
    let (_, _, name) = answer(&mut m, "GET", "http://app/api/products/1", None);
    assert_eq!(name, "product by id");
    match m.decide(&req("GET", "http://app/api/cart", None), 0) {
        Outcome::Unmatched {
            set,
            response: Response::Fulfill {
                status, headers, ..
            },
            outcome,
            ..
        } => {
            assert_eq!((set.as_str(), status), ("base", 501));
            assert_eq!(outcome, "unmatched, fulfill 501");
            assert!(
                headers
                    .iter()
                    .any(|(n, v)| n == "X-Chrome-Cdp-Unmatched" && v == "base")
            );
        }
        other => panic!("{other:?}"),
    }
    // Out of scope: the network.
    assert!(matches!(
        m.decide(&req("GET", "http://app/assets/app.js", None), 0),
        Outcome::Pass
    ));
    // The order they are tried: the last applied first.
    let names: Vec<String> = m.info(false).into_iter().map(|s| s.name).collect();
    assert_eq!(names, ["checkout", "base"]);
    // `add` layers on top; applying a set again moves it up.
    m.apply(vec![set(d.path(), "base")], true);
    let names: Vec<String> = m.info(false).into_iter().map(|s| s.name).collect();
    assert_eq!(names, ["base", "checkout"]);
    assert_eq!(m.remove(&["nope".into()]), Err("nope".into()));
    assert_eq!(m.remove(&["base".into()]), Ok(vec!["base".into()]));
    assert_eq!(m.clear(), ["checkout"]);
}

#[test]
fn unmatched_requests_name_the_closest_mocks() {
    let d = shop();
    let mut m = mocks(d.path(), &["base", "checkout"]);
    let closest = |m: &mut Mocks, method, url, body| match m.decide(&req(method, url, body), 0) {
        Outcome::Unmatched {
            closest, policy, ..
        } => {
            assert_eq!(policy, crate::Unmatched::Fail);
            closest
                .into_iter()
                .map(|c| {
                    format!(
                        "{} {}: {}",
                        c.mock.at,
                        c.mock.name,
                        c.reason.unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
        }
        other => panic!("{other:?}"),
    };
    // A typo in the path: the mocks of that path, in the order they are
    // tried.
    assert_eq!(
        closest(&mut m, "GET", "http://app/api/product", None),
        [
            "checkout.toml:2 one product only: path differs: expects /api/products, got /api/product",
            "base.toml:5 list products: path differs: expects /api/products, got /api/product",
        ]
    );
    // The method and path match, the body does not: that mock alone.
    assert_eq!(
        closest(
            &mut m,
            "POST",
            "http://app/api/login",
            Some(br#"{"email":"ada@other.org"}"#.as_slice())
        ),
        [r#"base.toml:20 login: json $.email: expects "*@example.com", got "ada@other.org""#]
    );
    // The method differs.
    assert_eq!(
        closest(&mut m, "PUT", "http://app/api/orders", None),
        ["base.toml:25 orders: method differs: expects POST, got PUT"]
    );
    // Nothing close.
    assert!(closest(&mut m, "GET", "http://app/api/cart", None).is_empty());
}

#[test]
fn unmatched_policies() {
    let files = |policy: &str| {
        format!(
            "[settings]\nscope = \"https://api.example.com/**\"\nunmatched = \"{policy}\"\n[[mock]]\nmatch = {{ path = \"/x\" }}\nrespond = {{ status = 204 }}\n"
        )
    };
    for (policy, expected) in [
        ("passthrough", "unmatched, passthrough"),
        ("abort", "unmatched, abort failed"),
        ("fail", "unmatched, fulfill 501"),
    ] {
        let d = dir(&[("base.toml", &files(policy))]);
        let mut m = mocks(d.path(), &["base"]);
        let Outcome::Unmatched { outcome, .. } =
            m.decide(&req("GET", "https://api.example.com/v1/y", None), 0)
        else {
            panic!("{policy}")
        };
        assert_eq!(outcome, expected);
        // Another host is not in the scope.
        assert!(matches!(
            m.decide(&req("GET", "https://cdn.example.com/v1/y", None), 0),
            Outcome::Pass
        ));
    }
}

#[test]
fn cors_preflights() {
    let d = shop();
    let mut m = mocks(d.path(), &["base"]);
    let headers = [(
        "Access-Control-Request-Method".to_owned(),
        "POST".to_owned(),
    )];
    let preflight = |url| Request {
        method: "OPTIONS",
        url,
        headers: &headers,
        body: None,
    };
    match m.decide(&preflight("http://api.test/api/orders"), 0) {
        Outcome::Preflight { mock: Some(mock) } => assert_eq!(mock.name, "orders"),
        other => panic!("{other:?}"),
    }
    // "login" matches on the body, which a preflight does not show: it is
    // allowed, but no mock is named, since another may answer the POST.
    assert!(matches!(
        m.decide(&preflight("http://api.test/api/login"), 0),
        Outcome::Preflight { mock: None }
    ));
    // In scope but unmocked: allowed too, so the page sees the 501.
    assert!(matches!(
        m.decide(&preflight("http://api.test/api/other"), 0),
        Outcome::Preflight { mock: None }
    ));
    assert!(matches!(
        m.decide(&preflight("http://api.test/elsewhere"), 0),
        Outcome::Pass
    ));
    let off = dir(&[(
        "base.toml",
        "[settings]\ncors = \"off\"\nscope = [\"/api/**\"]\n[[mock]]\nmatch = { path = \"/api/x\" }\nrespond = { status = 204 }\n",
    )]);
    let mut m = mocks(off.path(), &["base"]);
    assert!(matches!(
        m.decide(&preflight("http://api.test/api/x"), 0),
        Outcome::Answer(_)
    ));
}

#[test]
fn answers_of_every_kind() {
    let d = dir(&[
        (
            "base.toml",
            r#"
[settings]
latency = "20ms..40ms"

[[mock]]
match = { path = "/reset" }
respond = { abort = "connection-reset" }

[[mock]]
match = { path = "/proxy" }
respond = { passthrough = true, patch = { flag = true } }

[[mock]]
match = { path = "/text" }
respond = { body = "hi {{query.name}}", delay = "5ms" }

[[mock]]
match = { path = "/logo" }
respond = { file = "data/logo.png" }

[[mock]]
match = { path = "/greeting" }
respond = { body_file = "data/greeting.tmpl.txt" }

[[mock]]
match = { path = "/user" }
respond = { json_file = "data/user.tmpl.json" }

[[mock]]
match = { path = "/broken" }
respond = { json = { x = "{{json.missing}}" } }
"#,
        ),
        ("data/logo.png", "\u{89}PNG"),
        (
            "data/greeting.tmpl.txt",
            "hello {{params.none | default('you')}}",
        ),
        (
            "data/user.tmpl.json",
            r#"{"id": "{{seq}}", "name": "{{faker.name}}"}"#,
        ),
    ]);
    let mut m = mocks(d.path(), &["base"]);
    let mut get = |url: &str| match m.decide(&req("GET", url, None), 0) {
        Outcome::Answer(a) => a,
        other => panic!("{url}: {other:?}"),
    };
    let a = get("http://x/reset");
    assert!(matches!(
        a.response,
        Response::Abort {
            reason: "ConnectionReset"
        }
    ));
    assert!((20..=40).contains(&a.delay.as_millis()));
    let a = get("http://x/proxy");
    assert!(
        matches!(&a.response, Response::Passthrough { patch: Some(p) } if p == &json!({"flag": true}))
    );
    let a = get("http://x/text?name=Ada");
    assert_eq!(a.delay.as_millis(), 5);
    let Response::Fulfill { headers, body, .. } = a.response else {
        panic!()
    };
    assert_eq!(body, b"hi Ada");
    assert!(headers.contains(&("Content-Type".into(), "text/plain; charset=utf-8".into())));
    let Response::Fulfill { headers, .. } = get("http://x/logo").response else {
        panic!()
    };
    assert!(headers.contains(&("Content-Type".into(), "image/png".into())));
    let Response::Fulfill { body, .. } = get("http://x/greeting").response else {
        panic!()
    };
    assert_eq!(body, b"hello you");
    let Response::Fulfill { body, .. } = get("http://x/user").response else {
        panic!()
    };
    let user: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(user["id"], json!(1));
    assert!(user["name"].as_str().unwrap().contains(' '));
    let a = get("http://x/broken");
    assert!(
        a.outcome
            .starts_with("fulfill 500 (mock error: `{{json.missing}}`")
            || a.outcome.contains("undefined"),
        "{}",
        a.outcome
    );
}

#[test]
fn explain_says_why() {
    let d = shop();
    let m = mocks(d.path(), &["base", "checkout"]);
    let data = m.explain(&req("GET", "http://app/api/products/1", None));
    assert_eq!(data.answer.as_ref().unwrap().name, "product by id");
    assert_eq!(data.answer.as_ref().unwrap().at, "base.toml:10");
    assert_eq!(data.outcome, "fulfill 200 json_file data/products/1.json");
    assert_eq!(data.params["id"], "1");
    let reasons: Vec<(&str, Option<&str>)> = data
        .checked
        .iter()
        .map(|c| (c.mock.name.as_str(), c.reason.as_deref()))
        .collect();
    assert_eq!(
        reasons[..3],
        [
            (
                "one product only",
                Some("path differs: expects /api/products, got /api/products/1")
            ),
            (
                "list products",
                Some("path differs: expects /api/products, got /api/products/1")
            ),
            ("product by id", None),
        ]
    );
    let data = m.explain(&req("GET", "http://app/api/cart?user=7", None));
    assert!(data.answer.is_none());
    assert_eq!(data.scope.as_deref(), Some("base"));
    assert!(
        data.outcome.contains("unmatched = \"fail\""),
        "{}",
        data.outcome
    );
    // Explaining counts no hits.
    assert!(
        m.info(true)
            .iter()
            .all(|s| s.mocks.iter().all(|m| m.hits == 0))
    );
}

#[test]
fn changed_files_are_reloaded() {
    let d = shop();
    let mut m = mocks(d.path(), &["checkout"]);
    let (_, _, name) = answer(&mut m, "GET", "http://app/api/products", None);
    assert_eq!(name, "one product only");
    let file = d.path().join("checkout.toml");
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::write(
        &file,
        "[[mock]]\nname = \"renamed\"\nmatch = { path = \"/api/products\" }\nrespond = { json = [] }\n",
    )
    .unwrap();
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(later)
        .unwrap();
    m.refresh();
    let (_, body, name) = answer(&mut m, "GET", "http://app/api/products", None);
    assert_eq!((name.as_str(), body), ("renamed", json!([])));
    // A broken change keeps the set as it was, and says why.
    std::fs::write(&file, "[[mock]]\nmatch = {").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(later + std::time::Duration::from_secs(5))
        .unwrap();
    // `refresh` looks at most every 250ms.
    std::thread::sleep(std::time::Duration::from_millis(260));
    m.refresh();
    let (_, _, name) = answer(&mut m, "GET", "http://app/api/products", None);
    assert_eq!(name, "renamed");
    let error = m.info(false)[0].reload_error.clone().unwrap();
    assert!(error.contains("checkout.toml:2:"), "{error}");
}

#[test]
fn problems_point_at_the_line() {
    let cases: &[(&str, &str)] = &[
        (
            "[[mock]]\nmatch = { path = \"/a\" }\n",
            "base.toml:1:1: the mock has no `respond`",
        ),
        (
            "[[mock]]\nmatch = { path = \"/a\" }\nrespond = { json_file = \"data/nope.json\" }\n",
            "base.toml:3:25: respond.json_file: ",
        ),
        (
            "[[mock]]\nmatch = { path = \"a\" }\nrespond = {}\n",
            "base.toml:2:18: match.path: `a` is no path",
        ),
        (
            "[[mock]]\nmatch = {}\nrespond = {}\n",
            "base.toml:2:9: match: no condition",
        ),
        (
            "[[mock]]\nmatch = { path = \"/a\", url = \"http://x/a\" }\nrespond = {}\n",
            "match: give one of `path`, `url` and `regex`",
        ),
        (
            "[[mock]]\nmatch = { path = \"/a\" }\nrespond = { json = {}, body = \"x\" }\n",
            "respond: give one body, not json and body",
        ),
        (
            "[[mock]]\nmatch = { path = \"/a\" }\nrespond = { abort = \"boom\" }\n",
            "respond.abort: `boom` is not a network error",
        ),
        (
            "[[mock]]\nmatch = { path = \"/a\" }\nrespond = { status = 42 }\n",
            "respond.status: 42 cannot be answered",
        ),
        (
            "[[mock]]\nmatch = { path = \"/a\" }\nrespond = { body = \"{{ oops\" }\n",
            "base.toml:3:20: respond.body: template `{{ oops`",
        ),
        (
            "[[mock]]\nmatch = { path = \"/a\" }\nrespond = { patch = {} }\n",
            "respond.patch: needs passthrough = true",
        ),
        (
            "[[mock]]\nmatch = { path = \"/a\" }\nrespond = { passthrough = true, status = 200 }\n",
            "`passthrough` sends the request on",
        ),
        (
            "[settings]\nlatency = \"soon\"\n",
            "base.toml:2:11: settings.latency: ",
        ),
        (
            "[settings]\nscope = [\"api/**\"]\n",
            "settings.scope: `api/**` is neither a path glob",
        ),
        (
            "[settings]\nunmatched = \"ignore\"\n",
            "base.toml:2:13: unknown variant `ignore`",
        ),
        (
            "[[mock]]\nmatch = { path = \"/a\", methd = \"GET\" }\nrespond = {}\n",
            "base.toml:2:24: unknown field `methd`",
        ),
    ];
    for (text, expected) in cases {
        let message = load_err(&[("base.toml", text)]);
        assert!(message.contains(expected), "{expected}\n  got: {message}");
    }
}

#[test]
fn sets_are_found_by_name() {
    let d = dir(&[
        ("base.toml", ""),
        ("errors/api-down.toml", ""),
        ("data/products.json", "[]"),
    ]);
    assert_eq!(crate::available(d.path()), ["base", "errors/api-down"]);
    let source = Source::find(d.path(), d.path(), "errors/api-down").unwrap();
    assert_eq!(source.base, d.path());
    let err = Source::find(d.path(), d.path(), "bsae").unwrap_err();
    assert!(
        err.message.starts_with("no mock set `bsae`"),
        "{}",
        err.message
    );
    assert_eq!(err.notes[0].text, "base");
    let err = Source::find(d.path(), d.path(), "../x").unwrap_err();
    assert!(err.message.starts_with("no mock set"));
    // A path to a file: its data files are next to it.
    let source = Source::find(d.path(), d.path(), "errors/api-down.toml").unwrap();
    assert_eq!(source.base, d.path().join("errors"));
}

#[test]
fn the_schema_describes_the_format() {
    let schema = crate::schema();
    assert_eq!(schema["title"], "chrome-cdp mock file");
    let text = schema.to_string();
    for key in [
        "settings",
        "mock",
        "json_file",
        "passthrough",
        "graphql",
        "json_mode",
        "latency",
    ] {
        assert!(text.contains(&format!("\"{key}\"")), "{key}");
    }
}

#[test]
fn multipart_forms_match_and_render() {
    let d = dir(&[(
        "upload.toml",
        r#"
[[mock]]
name = "profile"
match = { method = "POST", path = "/api/profile", form = { email = "*@example.com", avatar = "*.png", tag = "b" } }
respond = { json = { email = "{{form.email}}", file = "{{form_files.avatar.filename}}", type = "{{form_files.avatar.content_type}}", size = "{{form_files.avatar.size}}" } }
"#,
    )]);
    let mut m = mocks(d.path(), &["upload"]);
    let sent = b"--XyZ\r\n\
        Content-Disposition: form-data; name=\"email\"\r\n\r\n\
        ada@example.com\r\n\
        --XyZ\r\n\
        Content-Disposition: form-data; name=\"tag\"\r\n\r\n\
        a\r\n\
        --XyZ\r\n\
        Content-Disposition: form-data; name=\"tag\"\r\n\r\n\
        b\r\n\
        --XyZ\r\n\
        Content-Disposition: form-data; name=\"avatar\"; filename=\"me.png\"\r\n\
        Content-Type: image/png\r\n\r\n\
        \x89PNG\x00\xff\r\n\
        --XyZ--\r\n";
    let headers = [(
        "Content-Type".to_owned(),
        "multipart/form-data; boundary=XyZ".to_owned(),
    )];
    let request = Request {
        method: "POST",
        url: "http://app/api/profile",
        headers: &headers,
        body: Some(sent),
    };
    let Outcome::Answer(a) = m.decide(&request, 0) else {
        panic!("{:?}", m.explain(&request).checked);
    };
    let Response::Fulfill { body, .. } = a.response else {
        panic!()
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"email": "ada@example.com", "file": "me.png", "type": "image/png", "size": 6})
    );
    // A multipart body cut off before its closing boundary is no form,
    // and the reason says why.
    let request = Request {
        body: Some(&sent[..sent.len() - 10]),
        ..request
    };
    let data = m.explain(&request);
    assert_eq!(
        data.checked[0].reason.as_deref(),
        Some("form: the body is not a form (multipart body without a closing boundary)")
    );
}
