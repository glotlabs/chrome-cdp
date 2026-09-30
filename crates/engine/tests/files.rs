//! `upload` and `clipboard` against a real Chrome (`CHROME_PATH`; skipped
//! without it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture
//! site's `actions/` pages (plan 05, milestone 5.4).
//!
//! The files fixture logs every input and change event of its file
//! inputs (the files by name and size, and whether the event is
//! trusted); the drop fixture logs the drag events of its drop zone
//! the same way (the files of `dataTransfer.files`); the clipboard fixture copies with the page's own button and
//! logs what a paste into its field carries.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use browser::Host;
use engine::{
    Action, ActionRequest, ClipboardOp, ClipboardRequest, Engine, LocateOptions, NavigateRequest,
    Navigation, OpenRequest, WaitUntil,
};
use fixtures::Fixtures;
use protocol::{ErrorInfo, ErrorKind, Output};
use serde_json::{Value, json};

const TIMEOUT_MS: u64 = 30_000;
const TIMEOUT: Duration = Duration::from_millis(TIMEOUT_MS);

/// How long a check that never passes is given.
const SHORT: Duration = Duration::from_millis(1200);

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

async fn act_within(
    engine: &Engine,
    action: Action,
    timeout: Duration,
) -> Result<Output, ErrorInfo> {
    engine
        .act(ActionRequest {
            action,
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap(),
            no_wait_after: false,
            snap: false,
            max_chars: 0,
        })
        .await
}

async fn act(engine: &Engine, action: Action) -> Output {
    let what = format!("{action:?}");
    act_within(engine, action, TIMEOUT)
        .await
        .unwrap_or_else(|e| panic!("{what}: {}", e.to_text()))
}

async fn fails(engine: &Engine, action: Action) -> ErrorInfo {
    let what = format!("{action:?}");
    match act_within(engine, action, SHORT).await {
        Ok(out) => panic!("{what} passed: {}", out.text),
        Err(err) => err,
    }
}

/// Calls `function` on the one element `locator` matches.
async fn call(engine: &Engine, locator: &str, function: &str) -> Value {
    engine
        .call_on_locator(None, locator, &LocateOptions::default(), function, TIMEOUT)
        .await
        .unwrap_or_else(|e| panic!("{locator}: {}", e.to_text()))
        .value
}

async fn text(engine: &Engine, locator: &str) -> String {
    call(engine, locator, "function () { return this.textContent; }")
        .await
        .as_str()
        .unwrap()
        .to_owned()
}

/// The lines of `#files` (the file inputs' event log) after the first
/// `before`.
async fn events(engine: &Engine, frame: &str, before: usize) -> Vec<String> {
    text(engine, &format!("{frame}#files"))
        .await
        .lines()
        .skip(before)
        .map(str::to_owned)
        .collect()
}

/// The names of the files the input `locator` holds.
async fn held(engine: &Engine, locator: &str) -> Vec<String> {
    let names = call(
        engine,
        locator,
        "function () { return [...this.files].map((f) => f.name); }",
    )
    .await;
    serde_json::from_value(names).unwrap()
}

