//! `record` (plan 18, milestone 18.6) end to end against a real Chrome
//! (`CHROME_PATH`): a fixture form driven headless through raw `cdp
//! Input.*` events (mouse, keys, `Input.insertText`) and a few chrome-cdp
//! commands, in the main frame and an iframe (same-process and
//! out-of-process); `record stop`'s test is compared with an expected file
//! (`tests/record/`), then passes with `test`: the round trip.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Output;

use common::{Sandbox, chrome_path, stderr, stdout};
use fixtures::Fixtures;
use serde_json::{Value, json};

/// A password typed while recording: it must appear nowhere.
const PASSWORD: &str = "hunter2-recorded-secret";

/// A project whose base_url is the fixture site.
struct Project {
    sandbox: Sandbox,
    root: PathBuf,
}

impl Project {
    fn new(chrome: &Path) -> Self {
        let sandbox = Sandbox::with_chrome(chrome);
        let root = sandbox.dir.path().join("app");
        std::fs::create_dir_all(root.join("scenarios")).unwrap();
        std::fs::write(
            root.join("chrome-cdp.toml"),
            format!(
                "[e2e]\nbase_url = \"{}\"\ntests = [\"scenarios/*.cdp\"]\n",
                Fixtures::shared().a()
            ),
        )
        .unwrap();
        Self { sandbox, root }
    }

    fn run(&self, args: &[&str]) -> Output {
        common::run(self.sandbox.cli().current_dir(&self.root).args(args))
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", stderr(&out));
        stdout(&out)
    }

    /// A raw CDP call to the active tab's page. A dialog the call opens
    /// makes it exit 5; that is what the test wants.
    fn cdp(&self, method: &str, params: &Value) {
        let out = self.run(&["cdp", "--target", "page", method, &params.to_string()]);
        let code = out.status.code();
        assert!(
            matches!(code, Some(0 | 5)),
            "{method} {params}: {}",
            stderr(&out)
        );
    }

    /// The middle of the element `locator` names, in the page's viewport.
    fn center(&self, locator: &str) -> (f64, f64) {
        let out = self.run(&["box", locator, "--json"]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "box {locator}: {}",
            stderr(&out)
        );
        let b = &common::json_out(&out)["data"]["box"];
        let at = |k: &str| b[k].as_f64().unwrap();
        (at("x") + at("width") / 2.0, at("y") + at("height") / 2.0)
    }

    /// A left click (or double click) of real mouse events.
    fn click(&self, locator: &str) {
        self.clicks(locator, 1);
    }

    fn clicks(&self, locator: &str, count: u32) {
        let (x, y) = self.center(locator);
        for n in 1..=count {
            for kind in ["mousePressed", "mouseReleased"] {
                let event =
                    json!({"type": kind, "x": x, "y": y, "button": "left", "clickCount": n});
                self.cdp("Input.dispatchMouseEvent", &event);
            }
        }
    }

    /// A key pressed and let go; `text` is what it types.
    fn key(&self, key: &str, code: &str, key_code: u32, text: Option<&str>) {
        let mut down =
            json!({"type": "keyDown", "key": key, "code": code, "windowsVirtualKeyCode": key_code});
        if let Some(text) = text {
            down["text"] = json!(text);
        }
        self.cdp("Input.dispatchKeyEvent", &down);
        let up =
            json!({"type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": key_code});
        self.cdp("Input.dispatchKeyEvent", &up);
    }

    /// Text, as an IME inserts it.
    fn type_text(&self, text: &str) {
        self.cdp("Input.insertText", &json!({ "text": text }));
    }

    /// Runs the project's tests, with the recorded password as its
    /// secret; asserts they pass and that the password shows nowhere.
    fn passes(&self) {
        let out = common::run(
            self.sandbox
                .cli()
                .current_dir(&self.root)
                .env("PASSWORD", PASSWORD)
                .args(["test"]),
        );
        let (out_text, err_text) = (stdout(&out), stderr(&out));
        assert_eq!(out.status.code(), Some(0), "{out_text}{err_text}");
        assert!(!out_text.contains(PASSWORD) && !err_text.contains(PASSWORD));
    }
}

/// The expected file `name` of `tests/record/`, with `{b}` for origin B.
fn expected(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/record")
        .join(name);
    std::fs::read_to_string(path)
        .unwrap()
        .replace("{b}", Fixtures::shared().b())
}

