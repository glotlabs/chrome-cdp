//! Frames and execution contexts against a real Chrome (`CHROME_PATH`;
//! skipped without it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the
//! fixture site's `frames/nested.html`:
//!
//! ```text
//! nested.html (A)
//! ├── local: local.html (A)
//! │   └── deep: leaf.html (B, an OOPIF)
//! └── remote: remote.html (B, an OOPIF)
//!     └── remote-leaf: leaf.html (B, in remote's process)
//! ```

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::Host;
use engine::{CdpRequest, Engine, FrameInfo, OpenRequest, Target, World};
use fixtures::Fixtures;
use protocol::ErrorKind;
use serde_json::{Value, json};

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

async fn open(path: &std::path::Path, url: String) -> Arc<Engine> {
    let engine = Engine::new("engine-frames-test", None, None);
    let chrome = browser::discover(Some(path), &Host::current()).unwrap();
    engine
        .open(OpenRequest {
            chrome: Some(chrome),
            url: Some(url),
            timeout_ms: TIMEOUT_MS,
            ..OpenRequest::default()
        })
        .await
        .unwrap();
    engine
}

/// Polls `check` until it returns a value, for up to 10 s.
async fn eventually<T>(what: &str, mut check: impl AsyncFnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(value) = check().await {
            return value;
        }
        assert!(start.elapsed() < TIMEOUT, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Waits until the active tab has `count` frames, all with a fixture page
/// loaded.
async fn frames_loaded(engine: &Engine, count: usize) -> Vec<FrameInfo> {
    eventually(&format!("{count} frames"), async || {
        let frames = engine.frames(None, TIMEOUT).await.unwrap();
        let loaded = frames.len() == count && frames.iter().all(|f| f.url.contains(".html"));
        loaded.then_some(frames)
    })
    .await
}

async fn eval(engine: &Engine, frame: Option<&str>, world: World, js: &str) -> Value {
    engine
        .evaluate(None, frame, world, js, TIMEOUT)
        .await
        .unwrap_or_else(|e| panic!("{js} in {frame:?}: {}", e.to_text()))
}

#[tokio::test]
async fn the_frame_tree_has_same_and_cross_site_frames() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = open(&path, site.url("/frames/nested.html")).await;
    let frames = frames_loaded(&engine, 5).await;
    let shape: Vec<(&str, usize, bool, &str)> = frames
        .iter()
        .map(|f| (f.name.as_str(), f.depth, f.oopif, f.url.as_str()))
        .collect();
    let (local, leaf_b) = (
        site.url("/frames/local.html"),
        site.url_b("/frames/leaf.html"),
    );
    let (nested, remote) = (
        site.url("/frames/nested.html"),
        site.url_b("/frames/remote.html"),
    );
    assert_eq!(
        shape,
        [
            ("", 0, false, nested.as_str()),
            ("local", 1, false, local.as_str()),
            ("deep", 2, true, leaf_b.as_str()),
            ("remote", 1, true, remote.as_str()),
            ("remote-leaf", 2, true, leaf_b.as_str()),
        ]
    );
    assert_eq!(frames[0].parent, None);
    assert_eq!(frames[2].parent.as_ref(), Some(&frames[1].id));
    assert_eq!(frames[4].parent.as_ref(), Some(&frames[3].id));
    engine.close().await;
}

#[tokio::test]
async fn frames_are_addressable_by_name_url_or_id() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = open(&path, site.url("/frames/nested.html")).await;
    let frames = frames_loaded(&engine, 5).await;
    let leaf_b = format!("leaf on {}", site.b());
    let main = World::Main;
    assert_eq!(eval(&engine, None, main, "where").await, json!("top"));
    assert_eq!(
        eval(&engine, Some("local"), main, "where").await,
        json!("local")
    );
    // Cross-site frames run in their own process and session.
    assert_eq!(
        eval(&engine, Some("remote"), main, "where").await,
        json!("remote")
    );
    assert_eq!(
        eval(&engine, Some("deep"), main, "where").await,
        json!(leaf_b)
    );
    assert_eq!(
        eval(&engine, Some("remote-leaf"), main, "where").await,
        json!(leaf_b)
    );
    assert_eq!(
        eval(&engine, Some("remote.html"), main, "where").await,
        json!("remote")
    );
    assert_eq!(
        eval(&engine, Some(&frames[2].id), main, "where").await,
        json!(leaf_b)
    );
    // Promises are awaited.
    let answer = eval(&engine, Some("deep"), main, "Promise.resolve(6 * 7)").await;
    assert_eq!(answer, json!(42));

    let err = engine
        .evaluate(None, Some("leaf.html"), main, "1", TIMEOUT)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Ambiguous, "{}", err.to_text());
    let names: Vec<&str> = err.details.as_ref().unwrap()["frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["deep", "remote-leaf"]);

    let err = engine
        .evaluate(None, Some("nowhere"), main, "1", TIMEOUT)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    let text = err.to_text();
    assert!(
        text.contains("no frame in tab t1 matches \"nowhere\""),
        "{text}"
    );
    assert!(
        text.contains("frame: name \"remote\" http://localhost:"),
        "{text}"
    );

    let err = engine
        .evaluate(None, Some("remote"), main, "missing.call()", TIMEOUT)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::EvalError);
    assert!(
        err.message.starts_with("ReferenceError: missing"),
        "{}",
        err.message
    );
    engine.close().await;
}