/// The upload fixture files, by name.
fn file(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/site/actions/upload")
        .join(name)
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn upload(locator: &str, files: &[&str]) -> Action {
    Action::Upload {
        locator: locator.to_owned(),
        files: files.iter().map(|f| file(f)).collect(),
        clear: false,
        force: false,
        drop: false,
    }
}

fn drop(locator: &str, files: &[&str]) -> Action {
    Action::Upload {
        locator: locator.to_owned(),
        files: files.iter().map(|f| file(f)).collect(),
        clear: false,
        force: false,
        drop: true,
    }
}

/// The lines of `#drops` (the drop zone's event log).
async fn drops(engine: &Engine, frame: &str) -> Vec<String> {
    text(engine, &format!("{frame}#drops"))
        .await
        .lines()
        .map(str::to_owned)
        .collect()
}

fn clear(locator: &str) -> Action {
    Action::Upload {
        locator: locator.to_owned(),
        files: Vec::new(),
        clear: true,
        force: false,
        drop: false,
    }
}

async fn clipboard(engine: &Engine, op: ClipboardOp) -> Result<Output, ErrorInfo> {
    engine
        .clipboard(ClipboardRequest {
            op,
            tab: None,
            timeout_ms: TIMEOUT_MS,
            max_chars: 20_000,
        })
        .await
}

#[tokio::test]
async fn uploads_into_file_inputs() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "files-input").await;
    goto(&engine, &site.url("/actions/files.html")).await;

    let out = act(&engine, upload("#avatar", &["hello.txt"])).await;
    assert_eq!(out.text, "uploaded hello.txt (6B) to e1 button \"Avatar\"");
    // The avatar input only accepts images: the page may ignore text.
    assert_eq!(
        out.warnings,
        ["the input accepts image/*; the page may refuse hello.txt"]
    );
    assert_eq!(out.data["chooser"], json!(false));
    assert_eq!(out.data["files"][0]["path"], json!(file("hello.txt")));
    assert_eq!(out.data["files"][0]["size"], json!(6));
    assert_eq!(
        events(&engine, "", 0).await,
        [
            "input avatar hello.txt(6) trusted",
            "change avatar hello.txt(6) trusted",
        ]
    );

    // A label names its input; a multiple input takes several files.
    let out = act(
        &engine,
        upload("label=Attachments", &["hello.txt", "notes.md"]),
    )
    .await;
    assert_eq!(
        out.text,
        "uploaded 2 files to e2 button \"Attachments\": hello.txt (6B), notes.md (31B)"
    );
    assert_eq!(
        held(&engine, "#attachments").await,
        ["hello.txt", "notes.md"]
    );

    // A hidden input takes files straight away, as Playwright's does.
    act(&engine, upload("#hidden", &["notes.md"])).await;
    assert_eq!(held(&engine, "#hidden").await, ["notes.md"]);

    // A single input refuses two files, before touching them.
    let err = fails(&engine, upload("#avatar", &["hello.txt", "notes.md"])).await;
    assert_eq!(err.kind, ErrorKind::BadArgs);
    assert_eq!(err.message, "e1 button \"Avatar\" takes one file, not 2");
    assert_eq!(held(&engine, "#avatar").await, ["hello.txt"]);

    // --clear empties it, with the events a page listens for.
    let before = events(&engine, "", 0).await.len();
    let out = act(&engine, clear("#avatar")).await;
    assert_eq!(out.text, "cleared the files of e1 button \"Avatar\"");
    assert_eq!(out.data["files"], json!([]));
    assert!(held(&engine, "#avatar").await.is_empty());
    assert_eq!(
        events(&engine, "", before).await,
        ["input avatar - synthetic", "change avatar - synthetic"]
    );
    let out = act(&engine, clear("#avatar")).await;
    assert_eq!(out.text, "e1 button \"Avatar\" had no files");

    // A disabled input is waited for, then refused; --force sets it.
    let err = fails(&engine, upload("#locked", &["hello.txt"])).await;
    assert_eq!(err.kind, ErrorKind::NotEnabled);
    assert_eq!(err.message, "e4 button \"Locked\" is disabled");
    assert!(held(&engine, "#locked").await.is_empty());
    let forced = Action::Upload {
        locator: "#locked".to_owned(),
        files: vec![file("hello.txt")],
        clear: false,
        force: true,
        drop: false,
    };
    act(&engine, forced).await;
    assert_eq!(held(&engine, "#locked").await, ["hello.txt"]);
}

