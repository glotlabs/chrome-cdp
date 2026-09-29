//! Navigation against a real Chrome (`CHROME_PATH`; skipped without it
//! unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture site:
//! `goto` with redirects, HTTP errors, failures and relative URLs, the
//! `--wait` conditions, `back`/`forward` (the back/forward cache and
//! pushState entries included) and `reload`.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use browser::Host;
use engine::{Engine, NavigateRequest, Navigation, OpenRequest, WaitUntil, World};
use fixtures::Fixtures;
use protocol::schema::NavigateData;
use protocol::{ErrorInfo, ErrorKind, Output};
use serde_json::json;

const TIMEOUT_MS: u64 = 30_000;
const TIMEOUT: Duration = Duration::from_secs(10);

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

async fn launch(path: &std::path::Path, req: OpenRequest) -> Arc<Engine> {
    let engine = Engine::new("engine-navigate-test", None, None);
    let chrome = browser::discover(Some(path), &Host::current()).unwrap();
    engine
        .open(OpenRequest {
            chrome: Some(chrome),
            timeout_ms: TIMEOUT_MS,
            ..req
        })
        .await
        .unwrap();
    engine
}

async fn start(path: &std::path::Path) -> Arc<Engine> {
    launch(path, OpenRequest::default()).await
}

async fn nav(
    engine: &Engine,
    navigation: Navigation,
    wait: WaitUntil,
    timeout_ms: u64,
) -> Result<Output, ErrorInfo> {
    engine
        .navigate(NavigateRequest {
            navigation,
            wait,
            tab: None,
            timeout_ms,
        })
        .await
}

fn goto(url: &str) -> Navigation {
    Navigation::Goto {
        url: url.to_owned(),
        base_url: None,
    }
}

fn data(output: &Output) -> NavigateData {
    serde_json::from_value(output.data.clone()).unwrap()
}

async fn ok(engine: &Engine, navigation: Navigation) -> (NavigateData, Output) {
    let output = nav(engine, navigation, WaitUntil::Load, TIMEOUT_MS)
        .await
        .unwrap_or_else(|e| panic!("{}", e.to_text()));
    (data(&output), output)
}

fn reason(err: &ErrorInfo) -> String {
    err.details.as_ref().unwrap()["reason"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A port nothing listens on.
fn closed_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn goto_follows_redirects_and_reports_the_final_page() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    let requested = site.url("/redirect?to=/nav/a.html&n=3");
    let (page, output) = ok(&engine, goto(&requested)).await;
    assert_eq!(page.requested_url.as_deref(), Some(requested.as_str()));
    assert_eq!(page.url, site.url("/nav/a.html"));
    assert_eq!(page.title, "Page A");
    assert_eq!(page.status, Some(200));
    assert_eq!(page.tab, "t1");
    assert_eq!(page.wait, "load");
    assert!(page.navigated && !page.same_document && !page.restored);
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
    assert!(
        output
            .text
            .starts_with(&format!("→ 200 {} \"Page A\" (", site.url("/nav/a.html"))),
        "{}",
        output.text
    );
    assert_eq!(output.quiet.as_deref(), Some(page.url.as_str()));
    engine.close().await;
}

#[tokio::test]
async fn http_error_statuses_are_warnings() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    let (page, output) = ok(&engine, goto(&site.url("/nav/missing.html"))).await;
    assert_eq!(page.status, Some(404));
    assert_eq!(
        output.warnings,
        [format!(
            "the server answered HTTP 404 Not Found for {}",
            site.url("/nav/missing.html")
        )]
    );
    let (page, output) = ok(&engine, goto(&site.url("/nav/a.html?status=503"))).await;
    assert_eq!(page.status, Some(503));
    assert_eq!(page.title, "Page A");
    assert!(output.warnings[0].contains("HTTP 503 Service Unavailable"));
    engine.close().await;
}

