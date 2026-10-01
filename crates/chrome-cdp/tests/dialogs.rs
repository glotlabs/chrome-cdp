//! JavaScript dialogs (plan 03, milestone 3.5) end to end: the default
//! policy, a command interrupted by a dialog (exit 5), `dialog accept` and
//! `dismiss`, prompts, `beforeunload`, `tabs`, and `open --dialogs`.
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::io::Write as _;
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::{Sandbox, chrome_path, code, json_out, stderr, stdout};
use fixtures::Fixtures;

fn on_load(kind: &str, message: &str) -> String {
    Fixtures::shared().url(&format!(
        "/dialogs/on-load.html?kind={kind}&message={message}"
    ))
}

#[test]
fn a_confirm_interrupts_the_command_until_answered() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);

    // An alert is accepted, and the command says so.
    let out = sandbox.run(&["goto", &on_load("alert", "Saved"), "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let json = json_out(&out);
    assert_eq!(json["events"][0]["type"], "dialog");
    assert_eq!(json["events"][0]["kind"], "alert");
    assert_eq!(json["events"][0]["by"], "policy");
    let out = sandbox.run(&["wait", "--text", "alert: undefined"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    // A confirm stops `goto` at once (not after its 30 s timeout).
    let start = Instant::now();
    let out = sandbox.run(&["goto", &on_load("confirm", "Delete%20item%3F")]);
    assert_eq!(code(&out), 5, "{}", stderr(&out));
    assert!(start.elapsed() < Duration::from_secs(10));
    assert_eq!(
        stderr(&out),
        "error: dialog open in tab t1: confirm \"Delete item?\"\n  \
         hint: chrome-cdp dialog accept, or chrome-cdp dialog dismiss\n"
    );

    // Commands that need the page's scripts fail fast while it is open.
    let out = sandbox.run(&["wait", "--text", "confirm:"]);
    assert_eq!(code(&out), 5, "{}", stderr(&out));

    // `tabs` shows it.
    let out = sandbox.run(&["tabs", "--json"]);
    let tab = &json_out(&out)["data"]["tabs"][0];
    assert_eq!(tab["dialog"]["kind"], "confirm");
    assert_eq!(tab["dialog"]["message"], "Delete item?");
    let out = sandbox.run(&["tabs"]);
    assert!(
        stdout(&out).contains(
            "t1 has a dialog open: confirm \"Delete item?\"; chrome-cdp dialog accept, or chrome-cdp dialog dismiss"
        ),
        "{}",
        stdout(&out)
    );

    // Text is only for prompts.
    let out = sandbox.run(&["dialog", "accept", "yes"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));

    let out = sandbox.run(&["dialog", "accept", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let data = &json_out(&out)["data"];
    assert_eq!(data["accepted"], true);
    assert_eq!(data["dialog"]["kind"], "confirm");
    let out = sandbox.run(&["wait", "--text", "confirm: true"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    // With none open, it waits for one up to --timeout, then says so.
    let out = sandbox.run(&["dialog", "dismiss", "--timeout", "300ms"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).starts_with("error: no dialog is open in tab t1\n  waited: 300ms\n"),
        "{}",
        stderr(&out)
    );

    // A prompt takes the text (or keeps its default).
    let prompt = format!("{}&default=Bob", on_load("prompt", "Name%3F"));
    let out = sandbox.run(&["goto", &prompt]);
    assert_eq!(code(&out), 5, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("hint: chrome-cdp dialog accept '<text>', or"),
        "{}",
        stderr(&out)
    );
    let out = sandbox.run(&["dialog", "accept", "Ada Lovelace"]);
    assert_eq!(
        stdout(&out),
        "accepted prompt \"Name?\" with \"Ada Lovelace\"\n",
        "{}",
        stderr(&out)
    );
    let out = sandbox.run(&["wait", "--text", "prompt: \"Ada Lovelace\""]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["goto", &prompt]);
    assert_eq!(code(&out), 5, "{}", stderr(&out));
    let out = sandbox.run(&["dialog", "accept", "-q"]);
    assert_eq!(stdout(&out), "accepted\n");
    let out = sandbox.run(&["wait", "--text", "prompt: \"Bob\""]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    // A navigation dismisses a dialog left open, and says so.
    let out = sandbox.run(&["goto", &on_load("confirm", "Leave%3F")]);
    assert_eq!(code(&out), 5, "{}", stderr(&out));
    let out = sandbox.run(&["goto", &Fixtures::shared().url("/nav/a.html")]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("warning: dialog: confirm \"Leave?\" (dismissed by a navigation"),
        "{}",
        stderr(&out)
    );

    // A raw call that opens one returns at once too.
    let out = sandbox.run(&[
        "cdp",
        "--target",
        "page",
        "Runtime.evaluate",
        r#"{"expression":"confirm('Really?')"}"#,
    ]);
    assert_eq!(code(&out), 5, "{}", stderr(&out));
    let out = sandbox.run(&["dialog", "dismiss", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(json_out(&out)["data"]["accepted"], false);
}

#[test]
fn dialog_waits_for_the_next_dialog() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let url = Fixtures::shared().url("/bench/delete.html");
    let out = sandbox.run(&["goto", &url]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    // The page opens a prompt as soon as its confirm closes: the second
    // `dialog accept` waits for it, with no `wait` between them.
    let mut cmd = sandbox.cli();
    cmd.args(["batch", "--continue"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            b"click 'role=button[name=\"Delete account\"]'\n\
              dialog accept\n\
              dialog accept DELETE\n\
              expect 'text=scheduled for deletion' visible\n",
        )
        .unwrap();
    let out = child.wait_with_output().unwrap();
    eprintln!("stdout: {}\nstderr: {}", stdout(&out), stderr(&out));
    // The click ran into the confirm, which the next step answers, so it
    // passes as in `test` (plan 18.6); every other step passed too.
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("accepted confirm \"Delete your account? This cannot be undone.\""),
        "{text}"
    );
    assert!(
        text.contains("accepted prompt \"Type DELETE to confirm.\" with \"DELETE\""),
        "{text}"
    );
    assert!(
        text.contains("[4/4] expect 'text=scheduled for deletion' visible ✓"),
        "{text}"
    );

    // A dialog that opens a while after the command starts is answered.
    let out = sandbox.run(&["eval", "setTimeout(() => confirm('Later'), 500), 1"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["dialog", "dismiss"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "dismissed confirm \"Later\"\n");
}

#[test]
fn before_unload_and_the_session_policy() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let click = |sandbox: &Sandbox| {
        // Chrome asks before unload only after a user gesture.
        for kind in ["mousePressed", "mouseReleased"] {
            let params =
                format!(r#"{{"type":"{kind}","x":5,"y":5,"button":"left","clickCount":1}}"#);
            let out = sandbox.run(&[
                "cdp",
                "--target",
                "page",
                "Input.dispatchMouseEvent",
                &params,
            ]);
            assert_eq!(code(&out), 0, "{}", stderr(&out));
        }
    };

    // By default beforeunload is accepted: the page is left.
    let out = sandbox.run(&["goto", &site.url("/dialogs/beforeunload.html")]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    click(&sandbox);
    let out = sandbox.run(&["goto", &site.url("/nav/a.html"), "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let json = json_out(&out);
    assert_eq!(json["data"]["title"], "Page A");
    assert_eq!(json["events"][0]["kind"], "beforeunload");
    assert_eq!(json["events"][0]["accepted"], true);

    // `manual`: every dialog waits, an alert too.
    let out = sandbox.run(&["open", "--dialogs", "manual"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["goto", &on_load("alert", "Hi")]);
    assert_eq!(code(&out), 5, "{}", stderr(&out));
    let out = sandbox.run(&["dialog", "accept"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    // Dismissing beforeunload stays on the page.
    let out = sandbox.run(&["goto", &site.url("/dialogs/beforeunload.html")]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    click(&sandbox);
    let out = sandbox.run(&["goto", &site.url("/nav/b.html")]);
    assert_eq!(code(&out), 5, "{}", stderr(&out));
    assert!(
        stderr(&out).starts_with(
            "error: dialog open in tab t1: beforeunload (leave the page?)\n  \
             hint: chrome-cdp dialog accept (leaves the page), or chrome-cdp dialog dismiss (stays)\n"
        ),
        "{}",
        stderr(&out)
    );
    let out = sandbox.run(&["dialog", "dismiss"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["wait", "--text", "Unsaved changes"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    // `dismiss`: every dialog is answered with Cancel, so a navigation
    // away from a page that asks first fails at once.
    let out = sandbox.run(&["open", "--dialogs", "dismiss"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let start = Instant::now();
    let out = sandbox.run(&["goto", &site.url("/nav/b.html"), "--json"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(start.elapsed() < Duration::from_secs(10));
    let error = &json_out(&out)["error"];
    assert_eq!(error["details"]["reason"], "beforeunload", "{error}");
    assert_eq!(
        error["hint"],
        "chrome-cdp open --dialogs auto   (accepts beforeunload; then navigate again)"
    );
    let out = sandbox.run(&["tab", "new", &on_load("confirm", "Sure%3F")]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("warning: dialog: confirm \"Sure?\" (dismissed)"),
        "{}",
        stderr(&out)
    );
    let out = sandbox.run(&["wait", "--text", "confirm: false"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    // `accept`: OK, and a prompt gets its default.
    let out = sandbox.run(&["open", "--dialogs", "accept"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let prompt = format!("{}&default=Bob", on_load("prompt", "Name%3F"));
    let out = sandbox.run(&["goto", &prompt]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["wait", "--text", "prompt: \"Bob\""]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

#[test]
fn a_dialog_in_another_tab() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let out = sandbox.run(&["open"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["tab", "new"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["goto", &on_load("confirm", "Other"), "--tab", "t1"]);
    assert_eq!(code(&out), 5, "{}", stderr(&out));
    assert!(
        stderr(&out).contains(
            "hint: chrome-cdp dialog accept --tab t1, or chrome-cdp dialog dismiss --tab t1"
        ),
        "{}",
        stderr(&out)
    );
    // The active tab (t2) has none; the error points at t1 at once,
    // without waiting for a dialog in t2.
    let start = Instant::now();
    let out = sandbox.run(&["dialog", "accept"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(start.elapsed() < Duration::from_secs(10));
    assert!(
        stderr(&out).contains("dialog open: tab t1: confirm \"Other\""),
        "{}",
        stderr(&out)
    );
    let out = sandbox.run(&["dialog", "accept", "--tab", "t1"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}
