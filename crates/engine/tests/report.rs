//! The post-action report against a real Chrome (`CHROME_PATH`; skipped
//! without it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture
//! site's `actions/report.html` (plan 05, milestone 5.5).
//!
//! Each test clicks something that causes one kind of report line (or
//! nothing) and checks the text and the `--json` events.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use browser::Host;
use engine::{
    Action, ActionRequest, Button, Engine, LocateOptions, NavigateRequest, Navigation, OpenRequest,
    WaitUntil,
};
use fixtures::Fixtures;
use protocol::Output;
use protocol::schema::Event;

const TIMEOUT_MS: u64 = 30_000;
const TIMEOUT: Duration = Duration::from_millis(TIMEOUT_MS);

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

async fn start(path: &Path, name: &str, downloads: Option<PathBuf>) -> Arc<Engine> {
    let engine = Engine::new(name, None, downloads);
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

fn click(locator: &str) -> Action {
    Action::Click {
        locator: locator.to_owned(),
        button: Button::Left,
        count: 1,
        modifiers: Vec::new(),
        position: None,
        force: false,
        dblclick: false,
    }
}

/// How an action is run.
#[derive(Default, Clone, Copy)]
struct After {
    no_wait_after: bool,
    snap: bool,
}

async fn act_with(engine: &Engine, action: Action, after: After) -> Output {
    let what = format!("{action:?}");
    engine
        .act(ActionRequest {
            action,
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: TIMEOUT_MS,
            no_wait_after: after.no_wait_after,
            snap: after.snap,
            max_chars: 20_000,
        })
        .await
        .unwrap_or_else(|e| panic!("{what}: {}", e.to_text()))
}

async fn act(engine: &Engine, action: Action) -> Output {
    act_with(engine, action, After::default()).await
}

/// The report's lines: the text after the action's own line.
fn report(out: &Output) -> Vec<&str> {
    out.text.lines().skip(1).collect()
}

/// The text of the one element `locator` matches.
async fn text(engine: &Engine, locator: &str) -> String {
    engine
        .call_on_locator(
            None,
            locator,
            &LocateOptions::default(),
            "function () { return this.textContent; }",
            TIMEOUT,
        )
        .await
        .unwrap_or_else(|e| panic!("{locator}: {}", e.to_text()))
        .value
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn navigations_are_followed_and_reported() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "report-navigate", None).await;
    let page = site.url("/actions/report.html");

    // Nothing happens: the action's line alone.
    goto(&engine, &page).await;
    let out = act(&engine, click("#nothing")).await;
    assert_eq!(out.text, "clicked e1 button \"Nothing\"");
    assert!(out.events.is_empty(), "{:?}", out.events);

    // A link: waits for the new page to load.
    let out = act(&engine, click("#next")).await;
    let next = site.url("/actions/report-next.html");
    assert_eq!(
        report(&out),
        [format!("→ navigated to {next} \"Next\" (200)")]
    );
    assert!(
        matches!(&out.events[..], [Event::Navigated { url, loaded: true, status: Some(200), .. }] if *url == next),
        "{:?}",
        out.events
    );
    assert_eq!(text(&engine, "css=h1").await, "Next page");

    // A script that navigates a moment after the click.
    goto(&engine, &page).await;
    let out = act(&engine, click("#later")).await;
    assert_eq!(
        report(&out),
        [format!("→ navigated to {next}?later \"Next\" (200)")]
    );

    // A 404 page: the status is on the navigation's line, not a failed
    // request's.
    goto(&engine, &page).await;
    let out = act(&engine, click("#missing")).await;
    let missing = site.url("/actions/report-missing.html");
    assert_eq!(
        report(&out),
        [format!("→ navigated to {missing} \"\" (404)")]
    );

    // pushState: a same-document navigation.
    goto(&engine, &page).await;
    let out = act(&engine, click("#push")).await;
    assert_eq!(
        report(&out),
        [format!(
            "→ navigated to {page}?tab=settings \"Settings\" (same document)"
        )]
    );

    // A link to a server that is not there: Chrome's error page.
    goto(&engine, &page).await;
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let refused = format!("http://127.0.0.1:{port}/");
    engine
        .call_on_locator(
            None,
            "#next",
            &LocateOptions::default(),
            &format!("function () {{ this.href = {refused:?}; }}"),
            TIMEOUT,
        )
        .await
        .unwrap();
    let out = act(&engine, click("#next")).await;
    assert_eq!(
        report(&out),
        [format!(
            "→ navigation to {refused} failed: net::ERR_CONNECTION_REFUSED"
        )]
    );

    // A link whose answer has no content: the page stays, nothing to say.
    goto(&engine, &page).await;
    let out = act(&engine, click("#no-content")).await;
    assert_eq!(report(&out), Vec::<&str>::new(), "{}", out.text);
}