#[tokio::test]
async fn failed_navigations_say_why() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    let port = closed_port();
    let http_port = site.a().rsplit_once(':').unwrap().1;
    let cases = [
        (format!("http://127.0.0.1:{port}/"), "connection_refused"),
        ("http://nope.invalid/".to_owned(), "dns"),
        (site.https().to_owned() + "/nav/a.html", "certificate"),
        (format!("https://127.0.0.1:{http_port}/"), "tls"),
        ("file:///nonexistent/page.html".to_owned(), "file_not_found"),
        ("http://127.0.0.1:6000/".to_owned(), "unsafe_port"),
    ];
    for (url, expected) in cases {
        let err = nav(&engine, goto(&url), WaitUntil::Load, TIMEOUT_MS)
            .await
            .unwrap_err();
        eprintln!("{}", err.to_text());
        assert_eq!(err.kind, ErrorKind::NavigationFailed, "{url}");
        assert_eq!(reason(&err), expected, "{url}");
        assert_eq!(err.details.as_ref().unwrap()["url"], json!(url));
    }
    let refused = nav(
        &engine,
        goto(&format!("127.0.0.1:{port}/app")),
        WaitUntil::Load,
        TIMEOUT_MS,
    )
    .await
    .unwrap_err();
    assert_eq!(
        refused.message,
        format!("connection refused on 127.0.0.1:{port}")
    );
    // The session is still usable.
    ok(&engine, goto(&site.url("/nav/a.html"))).await;
    engine.close().await;
}

#[tokio::test]
async fn ignore_https_errors_loads_a_self_signed_site() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = launch(
        &path,
        OpenRequest {
            ignore_https_errors: Some(true),
            ..OpenRequest::default()
        },
    )
    .await;
    let url = format!("{}/nav/a.html", site.https());
    let (page, _) = ok(&engine, goto(&url)).await;
    assert_eq!((page.url.as_str(), page.status), (url.as_str(), Some(200)));
    engine.close().await;
}

#[tokio::test]
async fn ignore_https_errors_applies_to_a_running_session() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    let url = format!("{}/nav/b.html", site.https());
    let err = nav(&engine, goto(&url), WaitUntil::Load, TIMEOUT_MS)
        .await
        .unwrap_err();
    assert_eq!(reason(&err), "certificate");
    // As the hint says: `open --ignore-https-errors`, then goto again.
    engine
        .open(OpenRequest {
            ignore_https_errors: Some(true),
            timeout_ms: TIMEOUT_MS,
            ..OpenRequest::default()
        })
        .await
        .unwrap();
    let (page, _) = ok(&engine, goto(&url)).await;
    assert_eq!(page.status, Some(200));
    engine.close().await;
}

#[tokio::test]
async fn relative_urls_resolve_against_base_url_or_the_page() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    let err = nav(&engine, goto("/nav/a.html"), WaitUntil::Load, TIMEOUT_MS)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadArgs, "{}", err.to_text());
    let (page, _) = ok(
        &engine,
        Navigation::Goto {
            url: "/nav/a.html".into(),
            base_url: Some(site.b().to_owned() + "/"),
        },
    )
    .await;
    assert_eq!(page.url, site.url_b("/nav/a.html"));
    let (page, _) = ok(&engine, goto("./b.html")).await;
    assert_eq!(page.url, site.url_b("/nav/b.html"));
    assert_eq!(page.title, "Page B");
    engine.close().await;
}

