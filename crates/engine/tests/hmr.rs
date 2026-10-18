//! Dev-server awareness (plan 15, milestone 15.6) against a real Chrome
//! (`CHROME_PATH`; skipped without it unless `CHROME_CDP_REQUIRE_CHROME`
//! is set) and the fixture server's fake Vite and webpack-dev-server
//! (`testdata/site/hmr`, `/hmr/vite`, `/hmr/webpack`).
//!
//! For each fake server: an update is `applied` with its file; a compile
//! error fails `wait --hmr` with the message and the place on disk; a full
//! reload succeeds once the page loaded. A page with no client fails at
//! once; an update between two commands makes the second warn, once; one
//! during a click is in its report.
//!
//! `a_real_vite_dev_server` runs Vite's dev server on a copy of the
//! React 19 fixture app, and only with `CHROME_CDP_NODE_TESTS=1` (it
//! needs Node and the app's `node_modules`, from `cargo xtask apps`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use browser::Host;
use engine::{
    Action, ActionRequest, Button, Engine, EvalRequest, EvalWorld, NavigateRequest, Navigation,
    OpenRequest, WaitFor, WaitRequest, WaitUntil,
};
use fixtures::Fixtures;
use protocol::schema::{Event, HotOutcome, HotUpdate, WaitData};
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

/// `testdata/site/hmr`: the project root the fake servers' places print
/// relative to (`src/cart.js`).
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/site/hmr")
        .canonicalize()
        .unwrap()
}