#[tokio::test]
async fn console_errors_and_failed_requests_are_reported() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "report-errors", None).await;
    goto(&engine, &site.url("/actions/report.html")).await;

    let out = act(&engine, click("#log-error")).await;
    assert_eq!(
        report(&out),
        ["! console error: save failed: 500 (report.html:16)"]
    );
    assert!(
        matches!(
            &out.events[..],
            [Event::ConsoleError {
                uncaught: false,
                line: Some(16),
                ..
            }]
        ),
        "{:?}",
        out.events
    );

    let out = act(&engine, click("#throw")).await;
    assert_eq!(
        report(&out),
        [
            "! console error: TypeError: Cannot read properties of null (reading 'x') (report.html:17)"
        ]
    );

    let out = act(&engine, click("#many-errors")).await;
    let lines = report(&out);
    assert_eq!(lines.len(), 6, "{lines:?}");
    assert_eq!(lines[0], "! console error: error 1 (report.html:18)");
    assert_eq!(
        lines[5],
        "! … and 2 more console errors (chrome-cdp errors --session report-errors shows them)"
    );
    assert_eq!(out.events.len(), 7);

    // The report waits for the request the click sent, and names it.
    let out = act(&engine, click("#save")).await;
    assert_eq!(
        report(&out),
        ["! request failed: POST /actions/report-api?status=500 (500)"]
    );
    assert_eq!(text(&engine, "#out").await, "saved: 500");

    // More failures than it lists: `network --failed` has them all.
    let out = act(&engine, click("#many-failures")).await;
    let lines = report(&out);
    assert_eq!(lines.len(), 6, "{lines:?}");
    assert_eq!(
        lines[5],
        "! … and 2 more failed requests (chrome-cdp network --failed --session report-errors shows them)"
    );

    // A slow request: the report waits for it and for what it renders.
    let out = act(&engine, click("#slow")).await;
    assert_eq!(report(&out), Vec::<&str>::new());
    assert_eq!(text(&engine, "#out").await, "loaded");

    // Another origin's failures are not the page's.
    let out = act(&engine, click("#other")).await;
    assert_eq!(report(&out), Vec::<&str>::new(), "{}", out.text);
}

#[tokio::test]
async fn tabs_dialogs_downloads_and_focus_are_reported() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let downloads = tempfile::tempdir().unwrap();
    let engine = start(&path, "report-events", Some(downloads.path().to_path_buf())).await;
    goto(&engine, &site.url("/actions/report.html")).await;

    // A target=_blank link: the popup attaches after the click returns.
    let out = act(&engine, click("#popup")).await;
    let help = site.url("/actions/report-next.html?popup");
    assert_eq!(
        report(&out),
        [format!(
            "! opened tab t2 ({help}); switch to it: chrome-cdp tab t2 --session report-events"
        )]
    );

    // The session's default policy accepts alerts.
    let out = act(&engine, click("#alert")).await;
    assert_eq!(report(&out), ["! dialog: alert \"Saved\" (accepted)"]);

    // The download finishes within the report's wait.
    let out = act(&engine, click("#download")).await;
    let lines = report(&out);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with("! downloaded: report.txt (")
            && lines[0].ends_with(&format!(
                "{}",
                downloads.path().join("report.txt").display()
            )),
        "{lines:?}"
    );

    // Focus moving to another form is reported; focus on what was acted
    // on, or within the same form, is not.
    let out = act(
        &engine,
        Action::Fill {
            locator: "#user".to_owned(),
            text: "ada".to_owned(),
            force: false,
        },
    )
    .await;
    assert_eq!(report(&out), Vec::<&str>::new());
    let out = act(&engine, click("#continue")).await;
    assert_eq!(
        report(&out),
        ["! focus moved to e6 textbox \"Code\""],
        "{}",
        out.text
    );
    let out = act(
        &engine,
        Action::Press {
            keys: "Shift+Tab".to_owned(),
            locator: None,
        },
    )
    .await;
    assert_eq!(
        report(&out),
        ["! focus moved to e7 textbox \"Password\""],
        "{}",
        out.text
    );
    let out = act(
        &engine,
        Action::Press {
            keys: "Shift+Tab".to_owned(),
            locator: None,
        },
    )
    .await;
    assert_eq!(report(&out), Vec::<&str>::new(), "{}", out.text);
}

#[tokio::test]
async fn no_wait_after_and_snap() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "report-options", None).await;
    let page = site.url("/actions/report.html");
    goto(&engine, &page).await;

    // Without the wait, the slow request's answer is not there yet.
    let quick = After {
        no_wait_after: true,
        snap: false,
    };
    let out = act_with(&engine, click("#slow"), quick).await;
    assert_eq!(report(&out), Vec::<&str>::new());
    assert_eq!(text(&engine, "#out").await, "");

    // --snap: what changed since the last snapshot (none yet: all of it).
    let snap = After {
        no_wait_after: false,
        snap: true,
    };
    let out = act_with(&engine, click("#save"), snap).await;
    let lines = report(&out);
    assert_eq!(
        lines[0],
        "! request failed: POST /actions/report-api?status=500 (500)"
    );
    assert_eq!(lines[1], format!("page t1 \"Report\" {page}"));
    assert_eq!(
        lines[2],
        "(no earlier snapshot of this tab; all of it is new)"
    );
    assert_eq!(out.data["snapshot"]["diff"], false);

    // The next one shows only what changed.
    let out = act_with(&engine, click("#push"), snap).await;
    let lines = report(&out);
    assert_eq!(
        lines[0],
        format!("→ navigated to {page}?tab=settings \"Settings\" (same document)")
    );
    assert_eq!(
        lines[1],
        format!("page t1 \"Settings\" {page}?tab=settings")
    );
    assert_eq!(
        lines[2],
        "~ - button \"Settings\" [e7] focused (was not focused)"
    );
    assert_eq!(out.data["snapshot"]["diff"], true);
}