#[tokio::test]
async fn wait_conditions() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    let slow = site.url("/nav/slow-image.html?img_delay=20000");
    for wait in [
        WaitUntil::DomContentLoaded,
        WaitUntil::Commit,
        WaitUntil::None,
    ] {
        let output = nav(&engine, goto(&slow), wait, TIMEOUT_MS)
            .await
            .unwrap_or_else(|e| panic!("{wait:?}: {}", e.to_text()));
        let page = data(&output);
        assert_eq!(page.wait, wait.as_str());
        assert!(page.elapsed_ms < 10_000, "{wait:?}: {}", page.elapsed_ms);
    }
    let err = nav(&engine, goto(&slow), WaitUntil::Load, 1_500)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Timeout);
    assert_eq!(err.exit_code(), 3);
    assert_eq!(
        err.message,
        format!("{slow} did not reach the load event within 1.5s")
    );
    let text = err.to_text();
    assert!(
        text.contains("reached: commit, domcontentloaded\n"),
        "{text}"
    );
    assert!(
        text.contains(&format!("page: {slow} \"Slow image\"")),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "hint: chrome-cdp goto '{slow}' --wait domcontentloaded --session engine-navigate-test   (does not wait"
        )),
        "{text}"
    );
    let details = err.details.unwrap();
    assert_eq!(details["wait"], "load");
    assert_eq!(details["reached"], json!(["commit", "domcontentloaded"]));
    assert_eq!(details["page"]["title"], json!("Slow image"));

    // A server that answers late: `Page.navigate` itself times out.
    let late = site.url("/nav/a.html?delay=5000");
    let err = nav(&engine, goto(&late), WaitUntil::Load, 1_000)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Timeout, "{}", err.to_text());
    assert_eq!(err.message, format!("{late} did not answer within 1s"));
    engine.close().await;
}

#[tokio::test]
async fn back_and_forward_walk_the_history() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    let (a, b) = (site.url("/nav/a.html"), site.url("/nav/b.html"));
    ok(&engine, goto(&a)).await;
    ok(&engine, goto(&b)).await;
    let (forward, output) = ok(&engine, Navigation::Forward).await;
    assert!(!forward.navigated);
    assert_eq!(forward.url, b);
    assert_eq!(
        output.text,
        format!("no later page in tab t1's history to go forward to; still at {b} \"Page B\"")
    );
    let (back, _) = ok(&engine, Navigation::Back).await;
    assert!(back.navigated);
    assert_eq!(
        (back.url.as_str(), back.title.as_str()),
        (a.as_str(), "Page A")
    );
    assert_eq!(back.status, Some(200));
    let (forward, _) = ok(&engine, Navigation::Forward).await;
    assert_eq!(forward.url, b);
    // The tab started at about:blank.
    ok(&engine, Navigation::Back).await;
    let (blank, _) = ok(&engine, Navigation::Back).await;
    assert_eq!(blank.url, "about:blank");
    let (start, output) = ok(&engine, Navigation::Back).await;
    assert!(!start.navigated);
    assert!(
        output
            .text
            .starts_with("no earlier page in tab t1's history"),
        "{}",
        output.text
    );
    engine.close().await;
}

#[tokio::test]
async fn same_document_navigations_count() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    let spa = site.url("/nav/spa.html");
    ok(&engine, goto(&spa)).await;
    let (anchor, output) = ok(&engine, goto("#view")).await;
    assert!(anchor.same_document, "{anchor:?}");
    assert_eq!(anchor.url, format!("{spa}#view"));
    assert_eq!(anchor.status, None);
    assert!(output.text.contains("(same document, "), "{}", output.text);

    engine
        .evaluate(
            None,
            None,
            World::Main,
            "document.querySelector('[data-view=about]').click()",
            TIMEOUT,
        )
        .await
        .unwrap();
    let (back, _) = ok(&engine, Navigation::Back).await;
    assert!(back.same_document, "{back:?}");
    assert_eq!(back.url, format!("{spa}#view"));
    let (forward, _) = ok(&engine, Navigation::Forward).await;
    assert!(forward.same_document);
    assert_eq!(forward.url, format!("{spa}?view=about"));
    engine.close().await;
}

#[tokio::test]
async fn reload_loads_the_page_again() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    ok(&engine, goto(&site.url("/nav/a.html"))).await;
    engine
        .evaluate(None, None, World::Main, "window.marker = 1", TIMEOUT)
        .await
        .unwrap();
    for hard in [false, true] {
        let (page, _) = ok(&engine, Navigation::Reload { hard }).await;
        assert_eq!(page.url, site.url("/nav/a.html"));
        assert_eq!(page.status, Some(200));
        assert!(!page.same_document);
        let marker = engine
            .evaluate(None, None, World::Main, "typeof window.marker", TIMEOUT)
            .await
            .unwrap();
        assert_eq!(marker, json!("undefined"), "hard: {hard}");
    }
    engine.close().await;
}

