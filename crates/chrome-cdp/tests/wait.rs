//! `wait` (plan 03, milestone 3.4) end to end: conditions through the
//! daemon, `-` for stdin, the 10 s default timeout and
//! `CHROME_CDP_TIMEOUT`, `--tab`, and `goto`'s default settle.
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::io::Write as _;
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::{Sandbox, chrome_path, code, json_out, run, stderr, stdout};
use fixtures::Fixtures;

#[test]
fn wait_through_the_daemon() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    // `wait` needs a running session; it does not start one.
    let out = sandbox.run(&["wait", "--idle"]);
    assert_eq!(code(&out), 4, "{}", stderr(&out));

    // `goto` settles by default: the SPA has rendered its data.
    let out = sandbox.run(&["goto", &site.url("/wait/spa-fetch.html"), "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let data = &json_out(&out)["data"];
    assert_eq!(data["wait"], "settle");
    assert_eq!(data["title"], "SPA: Inbox");

    let out = sandbox.run(&["goto", &site.url("/wait/lazy.html?after=300")]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["wait", "#welcome"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).starts_with("#welcome is visible ("),
        "{}",
        stdout(&out)
    );

    // The text from stdin.
    let mut child = sandbox
        .cli()
        .args(["wait", "--text", "-", "--in", "#main", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"Welcome, Ada\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        json_out(&out)["data"]["met"],
        "text \"Welcome, Ada\" is visible"
    );

    let out = sandbox.run(&["wait", "--fn", "window.appReady.user", "-q"]);
    assert_eq!(stdout(&out), "\"Ada\"\n");

    // Checked before the daemon is asked.
    let out = sandbox.run(&["wait", "--status", "200", "--request", "/api"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(stderr(&out).starts_with("error: --status needs --response\n"));

    // `--tab` names the tab.
    let out = sandbox.run(&["tab", "new", &site.url("/nav/a.html")]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["wait", "--text", "Page A", "--tab", "t2", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(json_out(&out)["data"]["tab"], "t2");

    // The timeout comes from CHROME_CDP_TIMEOUT, else it is 10 s.
    let out = run(sandbox
        .cli()
        .args(["wait", "#never"])
        .env("CHROME_CDP_TIMEOUT", "600ms"));
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert!(stderr(&out).starts_with("error: waited 600ms for #never to be visible\n"));
    let start = Instant::now();
    let out = sandbox.run(&["wait", "#never"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert!(stderr(&out).starts_with("error: waited 10s for #never to be visible\n"));
    assert!(start.elapsed() >= Duration::from_secs(10));
}

#[test]
fn reads_wait_for_their_element() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let lazy = site.url("/wait/lazy.html?after=500");
    let reads: [&[&str]; 5] = [
        &["text", "#welcome"],
        &["attr", "#welcome", "id"],
        &["eval", "--in", "#welcome", "el.id"],
        &["locator", "#welcome"],
        &["box", "#welcome", "-q"],
    ];
    for read in reads {
        // `goto` settles, which may outlast the delay: load only.
        let out = sandbox.run(&["goto", &lazy, "--wait", "load"]);
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        let started = Instant::now();
        let out = sandbox.run(read);
        assert_eq!(code(&out), 0, "{read:?}: {}", stderr(&out));
        assert!(!stdout(&out).trim().is_empty(), "{read:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{read:?} took {:?}",
            started.elapsed()
        );
    }
    let out = sandbox.run(&["text", "#welcome"]);
    assert_eq!(stdout(&out).trim(), "Welcome, Ada");

    // `count` answers at once: 0 is an answer.
    let out = sandbox.run(&["goto", &lazy, "--wait", "load"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["count", "#welcome"]);
    assert_eq!((code(&out), stdout(&out).trim()), (0, "0"));

    // Something that never shows up: a timeout's NotFound, saying so.
    let started = Instant::now();
    let out = sandbox.run(&["text", "#never", "--timeout", "700ms"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("no element matches #never"),
        "{}",
        stderr(&out)
    );
    assert!(stderr(&out).contains("waited: 700ms"), "{}", stderr(&out));
    assert!(started.elapsed() >= Duration::from_millis(700));
    assert_eq!(code(&sandbox.run(&["close"])), 0);
}
