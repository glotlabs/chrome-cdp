//! From page to source (plan 15) against a real Chrome (`CHROME_PATH`;
//! skipped without it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the
//! fixture apps' dev builds (`testdata/site/apps/dev`, on the localhost
//! origin).
//!
//! Milestone 15.1: node creation stacks map to the app's source files,
//! a node made before recording has none, and places print as paths
//! relative to the project root.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use browser::Host;
use engine::{
    Action, ActionRequest, Button, ConsoleRequest, CoverageStartRequest, CoverageStopRequest,
    Engine, NavigateRequest, Navigation, NodeStacks, OpenRequest, Since, WaitFor, WaitRequest,
    WaitUntil,
};
use fixtures::Fixtures;
use protocol::Output;
use protocol::schema::{ConsoleData, Event, Level};

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
    let url = Fixtures::shared().url_b(&format!("/apps/dev/{name}/index.html"));
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
    for _ in 0..200 {
        if engine.node_stack("testid=add-3", None, 200).await.is_ok() {
            return;
        }
    }
    panic!("{name} did not render its items");
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