/// Serves one page on a port until `stop` is called, then stops
/// listening. Every request gets the page, and a connection that sends
/// nothing (a preconnect) is let go, so the page load cannot miss it.
fn serve_until_stopped() -> (String, impl FnOnce()) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let server = std::thread::spawn({
        let stopped = stopped.clone();
        move || {
            for stream in listener.incoming() {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                // Chrome may preconnect a socket it never sends on; waiting
                // on it forever would hang `stop`.
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(1)));
                let mut buf = [0; 4096];
                if !matches!(stream.read(&mut buf), Ok(n) if n > 0) {
                    continue;
                }
                let body = "<title>Once</title>once";
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        }
    });
    let stop = move || {
        stopped.store(true, Ordering::SeqCst);
        // Wake the accept loop; it drops the listener as it ends.
        let _ = std::net::TcpStream::connect(addr);
        server.join().unwrap();
    };
    (format!("http://{addr}/"), stop)
}

#[tokio::test]
async fn a_reload_that_fails_says_why() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path).await;
    let (url, stop) = serve_until_stopped();
    let (page, _) = ok(&engine, goto(&url)).await;
    assert_eq!(page.title, "Once");
    stop();
    let err = nav(
        &engine,
        Navigation::Reload { hard: false },
        WaitUntil::Load,
        TIMEOUT_MS,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NavigationFailed, "{}", err.to_text());
    assert_eq!(reason(&err), "connection_refused");
    assert_eq!(err.details.unwrap()["url"], json!(url));
    engine.close().await;
}

#[tokio::test]
async fn a_client_side_redirect_is_followed() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    let (page, _) = ok(&engine, goto(&site.url("/nav/js-redirect.html"))).await;
    assert_eq!(page.url, site.url("/nav/b.html"), "{page:?}");
    assert_eq!(page.title, "Page B");
    engine.close().await;
}

/// The follow-up from milestone 3.2: a page restored from the
/// back/forward cache keeps its frames, cross-site ones included.
#[tokio::test]
async fn frames_survive_the_back_forward_cache() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    ok(&engine, goto(&site.url("/frames/nested.html"))).await;
    let loaded = |frames: &[engine::FrameInfo]| {
        frames.len() == 5 && frames.iter().all(|f| f.url.contains(".html"))
    };
    for _ in 0..100 {
        if loaded(&engine.frames(None, TIMEOUT).await.unwrap()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    ok(&engine, goto(&site.url("/nav/a.html"))).await;
    let (back, _) = ok(&engine, Navigation::Back).await;
    eprintln!("restored from the back/forward cache: {}", back.restored);
    let mut frames = Vec::new();
    for _ in 0..100 {
        frames = engine.frames(None, TIMEOUT).await.unwrap();
        if loaded(&frames) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let names: Vec<&str> = frames.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["", "local", "deep", "remote", "remote-leaf"]);
    let leaf = json!(format!("leaf on {}", site.b()));
    for (frame, expected) in [
        (None, json!("top")),
        (Some("local"), json!("local")),
        (Some("deep"), leaf.clone()),
        (Some("remote"), json!("remote")),
        (Some("remote-leaf"), leaf),
    ] {
        let value = engine
            .evaluate(None, frame, World::Main, "where", TIMEOUT)
            .await
            .unwrap_or_else(|e| panic!("{frame:?}: {}", e.to_text()));
        assert_eq!(value, expected, "{frame:?}");
        let isolated = engine
            .evaluate(None, frame, World::Isolated, "typeof where", TIMEOUT)
            .await
            .unwrap_or_else(|e| panic!("{frame:?}: {}", e.to_text()));
        assert_eq!(isolated, json!("undefined"), "{frame:?}");
    }
    engine.close().await;
}
