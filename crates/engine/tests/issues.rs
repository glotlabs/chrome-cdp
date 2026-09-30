//! `issues` against a real Chrome (`CHROME_PATH`; skipped without it
//! unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture site's
//! `issues/` pages (plan 17, milestone 17.1).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::Host;
use engine::{Engine, IssuesRequest, NavigateRequest, Navigation, OpenRequest, Since, WaitUntil};
use fixtures::Fixtures;
use protocol::Output;
use protocol::schema::{Issue, IssuesData, Severity, WorkerKind};

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
    start_with(path, name, OpenRequest::default()).await
}

async fn start_with(path: &Path, name: &str, open: OpenRequest) -> Arc<Engine> {
    let engine = Engine::new(name, None, None);
    let chrome = browser::discover(Some(path), &Host::current()).unwrap();
    engine
        .open(OpenRequest {
            chrome: Some(chrome),
            timeout_ms: TIMEOUT_MS,
            ..open
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

fn all() -> IssuesRequest {
    IssuesRequest {
        tab: None,
        all_tabs: false,
        since: Since::Seq { seq: 0 },
        grep: None,
        limit: 0,
        min_severity: Severity::Info,
        wait_ms: 0,
        timeout_ms: TIMEOUT_MS,
        max_chars: 0,
    }
}

async fn read(engine: &Engine, req: IssuesRequest) -> (Output, IssuesData) {
    let out = engine
        .issues(req)
        .await
        .unwrap_or_else(|e| panic!("issues: {}", e.to_text()));
    let data: IssuesData = serde_json::from_value(out.data.clone()).unwrap();
    (out, data)
}

/// Reads every issue until `done` holds for them (issues arrive while
/// and after a page loads), or fails after a while with what it saw.
async fn until(engine: &Engine, what: &str, done: impl Fn(&[Issue]) -> bool) -> Vec<Issue> {
    let start = Instant::now();
    loop {
        let (out, data) = read(engine, all()).await;
        if done(&data.issues) {
            return data.issues;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "{what}: never got there; saw:\n{}",
            out.text
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn find<'a>(issues: &'a [Issue], rule: &str, url: &str) -> Option<&'a Issue> {
    issues.iter().find(|i| {
        i.rule == rule
            && (i.target.url.as_deref().is_some_and(|u| u.contains(url))
                || i.target
                    .source
                    .as_ref()
                    .is_some_and(|s| s.url.contains(url)))
    })
}

/// The main frame, a cross-site iframe (an OOPIF) and a dedicated worker
/// each raise issues: CSP blocks, CORS blocks, a deprecation and a form
/// problem, with refs for elements and network log ids for requests.
#[tokio::test]
async fn issues_of_a_page_its_frames_and_its_worker() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "issues-page").await;
    goto(&engine, &site.url("/issues/index.html")).await;
    let issues = until(&engine, "every family", |issues| {
        let cors = issues
            .iter()
            .filter(|i| i.rule == "cors-missing-allow-origin-header")
            .count();
        let csp = issues
            .iter()
            .filter(|i| i.rule == "content-security-policy-inline-violation")
            .count();
        cors == 3
            && csp == 2
            && issues.iter().any(|i| i.rule.starts_with("deprecation-"))
            && issues.iter().any(|i| i.rule.starts_with("form-"))
    })
    .await;

    // The main frame's inline script, blocked, with a ref.
    let csp = find(
        &issues,
        "content-security-policy-inline-violation",
        "/issues/index.html",
    )
    .unwrap();
    assert_eq!(csp.severity, Severity::Error);
    assert_eq!(csp.category, "issues");
    assert!(csp.target.reference.is_some(), "{csp:?}");
    assert_eq!(csp.target.frame, None);
    assert_eq!(
        csp.key,
        "issues/content-security-policy-inline-violation/self/issues/index.html:17"
    );
    // The iframe's, in its own process: its frame is named.
    let framed = find(
        &issues,
        "content-security-policy-inline-violation",
        "/issues/frame.html",
    )
    .unwrap();
    assert!(framed.target.frame.is_some(), "{framed:?}");
    assert!(framed.target.reference.is_some(), "{framed:?}");

    // CORS: the page's, the frame's and the worker's requests, each with
    // its id in the network log.
    let page = find(
        &issues,
        "cors-missing-allow-origin-header",
        "/issues/data.json",
    )
    .unwrap();
    assert_eq!(page.severity, Severity::Error);
    assert!(page.target.request_id.is_some(), "{page:?}");
    let frame = issues
        .iter()
        .find(|i| {
            i.rule == "cors-missing-allow-origin-header" && i.key.ends_with("self/issues/data.json")
        })
        .unwrap();
    assert!(frame.target.url.as_deref().unwrap().contains("from=frame"));
    let worker = find(
        &issues,
        "cors-missing-allow-origin-header",
        "worker-data.json",
    )
    .unwrap();
    assert_eq!(
        worker.target.worker.as_ref().map(|w| w.kind),
        Some(WorkerKind::Dedicated)
    );
    // Keys leave out ports and queries.
    assert_eq!(
        page.key,
        "issues/cors-missing-allow-origin-header/http://localhost/issues/data.json"
    );

    let deprecation = issues
        .iter()
        .find(|i| i.rule.starts_with("deprecation-"))
        .unwrap();
    assert_eq!(deprecation.severity, Severity::Warning);
    assert_eq!(
        deprecation.message,
        "a synchronous XMLHttpRequest on the main thread is deprecated"
    );
    assert_eq!(deprecation.target.source.as_ref().map(|s| s.line), Some(5));

    // The label for an id no field has: its element, a stable locator,
    // and a key made from it.
    let form = find_rule(&issues, "form-label-for-matches-non-existing-id");
    assert_eq!(form.target.element.as_deref(), Some("label \"Email\""));
    assert!(form.target.reference.is_some());
    let locator = form.target.locator.as_deref().unwrap();
    assert_eq!(
        form.key,
        format!("issues/form-label-for-matches-non-existing-id/{locator}")
    );

    // The text: a header, then a line each, severity first.
    let (out, _) = read(&engine, all()).await;
    let first = out.text.lines().next().unwrap();
    assert!(
        first.starts_with(&format!("{} issues (", issues.len())),
        "{}",
        out.text
    );
    assert!(
        out.text.lines().any(
            |l| l.starts_with("E cors-missing-allow-origin-header") && l.contains("(request #")
        ),
        "{}",
        out.text
    );
    engine.close().await;
}

fn find_rule<'a>(issues: &'a [Issue], rule: &str) -> &'a Issue {
    issues
        .iter()
        .find(|i| i.rule == rule)
        .unwrap_or_else(|| panic!("no {rule} in {issues:#?}"))
}