/// Runs `fut` as a CLI call in [`root`] would: with the root, noting the
/// hot updates the daemon adds as warnings.
async fn call<F: Future>(fut: F) -> (F::Output, engine::Notices) {
    engine::in_root(Some(root()), engine::noting(Box::pin(fut))).await
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

async fn goto(engine: &Engine, url: &str) {
    engine
        .navigate(NavigateRequest {
            navigation: Navigation::Goto {
                url: url.to_owned(),
                base_url: None,
            },
            wait: WaitUntil::Load,
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("goto {url}: {}", e.to_text()));
}

/// Opens the fake `client` page (`vite` or `webpack`) on `channel` and
/// waits until its socket greeted it (so Chrome reported the socket).
async fn open_client(engine: &Engine, client: &str, channel: &str) {
    let f = Fixtures::shared();
    goto(
        engine,
        &f.url_b(&format!("/hmr/{client}.html?channel={channel}")),
    )
    .await;
    until(engine, "window.hmrConnected === true").await;
}

/// Sends `message` to the channel's sockets (one page).
fn send(channel: &str, message: &str) {
    assert_eq!(
        Fixtures::shared().hmr_send(channel, message),
        1,
        "{message}"
    );
}

fn vite_update(path: &str) -> String {
    format!(
        r#"{{"type":"update","updates":[{{"type":"js-update","timestamp":{},"path":"{path}","acceptedPath":"{path}"}}]}}"#,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    )
}

async fn wait_hmr(engine: &Engine, timeout_ms: u64) -> Result<Output, ErrorInfo> {
    call(engine.wait(WaitRequest {
        condition: WaitFor::Hmr,
        tab: None,
        strict_refs: false,
        testid_attribute: None,
        timeout_ms,
        max_chars: 0,
    }))
    .await
    .0
}

/// The hot update `wait --hmr` returned.
fn hot_update(output: &Output) -> HotUpdate {
    let data: WaitData = serde_json::from_value(output.data.clone()).unwrap();
    assert_eq!(data.condition, "hmr");
    data.hot_update.unwrap()
}

/// Runs `script` in the page, outside a CLI call (so it reports nothing).
async fn eval(engine: &Engine, script: &str) -> serde_json::Value {
    let output = engine
        .eval(EvalRequest {
            script: script.into(),
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
        .unwrap_or_else(|e| panic!("eval {script}: {}", e.to_text()));
    output.data["value"].clone()
}

/// Waits (outside a CLI call) until `script` is true in the page.
async fn until(engine: &Engine, script: &str) {
    for _ in 0..200 {
        if eval(engine, script).await == serde_json::Value::Bool(true) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{script} never became true");
}

/// An update is applied with its file on disk, whichever server sent it.
#[tokio::test(flavor = "multi_thread")]
async fn an_update_is_applied_with_its_file() {
    let Some(chrome) = chrome_path() else { return };
    let engine = start(&chrome, "hmr-applied").await;
    let cart = root().join("src/cart.js");

    open_client(&engine, "vite", "applied-vite").await;
    send("applied-vite", &vite_update("/hmr/src/cart.js"));
    let output = wait_hmr(&engine, TIMEOUT_MS).await.unwrap();
    let update = hot_update(&output);
    assert_eq!(update.client, "vite");
    assert_eq!(update.outcome, HotOutcome::Applied);
    assert_eq!(update.files.len(), 1, "{update:?}");
    assert_eq!(update.files[0].source, "/hmr/src/cart.js");
    assert_eq!(
        update.files[0].file.as_deref(),
        Some(cart.to_str().unwrap())
    );
    assert!(
        output
            .text
            .starts_with("hot update applied (src/cart.js) ("),
        "{}",
        output.text
    );
    assert_eq!(output.quiet.as_deref(), Some("applied"));
    // It ran: the module counted a second run.
    assert_eq!(eval(&engine, "window.cartRuns").await, 2);

    open_client(&engine, "webpack", "applied-webpack").await;
    for message in [
        r#"{"type":"invalid"}"#,
        r#"{"type":"fake-modules","data":["./src/cart.js"]}"#,
        r#"{"type":"hash","data":"1111"}"#,
        r#"{"type":"ok"}"#,
    ] {
        send("applied-webpack", message);
    }
    let output = wait_hmr(&engine, TIMEOUT_MS).await.unwrap();
    let update = hot_update(&output);
    assert_eq!(update.client, "webpack");
    assert_eq!(update.outcome, HotOutcome::Applied);
    assert_eq!(update.files[0].source, "./src/cart.js");
    assert_eq!(
        update.files[0].file.as_deref(),
        Some(cart.to_str().unwrap())
    );
    assert!(
        output.text.starts_with("hot update applied (src/cart.js)"),
        "{}",
        output.text
    );
}

/// A compile error fails `wait --hmr` with its message and its place on
/// disk; the next good update is applied.
#[tokio::test(flavor = "multi_thread")]
async fn a_compile_error_fails_the_wait() {
    let Some(chrome) = chrome_path() else { return };
    let engine = start(&chrome, "hmr-failed").await;

    open_client(&engine, "vite", "failed-vite").await;
    // Vite sends the error of the edited module (and its client's import
    // of it fails).
    send(
        "failed-vite",
        r#"{"type":"error","err":{"message":"Transform failed with 1 error:\n/hmr/src/cart.js:3:9: ERROR: Unexpected \"}\"","stack":"","id":"/hmr/src/cart.js","plugin":"vite:esbuild","loc":{"file":"/hmr/src/cart.js","line":3,"column":9}}}"#,
    );
    let err = wait_hmr(&engine, TIMEOUT_MS).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::HotUpdateFailed, "{}", err.to_text());
    assert_eq!(
        err.message,
        r#"hot update failed: /hmr/src/cart.js:3:9: ERROR: Unexpected "}" (src/cart.js:3)"#
    );
    let update: HotUpdate =
        serde_json::from_value(err.details.as_ref().unwrap()["hot_update"].clone()).unwrap();
    let place = update.place.unwrap();
    assert_eq!((place.line, place.column), (3, 10));
    assert_eq!(
        place.file.as_deref(),
        Some(root().join("src/cart.js").to_str().unwrap())
    );
    assert!(
        err.to_text().contains("chrome-cdp wait --hmr"),
        "{}",
        err.to_text()
    );
    // The overlay shows it; the page still runs the old code.
    assert_eq!(eval(&engine, "window.cartRuns").await, 1);

    // The fix: the next wait is for it, not the failure again.
    send("failed-vite", &vite_update("/hmr/src/cart.js"));
    let output = wait_hmr(&engine, TIMEOUT_MS).await.unwrap();
    assert_eq!(hot_update(&output).outcome, HotOutcome::Applied);
    assert_eq!(eval(&engine, "window.cartRuns").await, 2);

    open_client(&engine, "webpack", "failed-webpack").await;
    send("failed-webpack", r#"{"type":"invalid"}"#);
    send(
        "failed-webpack",
        r#"{"type":"errors","data":[{"message":"Module build failed (from ./node_modules/babel-loader/lib/index.js):\nSyntaxError: src/cart.js: Unexpected token (2:12)","moduleName":"./src/cart.js","loc":"2:12"}]}"#,
    );
    let err = wait_hmr(&engine, TIMEOUT_MS).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::HotUpdateFailed, "{}", err.to_text());
    assert_eq!(
        err.message,
        "hot update failed: SyntaxError: src/cart.js: Unexpected token (2:12) (src/cart.js:2)"
    );
}

/// A full reload succeeds once the page loaded again.
#[tokio::test(flavor = "multi_thread")]
async fn a_full_reload_succeeds_after_the_load() {
    let Some(chrome) = chrome_path() else { return };
    let engine = start(&chrome, "hmr-reload").await;

    for (client, message) in [
        (
            "vite",
            r#"{"type":"full-reload","path":"/hmr/src/cart.js"}"#,
        ),
        (
            "webpack",
            r#"{"type":"static-changed","data":"src/cart.js"}"#,
        ),
    ] {
        let channel = format!("reload-{client}");
        open_client(&engine, client, &channel).await;
        eval(&engine, "window.before = true").await;
        send(&channel, message);
        let output = wait_hmr(&engine, TIMEOUT_MS).await.unwrap();
        let update = hot_update(&output);
        assert_eq!(update.outcome, HotOutcome::FullReload, "{client}");
        assert_eq!(update.client, client);
        assert!(
            output.text.starts_with("full reload (src/cart.js)"),
            "{client}: {}",
            output.text
        );
        // A new document, loaded.
        assert_eq!(
            eval(
                &engine,
                "window.before === undefined && document.readyState === 'complete'"
            )
            .await,
            true,
            "{client}"
        );
    }
}

/// A page without a hot-reload client fails at once; one whose client
/// sends nothing times out and says so.
#[tokio::test(flavor = "multi_thread")]
async fn no_client_fails_at_once() {
    let Some(chrome) = chrome_path() else { return };
    let engine = start(&chrome, "hmr-none").await;
    goto(&engine, &Fixtures::shared().url_b("/index.html")).await;
    let started = std::time::Instant::now();
    let err = wait_hmr(&engine, TIMEOUT_MS).await.unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert_eq!(
        err.message,
        "no hot-reload client on this page (not a Vite, webpack or Next.js dev server?)"
    );
    assert_eq!(err.details.as_ref().unwrap()["reason"], "no_client");

    open_client(&engine, "vite", "none-vite").await;
    let err = wait_hmr(&engine, 500).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Timeout, "{}", err.to_text());
    assert!(
        err.to_text()
            .contains("no hot update since the previous command began"),
        "{}",
        err.to_text()
    );
}

/// An update between two commands makes the second warn, once; one that
/// comes during a click is a line of its report instead.
#[tokio::test(flavor = "multi_thread")]
async fn commands_say_what_came_since_the_last() {
    let Some(chrome) = chrome_path() else { return };
    let engine = start(&chrome, "hmr-warn").await;
    open_client(&engine, "vite", "warn-vite").await;
    let text = || {
        engine.eval(EvalRequest {
            script: "document.title".into(),
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
    };
    let (_, notices) = call(text()).await;
    assert!(notices.is_empty(), "{notices:?}");

    send("warn-vite", &vite_update("/hmr/src/cart.js"));
    // The update's log, which the notice comes from, is out after the
    // module ran; and the engine reads it from an event of its own.
    until(&engine, "window.cartRuns === 2 && window.hotLogged === 1").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (output, notices) = call(text()).await;
    output.unwrap();
    assert_eq!(notices.warnings, ["hot update applied (src/cart.js)"]);
    assert!(
        matches!(
            &notices.events[..],
            [Event::HotUpdate { tab, update }] if tab == "t1" && update.outcome == HotOutcome::Applied
        ),
        "{:?}",
        notices.events
    );
    // Once.
    let (_, notices) = call(text()).await;
    assert!(notices.is_empty(), "{notices:?}");

    // The button posts an update while the click runs.
    let (output, notices) = call(engine.act(ActionRequest {
        action: Action::Click {
            locator: "#edit".into(),
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
    }))
    .await;
    let output = output.unwrap();
    if output.text.contains("! hot update applied") {
        assert!(
            output.text.contains("! hot update applied (src/cart.js)"),
            "{}",
            output.text
        );
        assert!(
            output
                .events
                .iter()
                .any(|e| matches!(e, Event::HotUpdate { .. })),
            "{:?}",
            output.events
        );
        assert!(notices.is_empty(), "{notices:?}");
    } else {
        // The update landed after the report: the next command says so.
        eprintln!("the update came after the click's report");
        until(&engine, "window.cartRuns === 3").await;
        let (_, notices) = call(text()).await;
        assert_eq!(notices.warnings, ["hot update applied (src/cart.js)"]);
    }
    // Reported once, either way.
    until(&engine, "window.cartRuns === 3").await;
    let (_, notices) = call(text()).await;
    assert!(notices.is_empty(), "{notices:?}");
}

/// Vite's dev server on a copy of the React 19 app: an edit is applied,
/// a syntax error fails with its place, and an edit of the entry module
/// reloads the page.
#[tokio::test(flavor = "multi_thread")]
async fn a_real_vite_dev_server() {
    if std::env::var_os("CHROME_CDP_NODE_TESTS").is_none_or(|v| v != "1") {
        eprintln!("skipping: set CHROME_CDP_NODE_TESTS=1 to run Vite's dev server (needs Node)");
        return;
    }
    let Some(chrome) = chrome_path() else { return };
    let apps = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/apps-src");
    let modules = apps.join("react19/node_modules");
    assert!(
        modules.join("vite").is_dir(),
        "no node_modules in testdata/apps-src/react19: run cargo xtask apps react19"
    );
    // The app and shared/ (its config imports ../shared/build.mjs), and
    // its node_modules linked.
    let dir = tempfile::tempdir().unwrap();
    for name in ["react19", "shared"] {
        copy_dir(&apps.join(name), &dir.path().join(name));
    }
    let app = dir.path().join("react19").canonicalize().unwrap();
    std::os::unix::fs::symlink(modules.canonicalize().unwrap(), app.join("node_modules")).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut server = std::process::Command::new("node")
        .arg("node_modules/vite/bin/vite.js")
        .args([
            "--port",
            &port.to_string(),
            "--strictPort",
            "--host",
            "127.0.0.1",
        ])
        .current_dir(&app)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let _kill = KillOnDrop(&mut server);
    let url = format!("http://127.0.0.1:{port}/");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(std::time::Instant::now() < deadline, "Vite never listened");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let engine = start(&chrome, "hmr-real-vite").await;
    goto(&engine, &url).await;
    until(&engine, "!!document.querySelector('[data-testid=add-3]')").await;
    let wait = || {
        engine.wait(WaitRequest {
            condition: WaitFor::Hmr,
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
    };
    let item = app.join("src/components/CartItem.tsx");
    let source = std::fs::read_to_string(&item).unwrap();
    assert!(source.contains("        Add\n"), "{source}");

    // An edit: applied, with the file.
    std::fs::write(&item, source.replace("        Add\n", "        Add one\n")).unwrap();
    let (output, _) = in_dir(&app, wait()).await;
    let output = output.unwrap_or_else(|e| panic!("{}", e.to_text()));
    let update = hot_update(&output);
    assert_eq!(update.client, "vite");
    assert_eq!(update.outcome, HotOutcome::Applied, "{update:?}");
    assert!(
        output
            .text
            .starts_with("hot update applied (src/components/CartItem.tsx)"),
        "{}",
        output.text
    );
    until(
        &engine,
        "document.querySelector('[data-testid=add-3]').textContent === 'Add one'",
    )
    .await;

    // A syntax error: failed, at its place.
    std::fs::write(&item, source.replace("        Add\n", "        Add {\n")).unwrap();
    let (result, _) = in_dir(&app, wait()).await;
    let err = result.unwrap_err();
    assert_eq!(err.kind, ErrorKind::HotUpdateFailed, "{}", err.to_text());
    let line = source
        .lines()
        .position(|l| l.contains("</button>"))
        .unwrap()
        + 1;
    assert_eq!(
        err.message,
        format!(
            "hot update failed: [PARSE_ERROR] Unexpected token (src/components/CartItem.tsx:{line})"
        )
    );

    // Fixed: applied again.
    std::fs::write(&item, &source).unwrap();
    let (output, _) = in_dir(&app, wait()).await;
    let output = output.unwrap_or_else(|e| panic!("{}", e.to_text()));
    assert_eq!(hot_update(&output).outcome, HotOutcome::Applied);

    // The entry module accepts no update: a full reload.
    let main = app.join("src/main.tsx");
    let entry = std::fs::read_to_string(&main).unwrap();
    std::fs::write(&main, format!("{entry}\n// edited\n")).unwrap();
    let (output, _) = in_dir(&app, wait()).await;
    let output = output.unwrap_or_else(|e| panic!("{}", e.to_text()));
    let update = hot_update(&output);
    assert_eq!(update.outcome, HotOutcome::FullReload, "{update:?}");
    assert!(
        output.text.starts_with("full reload (src/main.tsx)"),
        "{}",
        output.text
    );
}

/// Runs `fut` as a CLI call in `dir` would.
async fn in_dir<F: Future>(dir: &Path, fut: F) -> (F::Output, engine::Notices) {
    engine::in_root(Some(dir.to_path_buf()), engine::noting(Box::pin(fut))).await
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == "node_modules" {
            continue;
        }
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

struct KillOnDrop<'a>(&'a mut std::process::Child);

impl Drop for KillOnDrop<'_> {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