#[tokio::test]
async fn the_isolated_world_is_out_of_the_pages_reach() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = open(&path, site.url("/frames/nested.html")).await;
    frames_loaded(&engine, 5).await;
    let (main, ours) = (World::Main, World::Isolated);
    for frame in [None, Some("local"), Some("remote"), Some("deep")] {
        // The same DOM, other globals, both ways.
        let title = eval(&engine, frame, main, "document.title").await;
        assert_eq!(eval(&engine, frame, ours, "document.title").await, title);
        assert_eq!(
            eval(&engine, frame, ours, "typeof where").await,
            json!("undefined")
        );
        eval(&engine, frame, ours, "window.__ours = 1").await;
        assert_eq!(
            eval(&engine, frame, main, "typeof __ours").await,
            json!("undefined")
        );
        assert_eq!(eval(&engine, frame, ours, "__ours").await, json!(1));
        // The page's changes to built-ins do not reach it.
        eval(
            &engine,
            frame,
            main,
            "document.querySelector = () => 'tampered'",
        )
        .await;
        let found = eval(
            &engine,
            frame,
            ours,
            "document.querySelector('p').localName",
        )
        .await;
        assert_eq!(found, json!("p"));
    }

    // A new document gets a new isolated world.
    eval(&engine, None, main, "location.reload()").await;
    eventually("the reloaded page", async || {
        let fresh = engine
            .evaluate(None, None, ours, "typeof __ours", TIMEOUT)
            .await
            .ok()?;
        (fresh == json!("undefined")).then_some(())
    })
    .await;
    frames_loaded(&engine, 5).await;
    let fresh = eval(&engine, Some("remote"), ours, "typeof __ours").await;
    assert_eq!(fresh, json!("undefined"));
    engine.close().await;
}

#[tokio::test]
async fn frames_follow_navigation_and_removal() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = open(&path, site.url("/frames/nested.html")).await;
    frames_loaded(&engine, 5).await;

    // The cross-site frame comes back into the page's process.
    let js = "document.querySelector('iframe[name=remote]').src = '/frames/leaf.html'";
    eval(&engine, None, World::Main, js).await;
    let leaf_a = site.url("/frames/leaf.html");
    let frames = eventually("the remote frame to navigate", async || {
        let frames = engine.frames(None, TIMEOUT).await.unwrap();
        let remote = frames.iter().find(|f| f.name == "remote")?;
        (remote.url == leaf_a && !remote.oopif && frames.len() == 4).then_some(frames)
    })
    .await;
    assert!(frames.iter().all(|f| f.name != "remote-leaf"), "{frames:?}");
    let answer = format!("leaf on {}", site.a());
    for world in [World::Main, World::Isolated] {
        let found = eval(&engine, Some("remote"), world, "document.title").await;
        assert_eq!(found, json!("Leaf frame"));
    }
    assert_eq!(
        eval(&engine, Some("remote"), World::Main, "where").await,
        json!(answer)
    );

    // A same-document navigation updates the URL.
    eval(
        &engine,
        Some("local"),
        World::Main,
        "history.pushState({}, '', 'moved.html')",
    )
    .await;
    eventually("the pushState URL", async || {
        let frames = engine.frames(None, TIMEOUT).await.unwrap();
        let moved = site.url("/frames/moved.html");
        frames
            .iter()
            .any(|f| f.name == "local" && f.url == moved)
            .then_some(())
    })
    .await;

    // Removing a frame removes its subtree, OOPIFs included.
    eval(
        &engine,
        None,
        World::Main,
        "document.querySelector('iframe[name=local]').remove()",
    )
    .await;
    let frames = eventually("the local frame to go", async || {
        let frames = engine.frames(None, TIMEOUT).await.unwrap();
        (frames.len() == 2).then_some(frames)
    })
    .await;
    let names: Vec<&str> = frames.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["", "remote"]);
    let err = engine
        .evaluate(None, Some("deep"), World::Main, "1", TIMEOUT)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    engine.close().await;
}

