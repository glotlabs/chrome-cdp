//! `components [loc]` (plan 15, milestone 15.3) against a real Chrome
//! (`CHROME_PATH`; skipped without it unless `CHROME_CDP_REQUIRE_CHROME`
//! is set) and the fixture apps' dev builds (`testdata/site/apps/dev`, on
//! the localhost origin): each app's tree, an item's props and the app's
//! state before and after a click, a long list folded, a deep tree
//! bounded, Svelte's missing state, and the errors.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use browser::Host;
use engine::{
    Action, ActionRequest, Button, ComponentsRequest, Engine, NavigateRequest, Navigation,
    OpenRequest, RouteAction, RouteBody, RouteRequest, RouteSpec, WaitUntil,
};
use fixtures::Fixtures;
use protocol::schema::{ComponentDetail, ComponentNode, ComponentsData, SourcePlace};
use protocol::{ErrorInfo, ErrorKind, Mode, Output, Style, render};
use serde_json::json;

const TIMEOUT_MS: u64 = 30_000;

fn chrome_path() -> Option<PathBuf> {
    match std::env::var_os("CHROME_PATH") {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => {
            assert!(
                std::env::var_os("CHROME_CDP_REQUIRE_CHROME").is_none(),
                "CHROME_CDP_REQUIRE_CHROME is set but CHROME_PATH is not"
            );
            eprintln!("skipping: set CHROME_PATH to run tests against a real Chrome");
            None
        }
    }
}

/// The sources of fixture app `name`: the project root its places print
/// relative to.
fn app_root(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/apps-src")
        .join(name)
        .canonicalize()
        .unwrap()
}

/// The 1-based line of the app's `path` that contains `needle`.
fn line_of(app: &str, path: &str, needle: &str) -> u32 {
    let text = std::fs::read_to_string(app_root(app).join(path)).unwrap();
    let index = text
        .lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} is not in {path}"));
    u32::try_from(index).unwrap() + 1
}

/// Runs `fut` as a call of a CLI in app `name`'s directory.
async fn in_app<F: Future>(name: &str, fut: F) -> F::Output {
    engine::in_root(Some(app_root(name)), Box::pin(fut)).await
}

async fn start(path: &Path, name: &str) -> Arc<Engine> {
    let engine = Engine::new(name, None, None);
    let chrome = browser::discover(Some(path), &Host::current()).unwrap();
    engine
        .open(OpenRequest {
            chrome: Some(chrome),
            timeout_ms: TIMEOUT_MS,
            ..OpenRequest::default()
        })
        .await
        .unwrap();
    engine
}

