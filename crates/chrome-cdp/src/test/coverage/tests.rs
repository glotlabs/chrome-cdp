use protocol::schema::FingerprintContainer;
use serde_json::json;

use super::*;

/// The page with two "Buy" buttons in main and one in the navigation,
/// and a dialog's buttons once it opened.
fn shop() -> Value {
    json!({
        "tab": "t1", "url": "http://localhost:4173/products/12?ref=x", "title": "Shop", "engine": "ax",
        "nodes": [
            {"role": "main", "children": [
                {"role": "button", "name": "Open", "ref": "e1"},
                {"role": "button", "name": "Buy", "ref": "e2"},
                {"role": "button", "name": "Buy", "ref": "e3"},
                {"role": "iframe", "ref": "e4"}
            ]},
            {"role": "navigation", "children": [
                {"role": "button", "name": "Buy", "ref": "e5"}
            ]}
        ]
    })
}

fn dialog() -> Value {
    json!({
        "tab": "t1", "url": "http://localhost:4173/products/12", "title": "Shop", "engine": "ax",
        "diff": true,
        "nodes": [
            {"role": "dialog", "name": "Menu", "change": "added", "children": [
                {"role": "button", "name": "Close", "ref": "e6", "change": "added"},
                {"role": "link", "name": "Terms", "ref": "e7", "change": "added"}
            ]},
            {"role": "main", "change": "removed"},
            {"role": "navigation", "children": [
                {"role": "button", "name": "Buy", "ref": "e5"}
            ]}
        ]
    })
}

fn fingerprint(role: &str, name: &str, page: &str, container: Option<&str>) -> Fingerprint {
    Fingerprint {
        role: role.into(),
        name: name.into(),
        page: page.into(),
        container: container.map(|role| FingerprintContainer {
            role: role.into(),
            name: String::new(),
        }),
        ..Fingerprint::default()
    }
}

fn keys(page: &UiCoveragePage) -> Vec<&str> {
    page.controls.iter().map(|c| c.key.as_str()).collect()
}

#[test]
fn pages_are_path_templates_or_the_configs_patterns() {
    assert_eq!(page_of("/products/12", &[]), "/products/:id");
    assert_eq!(page_of("/cart/", &[]), "/cart");
    assert_eq!(page_of("/", &[]), "/");
    let patterns = [
        "/products/:slug".to_owned(),
        "/docs/**".to_owned(),
        "/a/*/b".to_owned(),
    ];
    assert_eq!(page_of("/products/red-shoes", &patterns), "/products/:slug");
    assert_eq!(page_of("/products/:id", &patterns), "/products/:slug");
    assert_eq!(page_of("/products", &patterns), "/products");
    assert_eq!(page_of("/docs/a/b/c", &patterns), "/docs/**");
    assert_eq!(page_of("/a/x/b", &patterns), "/a/*/b");
    assert_eq!(page_of("/a/x/c", &patterns), "/a/x/c");
    assert_eq!(path_of("http://h:1/x/y?q=1#f"), Some("/x/y"));
    assert_eq!(path_of("http://h:1"), Some("/"));
    assert_eq!(path_of("about:blank"), None);
}

#[test]
fn a_shared_role_and_name_gets_its_container() {
    let mut coverage = Coverage::default();
    coverage.snapshot(&shop(), Snap::Full, "a.cdp:1", &[]);
    let data = coverage.report(&[]);
    assert_eq!(data.pages.len(), 1);
    let page = &data.pages[0];
    assert_eq!(page.page, "/products/:id");
    assert_eq!(page.tests, 1);
    // The two in main are one control; the frame is none.
    assert_eq!(
        keys(page),
        [
            "button \"Open\"",
            "button \"Buy\" in main",
            "button \"Buy\" in navigation"
        ]
    );
    assert_eq!(data.controls, 3);
}

#[test]
fn a_diff_adds_only_what_it_marks_added() {
    let mut coverage = Coverage::default();
    coverage.snapshot(&shop(), Snap::Full, "a.cdp:1", &[]);
    coverage.snapshot(&dialog(), Snap::Added, "a.cdp:1", &[]);
    let data = coverage.report(&[]);
    assert_eq!(
        keys(&data.pages[0]),
        [
            "button \"Open\"",
            "button \"Buy\" in main",
            "button \"Buy\" in navigation",
            "button \"Close\"",
            "link \"Terms\""
        ]
    );
    // A diff with nothing to compare with is the whole page.
    let mut first = dialog();
    first["diff"] = json!(false);
    let mut coverage = Coverage::default();
    coverage.snapshot(&first, Snap::Added, "a.cdp:1", &[]);
    assert_eq!(coverage.report(&[]).controls, 3);
}