/// `--since last` shows each issue once, `--min-severity` hides the
/// less severe (and says so), and the same issue raised again is shown
/// again with its count.
#[tokio::test]
async fn cursors_severities_and_repeats() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "issues-cursor").await;
    goto(&engine, &site.url("/issues/clean.html")).await;
    let (out, data) = read(&engine, all()).await;
    assert!(data.issues.is_empty(), "a clean page: {}", out.text);

    goto(&engine, &site.url("/issues/quirks.html")).await;
    let issues = until(&engine, "quirks mode", |i| {
        i.iter().any(|i| i.rule == "quirks-mode")
    })
    .await;
    assert_eq!(find_rule(&issues, "quirks-mode").severity, Severity::Info);
    // Info is hidden by default.
    let warnings = IssuesRequest {
        min_severity: Severity::Warning,
        limit: 50,
        ..all()
    };
    let (out, data) = read(&engine, warnings.clone()).await;
    assert!(data.issues.is_empty());
    assert_eq!(data.hidden, 1);
    assert!(
        out.text.contains(
            "1 less severe issue hidden; chrome-cdp issues --session issues-cursor \
             --min-severity info --since 0 shows them"
        ),
        "{}",
        out.text
    );
    let last = IssuesRequest {
        since: Since::Last,
        ..warnings.clone()
    };

    // 50 identical requests blocked: one issue, ×50.
    goto(&engine, &site.url("/issues/repeat.html")).await;
    let issues = until(&engine, "50 repeats", |i| {
        i.iter()
            .any(|i| i.rule == "cors-missing-allow-origin-header" && i.count == 50)
    })
    .await;
    assert_eq!(
        issues
            .iter()
            .filter(|i| i.rule == "cors-missing-allow-origin-header")
            .count(),
        1
    );
    let (out, data) = read(&engine, warnings.clone()).await;
    assert_eq!(data.issues.len(), 1, "{}", out.text);
    assert!(
        out.text.lines().nth(1).unwrap().ends_with("×50"),
        "{}",
        out.text
    );
    // Every read moves the cursor, so this is nothing new.
    let (out, data) = read(&engine, last.clone()).await;
    assert!(data.issues.is_empty());
    assert!(
        out.text.starts_with(
            "no new issues in t1; chrome-cdp issues --session issues-cursor --since 0"
        ),
        "{}",
        out.text
    );

    // A cookie the other site's frame may not set.
    goto(&engine, &site.url("/issues/cookie.html")).await;
    let issues = until(&engine, "a cookie", |i| {
        i.iter().any(|i| i.rule.starts_with("cookie-"))
    })
    .await;
    let cookie = issues
        .iter()
        .find(|i| i.rule.starts_with("cookie-"))
        .unwrap();
    assert_eq!(cookie.severity, Severity::Warning);
    assert_eq!(
        cookie.target.cookie.as_ref().map(|c| c.name.as_str()),
        Some("sid")
    );
    assert_eq!(
        cookie.key,
        format!("issues/{}/cookie sid@localhost", cookie.rule)
    );
    engine.close().await;
}