/// Opens `path` of the fixture site on the localhost origin.
async fn goto(engine: &Engine, path: &str) {
    let url = Fixtures::shared().url_b(path);
    engine
        .navigate(NavigateRequest {
            navigation: Navigation::Goto {
                url: url.clone(),
                base_url: None,
            },
            wait: WaitUntil::Settle,
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("goto {url}: {}", e.to_text()));
}

/// Opens app `name` on the localhost origin and waits for its items.
async fn open_app(engine: &Engine, name: &str) {
    goto(engine, &format!("/apps/dev/{name}/index.html")).await;
    for _ in 0..200 {
        if engine.node_stack("testid=add-3", None, 200).await.is_ok() {
            return;
        }
    }
    panic!("{name} did not render its items");
}

async fn components(
    engine: &Engine,
    app: &str,
    locator: Option<&str>,
    depth: Option<u32>,
    max_chars: usize,
) -> Result<Output, ErrorInfo> {
    in_app(
        app,
        engine.components(ComponentsRequest {
            tab: None,
            locator: locator.map(str::to_owned),
            depth,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: TIMEOUT_MS,
            max_chars,
        }),
    )
    .await
}

fn data(output: &Output) -> ComponentsData {
    serde_json::from_value(output.data.clone()).unwrap()
}

/// What `components` prints, as the CLI would.
fn text(output: &Output) -> String {
    render(Mode::Text, Ok(output.clone()), Style::PLAIN).stdout
}

/// A place as `(file relative to the app, line)`.
fn place(app: &str, place: Option<&SourcePlace>) -> (String, Option<u32>) {
    let place = place.unwrap_or_else(|| panic!("{app}: no place"));
    let file = place
        .file
        .as_deref()
        .unwrap_or_else(|| panic!("{app}: {place:?} is not on disk"));
    let relative = Path::new(file)
        .strip_prefix(app_root(app))
        .unwrap_or_else(|_| panic!("{app}: {file} is not in the app"));
    (relative.to_string_lossy().into_owned(), place.line)
}

/// Each app's component files and the element each renders first.
fn files(app: &str) -> [(&'static str, String, &'static str); 3] {
    let (ext, dir) = match app {
        "vue" => ("vue", "src/components"),
        "svelte" => ("svelte", "src/components"),
        _ => ("tsx", "src/components"),
    };
    [
        ("App", format!("src/App.{ext}"), "<main"),
        ("CartList", format!("{dir}/CartList.{ext}"), "<ul"),
        ("CartItem", format!("{dir}/CartItem.{ext}"), "<li"),
    ]
}

fn check_node(app: &str, node: &ComponentNode, expected: &(&str, String, &str)) {
    let (name, file, needle) = expected;
    assert_eq!(node.name, *name, "{app}: {node:?}");
    assert_eq!(
        place(app, node.place.as_ref()),
        (file.clone(), Some(line_of(app, file, needle))),
        "{app}: {name}'s place"
    );
}

/// Each app's tree at `--depth 3`: App > CartList > CartItem ×3, each at
/// its own file and the line of the first element it renders; `--depth 2`
/// counts the items instead.
#[tokio::test]
async fn each_app_s_tree() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "components-tree").await;
    for app in ["react18", "react19", "vue", "svelte"] {
        open_app(&engine, app).await;
        let output = components(&engine, app, None, Some(3), 20_000)
            .await
            .unwrap_or_else(|e| panic!("{app}: {}", e.to_text()));
        let tree = data(&output);
        let [app_file, list_file, item_file] = files(app);
        assert_eq!(tree.roots.len(), 1, "{app}: {:?}", tree.roots);
        let root = &tree.roots[0];
        check_node(app, root, &app_file);
        assert_eq!(
            root.framework.as_deref(),
            Some(tree.frameworks[0].name.as_str())
        );
        let list = &root.children[0];
        check_node(app, list, &list_file);
        assert_eq!(list.children.len(), 1, "{app}: the items fold: {list:?}");
        let items = &list.children[0];
        check_node(app, items, &item_file);
        assert_eq!(items.count, 3, "{app}");
        let shown = text(&output);
        let item_line = line_of(app, &item_file.1, item_file.2);
        assert!(
            shown.contains(&format!("    <CartItem> ×3 {}:{item_line}", item_file.1)),
            "{app}:\n{shown}"
        );
        if app != "svelte" {
            assert!(
                shown.contains("  items=[3 items]"),
                "{app}: CartList's props:\n{shown}"
            );
            assert!(shown.contains("item={…4 keys}"), "{app}:\n{shown}");
        }
        assert!(
            shown.trim_end().ends_with("(the page's debug info)"),
            "{app}:\n{shown}"
        );

        let output = components(&engine, app, None, Some(2), 20_000)
            .await
            .unwrap();
        let tree = data(&output);
        let list = &tree.roots[0].children[0];
        assert!(list.children.is_empty(), "{app}: {list:?}");
        assert_eq!(list.more, 3, "{app}");
        assert!(
            text(&output).contains("    … 3 components below (a larger --depth shows them)"),
            "{app}:\n{}",
            text(&output)
        );
    }
}

fn detail(output: &Output) -> ComponentDetail {
    data(output).component.unwrap()
}

fn value<'a>(values: Option<&'a Vec<protocol::schema::ComponentValue>>, name: &str) -> &'a str {
    values
        .and_then(|v| v.iter().find(|v| v.name == name))
        .map_or_else(|| panic!("no {name} in {values:?}"), |v| v.value.as_str())
}

