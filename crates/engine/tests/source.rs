//! From page to source (plan 15) against a real Chrome (`CHROME_PATH`;
//! skipped without it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the
//! fixture apps' dev builds (`testdata/site/apps/dev`, on the localhost
//! origin).
//!
//! Milestone 15.1: node creation stacks map to the app's source files,
//! a node made before recording has none, and places print as paths
//! relative to the project root.
//!
//! Milestone 15.2: `source` names the file, line and component chain of
//! ten elements in each app, says why when it cannot, and `snapshot
//! --source` names the component of each ref.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use browser::Host;
use engine::{
    Action, ActionRequest, Button, ConsoleRequest, CoverageStartRequest, CoverageStopRequest,
    Engine, NavigateRequest, Navigation, NodeStacks, OpenRequest, Since, SnapshotRequest,
    SourceRequest, WaitFor, WaitRequest, WaitUntil,
};
use fixtures::Fixtures;
use protocol::schema::{ConsoleData, Event, Level, SnapshotData, SnapshotNode, SourceData};
use protocol::{ErrorInfo, ErrorKind, Output};

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

async fn start(path: &Path, name: &str, node_stacks: Option<NodeStacks>) -> Arc<Engine> {
    let engine = Engine::new(name, None, None);
    let chrome = browser::discover(Some(path), &Host::current()).unwrap();
    engine
        .open(OpenRequest {
            chrome: Some(chrome),
            node_stacks,
            timeout_ms: TIMEOUT_MS,
            ..OpenRequest::default()
        })
        .await
        .unwrap();
    engine
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

async fn click(engine: &Engine, locator: &str) -> Output {
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
        .unwrap_or_else(|e| panic!("click {locator}: {}", e.to_text()))
}

/// The plain app makes its total line in `renderTotal`, in src/cart.ts:
/// the node's creation stack says so, at that line, and goes on through
/// the app's own callers.
#[tokio::test]
async fn a_node_s_creation_stack_maps_to_its_line() {
    let Some(path) = chrome_path() else { return };
    // Node stacks are on by default for localhost pages.
    let engine = start(&path, "engine-source-node-stack", None).await;
    open_app(&engine, "plain").await;
    let stack = in_app("plain", engine.node_stack("testid=total", None, TIMEOUT_MS))
        .await
        .unwrap()
        .expect("a creation stack");
    let top = &stack[0];
    let original = top
        .original
        .as_ref()
        .unwrap_or_else(|| panic!("{stack:#?}"));
    assert_eq!(top.function, "renderTotal");
    let file = original
        .file
        .as_deref()
        .unwrap_or_else(|| panic!("{stack:#?}"));
    assert_eq!(Path::new(file), app_root("plain").join("src/cart.ts"));
    assert_eq!(
        original.line,
        line_of("plain", "src/cart.ts", "document.createElement(\"p\")")
    );
    // The callers: App, then main's render.
    let files: Vec<&str> = stack
        .iter()
        .filter_map(|f| f.original.as_ref()?.file.as_deref())
        .collect();
    assert!(files.iter().any(|f| f.ends_with("src/App.ts")), "{files:?}");
    assert!(
        files.iter().any(|f| f.ends_with("src/main.ts")),
        "{files:?}"
    );

    // A button made in CartItem.ts.
    let stack = in_app("plain", engine.node_stack("testid=add-2", None, TIMEOUT_MS))
        .await
        .unwrap()
        .unwrap();
    let original = stack[0].original.as_ref().unwrap();
    assert!(original.source.ends_with("/src/components/CartItem.ts"));
    assert_eq!(
        original.line,
        line_of(
            "plain",
            "src/components/CartItem.ts",
            "const add = document.createElement"
        )
    );

    // An element parsed from the HTML has no stack.
    let parsed = engine
        .node_stack("css=#app", None, TIMEOUT_MS)
        .await
        .unwrap();
    assert_eq!(parsed, None);
}

/// A node made before node stacks were on has none; one made after
/// turning them on (on the running browser) has one.
#[tokio::test]
async fn a_node_made_before_stacks_were_on_has_none() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "engine-source-before-stacks", Some(NodeStacks::Off)).await;
    open_app(&engine, "plain").await;
    let stack = engine
        .node_stack("testid=total", None, TIMEOUT_MS)
        .await
        .unwrap();
    assert_eq!(stack, None);

    engine
        .open(OpenRequest {
            node_stacks: Some(NodeStacks::On),
            timeout_ms: TIMEOUT_MS,
            ..OpenRequest::default()
        })
        .await
        .unwrap();
    // Still none: it was made before.
    let stack = engine
        .node_stack("testid=total", None, TIMEOUT_MS)
        .await
        .unwrap();
    assert_eq!(stack, None);
    // "Remove" draws the app again: the new total line has one.
    click(&engine, "testid=remove-3").await;
    let stack = engine
        .node_stack("testid=total", None, TIMEOUT_MS)
        .await
        .unwrap()
        .expect("a stack for the node made after");
    assert_eq!(stack[0].function, "renderTotal");
}

