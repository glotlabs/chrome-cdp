//! `network` end to end (plan 06, milestone 6.2): the read cursor across
//! CLI calls, `network show`, `network clear` and `open
//! --network-buffer`.
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::process::Stdio;
use std::time::{Duration, Instant};

use common::{Sandbox, chrome_path, code, json_out, run, stderr, stdout};
use fixtures::Fixtures;

#[test]
fn list_show_and_clear() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let page = Fixtures::shared().url("/network/index.html");
    let out = sandbox.run(&["open", &page, "--network-buffer", "500"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let out = sandbox.run(&["network"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.starts_with("#1 GET 200 /network/index.html document "),
        "{text}"
    );
    assert!(
        text.ends_with("chrome-cdp network show 3 shows one in full\n"),
        "{text}"
    );
    let out = sandbox.run(&["network"]);
    assert_eq!(
        stdout(&out),
        "no new requests in t1; chrome-cdp network --since 0 shows the earlier ones\n"
    );

    let out = sandbox.run(&["click", "#items"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["network", "-q", "--type", "fetch"]);
    assert_eq!(stdout(&out), "#4\n");
    // `-q` with `--body` is the body alone, for `> file`.
    let out = sandbox.run(&["network", "show", "#4", "--body", "-q"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let body: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(body.as_array().unwrap().len(), 300);
    let out = sandbox.run(&["network", "show", "4", "--body", "--max-chars", "300"]);
    let text = stdout(&out);
    assert!(text.starts_with("#4 GET 200 OK "), "{text}");
    assert!(
        text.contains("\ninitiator: script /network/app.js:6 (loadItems)\n"),
        "{text}"
    );
    assert!(text.contains("\nresponse body ("), "{text}");
    assert!(text.contains("--max-chars 0 for all of it"), "{text}");
    // `-o` saves the body relative to the current directory and prints
    // the absolute path.
    let dir = tempfile::tempdir().unwrap();
    let out = run(sandbox.cli().current_dir(dir.path()).args([
        "network",
        "show",
        "4",
        "-o",
        "out/items.json",
        "-q",
    ]));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let saved = dir.path().join("out/items.json");
    assert_eq!(stdout(&out), format!("{}\n", saved.display()));
    let body: serde_json::Value = serde_json::from_slice(&std::fs::read(&saved).unwrap()).unwrap();
    assert_eq!(body.as_array().unwrap().len(), 300);
    let out = sandbox.run(&["network", "show", "4", "-o", "-"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("hint: chrome-cdp network show 4 --body -q"),
        "{}",
        stderr(&out)
    );

    let out = sandbox.run(&["network", "--type", "nope"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("`nope` is not a resource type; they are document, stylesheet,"),
        "{}",
        stderr(&out)
    );

    let out = sandbox.run(&["network", "clear", "--json"]);
    let json = json_out(&out);
    assert_eq!(json["data"]["cleared"], 4, "{json}");
    let out = sandbox.run(&["network", "--since", "0"]);
    assert_eq!(stdout(&out), "no requests in t1\n");
    let out = sandbox.run(&["network", "show", "4"]);
    assert_eq!(code(&out), 1);
    assert!(
        stderr(&out).contains("error: request #4 is no longer in t1's network log"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn follow_prints_requests_as_they_come() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let page = Fixtures::shared().url("/network/index.html");
    let out = sandbox.run(&["open", &page]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    sandbox.run(&["network"]);

    let start = Instant::now();
    let follow = sandbox
        .cli()
        .args(["network", "--follow", "--type", "fetch", "--timeout", "4s"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let out = sandbox.run(&["click", "#items"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["click", "#missing"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = follow.wait_with_output().unwrap();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    // It ran until --timeout.
    assert!(start.elapsed() >= Duration::from_secs(4));
    assert_eq!(stderr(&out), "following t1's network; Ctrl-C stops\n");
    let text = stdout(&out);
    let items = text.find(" GET 200 /items?n=300 fetch ").expect(&text);
    // The page never reads the 404's body, so Chrome never ends it.
    let missing = text
        .find(" GET pending /network/missing.json fetch")
        .expect(&text);
    assert!(items < missing, "{text}");
    // Pending requests show again when they end.
    assert!(text.contains(" GET pending /items?n=300 fetch"), "{text}");
    // The hint to `network show` came with the first batch only.
    assert!(!text.contains("shows one in full"), "{text}");
    // The first, empty batch printed nothing.
    assert!(!text.contains("no new requests"), "{text}");
}
