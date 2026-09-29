//! `console` and `errors` against a real Chrome (`CHROME_PATH`; skipped
//! without it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture
//! site's `console/` pages (plan 06, milestone 6.1).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use browser::Host;
use engine::{
    Action, ActionRequest, Button, ConsoleRequest, Engine, NavigateRequest, Navigation,
    OpenRequest, Since, WaitFor, WaitRequest, WaitUntil,
};
use fixtures::Fixtures;
use protocol::Output;
use protocol::schema::{ConsoleData, Level, WorkerKind};

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

fn request(errors: bool) -> ConsoleRequest {
    ConsoleRequest {
        errors,
        tab: None,
        level: Level::Info,
        since: Since::Last,
        grep: None,
        limit: 50,
        wait_ms: 0,
        sourcemaps: false,
        timeout_ms: TIMEOUT_MS,
        max_chars: 0,
    }
}

async fn read(engine: &Engine, req: ConsoleRequest) -> (Output, ConsoleData) {
    let out = engine
        .console(req)
        .await
        .unwrap_or_else(|e| panic!("console: {}", e.to_text()));
    let data: ConsoleData = serde_json::from_value(out.data.clone()).unwrap();
    (out, data)
}

fn all() -> ConsoleRequest {
    ConsoleRequest {
        since: Since::Seq { seq: 0 },
        level: Level::Debug,
        limit: 0,
        ..request(false)
    }
}

#[tokio::test]
async fn console_shows_each_entry_once() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "console-cursor").await;
    goto(&engine, &site.url("/console/index.html")).await;

    let (out, data) = read(&engine, request(false)).await;
    let text = &out.text;
    // `--level info` by default: no `console.debug`.
    assert!(!text.contains("debug details"), "{text}");
    assert!(
        text.contains("info page loaded 42 {a: 1, b: 'x'} [1, 2, 3] (/console/index.html:"),
        "{text}"
    );
    assert!(text.contains("info info message"), "{text}");
    assert!(text.contains("warn deprecated call"), "{text}");
    assert!(
        text.contains("error failed to save Error: disk full\n    at /console/index.html:"),
        "{text}"
    );
    assert!(
        text.contains("error Assertion failed: one is not two"),
        "{text}"
    );
    assert_eq!(data.tab, "t1");
    assert_eq!(data.dropped, 0);
    let logged = data
        .entries
        .iter()
        .find(|e| e.text.starts_with("page loaded"))
        .unwrap();
    assert_eq!(logged.kind.as_deref(), Some("log"));
    assert_eq!(logged.source, "console");

    // Nothing new: nothing repeated.
    let (out, data) = read(&engine, request(false)).await;
    assert!(data.entries.is_empty());
    assert_eq!(
        out.text,
        "no new console entries in t1; \
         chrome-cdp console --session console-cursor --since 0 shows the earlier ones"
    );

    // New entries only.
    click(&engine, "#sync").await;
    let (out, data) = read(&engine, request(false)).await;
    assert_eq!(data.entries.len(), 1, "{}", out.text);
    assert!(out.text.contains("uncaught TypeError"), "{}", out.text);

    // `--since 0 --level debug` shows everything again.
    let (out, _) = read(&engine, all()).await;
    assert!(out.text.contains("#1 debug debug details"), "{}", out.text);
    engine.close().await;
}

#[tokio::test]
async fn errors_surface_the_root_cause() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "console-errors").await;
    goto(&engine, &site.url("/console/index.html")).await;
    // What the page logged while loading.
    let (out, _) = read(&engine, request(true)).await;
    assert!(
        out.text.contains("failed to save Error: disk full"),
        "{}",
        out.text
    );
    assert!(!out.text.contains("page loaded"), "{}", out.text);

    let report = click(&engine, "#sync").await;
    assert!(
        report.text.contains("! console error: TypeError"),
        "{}",
        report.text
    );
    let (out, data) = read(&engine, request(true)).await;
    assert_eq!(data.entries.len(), 1, "{}", out.text);
    assert_eq!(
        out.text,
        "#8 error uncaught TypeError: Cannot read properties of null (reading 'items')\n    \
         at failSync (/console/app.js:5:15)\n    \
         at onclick (/console/index.html:12:42)"
            .replace("#8", &format!("#{}", data.entries[0].seq))
    );
    let error = &data.entries[0];
    assert_eq!(error.source, "exception");
    assert_eq!(error.kind.as_deref(), Some("uncaught"));
    assert_eq!(error.stack[0].function, "failSync");
    assert_eq!((error.line, error.column), (Some(5), Some(15)));
    engine.close().await;
}