/// `coverage` turns `DOM` on and off in the sessions it measures; where
/// node stacks record, stopping it leaves `DOM` on, so they go on
/// recording.
#[tokio::test]
async fn coverage_leaves_node_stacks_recording() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "engine-source-coverage", None).await;
    open_app(&engine, "plain").await;
    engine
        .coverage_start(CoverageStartRequest {
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
    engine
        .coverage_stop(CoverageStopRequest {
            limit: 0,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap();
    click(&engine, "testid=remove-1").await;
    let stack = engine
        .node_stack("testid=total", None, TIMEOUT_MS)
        .await
        .unwrap()
        .expect("a stack for the node made after coverage stopped");
    assert_eq!(stack[0].function, "renderTotal");
}

/// "Checkout" throws in `lineTotal`: the action's report names the
/// error's place in src/cart.ts, relative to the app, and `errors` shows
/// the stack with paths on disk.
#[tokio::test]
async fn errors_print_places_on_disk() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "engine-source-errors", None).await;
    for app in ["plain", "react19", "vue", "svelte"] {
        open_app(&engine, app).await;
        let out = in_app(app, click(&engine, "testid=checkout")).await;
        let line = line_of(app, "src/cart.ts", "return item.price * qty;");
        let report: Vec<&str> = out.text.lines().filter(|l| l.starts_with("! ")).collect();
        assert!(
            report.iter().any(|l| l.starts_with("! console error: ")
                && l.contains("TypeError")
                && l.ends_with(&format!(" (src/cart.ts:{line})"))),
            "{app}: {}",
            out.text
        );
        let error = out.events.iter().find_map(|e| match e {
            Event::ConsoleError {
                original: Some(original),
                ..
            } => Some(original),
            _ => None,
        });
        let error = error.unwrap_or_else(|| panic!("{app}: {:?}", out.events));
        assert_eq!(
            error.file.as_deref().map(Path::new),
            Some(app_root(app).join("src/cart.ts").as_path())
        );

        // `wait --console` shows the entry as `console` does.
        let waited = in_app(
            app,
            engine.wait(WaitRequest {
                condition: WaitFor::Console {
                    pattern: "TypeError".into(),
                },
                tab: None,
                strict_refs: false,
                testid_attribute: None,
                timeout_ms: TIMEOUT_MS,
                max_chars: 0,
            }),
        )
        .await
        .unwrap();
        assert!(
            waited
                .text
                .contains(&format!("    at lineTotal (src/cart.ts:{line}:")),
            "{app}: {}",
            waited.text
        );

        let errors = in_app(
            app,
            engine.console(ConsoleRequest {
                errors: true,
                tab: None,
                level: Level::Info,
                since: Since::Last,
                grep: Some("TypeError".into()),
                limit: 1,
                wait_ms: 0,
                sourcemaps: false,
                timeout_ms: TIMEOUT_MS,
                max_chars: 0,
            }),
        )
        .await
        .unwrap();
        assert!(
            errors
                .text
                .contains(&format!("    at lineTotal (src/cart.ts:{line}:")),
            "{app}: {}",
            errors.text
        );
        assert!(
            errors.text.contains("    at total (src/cart.ts:"),
            "{app}: {}",
            errors.text
        );
        let data: ConsoleData = serde_json::from_value(errors.data).unwrap();
        let frame = &data.entries[0].stack[0];
        assert!(frame.original.as_ref().unwrap().file.is_some(), "{frame:?}");
    }
}

/// `source` of `locator` in the active tab, run from app `app`'s
/// directory.
async fn source(engine: &Engine, app: &str, locator: &str) -> Result<Output, ErrorInfo> {
    in_app(
        app,
        engine.source(SourceRequest {
            tab: None,
            locator: locator.to_owned(),
            depth: 8,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: TIMEOUT_MS,
        }),
    )
    .await
}

/// A place as `(file relative to the app, line)`.
fn place(app: &str, place: Option<&protocol::schema::SourcePlace>) -> (String, Option<u32>) {
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

/// One element of the table: its testid, the file and line (the line
/// holding `needle`) of its place, and its components, innermost first,
/// each with the file and the line holding its needle.
struct Expected {
    testid: &'static str,
    file: String,
    needle: String,
    components: Vec<(&'static str, String, String)>,
}

/// What `source` should say for the ten elements of app `app`.
fn expected(app: &str) -> Vec<Expected> {
    // The file extension and how an attribute is bound, per framework.
    let (ext, bind, open) = match app {
        "react18" | "react19" => ("tsx", "", "{`"),
        "vue" => ("vue", ":", "\"`"),
        _ => ("svelte", "", "{`"),
    };
    let app_file = format!("src/App.{ext}");
    let list_file = format!("src/components/CartList.{ext}");
    let item_file = format!("src/components/CartItem.{ext}");
    let attr = |id: &str| format!("data-testid=\"{id}\"");
    let item_attr = |id: &str| format!("{bind}data-testid={open}{id}-");
    // Where the components render the next step: CartList its
    // `<CartItem`, App its `<CartList`.
    let in_list = || ("CartList", list_file.clone(), "<CartItem".to_owned());
    let in_app = || ("App", app_file.clone(), "<CartList".to_owned());
    let own =
        |name: &'static str, file: &str, needle: &str| (name, file.to_owned(), needle.to_owned());
    let app_level = |id: &'static str, tag: &str| {
        let needle = format!("<{tag} {}", attr(id));
        Expected {
            testid: id,
            file: app_file.clone(),
            needle: needle.clone(),
            components: vec![own("App", &app_file, &needle)],
        }
    };
    let item_level = |id: &'static str, tag: &str| {
        // React writes a button's attributes on their own lines.
        let needle = if ext == "tsx" && tag == "button" {
            format!("{}{id}-", "data-testid={`")
        } else {
            format!("<{tag} {}", item_attr(id))
        };
        Expected {
            testid: match id {
                "item" => "item-2",
                "name" => "name-2",
                "qty" => "qty-2",
                "add" => "add-2",
                _ => "remove-2",
            },
            file: item_file.clone(),
            needle: needle.clone(),
            components: vec![own("CartItem", &item_file, &needle), in_list(), in_app()],
        }
    };
    let list_needle = format!("<ul {}", attr("cart-list"));
    vec![
        app_level("title", "h1"),
        Expected {
            testid: "cart-list",
            file: list_file.clone(),
            needle: list_needle.clone(),
            components: vec![own("CartList", &list_file, &list_needle), in_app()],
        },
        item_level("item", "li"),
        item_level("name", "span"),
        item_level("qty", "span"),
        item_level("add", "button"),
        item_level("remove", "button"),
        app_level("total", "p"),
        app_level("checkout", "button"),
        app_level("status", "p"),
    ]
}

/// The line of `needle` in `file`, or of the `<button` above it when
/// `needle` is on a line of its own attributes (React's buttons).
fn place_line(app: &str, file: &str, needle: &str) -> u32 {
    let line = line_of(app, file, needle);
    if needle.starts_with('<') {
        return line;
    }
    let text = std::fs::read_to_string(app_root(app).join(file)).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let above = (0..line as usize)
        .rev()
        .find(|&i| lines[i].trim_start().starts_with('<'))
        .unwrap();
    u32::try_from(above).unwrap() + 1
}

/// Ten elements of each framework app: their file and line, and the
/// components around them with theirs.
#[tokio::test]
async fn source_names_file_line_and_components() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "engine-source-frameworks", None).await;
    for app in ["react18", "react19", "vue", "svelte"] {
        open_app(&engine, app).await;
        for want in expected(app) {
            let out = source(&engine, app, &format!("testid={}", want.testid))
                .await
                .unwrap_or_else(|e| panic!("{app} {}: {}", want.testid, e.to_text()));
            let data: SourceData = serde_json::from_value(out.data.clone()).unwrap();
            let what = format!("{app} {}:\n{}", want.testid, out.text);
            assert_eq!(
                data.via,
                if app.starts_with("react") {
                    "react"
                } else {
                    app
                },
                "{what}"
            );
            let line = place_line(app, &want.file, &want.needle);
            assert_eq!(
                place(app, data.at.as_ref()),
                (want.file.clone(), Some(line)),
                "{what}"
            );
            let names: Vec<&str> = data.components.iter().map(|c| c.name.as_str()).collect();
            let wanted: Vec<&str> = want.components.iter().map(|c| c.0).collect();
            assert_eq!(names, wanted, "{what}");
            for (component, (_, file, needle)) in data.components.iter().zip(&want.components) {
                let line = place_line(app, file, needle);
                assert_eq!(
                    place(app, component.place.as_ref()),
                    (file.clone(), Some(line)),
                    "{what}"
                );
            }
            assert!(out.text.contains("(the page's debug info"), "{what}");
            // -q: the element's place alone.
            assert!(
                out.quiet
                    .as_deref()
                    .is_some_and(|q| q.starts_with(&format!("{}:{line}", want.file))),
                "{what}: {:?}",
                out.quiet
            );
        }
    }
}

