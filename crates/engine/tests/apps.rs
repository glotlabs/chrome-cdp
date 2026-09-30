//! The fixture apps of `testdata/apps-src` (plan 15, milestone 15.1):
//! their committed dev builds are built from the current sources, and
//! each has the debug info only a dev build has, so a production build
//! fails here instead of making `source` look broken.
//!
//! The presence tests need a real Chrome (`CHROME_PATH`; skipped without
//! it unless `CHROME_CDP_REQUIRE_CHROME` is set).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use browser::Host;
use engine::{Engine, EvalRequest, EvalWorld, NavigateRequest, Navigation, OpenRequest, WaitUntil};
use fixtures::Fixtures;
use protocol::schema::EvalData;

const TIMEOUT_MS: u64 = 30_000;

const APPS: [&str; 5] = ["plain", "react18", "react19", "vue", "svelte"];

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

/// The files below `path`, less `node_modules`, relative to `root` with
/// `/` separators.
fn files(root: &Path, path: &Path) -> Vec<String> {
    if path.is_file() {
        let relative = path.strip_prefix(root).unwrap();
        let parts: Vec<_> = relative.iter().map(|p| p.to_string_lossy()).collect();
        return vec![parts.join("/")];
    }
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.file_name().is_none_or(|n| n != "node_modules"))
        .flat_map(|p| files(root, &p))
        .collect()
}

/// What `sourcesHash` in `testdata/apps-src/shared/build.mjs` computes:
/// FNV-1a over each file of the app and of `shared/` (its path relative
/// to `apps-src`, a NUL, its bytes and a NUL), in path order.
fn sources_hash(name: &str) -> String {
    let root = testdata().join("apps-src");
    let mut paths: Vec<String> = [name, "shared"]
        .iter()
        .flat_map(|dir| files(&root, &root.join(dir)))
        .collect();
    paths.sort();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for path in paths {
        let bytes = std::fs::read(root.join(&path)).unwrap();
        for byte in path.bytes().chain([0]).chain(bytes).chain([0]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

#[test]
fn the_committed_builds_are_built_from_the_current_sources() {
    let mut stale = Vec::new();
    for name in APPS {
        let built = testdata()
            .join("site/apps/dev")
            .join(name)
            .join("sources.txt");
        let recorded = std::fs::read_to_string(&built).unwrap_or_default();
        if recorded.trim() != sources_hash(name) {
            stale.push(name);
        }
    }
    assert!(
        stale.is_empty(),
        "the dev builds of {} are out of date: run `cargo xtask apps {}` (needs Node)",
        stale.join(", "),
        stale.join(" ")
    );
}

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

/// Opens app `name` (on the localhost origin) and waits for its items.
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
    let items = eval(
        engine,
        "(async () => {
            for (let i = 0; i < 200 && !document.querySelector('[data-testid=\"add-3\"]'); i++) {
                await new Promise((r) => setTimeout(r, 25));
            }
            return document.querySelectorAll('[data-testid^=\"item-\"]').length;
        })()",
    )
    .await;
    assert_eq!(items, "3", "{name} did not render its 3 items");
}

/// `script`'s value in the page's own world, as text.
async fn eval(engine: &Engine, script: &str) -> String {
    let output = engine
        .eval(EvalRequest {
            script: script.to_owned(),
            locator: None,
            frame: None,
            world: EvalWorld::Main,
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            sourcemaps: false,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap_or_else(|e| panic!("{script}: {}", e.to_text()));
    let _: EvalData = serde_json::from_value(output.data.clone()).unwrap();
    output.text
}

/// The 1-based line of `path` (in `apps-src`) that contains `needle`.
fn line_of(path: &str, needle: &str) -> usize {
    let text = std::fs::read_to_string(testdata().join("apps-src").join(path)).unwrap();
    1 + text
        .lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} is not in {path}"))
}

/// Each app's dev-only debug info, read off the "Add" button of item 1.
#[tokio::test]
async fn each_app_has_its_dev_build_debug_info() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "engine-apps-debug-info").await;
    let button = "const el = document.querySelector('[data-testid=\"add-1\"]');";
    let fiber = "const fiber = el[Object.keys(el).find((k) => k.startsWith('__reactFiber$'))];";

    open_app(&engine, "plain").await;

    open_app(&engine, "react18").await;
    let source = eval(
        &engine,
        &format!(
            "{{ {button} {fiber} const s = fiber._debugSource; `${{s.fileName}}:${{s.lineNumber}}` }}"
        ),
    )
    .await;
    // The JSX element's place: its `<button` line.
    let line = line_of("react18/src/components/CartItem.tsx", "<button");
    assert_eq!(
        source,
        format!("/build/apps-src/react18/src/components/CartItem.tsx:{line}"),
        "React 18's _debugSource"
    );

    open_app(&engine, "react19").await;
    let stack = eval(
        &engine,
        &format!(
            "{{ {button} {fiber} [fiber._debugStack instanceof Error, fiber._debugOwner?.type?.name, \
             'index.js' + fiber._debugStack.stack.split('index.js').length].join(' ') }}"
        ),
    )
    .await;
    assert!(
        stack.starts_with("true CartItem index.js"),
        "React 19's _debugStack: {stack}"
    );

    open_app(&engine, "vue").await;
    let component = eval(
        &engine,
        &format!(
            "{{ {button} const c = el.__vueParentComponent; `${{c.type.__name}} ${{c.type.__file}} ${{c.parent.type.__name}}` }}"
        ),
    )
    .await;
    assert_eq!(
        component, "CartItem /build/apps-src/vue/src/components/CartItem.vue CartList",
        "Vue's __vueParentComponent and __file"
    );

    open_app(&engine, "svelte").await;
    let meta = eval(
        &engine,
        &format!("{{ {button} const l = el.__svelte_meta.loc; `${{l.file}}:${{l.line}}` }}"),
    )
    .await;
    let line = line_of(
        "svelte/src/components/CartItem.svelte",
        "data-testid={`add-",
    );
    assert_eq!(
        meta,
        format!("src/components/CartItem.svelte:{line}"),
        "Svelte's __svelte_meta"
    );
}