async fn click(engine: &Engine, locator: &str) {
    engine
        .act(ActionRequest {
            action: Action::Click {
                locator: locator.to_owned(),
                button: Button::Left,
                count: 1,
                modifiers: Vec::new(),
                position: None,
                force: false,
                dblclick: false,
            },
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: TIMEOUT_MS,
            no_wait_after: false,
            snap: false,
            max_chars: 20_000,
        })
        .await
        .unwrap_or_else(|e| panic!("click {locator}: {}", e.to_text()));
}

/// An item's button: its component is CartItem, with its `item` prop and
/// owners; the app's state holds the items. After "Add", both show the
/// new quantity.
#[tokio::test]
async fn an_item_s_props_and_the_app_s_state() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "components-item").await;
    for app in ["react18", "react19", "vue"] {
        open_app(&engine, app).await;
        let output = components(&engine, app, Some("testid=add-1"), None, 20_000)
            .await
            .unwrap_or_else(|e| panic!("{app}: {}", e.to_text()));
        let item = detail(&output);
        let [_, _, item_file] = files(app);
        assert_eq!(item.name, "CartItem", "{app}");
        assert_eq!(
            place(app, item.place.as_ref()),
            (item_file.1.clone(), Some(line_of(app, &item_file.1, "<li")))
        );
        assert_eq!(
            value(item.props.as_ref(), "item"),
            r#"{id: 1, name: "Mug", price: 12, qty: 2}"#,
            "{app}"
        );
        let owners: Vec<&str> = item.owners.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(owners, ["CartList", "App"], "{app}");
        let shown = text(&output);
        let element = data(&output).element.unwrap();
        assert!(
            shown.starts_with(&format!(
                "<CartItem> {}:{}  (holds {})\n",
                item_file.1,
                line_of(app, &item_file.1, "<li"),
                element.reference.unwrap()
            )),
            "{app}:\n{shown}"
        );
        assert!(
            shown.contains("  owners: <CartList> ← <App>\n"),
            "{app}:\n{shown}"
        );

        // The app's state: React's first useState hook, Vue's `items`.
        let state_name = if app == "vue" { "items" } else { "#0" };
        let the_app = detail(
            &components(&engine, app, Some("testid=title"), None, 20_000)
                .await
                .unwrap(),
        );
        assert_eq!(the_app.name, "App");
        assert!(
            value(the_app.state.as_ref(), state_name)
                .starts_with(r#"[{id: 1, name: "Mug", price: 12, qty: 2}"#),
            "{app}: {:?}",
            the_app.state
        );
        if app != "vue" {
            let hook = the_app.state.as_ref().unwrap()[0].hook.as_deref();
            assert_eq!(hook, Some("useState"), "{app}");
        }

        click(&engine, "testid=add-1").await;
        let mut after = String::new();
        for _ in 0..50 {
            let item = detail(
                &components(&engine, app, Some("testid=add-1"), None, 20_000)
                    .await
                    .unwrap(),
            );
            after = value(item.props.as_ref(), "item").to_owned();
            if after.contains("qty: 3") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(after, r#"{id: 1, name: "Mug", price: 12, qty: 3}"#, "{app}");
        let the_app = detail(
            &components(&engine, app, Some("testid=title"), None, 20_000)
                .await
                .unwrap(),
        );
        assert!(
            value(the_app.state.as_ref(), state_name)
                .starts_with(r#"[{id: 1, name: "Mug", price: 12, qty: 3}"#),
            "{app}: {:?}",
            the_app.state
        );
    }
}

/// Svelte 5 keeps no props or state that can be read: the tree and the
/// owners are there, and the text says state is not available.
#[tokio::test]
async fn svelte_says_state_is_not_available() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "components-svelte").await;
    open_app(&engine, "svelte").await;
    let output = components(&engine, "svelte", Some("testid=add-1"), None, 20_000)
        .await
        .unwrap();
    let item = detail(&output);
    assert_eq!(item.name, "CartItem");
    assert!(item.props.is_none() && item.state.is_none(), "{item:?}");
    assert!(item.unavailable.is_some());
    let owners: Vec<&str> = item.owners.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(owners, ["CartList", "App"]);
    let shown = text(&output);
    assert!(
        shown.contains("\n  state: not available (Svelte 5 has no public API"),
        "{shown}"
    );
}

/// A 200-item cart folds into one line of 200 alike CartItems.
#[tokio::test]
async fn a_long_list_folds() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "components-long").await;
    let items: Vec<_> = (1..=200)
        .map(|id| json!({ "id": id, "name": format!("Item {id}"), "price": id, "qty": 1 }))
        .collect();
    engine
        .route(RouteRequest {
            rules: vec![RouteSpec {
                pattern: "**/react19/cart.json".to_owned(),
                label: None,
                methods: Vec::new(),
                resource_types: Vec::new(),
                times: None,
                delay_ms: 0,
                context: None,
                action: RouteAction::Fulfill {
                    status: 200,
                    headers: Vec::new(),
                    body: RouteBody::Json {
                        value: json!({ "items": items }),
                    },
                },
            }],
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
    open_app(&engine, "react19").await;
    let output = components(&engine, "react19", None, None, 20_000)
        .await
        .unwrap();
    let tree = data(&output);
    let list = &tree.roots[0].children[0];
    assert_eq!(list.children.len(), 1, "{list:?}");
    assert_eq!(list.children[0].count, 200);
    let shown = text(&output);
    assert!(shown.contains("  items=[200 items]"), "{shown}");
    assert!(shown.contains("<CartItem> ×200 "), "{shown}");
    assert!(shown.lines().count() < 6, "{shown}");
}

/// A 41-level tree: `--depth` cuts it (and counts the rest), and
/// `--max-chars` bounds the whole of it.
#[tokio::test]
async fn depth_and_max_chars_bound_a_deep_tree() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "components-deep").await;
    goto(&engine, "/source/deep-svelte.html").await;
    let output = components(&engine, "svelte", None, None, 20_000)
        .await
        .unwrap();
    let shown = text(&output);
    assert_eq!(
        shown,
        "<App> src/App.svelte:3\n  <Level1> src/Level1.svelte:3\n    <Level2> \
         src/Level2.svelte:3\n      <Level3> src/Level3.svelte:3\n        … 37 components \
         below (a larger --depth shows them)\nvia svelte 5 (the page's debug info)\n"
    );
    let output = components(&engine, "svelte", None, Some(50), 0)
        .await
        .unwrap();
    assert!(text(&output).contains("<Level40>"));
    let output = components(&engine, "svelte", None, Some(50), 500)
        .await
        .unwrap();
    let shown = text(&output);
    assert!(shown.chars().count() < 700, "{shown}");
    assert!(!shown.contains("<Level40>"));
    assert!(shown.contains("truncated"), "{shown}");
}