/// The plain app has no framework: the node creation stack answers,
/// with no components.
#[tokio::test]
async fn source_falls_back_to_node_stacks() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "engine-source-plain", None).await;
    open_app(&engine, "plain").await;
    let table = [
        (
            "title",
            "src/App.ts",
            "const title = document.createElement",
        ),
        (
            "cart-list",
            "src/components/CartList.ts",
            "const list = document.createElement",
        ),
        (
            "item-2",
            "src/components/CartItem.ts",
            "const row = document.createElement",
        ),
        (
            "name-2",
            "src/components/CartItem.ts",
            "const name = document.createElement",
        ),
        (
            "qty-2",
            "src/components/CartItem.ts",
            "const qty = document.createElement",
        ),
        (
            "add-2",
            "src/components/CartItem.ts",
            "const add = document.createElement",
        ),
        (
            "remove-2",
            "src/components/CartItem.ts",
            "const remove = document.createElement",
        ),
        ("total", "src/cart.ts", "document.createElement(\"p\")"),
        (
            "checkout",
            "src/App.ts",
            "const checkout = document.createElement",
        ),
        (
            "status",
            "src/App.ts",
            "const status = document.createElement",
        ),
    ];
    for (testid, file, needle) in table {
        let out = source(&engine, "plain", &format!("testid={testid}"))
            .await
            .unwrap_or_else(|e| panic!("{testid}: {}", e.to_text()));
        let data: SourceData = serde_json::from_value(out.data.clone()).unwrap();
        assert_eq!(data.via, "node_stack", "{}", out.text);
        assert!(data.components.is_empty(), "{}", out.text);
        assert_eq!(
            place("plain", data.at.as_ref()),
            (file.to_owned(), Some(line_of("plain", file, needle))),
            "{testid}: {}",
            out.text
        );
        assert!(
            out.text.ends_with("\nvia node creation stack"),
            "{}",
            out.text
        );
    }

    // An element parsed from the HTML.
    let out = source(&engine, "plain", "css=#app").await.unwrap();
    let data: SourceData = serde_json::from_value(out.data.clone()).unwrap();
    assert_eq!(data.via, "html");
    assert!(
        out.text
            .ends_with("\n  in the HTML of /apps/dev/plain/index.html"),
        "{}",
        out.text
    );
}

