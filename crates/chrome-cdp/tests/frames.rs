//! Frames (plan 03, milestone 3.2) end to end: `cdp --frame` reaches
//! same-site and cross-site (out-of-process) iframes of the fixture site's
//! `frames/nested.html`.
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use common::{Sandbox, chrome_path, code, json_out, stderr, stdout, wait_until};
use fixtures::Fixtures;
use serde_json::{Value, json};

/// Evaluates `expression` in frame `frame` and returns the command's
/// output.
fn eval_in(sandbox: &Sandbox, frame: &str, expression: &str) -> std::process::Output {
    let params = json!({ "expression": expression, "returnByValue": true }).to_string();
    sandbox.run(&[
        "cdp",
        "--frame",
        frame,
        "Runtime.evaluate",
        &params,
        "--json",
    ])
}

fn value(out: &std::process::Output) -> Value {
    assert_eq!(code(out), 0, "{}", stderr(out));
    json_out(out)["data"]["result"]["value"].clone()
}

#[test]
fn cdp_reaches_same_and_cross_site_frames() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let out = sandbox.run(&["open", &site.url("/frames/nested.html")]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    // The innermost cross-site frame loads last.
    wait_until("the deep frame", || {
        code(&eval_in(&sandbox, "deep", "where")) == 0
    });

    assert_eq!(value(&eval_in(&sandbox, "local", "where")), json!("local"));
    assert_eq!(
        value(&eval_in(&sandbox, "remote", "where")),
        json!("remote")
    );
    let leaf = json!(format!("leaf on {}", site.b()));
    assert_eq!(value(&eval_in(&sandbox, "deep", "where")), leaf);
    assert_eq!(value(&eval_in(&sandbox, "remote-leaf", "where")), leaf);
    assert_eq!(
        value(&eval_in(&sandbox, "remote.html", "where")),
        json!("remote")
    );

    // Not found and ambiguous frames list the candidates.
    let out = eval_in(&sandbox, "nowhere", "1");
    assert_eq!(code(&out), 1);
    let err = &json_out(&out)["error"];
    assert_eq!(err["kind"], json!("NotFound"));
    assert_eq!(err["details"]["frames"].as_array().unwrap().len(), 5);
    let out = sandbox.run(&["cdp", "--frame", "leaf.html", "Page.getFrameTree"]);
    assert_eq!(code(&out), 1);
    let text = stderr(&out);
    assert!(
        text.contains("error: 2 frames in tab t1 match \"leaf.html\""),
        "{text}"
    );
    assert!(
        text.contains("  match: name \"deep\" http://localhost:"),
        "{text}"
    );
    assert!(
        text.contains("  match: name \"remote-leaf\" http://localhost:"),
        "{text}"
    );
    assert!(
        text.contains("hint: use a longer part of the URL"),
        "{text}"
    );

    // --frame picks the page even with the default --target browser, and
    // other methods go to the session that owns the frame.
    let out = sandbox.run(&["cdp", "--frame", "deep", "Page.getFrameTree", "-q"]);
    let tree: Value = serde_json::from_str(&stdout(&out)).unwrap();
    let url = site.url_b("/frames/leaf.html");
    assert_eq!(tree["frameTree"]["frame"]["url"], json!(url));
}
