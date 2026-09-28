//! `console` and `errors` end to end (plan 06, milestone 6.1): the read
//! cursor across CLI calls, `--follow`, and `open --console-buffer`.
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::process::Stdio;
use std::time::{Duration, Instant};

use common::{Sandbox, chrome_path, code, json_out, stderr, stdout};
use fixtures::Fixtures;

#[test]
fn each_call_shows_what_is_new() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let page = Fixtures::shared().url("/console/index.html");
    let out = sandbox.run(&["open", &page]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let out = sandbox.run(&["console"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("warn deprecated call"),
        "{}",
        stdout(&out)
    );
    let out = sandbox.run(&["console"]);
    assert_eq!(
        stdout(&out),
        "no new console entries in t1; chrome-cdp console --since 0 shows the earlier ones\n"
    );

    let out = sandbox.run(&["click", "#sync"]);
    assert!(
        stdout(&out).contains("! console error: TypeError"),
        "{}",
        stdout(&out)
    );
    let out = sandbox.run(&["errors", "--json"]);
    let json = json_out(&out);
    let entries = json["data"]["entries"].as_array().unwrap();
    // `errors` has its own cursor: the page's two errors and the new one.
    assert_eq!(entries.len(), 3, "{json}");
    assert_eq!(entries[2]["kind"], "uncaught");
    assert_eq!(entries[2]["stack"][0]["function"], "failSync");
    let out = sandbox.run(&["console", "-q"]);
    assert_eq!(
        stdout(&out),
        "TypeError: Cannot read properties of null (reading 'items')\n"
    );
}

#[test]
fn follow_prints_entries_as_they_come() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let page = Fixtures::shared().url("/console/index.html");
    let out = sandbox.run(&["open", &page, "--console-buffer", "500"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    sandbox.run(&["console"]);

    let start = Instant::now();
    let follow = sandbox
        .cli()
        .args(["errors", "--follow", "--timeout", "6s"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let out = sandbox.run(&["click", "#later"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["click", "#reject"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = follow.wait_with_output().unwrap();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    // It ran until --timeout.
    assert!(start.elapsed() >= Duration::from_secs(6));
    assert_eq!(stderr(&out), "following t1's errors; Ctrl-C stops\n");
    let text = stdout(&out);
    // The page's own errors first (the first batch), then the new ones.
    assert!(text.contains("failed to save Error: disk full"), "{text}");
    let later = text.find("uncaught Error: later failure").unwrap();
    let rejected = text
        .find("uncaught (in promise) Error: rejected promise")
        .unwrap();
    assert!(later < rejected, "{text}");

    // The burst overflows the 500 entries `open` asked for.
    sandbox.run(&["click", "#burst"]);
    let out = sandbox.run(&["console", "--limit", "1"]);
    let text = stdout(&out);
    let first = text.lines().next().unwrap();
    assert!(
        first.ends_with(" info burst 1499 (/console/app.js:48)"),
        "{text}"
    );
    assert!(stderr(&out).contains("console entries were dropped from t1's buffer before they were read; it keeps the newest 500"), "{}", stderr(&out));
}
