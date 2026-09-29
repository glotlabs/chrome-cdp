//! Tabs (plan 03, milestone 3.1): ids, the active tab, `--tab`, popups
//! and closing, end to end against the fixture site.
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use common::{Sandbox, chrome_path, code, json_out, stderr, stdout, wait_until};
use fixtures::Fixtures;
use serde_json::{Value, json};

/// `tabs --json`'s list.
fn tabs(sandbox: &Sandbox) -> Vec<Value> {
    let out = sandbox.run(&["tabs", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    json_out(&out)["data"]["tabs"].as_array().unwrap().clone()
}

fn active(sandbox: &Sandbox) -> String {
    let tabs = tabs(sandbox);
    let active: Vec<&Value> = tabs.iter().filter(|t| t["active"] == json!(true)).collect();
    assert_eq!(active.len(), 1, "{tabs:?}");
    active[0]["id"].as_str().unwrap().to_owned()
}

/// Evaluates `expression` in the active tab (or `extra`'s `--tab`) and
/// returns the value.
fn eval(sandbox: &Sandbox, expression: &str, extra: &[&str]) -> Value {
    let params = json!({ "expression": expression, "returnByValue": true }).to_string();
    let mut args = vec![
        "cdp",
        "--target",
        "page",
        "Runtime.evaluate",
        &params,
        "--json",
    ];
    args.extend(extra);
    let out = sandbox.run(&args);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    json_out(&out)["data"]["result"]["value"].clone()
}

fn click(sandbox: &Sandbox, selector: &str) {
    eval(
        sandbox,
        &format!("document.querySelector('{selector}').click()"),
        &[],
    );
}

/// Waits until tab `id` shows the popup page and returns it.
fn popup(sandbox: &Sandbox, id: &str) -> Value {
    let mut found = None;
    wait_until(&format!("popup {id}"), || {
        found = tabs(sandbox)
            .into_iter()
            .find(|t| t["id"] == json!(id) && t["title"].as_str().unwrap().starts_with("Popup: "));
        found.is_some()
    });
    found.unwrap()
}

#[test]
fn popups_open_as_background_tabs_with_stable_ids() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    assert_eq!(
        code(&sandbox.run(&["open", &site.url("/tabs/popup.html")])),
        0
    );
    assert_eq!(stdout(&sandbox.run(&["tabs", "-q"])), "t1\n");

    // window.open (with an opener) and target=_blank (noopener).
    click(&sandbox, "#open");
    let t2 = popup(&sandbox, "t2");
    assert_eq!(t2["url"], json!(site.url("/tabs/popup-target.html")));
    assert_eq!(t2["opener"], json!("t1"));
    click(&sandbox, "#blank");
    assert_eq!(popup(&sandbox, "t3")["opener"], json!("t1"));
    // Popups do not take over.
    assert_eq!(active(&sandbox), "t1");

    // --tab picks a tab for one command; `tab` switches.
    assert_eq!(
        eval(&sandbox, "document.title", &["--tab", "t2"]),
        json!("Popup: no init script")
    );
    assert_eq!(eval(&sandbox, "document.title", &[]), json!("Popup opener"));
    let out = sandbox.run(&["tab", "t3"]);
    assert_eq!(code(&out), 0);
    assert!(
        stdout(&out).starts_with("switched to tab t3 "),
        "{}",
        stdout(&out)
    );
    assert_eq!(active(&sandbox), "t3");
    let env_tab = sandbox
        .cli()
        .env("CHROME_CDP_TAB", "t1")
        .args(["cdp", "--target", "page", "Runtime.evaluate"])
        .arg(r#"{"expression":"document.title","returnByValue":true}"#)
        .args(["-q"])
        .output()
        .unwrap();
    assert!(
        stdout(&env_tab).contains("Popup opener"),
        "{}",
        stdout(&env_tab)
    );

    // Closing the active tab falls back to the one active before it.
    let out = sandbox.run(&["tab", "close", "--json"]);
    assert_eq!(code(&out), 0);
    let data = &json_out(&out)["data"];
    assert_eq!(data["closed"], json!("t3"));
    assert_eq!(data["active"]["id"], json!("t1"));

    // Ids are never reused.
    let out = sandbox.run(&["tab", "new", "-q"]);
    assert_eq!(stdout(&out), "t4\n");
    assert_eq!(stdout(&sandbox.run(&["tabs", "-q"])), "t1\nt2\nt4\n");
    assert_eq!(active(&sandbox), "t4");
    let out = sandbox.run(&["tab", "t3"]);
    assert_eq!(code(&out), 1);
    assert!(
        stderr(&out).contains("no tab t3 in session default"),
        "{}",
        stderr(&out)
    );
    let status = json_out(&sandbox.run(&["status", "--json"]));
    assert_eq!(status["data"]["tabs"], json!(3));
}

#[test]
fn a_command_reports_the_popup_it_opened() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    assert_eq!(
        code(&sandbox.run(&["open", &site.url("/tabs/popup.html")])),
        0
    );
    // window.open creates the tab before it returns.
    let out = sandbox.run(&[
        "cdp",
        "--target",
        "page",
        "Runtime.evaluate",
        r#"{"expression":"window.open('/tabs/popup-target.html'), 1"}"#,
        "--json",
    ]);
    assert_eq!(code(&out), 0);
    let envelope = json_out(&out);
    assert_eq!(
        envelope["events"],
        json!([{
            "type": "tab_opened",
            "tab": "t2",
            "url": site.url("/tabs/popup-target.html"),
            "opener": "t1",
            "active": false,
        }]),
        "{envelope}"
    );
    assert_eq!(
        envelope["warnings"],
        json!([format!(
            "opened tab t2 ({}); switch to it: chrome-cdp tab t2",
            site.url("/tabs/popup-target.html")
        )])
    );
}

/// `eval`'s value, as JSON.
fn eval_json(sandbox: &Sandbox, expression: &str, extra: &[&str]) -> Value {
    let mut args = vec!["eval", expression, "--json"];
    args.extend(extra);
    let out = sandbox.run(&args);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    json_out(&out)["data"]["value"].clone()
}

#[test]
fn the_active_tab_stays_visible_behind_a_popup() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let url = site.url("/emulate/permissions.html");
    assert_eq!(code(&sandbox.run(&["open", &url])), 0);
    assert_eq!(
        code(&sandbox.run(&["permissions", "grant", "geolocation"])),
        0
    );
    assert_eq!(code(&sandbox.run(&["geo", "59.9139,10.7522"])), 0);
    eval(&sandbox, "window.open('/tabs/popup-target.html'), 1", &[]);
    popup(&sandbox, "t2");
    assert_eq!(active(&sandbox), "t1");

    // Chrome shows the popup in front; chrome-cdp brings the active tab
    // back, so it is not throttled.
    assert_eq!(
        eval_json(&sandbox, "document.visibilityState", &[]),
        json!("visible")
    );
    assert_eq!(
        eval_json(
            &sandbox,
            "new Promise((r) => requestAnimationFrame(() => r('frame')))",
            &["--timeout", "5s"]
        ),
        json!("frame")
    );
    assert_eq!(
        eval_json(&sandbox, "position()", &["--timeout", "10s"]),
        json!([59.9139, 10.7522, 10])
    );
    assert_eq!(
        eval_json(&sandbox, "document.visibilityState", &["--tab", "t2"]),
        json!("hidden")
    );
    // Switching brings the other tab to the front.
    assert_eq!(code(&sandbox.run(&["tab", "t2"])), 0);
    assert_eq!(
        eval_json(&sandbox, "document.visibilityState", &[]),
        json!("visible")
    );
}