/// Fills the sign-up form of `record/form.html` (opened with `query`) in
/// the main frame and its iframe, records it, and replays it.
fn records_the_form(query: &str, expected_file: &str) {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let site = Fixtures::shared().a();
    std::fs::write(project.root.join("me.png"), b"not really a png").unwrap();

    project.ok(&["open", &format!("{site}/record/form.html{query}")]);
    project.ok(&["wait", "frame=card >> label=Card number"]);
    let started = project.ok(&["record", "start", "--name", "signs up"]);
    assert!(
        started.contains("recording \"signs up\" from tab t1"),
        "{started}"
    );

    project.click("label=Name");
    project.type_text("Ada");
    project.key("Tab", "Tab", 9, None);
    project.type_text("ada@example.com");
    project.key("Tab", "Tab", 9, None);
    project.type_text(PASSWORD);
    project.click("label=Bio");
    project.type_text("line one");
    project.key("Enter", "Enter", 13, Some("\r"));
    project.type_text("line two");
    project.click("label=Size");
    // Type-ahead picks "Medium" everywhere; ArrowDown would open the
    // list on macOS instead of choosing the next option.
    project.key("m", "KeyM", 77, Some("m"));
    project.click("text=Pro");
    project.click("label=I accept the terms");
    project.click("text=Send me news");
    // An agent's command records alike.
    project.ok(&["upload", "label=Photo", "me.png"]);
    project.click("frame=card >> label=Card number");
    project.type_text("4242");
    project.click("frame=card >> role=button[name=\"Pay\"]");
    project.click("role=button[name=\"Delete draft\"]");
    project.cdp("Page.handleJavaScriptDialog", &json!({"accept": true}));
    project.click("role=button[name=\"Sign up\"]");
    project.ok(&["expect", "role=status", "text", "me.png"]);

    let status = project.ok(&["record", "status", "--json"]);
    let status: Value = serde_json::from_str(&status).unwrap();
    assert_eq!(status["data"]["recording"], true);
    assert_eq!(status["data"]["steps"], 17, "{status}");

    let out = project.run(&["record", "stop", "-o", "scenarios/signup.cdp"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("appended \"signs up\" (17 steps) to "),
        "{}",
        stdout(&out)
    );
    let file = project.root.join("scenarios/signup.cdp");
    let recorded = std::fs::read_to_string(&file).unwrap();
    assert_eq!(recorded, expected(expected_file));
    assert!(!recorded.contains(PASSWORD));
    assert!(!stdout(&out).contains(PASSWORD) && !stderr(&out).contains(PASSWORD));
    let stopped = project.run(&["record", "stop"]);
    assert_eq!(stopped.status.code(), Some(2), "{}", stderr(&stopped));

    // The recorder adds no checks; the person (here, the test) does.
    std::fs::write(
        &file,
        format!(
            "{recorded}  expect role=status text 'Ada, ada@example.com, password {}, line one'\n  \
             expect role=status text 'line two, m, pro, terms, no news, me.png'\n  \
             expect 'frame=card >> role=status' text 'paid with 4242'\n",
            PASSWORD.len()
        ),
    )
    .unwrap();
    project.ok(&["close"]);
    project.passes();
}

#[test]
fn a_form_and_its_iframe_record_and_replay() {
    records_the_form("", "signup.cdp");
}

#[test]
fn an_out_of_process_iframe_records_and_replays() {
    let card = format!("?card={}/record/card.html", Fixtures::shared().b());
    records_the_form(&card, "signup-oopif.cdp");
}

#[test]
fn navigations_dialogs_double_clicks_and_keys_record_and_replay() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let site = Fixtures::shared().a();

    project.ok(&["open", &format!("{site}/record/form.html")]);
    project.ok(&["record", "start", "--name", "finds help"]);
    // A link the page follows is no step of its own; a URL typed is.
    project.click("role=link[name=\"Help\"]");
    project.ok(&["wait", "text=Back to the form"]);
    project.cdp(
        "Page.navigate",
        &json!({ "url": format!("{site}/record/form.html") }),
    );
    project.ok(&["wait", "label=Name"]);
    project.ok(&["back"]);
    project.ok(&["forward"]);
    project.ok(&["reload"]);
    project.clicks("text=Double-click to edit", 2);
    project.ok(&["expect", "role=status", "text", "editing"]);
    project.click("role=button[name=\"Pick a nickname\"]");
    project.ok(&["dialog", "accept", "Grace"]);
    project.click("role=button[name=\"Delete draft\"]");
    project.ok(&["dialog", "dismiss"]);
    project.click("label=Name");
    project.key("Escape", "Escape", 27, None);

    let out = project.run(&["record", "stop"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let recorded = stdout(&out);
    assert_eq!(recorded, expected("nav.cdp"));
    std::fs::write(
        project.root.join("scenarios/nav.cdp"),
        format!("{recorded}  expect role=status text 'draft kept'\n"),
    )
    .unwrap();
    project.ok(&["close"]);
    project.passes();
}

#[test]
fn a_new_tab_and_a_switch_back_are_tab_steps() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let site = Fixtures::shared().a();

    project.ok(&["open", &format!("{site}/record/form.html")]);
    project.ok(&["record", "start", "--name", "two tabs"]);
    project.ok(&["tab", "new", &format!("{site}/record/help.html")]);
    project.click("role=link[name=\"Back to the form\"]");
    project.ok(&["wait", "label=Name"]);
    project.ok(&["tab", "t1"]);
    project.click("label=I accept the terms");
    let out = project.run(&["record", "stop", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let data = common::json_out(&out)["data"].clone();
    assert_eq!(data["steps"], 5, "{data}");
    assert_eq!(
        data["scenario"],
        "@test \"two tabs\"\n  goto /record/form.html\n  tab new /record/help.html\n  click 'role=link[name=\"Back to the form\"]'\n  tab t1\n  check 'role=checkbox[name=\"I accept the terms\"]'\n  # add expect steps: the recorder does not guess what to check\n"
    );
}

#[test]
fn run_passes_a_step_whose_dialog_the_next_step_answers() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let site = Fixtures::shared().a();
    std::fs::write(
        project.root.join("flow.cdp"),
        format!(
            "goto {site}/record/form.html\nclick 'role=button[name=\"Delete draft\"]'\ndialog accept\nexpect role=status text 'draft deleted'\n"
        ),
    )
    .unwrap();
    let out = project.run(&["run", "flow.cdp"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(
        stdout(&out).contains(
            "dialog open in tab t1: confirm \"Delete the draft?\"; the next step answers it"
        ),
        "{}",
        stdout(&out)
    );
    // Not when nothing answers it.
    std::fs::write(
        project.root.join("flow.cdp"),
        "click 'role=button[name=\"Delete draft\"]'\n",
    )
    .unwrap();
    let out = project.run(&["run", "flow.cdp"]);
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
}