#[tokio::test]
async fn cdp_calls_reach_a_frame_and_its_main_world() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = open(&path, site.url("/frames/nested.html")).await;
    frames_loaded(&engine, 5).await;
    let cdp = async |method: &str, params: Value, frame: &str| {
        engine
            .cdp(CdpRequest {
                method: Some(method.into()),
                params: params.as_object().unwrap().clone(),
                target: Target::Page,
                tab: None,
                frame: Some(frame.into()),
                listen: Vec::new(),
                count: 0,
                timeout_ms: TIMEOUT_MS,
                max_chars: 0,
            })
            .await
            .unwrap()
            .data
    };
    let params = json!({ "expression": "where", "returnByValue": true });
    let result = cdp("Runtime.evaluate", params, "remote").await;
    assert_eq!(result["result"]["value"], json!("remote"));
    // Other calls go to the session that owns the frame: the OOPIF's own.
    let tree = cdp("Page.getFrameTree", json!({}), "deep").await;
    let url = tree["frameTree"]["frame"]["url"].as_str().unwrap();
    assert_eq!(url, site.url_b("/frames/leaf.html"));
    engine.close().await;
}

/// A Chrome this test started; killed (and its profile removed) on drop,
/// so a failing assertion does not leave it running.
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
fn debug_chrome(path: &std::path::Path, url: &str) -> (DebugChrome, String) {
    let profile = std::env::temp_dir().join(format!("engine-frames-{}", std::process::id()));
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
        assert!(start.elapsed() < TIMEOUT, "no DevTools port");
        std::thread::sleep(Duration::from_millis(50));
    };
    (chrome, port)
}

async fn connect(port: &str) -> Arc<Engine> {
    let engine = Engine::new("engine-frames-connect", None, None);
    engine
        .connect(engine::ConnectRequest {
            endpoint: port.to_owned(),
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
    engine
}

#[tokio::test]
async fn frames_that_existed_before_connecting_are_found() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let (chrome, port) = debug_chrome(&path, &site.url("/frames/nested.html"));
    // Let the page and all its frames load while nobody is attached.
    let first = connect(&port).await;
    frames_loaded(&first, 5).await;
    first.close().await;
    drop(first);

    let engine = connect(&port).await;
    let frames = frames_loaded(&engine, 5).await;
    // Siblings are listed in the order they were found, and on connect
    // the cross-site frame's session may report before `getFrameTree`
    // (which leaves it out) answers; the shape is fixed.
    let mut shape: Vec<(&str, usize, Option<&str>)> = frames
        .iter()
        .map(|f| {
            let parent = f.parent.as_deref().and_then(|p| {
                frames
                    .iter()
                    .find(|other| other.id == p)
                    .map(|other| other.name.as_str())
            });
            (f.name.as_str(), f.depth, parent)
        })
        .collect();
    shape.sort_unstable();
    assert_eq!(
        shape,
        [
            ("", 0, None),
            ("deep", 2, Some("local")),
            ("local", 1, Some("")),
            ("remote", 1, Some("")),
            ("remote-leaf", 2, Some("remote")),
        ]
    );
    let leaf_b = json!(format!("leaf on {}", site.b()));
    assert_eq!(
        eval(&engine, Some("deep"), World::Main, "where").await,
        leaf_b
    );
    for frame in [None, Some("local"), Some("remote")] {
        let ours = eval(&engine, frame, World::Isolated, "typeof where").await;
        assert_eq!(ours, json!("undefined"), "{frame:?}");
        let title = eval(&engine, frame, World::Isolated, "document.title").await;
        assert_ne!(title, json!(""), "{frame:?}");
    }
    engine.close().await;
    drop(chrome);
}