#[test]
fn touches_count_on_the_control_they_are() {
    let mut coverage = Coverage::default();
    coverage.snapshot(&shop(), Snap::Full, "a.cdp:1", &[]);
    let page = "/products/:id";
    coverage.touch(
        &fingerprint("button", "Open", page, Some("main")),
        true,
        "a.cdp:1",
        &[],
    );
    coverage.touch(
        &fingerprint("button", "Open", page, Some("main")),
        true,
        "b.cdp:1",
        &[],
    );
    coverage.touch(
        &fingerprint("button", "Buy", page, Some("navigation")),
        false,
        "a.cdp:1",
        &[],
    );
    // A list item is no landmark: the first "Buy" it is.
    coverage.touch(
        &fingerprint("button", "Buy", page, Some("listitem")),
        true,
        "a.cdp:1",
        &[],
    );
    // Text an `expect` checked is no control; a control no snapshot saw
    // is one.
    coverage.touch(
        &fingerprint("paragraph", "Total", page, None),
        false,
        "a.cdp:1",
        &[],
    );
    coverage.touch(
        &fingerprint("checkbox", "Gift", page, Some("form")),
        false,
        "a.cdp:1",
        &[],
    );
    let data = coverage.report(&[]);
    let page = &data.pages[0];
    let counts: Vec<(&str, u64, u64)> = page
        .controls
        .iter()
        .map(|c| (c.key.as_str(), c.acted, c.checked))
        .collect();
    assert_eq!(
        counts,
        [
            ("button \"Open\"", 2, 0),
            ("button \"Buy\" in main", 1, 0),
            ("button \"Buy\" in navigation", 0, 1),
            ("checkbox \"Gift\"", 0, 1),
        ]
    );
    assert_eq!(page.tests, 2);
    assert_eq!((data.controls, data.acted, data.checked), (4, 2, 2));
}

#[test]
fn listed_pages_no_test_visited_are_named() {
    let mut coverage = Coverage::default();
    let patterns = ["/products/:id".to_owned(), "/admin".to_owned()];
    coverage.snapshot(&shop(), Snap::Full, "a.cdp:1", &patterns);
    let data = coverage.report(&patterns);
    assert_eq!(data.pages[0].page, "/products/:id");
    assert_eq!(data.unvisited, ["/admin"]);
}

fn mock(at: &str, line: usize, hits: u64) -> Value {
    json!({"set": "base", "name": format!("mock {line}"), "file": "/m/base.toml", "line": line,
           "at": at, "matches": "GET /api/x", "responds": "200", "hits": hits})
}

#[test]
fn runs_merge_in_order_and_add_up() {
    let list = |a: u64, b: u64| {
        json!({"sets": [{"name": "base", "file": "/m/base.toml", "count": 2, "unmatched": "fail",
                         "cors": "auto", "latency": "0ms",
                         "mocks": [mock("base.toml:1", 1, a), mock("base.toml:9", 9, b)]}]})
    };
    let mut one = Coverage::default();
    one.snapshot(&shop(), Snap::Full, "a.cdp:1", &[]);
    one.mocks(&list(1, 0), &[]);
    one.requests(
        [
            ("GET", "http://h/api/products/3?x=1", "fetch"),
            ("GET", "http://h/app.js", "script"),
        ]
        .into_iter(),
    );
    let mut two = Coverage::default();
    two.snapshot(&dialog(), Snap::Full, "b.cdp:1", &[]);
    two.mocks(&list(2, 0), &[]);
    two.requests([("GET", "http://h/api/products/4", "xhr")].into_iter());
    one.merge(two);
    let data = one.report(&[]);
    assert_eq!(data.pages[0].tests, 2);
    assert_eq!(data.controls, 5);
    let hits: Vec<(&str, u64)> = data
        .mocks
        .iter()
        .map(|m| (m.mock.at.as_str(), m.hits))
        .collect();
    assert_eq!(hits, [("base.toml:1", 3), ("base.toml:9", 0)]);
    assert_eq!(data.requests.len(), 1);
    assert_eq!(
        (data.requests[0].path.as_str(), data.requests[0].count),
        ("/api/products/:id", 2)
    );
    let text = text(&data, false);
    assert!(
        text.starts_with("ui coverage: 1 page, 5 controls, 0 acted on (0%)\n"),
        "{text}"
    );
    assert!(
        text.contains("mocks never used: base.toml:9 \"mock 9\"\n"),
        "{text}"
    );
    assert!(
        text.contains("api requests: GET /api/products/:id ×2\n"),
        "{text}"
    );
}

#[test]
fn the_summary_is_bounded() {
    let mut coverage = Coverage::default();
    for n in 0..25 {
        let mut page = shop();
        page["url"] = json!(format!("http://h/p{n}"));
        coverage.snapshot(&page, Snap::Full, "a.cdp:1", &[]);
    }
    coverage.touch(
        &fingerprint("button", "Open", "/p0", Some("main")),
        true,
        "a.cdp:1",
        &[],
    );
    let data = coverage.report(&[]);
    let text = text(&data, false);
    assert_eq!(text.lines().count(), 1 + 20 + 1 + 1, "{text}");
    assert!(
        text.contains(
            "\n/p0  1 of 3 acted on; never: button \"Buy\" in main, button \"Buy\" in navigation\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("\n… 5 more pages in ui-coverage.json\n"),
        "{text}"
    );
    assert!(
        text.contains("pages no test visited are not known"),
        "{text}"
    );
}