/// A production build, a page with no framework, and an element no
/// component rendered each fail with `NoSource` and say why.
#[tokio::test]
async fn it_says_why_it_cannot_answer() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "components-errors").await;
    let reason = |e: &ErrorInfo| {
        e.details
            .as_ref()
            .and_then(|d| d["reason"].as_str())
            .unwrap_or_default()
            .to_owned()
    };

    goto(&engine, "/apps/react.html").await;
    engine
        .node_stack("role=button >> nth=0", None, TIMEOUT_MS)
        .await
        .ok();
    let err = components(&engine, "react19", None, None, 20_000)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NoSource);
    assert_eq!(reason(&err), "not_dev_build", "{}", err.to_text());
    let err = components(
        &engine,
        "react19",
        Some("role=button >> nth=0"),
        None,
        20_000,
    )
    .await
    .unwrap_err();
    assert_eq!(reason(&err), "not_dev_build", "{}", err.to_text());

    goto(&engine, "/apps/dev/plain/index.html").await;
    let err = components(&engine, "plain", None, None, 20_000)
        .await
        .unwrap_err();
    assert_eq!(reason(&err), "no_framework", "{}", err.to_text());
    let err = components(&engine, "plain", Some("testid=title"), None, 20_000)
        .await
        .unwrap_err();
    assert_eq!(reason(&err), "no_component", "{}", err.to_text());
    assert!(
        err.to_text().contains("chrome-cdp source e"),
        "{}",
        err.to_text()
    );
}