#[test]
fn follow_popups_makes_each_popup_active() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let out = sandbox.run(&["open", "--follow-popups", &site.url("/tabs/popup.html")]);
    assert_eq!(code(&out), 0);
    click(&sandbox, "#open");
    popup(&sandbox, "t2");
    assert_eq!(active(&sandbox), "t2");
    // A followed popup stays in front.
    assert_eq!(
        eval_json(&sandbox, "document.visibilityState", &[]),
        json!("visible")
    );
    // The session keeps the setting.
    assert_eq!(code(&sandbox.run(&["tab", "t1"])), 0);
    click(&sandbox, "#blank");
    popup(&sandbox, "t3");
    assert_eq!(active(&sandbox), "t3");
}

#[test]
fn closing_the_last_tab_keeps_the_session_unless_asked() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    assert_eq!(
        code(&sandbox.run(&["open", "data:text/html,<title>One</title>"])),
        0
    );
    let out = sandbox.run(&["tab", "close", "t1", "-q"]);
    assert_eq!(stdout(&out), "t2\n");
    assert_eq!(stdout(&sandbox.run(&["status", "-q"])), "running\n");
    // A page that closes itself leaves no tab; the next command gets a
    // blank one.
    eval(&sandbox, "window.close()", &[]);
    wait_until("the tab to close", || tabs(&sandbox).is_empty());
    assert_eq!(eval(&sandbox, "location.href", &[]), json!("about:blank"));
    assert_eq!(stdout(&sandbox.run(&["tabs", "-q"])), "t3\n");

    let out = sandbox.run(&["tab", "close", "--close-browser"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "closed tab t3 and session default\n");
    assert_eq!(stdout(&sandbox.run(&["status", "-q"])), "stopped\n");
    sandbox.assert_all_gone();
}
