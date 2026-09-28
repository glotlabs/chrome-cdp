//! `route`, `block`, `routes` and `unroute` end to end (plan 07,
//! milestone 7.1): rules added by one CLI call apply to the page loaded
//! by the next.
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::io::Write as _;
use std::process::{Command, Output, Stdio};

use common::{Sandbox, chrome_path, code, json_out, run, stderr, stdout};
use fixtures::Fixtures;

/// Runs `cmd` with `input` on its stdin.
fn with_stdin(cmd: &mut Command, input: &[u8]) -> Output {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn mock_then_load() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let site = Fixtures::shared();

    // No session yet: nothing to add rules to.
    let out = sandbox.run(&["route", "/api/", "--abort"]);
    assert_eq!(code(&out), 4, "{}", stderr(&out));
    assert!(stderr(&out).contains("chrome-cdp open"), "{}", stderr(&out));

    let out = sandbox.run(&["open"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    // A mocked first request of the page, from a file; its text comes
    // from stdin.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("first.txt"), "from a file\n").unwrap();
    let out = run(sandbox.cli().current_dir(dir.path()).args([
        "route",
        "/route/api/first",
        "--body-file",
        "first.txt",
        "-q",
    ]));
    assert_eq!(
        (code(&out), stdout(&out).as_str()),
        (0, "r1\n"),
        "{}",
        stderr(&out)
    );
    let out = with_stdin(
        sandbox
            .cli()
            .args(["route", "**/route/api/items", "--json-body", "-"]),
        b"{\"items\": [\"mocked\"]}",
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "r2 **/route/api/items → fulfill 200 application/json 20B\n"
    );

    let out = sandbox.run(&["open", &site.url("/route/inline.html")]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["wait", "--fn", "document.title === 'from a file'"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&[
        "eval",
        "fetch('/route/api/items').then((r) => r.json())",
        "--json",
    ]);
    assert_eq!(
        json_out(&out)["data"]["value"],
        serde_json::json!({"items": ["mocked"]})
    );

    // `--tab` is no scope for routes.
    let out = sandbox.run(&["routes", "--tab", "t1"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "r1 /route/api/first → fulfill 200 text/plain 12B (1 hit)\n\
         r2 **/route/api/items → fulfill 200 application/json 20B (1 hit)\n"
    );
    assert_eq!(
        stderr(&out),
        "warning: routes apply to every tab of the session; --tab is ignored\n"
    );

    // The journal: the page, its first request, the mocked fetch (and
    // the favicon, at some point).
    let out = sandbox.run(&["routes", "--log", "--limit", "0"]);
    let text = stdout(&out);
    assert!(
        text.contains(" GET http://127.0.0.1:")
            && text.contains("/route/inline.html document → continue\n"),
        "{text}"
    );
    assert!(
        text.contains("/route/api/first xhr → r1 fulfill 200\n"),
        "{text}"
    );
    assert!(
        text.contains("/route/api/items xhr → r2 fulfill 200\n"),
        "{text}"
    );
    let all = text.lines().count();
    let out = sandbox.run(&["routes", "--log", "--limit", "1"]);
    let text = stdout(&out);
    let more = format!(
        "… {} earlier requests not shown; chrome-cdp routes --log --limit {all} shows them\n",
        all - 1
    );
    assert!(text.starts_with(&more), "{text}");
    assert_eq!(text.lines().count(), 2, "{text}");

    let out = sandbox.run(&["unroute", "--all"]);
    assert_eq!(
        stdout(&out),
        "removed r1, r2; no routes left\n",
        "{}",
        stderr(&out)
    );
    let out = sandbox.run(&["reload"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["wait", "--fn", "document.title.startsWith('no fixture')"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["close"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}