#[tokio::test]
async fn uploads_through_file_choosers() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "files-chooser").await;
    goto(&engine, &site.url("/actions/files.html")).await;

    let out = act(&engine, upload("#choose", &["hello.txt", "notes.md"])).await;
    assert_eq!(
        out.text,
        "uploaded 2 files through the file chooser of e1 button \"Choose files\": \
         hello.txt (6B), notes.md (31B)"
    );
    assert_eq!(out.data["chooser"], json!(true));
    assert_eq!(held(&engine, "#hidden").await, ["hello.txt", "notes.md"]);
    assert_eq!(
        events(&engine, "", 0).await,
        [
            "input hidden hello.txt(6) notes.md(31) trusted",
            "change hidden hello.txt(6) notes.md(31) trusted",
        ]
    );
    // The click was a real one (and the page clicked its input).
    assert_eq!(text(&engine, "#log").await, "hidden choose");

    // --clear goes through the chooser too.
    let out = act(&engine, clear("#choose")).await;
    assert_eq!(out.text, "cleared the files of e1 button \"Choose files\"");
    assert!(held(&engine, "#hidden").await.is_empty());

    // A chooser for one file refuses two.
    let err = fails(&engine, upload("#choose-one", &["hello.txt", "notes.md"])).await;
    assert_eq!(err.kind, ErrorKind::BadArgs);
    assert_eq!(
        err.message,
        "e2 button \"Choose one file\" takes one file, not 2"
    );
    act(&engine, upload("#choose-one", &["notes.md"])).await;
    assert_eq!(held(&engine, "#hidden-one").await, ["notes.md"]);

    // A click that opens nothing says so, and points at the file input.
    let err = fails(&engine, upload("#nothing", &["hello.txt"])).await;
    assert_eq!(err.kind, ErrorKind::NotEnabled);
    assert_eq!(
        err.message,
        "clicking e3 button \"Opens nothing\" opened no file chooser, and it is not a file input"
    );
    assert_eq!(
        err.hint.as_deref(),
        Some(
            "chrome-cdp upload 'css=input[type=file]' <file> --session files-chooser   \
             (the page's file input, even a hidden one)"
        )
    );

    // Interception was turned off after each chooser, and on again for
    // the next.
    let out = act(&engine, upload("#choose", &["hello.txt"])).await;
    assert_eq!(out.data["chooser"], json!(true));
}

#[tokio::test]
async fn uploads_in_frames() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "files-frames").await;
    goto(&engine, &site.url("/actions/files-frames.html")).await;

    for frame in ["frame=same >> ", "frame=other >> "] {
        act(&engine, upload(&format!("{frame}#avatar"), &["hello.txt"])).await;
        assert_eq!(
            held(&engine, &format!("{frame}#avatar")).await,
            ["hello.txt"],
            "{frame}"
        );
        let out = act(&engine, upload(&format!("{frame}#choose"), &["notes.md"])).await;
        assert_eq!(out.data["chooser"], json!(true), "{frame}");
        assert_eq!(
            held(&engine, &format!("{frame}#hidden")).await,
            ["notes.md"],
            "{frame}"
        );
        assert_eq!(
            events(&engine, frame, 0).await,
            [
                "input avatar hello.txt(6) trusted",
                "change avatar hello.txt(6) trusted",
                "input hidden notes.md(31) trusted",
                "change hidden notes.md(31) trusted",
            ],
            "{frame}"
        );
    }
}

#[tokio::test]
async fn drops_files_on_drop_zones() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "files-drop").await;
    goto(&engine, &site.url("/actions/drop.html")).await;

    let out = act(&engine, drop("#zone", &["hello.txt", "notes.md"])).await;
    assert_eq!(
        out.text,
        "dropped 2 files on e1 region \"Drop files here\": hello.txt (6B), notes.md (31B)"
    );
    assert_eq!(out.data["drop"], json!(true));
    assert_eq!(out.data["chooser"], json!(false));
    assert_eq!(out.data["files"][1]["name"], json!("notes.md"));
    // The page read the files from the drop's dataTransfer, and the
    // events were trusted, as a user's drag from the desktop.
    assert_eq!(
        drops(&engine, "").await,
        [
            "dragenter zone - trusted",
            "drop zone hello.txt(6) notes.md(31) trusted",
        ]
    );

    // A zone that does not cancel dragover takes no drop: no drop event,
    // and upload says so.
    let err = fails(&engine, drop("#inert", &["hello.txt"])).await;
    assert_eq!(err.kind, ErrorKind::NotEnabled);
    assert_eq!(
        err.message,
        "e2 region \"Not a drop zone\" took no drop: the page did not accept files dropped there"
    );
    assert_eq!(err.details.as_ref().unwrap()["check"], json!("drop"));
    assert_eq!(drops(&engine, "").await.len(), 2);

    // A drop an overlay takes in the element's place counts.
    let out = act(&engine, drop("#comment", &["hello.txt"])).await;
    assert_eq!(out.text, "dropped hello.txt (6B) on e3 region \"Comment\"");
    assert_eq!(
        drops(&engine, "").await[2],
        "drop overlay hello.txt(6) trusted"
    );

    // The refused drag ended: the next drop works, and so does a click.
    act(&engine, drop("#zone", &["notes.md"])).await;
    assert_eq!(
        drops(&engine, "").await[4],
        "drop zone notes.md(31) trusted"
    );
    act(
        &engine,
        Action::Click {
            locator: "#zone".to_owned(),
            button: engine::Button::Left,
            count: 1,
            modifiers: Vec::new(),
            position: None,
            force: false,
            dblclick: false,
        },
    )
    .await;
}