#[tokio::test]
async fn errors_are_exceptions_error_logs_and_own_failed_loads() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "console-kinds").await;
    goto(&engine, &site.url("/console/index.html")).await;
    read(&engine, request(true)).await;
    for button in [
        "#later", "#reject", "#handled", "#missing", "#cors", "#dns", "#abort", "#third",
    ] {
        click(&engine, button).await;
    }
    let (out, data) = read(&engine, request(true)).await;
    let text = &out.text;
    assert!(
        text.contains("error uncaught Error: later failure\n    at later (/console/app.js:10:11)"),
        "{text}"
    );
    assert!(
        text.contains("error uncaught (in promise) Error: rejected promise"),
        "{text}"
    );
    // A rejection handled after all is gone.
    assert!(!text.contains("handled late"), "{text}");
    assert!(
        text.contains("error [network] Failed to load resource: the server responded with a status of 404 (Not Found) (/console/missing.json)"),
        "{text}"
    );
    assert!(text.contains("has been blocked by CORS policy"), "{text}");
    // Failed loads from other origins are not the page's errors.
    assert!(!text.contains("ERR_NAME_NOT_RESOLVED"), "{text}");
    assert!(!text.contains("ERR_FAILED"), "{text}");
    assert!(
        !text.contains(&site.url_b("/console/missing.json")),
        "{text}"
    );
    // An aborted request is no error.
    assert!(!text.contains("data.json?delay"), "{text}");
    assert!(data.entries.iter().all(|e| e.level == Level::Error));

    // `console` shows them all, other origins too.
    let (out, _) = read(&engine, all()).await;
    assert!(
        out.text
            .contains("net::ERR_NAME_NOT_RESOLVED (http://no-such-host.invalid/x)"),
        "{}",
        out.text
    );

    // A CSP violation. Chrome may report it after the load, so wait for it.
    let csp = |text: &str| {
        text.contains("error [security] Loading the image") && text.contains("(/console/csp.html:")
    };
    goto(&engine, &site.url("/console/csp.html")).await;
    let mut text = String::new();
    for _ in 0..20 {
        let wait = ConsoleRequest {
            wait_ms: 500,
            ..request(true)
        };
        text.push_str(&read(&engine, wait).await.0.text);
        if csp(&text) {
            break;
        }
    }
    assert!(csp(&text), "{text}");
    engine.close().await;
}

#[tokio::test]
async fn workers_log_to_their_tab() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "console-worker").await;
    goto(&engine, &site.url("/console/index.html")).await;
    read(&engine, request(false)).await;
    click(&engine, "#worker").await;
    let mut texts = Vec::new();
    while texts.len() < 2 {
        let (_, data) = read(
            &engine,
            ConsoleRequest {
                wait_ms: 10_000,
                ..request(false)
            },
        )
        .await;
        assert!(!data.entries.is_empty(), "the worker logged nothing");
        for entry in data.entries {
            assert_eq!(entry.worker, Some(site.url("/console/worker.js")));
            assert_eq!(entry.worker_kind, Some(WorkerKind::Dedicated));
            texts.push(entry.text);
        }
    }
    // Once each.
    assert_eq!(
        texts,
        ["worker started", "worker failed Error: worker boom"]
    );
    let (out, _) = read(
        &engine,
        ConsoleRequest {
            grep: Some("worker".into()),
            ..all()
        },
    )
    .await;
    assert!(
        out.text.contains(
            "error [dedicated worker worker.js] worker failed Error: worker boom\n    at /console/worker.js:2:"
        ),
        "{}",
        out.text
    );
    engine.close().await;
}

#[tokio::test]
async fn follow_waits_for_the_next_entry() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "console-follow").await;
    goto(&engine, &site.url("/console/index.html")).await;
    read(&engine, request(false)).await;
    let clicker = engine.clone();
    let clicked = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        click(&clicker, "#later").await;
    });
    let (_, data) = read(
        &engine,
        ConsoleRequest {
            wait_ms: 20_000,
            ..request(false)
        },
    )
    .await;
    clicked.await.unwrap();
    assert_eq!(data.entries.len(), 1);
    assert_eq!(data.entries[0].text, "Error: later failure");
    engine.close().await;
}