/// Why `source` cannot answer: a production build, a node made before
/// stacks were on, stacks off.
#[tokio::test]
async fn source_says_why_it_cannot_answer() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "engine-source-no-source", None).await;
    // React's production build keeps no debug info.
    goto(&engine, "/apps/react.html").await;
    let err = source(&engine, "react19", "role=button >> nth=0")
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NoSource, "{}", err.to_text());
    assert!(
        err.message.contains("production build of react"),
        "{}",
        err.to_text()
    );
    assert!(
        err.hint.as_deref().unwrap().contains("dev server"),
        "{}",
        err.to_text()
    );
    assert_eq!(err.details.as_ref().unwrap()["reason"], "not_dev_build");

    // Stacks turned on after the page was made: its nodes have none.
    let engine = start(&path, "engine-source-late-stacks", Some(NodeStacks::Off)).await;
    open_app(&engine, "plain").await;
    let err = source(&engine, "plain", "testid=title").await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::NoSource, "{}", err.to_text());
    assert_eq!(err.details.as_ref().unwrap()["reason"], "stacks_off");
    assert!(
        err.hint.as_deref().unwrap().contains("--node-stacks on"),
        "{}",
        err.to_text()
    );
    engine
        .open(OpenRequest {
            node_stacks: Some(NodeStacks::On),
            timeout_ms: TIMEOUT_MS,
            ..OpenRequest::default()
        })
        .await
        .unwrap();
    let err = source(&engine, "plain", "testid=title").await.unwrap_err();
    assert_eq!(err.details.as_ref().unwrap()["reason"], "before_stacks");
    assert!(
        err.message
            .contains("made before chrome-cdp recorded node stacks"),
        "{}",
        err.to_text()
    );
    assert!(
        err.hint
            .as_deref()
            .unwrap()
            .starts_with("chrome-cdp reload"),
        "{}",
        err.to_text()
    );
    // After a reload it answers.
    open_app(&engine, "plain").await;
    let out = source(&engine, "plain", "testid=title").await.unwrap();
    assert!(out.text.contains("at src/App.ts:"), "{}", out.text);
}