#[tokio::test]
async fn drops_files_in_frames() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "files-drop-frames").await;
    goto(&engine, &site.url("/actions/drop-frames.html")).await;

    for frame in ["frame=same >> ", "frame=other >> "] {
        let out = act(&engine, drop(&format!("{frame}#zone"), &["hello.txt"])).await;
        assert_eq!(out.data["drop"], json!(true), "{frame}");
        assert_eq!(
            drops(&engine, frame).await,
            ["dragenter zone - trusted", "drop zone hello.txt(6) trusted",],
            "{frame}"
        );
        let err = fails(&engine, drop(&format!("{frame}#inert"), &["hello.txt"])).await;
        assert_eq!(err.kind, ErrorKind::NotEnabled, "{frame}");
    }
}

#[tokio::test]
async fn reads_and_writes_the_clipboard() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "clipboard").await;

    // about:blank has no origin to grant the clipboard to.
    let err = clipboard(&engine, ClipboardOp::read_text())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotEnabled);
    assert_eq!(
        err.message,
        "the page in tab t1 cannot use the clipboard: it has no origin"
    );

    goto(&engine, &site.url("/actions/clipboard.html")).await;
    let out = clipboard(&engine, ClipboardOp::write_text("pasted ✓\nline 2"))
        .await
        .unwrap();
    assert_eq!(out.text, "wrote 15 characters to the clipboard");
    assert_eq!(out.data["chars"], json!(15));

    // What was written pastes with the keyboard, as a user's copy does.
    act(
        &engine,
        Action::Press {
            keys: "Mod+V".to_owned(),
            locator: Some("#field".to_owned()),
        },
    )
    .await;
    assert_eq!(
        text(&engine, "#paste-log").await,
        "paste \"pasted ✓\\nline 2\"\n"
    );
    let out = clipboard(&engine, ClipboardOp::read_text()).await.unwrap();
    assert_eq!(out.text, "pasted ✓\nline 2");

    // What the page's own Copy button copied can be read back.
    act(
        &engine,
        Action::Click {
            locator: "#copy".to_owned(),
            button: engine::Button::Left,
            count: 1,
            modifiers: Vec::new(),
            position: None,
            force: false,
            dblclick: false,
        },
    )
    .await;
    let mut read = String::new();
    for _ in 0..40 {
        read = clipboard(&engine, ClipboardOp::read_text())
            .await
            .unwrap()
            .text;
        if read == "INVITE-4821" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(read, "INVITE-4821");

    // The page keeps the permission: its own Paste button works now.
    act(
        &engine,
        Action::Click {
            locator: "#paste".to_owned(),
            button: engine::Button::Left,
            count: 1,
            modifiers: Vec::new(),
            position: None,
            force: false,
            dblclick: false,
        },
    )
    .await;
    let mut pasted = String::new();
    for _ in 0..40 {
        pasted = text(&engine, "#pasted").await;
        if pasted == "INVITE-4821" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(pasted, "INVITE-4821");

    // An empty clipboard reads as nothing, with a warning.
    clipboard(&engine, ClipboardOp::write_text(""))
        .await
        .unwrap();
    let out = clipboard(&engine, ClipboardOp::read_text()).await.unwrap();
    assert_eq!(out.text, "");
    assert_eq!(out.warnings, ["the clipboard holds no text"]);
}

/// Clicks `locator` with the left button.
async fn click(engine: &Engine, locator: &str) {
    act(
        engine,
        Action::Click {
            locator: locator.to_owned(),
            button: engine::Button::Left,
            count: 1,
            modifiers: Vec::new(),
            position: None,
            force: false,
            dblclick: false,
        },
    )
    .await;
}

fn read_type(mime: &str, path: Option<PathBuf>) -> ClipboardOp {
    ClipboardOp::Read {
        mime: Some(mime.to_owned()),
        path,
    }
}

#[tokio::test]
async fn reads_and_writes_html_and_images() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "clipboard-rich").await;
    goto(&engine, &site.url("/actions/clipboard.html")).await;
    let dir = tempfile::tempdir().unwrap();

    // HTML goes with its text, which pastes as a user's rich copy does.
    let out = clipboard(
        &engine,
        ClipboardOp::Write {
            text: None,
            html: Some("<p>Hello <b>world</b></p>".to_owned()),
            image: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        out.text,
        "wrote text/html (25B), text/plain (11 characters) to the clipboard"
    );
    assert_eq!(out.data["types"], json!(["text/html", "text/plain"]));
    assert_eq!(out.data["chars"], json!(11));
    let out = clipboard(&engine, read_type("text/html", None))
        .await
        .unwrap();
    assert!(
        out.text.contains("<p>Hello <b>world</b></p>"),
        "{}",
        out.text
    );
    assert_eq!(out.data["type"], json!("text/html"));
    let out = clipboard(&engine, ClipboardOp::read_text()).await.unwrap();
    assert_eq!(out.text, "Hello world");
    act(
        &engine,
        Action::Press {
            keys: "Mod+V".to_owned(),
            locator: Some("#field".to_owned()),
        },
    )
    .await;
    assert!(
        text(&engine, "#html-log")
            .await
            .contains("<p>Hello <b>world</b></p>")
    );

    // No image on it: the error lists what there is.
    let err = clipboard(&engine, read_type("image/png", None))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert_eq!(
        err.message,
        "the clipboard holds no image/png: the clipboard has text/plain, text/html"
    );

    // A PNG goes on as image/png, and comes back as a file.
    let out = clipboard(
        &engine,
        ClipboardOp::Write {
            text: None,
            html: None,
            image: Some(PathBuf::from(file("dot.png"))),
        },
    )
    .await
    .unwrap();
    assert_eq!(out.text, "wrote image/png (2x2, 74B) to the clipboard");
    let saved = dir.path().join("dot.png");
    let out = clipboard(&engine, read_type("image/png", Some(saved.clone())))
        .await
        .unwrap();
    let bytes = std::fs::read(&saved).unwrap();
    assert!(bytes.starts_with(b"\x89PNG"));
    assert_eq!(out.data["path"], json!(saved.display().to_string()));
    assert_eq!(out.data["bytes"], json!(bytes.len()));
    assert!(out.text.contains(": image/png 2x2, "), "{}", out.text);

    // Only PNG goes on the clipboard.
    let err = clipboard(
        &engine,
        ClipboardOp::Write {
            text: None,
            html: None,
            image: Some(PathBuf::from(file("hello.txt"))),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadArgs);
    assert!(
        err.message
            .ends_with("is not a PNG: the clipboard takes image/png only")
    );

    // What the page copies as a ClipboardItem reads back by type.
    click(&engine, "#copy-rich").await;
    let mut read = String::new();
    for _ in 0..40 {
        read = clipboard(&engine, ClipboardOp::read_text())
            .await
            .unwrap()
            .text;
        if read == "Bold move" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(read, "Bold move");
    let out = clipboard(&engine, read_type("text/html", None))
        .await
        .unwrap();
    assert!(out.text.contains("<b>Bold</b> move"), "{}", out.text);

    click(&engine, "#copy-image").await;
    let mut out = None;
    for _ in 0..40 {
        if let Ok(read) = clipboard(&engine, read_type("image/png", None)).await {
            out = Some(read);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let out = out.expect("the page's image reached the clipboard");
    // Without -o, it is saved in the session's clipboard/ directory.
    let path = out.data["path"].as_str().unwrap();
    assert!(path.contains("/clipboard/"), "{path}");
    assert!(out.text.contains(": image/png 3x1, "), "{}", out.text);
    std::fs::remove_file(path).unwrap();
    engine.close().await;
}