#[tokio::test]
async fn the_buffer_is_bounded_and_says_what_it_dropped() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = Engine::new("console-bounded", None, None);
    let chrome = browser::discover(Some(&path), &Host::current()).unwrap();
    engine
        .open(OpenRequest {
            chrome: Some(chrome),
            console_buffer: Some(200),
            timeout_ms: TIMEOUT_MS,
            ..OpenRequest::default()
        })
        .await
        .unwrap();
    goto(&engine, &site.url("/console/index.html")).await;
    read(&engine, request(false)).await;
    click(&engine, "#burst").await;
    let (out, data) = read(
        &engine,
        ConsoleRequest {
            limit: 10,
            ..request(false)
        },
    )
    .await;
    let last = data.entries.last().unwrap();
    assert_eq!(last.text, "burst 1499");
    assert_eq!(data.entries.len(), 10);
    // 1,500 logged, 200 kept: the first 1,300 are gone, the rest but 10
    // skipped.
    assert_eq!(data.dropped, 1300);
    assert_eq!(data.skipped, 190);
    assert!(
        out.warnings[0].contains("1300 console entries were dropped from t1's buffer"),
        "{:?}",
        out.warnings
    );
    assert!(
        out.text
            .contains("… 190 earlier console entries not shown; chrome-cdp console --session console-bounded --since ")
            && out.text.ends_with(" --limit 200 shows them"),
        "{}",
        out.text
    );
    engine.close().await;
}

#[tokio::test]
async fn source_maps_resolve_stack_frames() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "console-sourcemaps").await;
    for page in [
        "/console/sourcemap/index.html",
        "/console/sourcemap/index.html?inline",
    ] {
        goto(&engine, &site.url(page)).await;
        click(&engine, "#checkout").await;
        let (out, data) = read(
            &engine,
            ConsoleRequest {
                level: Level::Debug,
                ..request(false)
            },
        )
        .await;
        let text = &out.text;
        assert!(
            text.contains("info checking out 1 (/console/sourcemap/src/main.ts:6)"),
            "{page}: {text}"
        );
        assert!(
            text.contains(
                "uncaught TypeError: Cannot read properties of undefined (reading 'code')"
            ),
            "{page}: {text}"
        );
        assert!(
            text.contains("    at e (/console/sourcemap/src/cart.ts:7:"),
            "{page}: {text}"
        );
        assert!(
            text.contains("/console/sourcemap/src/main.ts:7:"),
            "{page}: {text}"
        );
        let error = data
            .entries
            .iter()
            .find(|e| e.source == "exception")
            .unwrap();
        let original = error.stack[0].original.as_ref().unwrap();
        assert_eq!(original.source, site.url("/console/sourcemap/src/cart.ts"));
        assert_eq!(original.line, 7);
        // The generated place stays.
        assert!(
            error.stack[0]
                .url
                .ends_with("/console/sourcemap/dist/app.js")
                || error.stack[0].url.ends_with("/dist/inline.js")
        );
    }
    engine.close().await;
}

#[tokio::test]
async fn bad_grep_is_a_usage_error() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "console-grep").await;
    let err = engine
        .console(ConsoleRequest {
            grep: Some("fetch(".into()),
            ..request(false)
        })
        .await
        .unwrap_err();
    assert_eq!(err.exit_code(), 2);
    assert!(
        err.message
            .starts_with("--grep \"fetch(\" is not a regular expression"),
        "{}",
        err.message
    );
    engine.close().await;
}

#[tokio::test]
async fn wait_timeouts_name_unread_errors() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "console-wait").await;
    goto(&engine, &site.url("/console/index.html")).await;
    read(&engine, request(true)).await;
    let wait = || WaitRequest {
        condition: WaitFor::Text {
            text: "Order placed".into(),
            within: None,
            gone: false,
        },
        tab: None,
        strict_refs: false,
        testid_attribute: None,
        timeout_ms: 300,
        max_chars: 0,
    };
    // Every error was read: nothing to say.
    let err = engine.wait(wait()).await.unwrap_err();
    assert!(
        err.notes.iter().all(|n| n.label != "console errors"),
        "{}",
        err.to_text()
    );

    click(&engine, "#sync").await;
    let err = engine.wait(wait()).await.unwrap_err();
    let note = err
        .notes
        .iter()
        .find(|n| n.label == "console errors")
        .unwrap();
    assert_eq!(
        note.text,
        "1 unread; chrome-cdp errors --session console-wait shows them"
    );
    assert_eq!(err.details.as_ref().unwrap()["console_errors"], 1);
    engine.close().await;
}
