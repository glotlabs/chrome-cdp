//! End-to-end tests against a real Chrome.
//!
//! They need `CHROME_PATH`. Without it they pass as no-ops, unless
//! `CHROME_CDP_REQUIRE_CHROME=1` (set in CI), in which case they fail.
//! `CHROME_CDP_LOG=cdp=trace` logs every CDP frame (use `--nocapture`).

// clippy.toml's test allowances cover `#[test]` fns, not their helpers.
#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use cdp::domains::{browser, page, runtime, target};
use cdp::transport::{PipeTransport, WsTransport};
use cdp::{CdpError, Connection};
use tempfile::TempDir;
use tokio::process::{Child, Command};
use tokio::time::timeout;

/// Generous bound for anything Chrome does in these tests.
const STEP: Duration = Duration::from_secs(20);

fn chrome_path() -> Option<PathBuf> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_env("CHROME_CDP_LOG"))
        .with_test_writer()
        .try_init();
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

struct Chrome {
    child: Child,
    profile: TempDir,
}

/// Minimal launch for these tests; the real flag set lives in the `browser`
/// crate (plan 02).
fn chrome_command(path: &PathBuf, profile: &TempDir, extra: &[&str]) -> Command {
    let mut cmd = Command::new(path);
    cmd.arg("--headless")
        .arg(format!("--user-data-dir={}", profile.path().display()))
        .args([
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-dev-shm-usage",
            "about:blank",
        ])
        .args(extra)
        // Chrome keeps a socket dir in TMPDIR, which leaks when a test kills
        // it; keep it in the profile, which the test deletes.
        .env("TMPDIR", profile.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    cmd
}

fn launch_pipe(path: &PathBuf) -> (Connection, Chrome) {
    let profile = tempfile::tempdir().unwrap();
    let cmd = chrome_command(path, &profile, &["--remote-debugging-pipe"]);
    let (transport, child) = PipeTransport::spawn(cmd).unwrap();
    (Connection::new(transport), Chrome { child, profile })
}

impl Chrome {
    /// Waits for Chrome to exit (after the connection closed), so the profile
    /// can be deleted cleanly.
    async fn wait(mut self) {
        match timeout(STEP, self.child.wait()).await {
            Ok(status) => {
                status.unwrap();
            }
            Err(_) => {
                self.child.kill().await.unwrap();
            }
        }
        drop(self.profile);
    }
}

async fn new_page(conn: &Connection) -> cdp::Session {
    let created = conn
        .browser()
        .call(target::CreateTargetParams::new("about:blank".into()))
        .await
        .unwrap();
    conn.attach(created.target_id).await.unwrap()
}

#[tokio::test]
async fn pipe_version_attach_navigate_load() {
    let Some(path) = chrome_path() else { return };
    let (conn, chrome) = launch_pipe(&path);

    let version = timeout(
        STEP,
        conn.browser().call(browser::GetVersionParams::default()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(version.product.contains("Chrome"), "{}", version.product);
    assert_eq!(version.protocol_version, "1.3");

    let page = timeout(STEP, new_page(&conn)).await.unwrap();
    let mut loaded = page.listen::<page::LoadEventFired>();
    page.call(page::EnableParams::default()).await.unwrap();
    let nav = page
        .call(page::NavigateParams::new(
            "data:text/html,<title>hello</title><p>hi".into(),
        ))
        .await
        .unwrap();
    assert!(nav.error_text.is_none(), "{:?}", nav.error_text);
    timeout(STEP, loaded.next()).await.unwrap().unwrap();

    let title = page
        .call(runtime::EvaluateParams::new("document.title".into()))
        .await
        .unwrap();
    assert_eq!(title.result.value, Some(serde_json::json!("hello")));

    // Protocol errors come back typed.
    let err = page
        .call_raw("Nope.nope", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(
        matches!(err, CdpError::Protocol { code: -32601, .. }),
        "{err:?}"
    );

    // Dropping the connection closes the pipe; Chrome exits by itself.
    drop((page, conn));
    chrome.wait().await;
}

#[tokio::test]
async fn killing_chrome_fails_pending_calls_fast() {
    let Some(path) = chrome_path() else { return };
    let (conn, mut chrome) = launch_pipe(&path);
    let pages = [
        timeout(STEP, new_page(&conn)).await.unwrap(),
        timeout(STEP, new_page(&conn)).await.unwrap(),
    ];

    // Calls Chrome never answers, several per page session.
    let mut pending = Vec::new();
    for page in &pages {
        for _ in 0..5 {
            let mut forever = runtime::EvaluateParams::new("new Promise(() => {})".into());
            forever.await_promise = Some(true);
            let page = page.clone();
            pending.push(tokio::spawn(async move { page.call(forever).await }));
        }
    }
    let mut events = pages[0].events();
    // Give the calls time to reach Chrome.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(pending.iter().all(|p| !p.is_finished()));

    chrome.child.start_kill().unwrap();
    let killed = Instant::now();
    for call in pending {
        let err = timeout(STEP, call).await.unwrap().unwrap().unwrap_err();
        assert!(matches!(err, CdpError::Disconnected { .. }), "{err:?}");
    }
    let elapsed = killed.elapsed();
    eprintln!("all pending calls failed {elapsed:?} after SIGKILL");
    // Measured 6-55 ms when run alone. Nearly all of it is the kernel
    // tearing down Chrome's address space before it closes the pipe, which
    // can pass 100 ms on a loaded machine, so the bound here is loose. The
    // client's own reaction time is checked in the connection unit tests.
    assert!(
        elapsed < Duration::from_millis(500),
        "pending calls failed {elapsed:?} after the kill"
    );
    assert!(timeout(STEP, events.next()).await.unwrap().is_none());
    // New calls on any session fail at once too.
    for session in [conn.browser(), pages[1].clone()] {
        let err = session
            .call(page::EnableParams::default())
            .await
            .unwrap_err();
        assert!(matches!(err, CdpError::Disconnected { .. }), "{err:?}");
    }
    chrome.wait().await;
}

#[tokio::test]
async fn websocket_transport() {
    let Some(path) = chrome_path() else { return };
    let profile = tempfile::tempdir().unwrap();
    let cmd = chrome_command(&path, &profile, &["--remote-debugging-port=0"]);
    let mut child = { cmd }.spawn().unwrap();

    // Chrome writes "<port>\n<browser ws path>" once it is listening.
    let port_file = profile.path().join("DevToolsActivePort");
    let url = timeout(STEP, async {
        loop {
            if let Ok(text) = std::fs::read_to_string(&port_file)
                && let Some((port, ws_path)) = text.trim().split_once('\n')
            {
                return format!("ws://127.0.0.1:{port}{ws_path}");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();

    let conn = Connection::new(WsTransport::connect(&url).await.unwrap());
    let version = timeout(
        STEP,
        conn.browser().call(browser::GetVersionParams::default()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(version.product.contains("Chrome"));
    let page = timeout(STEP, new_page(&conn)).await.unwrap();
    let two = page
        .call(runtime::EvaluateParams::new("1+1".into()))
        .await
        .unwrap();
    assert_eq!(two.result.value, Some(serde_json::json!(2)));

    // `Browser.close` answers, then the socket goes away.
    let _ = conn.browser().call(browser::CloseParams::default()).await;
    timeout(STEP, conn.closed()).await.unwrap();
    timeout(STEP, child.wait()).await.unwrap().unwrap();
}

/// Latency baseline for plan 10: 10,000 sequential `Runtime.evaluate("1+1")`
/// calls. Run with
/// `cargo test -p cdp --release --test chrome -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "benchmark; run explicitly in release mode"]
async fn sequential_evaluate_latency() {
    let Some(path) = chrome_path() else { return };
    let (conn, chrome) = launch_pipe(&path);
    let page = timeout(STEP, new_page(&conn)).await.unwrap();
    let n = 10_000;
    let start = Instant::now();
    for _ in 0..n {
        page.call(runtime::EvaluateParams::new("1+1".into()))
            .await
            .unwrap();
    }
    let elapsed = start.elapsed();
    eprintln!(
        "{n} sequential Runtime.evaluate calls: {elapsed:?} ({:?}/call)",
        elapsed / n
    );
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    drop((page, conn));
    chrome.wait().await;
}