/// A page can claim a framework made an element: `source` shows the
/// claim as the page's debug info, and nothing more.
#[tokio::test]
async fn a_page_s_fake_debug_info_is_shown_as_the_page_s() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "engine-source-fake", None).await;
    goto(&engine, "/source/fake-svelte.html").await;
    let out = source(&engine, "plain", "css=#pay").await.unwrap();
    let data: SourceData = serde_json::from_value(out.data.clone()).unwrap();
    assert_eq!(data.via, "svelte");
    let at = data.at.unwrap();
    assert_eq!(at.source, "src/Nowhere.svelte");
    assert_eq!((at.line, at.column, at.file), (Some(7), Some(3), None));
    assert!(
        out.text.ends_with("\nvia svelte (the page's debug info)"),
        "{}",
        out.text
    );
    assert!(
        out.text.contains("  at src/Nowhere.svelte:7:3\n"),
        "{}",
        out.text
    );
}

/// The buttons of `nodes` and their descendants.
fn buttons(nodes: &[SnapshotNode], out: &mut Vec<SnapshotNode>) {
    for node in nodes {
        if node.role == "button" {
            out.push(node.clone());
        }
        buttons(&node.children, out);
    }
}

/// `snapshot --source` on the React 19 app names `CartItem` and its line
/// on each item's buttons.
#[tokio::test]
async fn snapshot_source_names_the_component_of_each_ref() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "engine-source-snapshot", None).await;
    for app in ["react19", "plain"] {
        open_app(&engine, app).await;
        let out = in_app(
            app,
            engine.snapshot(SnapshotRequest {
                tab: None,
                interactive: true,
                scope: None,
                depth: None,
                diff: false,
                all_refs: false,
                all: false,
                text_limit: 200,
                no_text: false,
                engine: None,
                strict_refs: false,
                testid_attribute: None,
                source: true,
                timeout_ms: TIMEOUT_MS,
                max_chars: 0,
            }),
        )
        .await
        .unwrap();
        let (component, file, add) = if app == "react19" {
            let file = "src/components/CartItem.tsx";
            (
                "CartItem ",
                file,
                place_line(app, file, "data-testid={`add-"),
            )
        } else {
            let file = "src/components/CartItem.ts";
            (
                "",
                file,
                line_of(app, file, "const add = document.createElement"),
            )
        };
        let adds: Vec<&str> = out
            .text
            .lines()
            .filter(|l| l.contains("button \"Add\""))
            .collect();
        assert_eq!(adds.len(), 3, "{}", out.text);
        for line in adds {
            assert!(
                line.ends_with(&format!("] <{component}{file}:{add}>")),
                "{app}: {line}"
            );
        }
        let data: SnapshotData = serde_json::from_value(out.data).unwrap();
        let mut found = Vec::new();
        buttons(&data.nodes, &mut found);
        let checkout = found.iter().find(|n| n.name == "Checkout").unwrap();
        let source = checkout.source.as_ref().unwrap();
        let want = if app == "react19" { Some("App") } else { None };
        assert_eq!(source.component.as_deref(), want);
        assert!(source.place.as_ref().unwrap().file.is_some());
    }
}