/// An https page's script over http is blocked (mixed.test maps to this
/// machine: 127.0.0.1 counts as secure, so it is never mixed content).
#[tokio::test]
async fn mixed_content_is_blocked() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start_with(
        &path,
        "issues-mixed",
        OpenRequest {
            ignore_https_errors: Some(true),
            chrome_args: vec!["--host-resolver-rules=MAP mixed.test 127.0.0.1".to_owned()],
            ..OpenRequest::default()
        },
    )
    .await;
    goto(&engine, &format!("{}/issues/mixed.html", site.https())).await;
    let issues = until(&engine, "mixed content", |i| {
        i.iter().any(|i| i.rule == "mixed-content-blocked")
    })
    .await;
    let mixed = find_rule(&issues, "mixed-content-blocked");
    assert_eq!(mixed.severity, Severity::Error);
    assert!(
        mixed.message.starts_with("script http://mixed.test:"),
        "{mixed:?}"
    );
    assert_eq!(
        mixed.key,
        "issues/mixed-content-blocked/http://mixed.test/issues/mixed.js"
    );
    engine.close().await;
}

struct DebugChrome {
    child: std::process::Child,
    profile: PathBuf,
}

impl Drop for DebugChrome {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

/// A Chrome started with a DevTools port on `url`, and that port.
fn debug_chrome(path: &Path, url: &str) -> (DebugChrome, String) {
    let profile = std::env::temp_dir().join(format!("engine-issues-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&profile);
    std::fs::create_dir_all(&profile).unwrap();
    let child = std::process::Command::new(path)
        .args([
            "--headless",
            "--remote-debugging-port=0",
            "--no-first-run",
            "--no-default-browser-check",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(url)
        .env("TMPDIR", &profile)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let chrome = DebugChrome {
        child,
        profile: profile.clone(),
    };
    let active = profile.join("DevToolsActivePort");
    let start = Instant::now();
    let port = loop {
        if let Ok(text) = std::fs::read_to_string(&active)
            && text.lines().count() >= 2
        {
            break text.lines().next().unwrap().to_owned();
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "no DevTools port"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    (chrome, port)
}

/// Issues a page raised before chrome-cdp attached (a `connect` to a
/// running Chrome) are replayed when it enables them.
#[tokio::test]
async fn issues_raised_before_connecting_are_replayed() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let (_chrome, port) = debug_chrome(&path, &site.url("/issues/index.html"));
    // Let the page load while nobody is attached.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let engine = Engine::new("issues-connect", None, None);
    engine
        .connect(engine::ConnectRequest {
            endpoint: port,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
    let issues = until(&engine, "the replayed issues", |i| {
        i.iter()
            .any(|i| i.rule == "content-security-policy-inline-violation")
    })
    .await;
    assert!(
        issues.iter().any(|i| i.rule.starts_with("deprecation-")),
        "{issues:#?}"
    );
    engine.close().await;
}
