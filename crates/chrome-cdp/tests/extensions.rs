//! `extensions` (plan 14, milestone 14.4): list, load, reload, uninstall,
//! the action popup and `chrome.storage`, and the console of an
//! extension's service worker.
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::path::{Path, PathBuf};

use common::{Sandbox, chrome_path, stdout, wait_until};
use fixtures::Fixtures;
use serde_json::json;

/// A copy of `testdata/extensions/basic` in `dir`, to edit: its popup
/// shows the service worker's answer and counts its openings in
/// `chrome.storage.local`.
fn fixture_copy(dir: &Path) -> PathBuf {
    let from = fixtures::site_dir()
        .parent()
        .unwrap()
        .join("extensions/basic");
    let to = dir.join("basic");
    std::fs::create_dir_all(&to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
    to.canonicalize().unwrap()
}

#[test]
fn extensions_load_reload_popup_storage_and_uninstall() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let session = sandbox.session();
    let dir = fixture_copy(&sandbox.tmp());
    let dir_arg = dir.to_str().unwrap();

    // A browser started without --extension loads one all the same.
    session.ok(&["open", &site.url("/")]);
    assert_eq!(session.json(&["extensions"]), json!({"extensions": []}));
    let loaded = session.json(&["extensions", "load", dir_arg]);
    let id = loaded["extension"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        loaded["extensions"],
        json!([{"id": id, "name": "chrome-cdp fixture", "version": "1.2.3", "path": dir_arg}])
    );
    // The text cuts a long path (macOS's temp dir makes one); --json above
    // has it whole.
    let text = session.ok(&["extensions"]);
    let path = text
        .strip_prefix(&format!("{id}  chrome-cdp fixture  1.2.3  "))
        .and_then(|rest| rest.strip_suffix('\n'))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(
        path == dir_arg
            || path
                .strip_suffix('…')
                .is_some_and(|cut| dir_arg.starts_with(cut)),
        "{text}"
    );

    // The popup opens as a tab, where its script ran: it reached the
    // service worker and wrote to storage.
    let popup = session.json(&["extensions", "popup", "chrome-cdp fixture"]);
    assert_eq!(popup["tab"], "t1", "{popup}");
    let tab = popup["popup"].as_str().unwrap().to_owned();
    assert_eq!(
        popup["url"],
        format!("chrome-extension://{id}/popup.html"),
        "{popup}"
    );
    let tabs = session.ok(&["tabs"]);
    assert!(
        tabs.contains(&format!("{tab}  chrome-extension://{id}/popup.html")),
        "{tabs}"
    );
    session.ok(&[
        "--tab",
        &tab,
        "wait",
        "--text",
        "pong from the service worker",
    ]);
    session.ok(&["--tab", &tab, "wait", "--text", "opened 1 times"]);

    // The service worker's console reaches the web page's tab too.
    let console = session.ok(&["console", "--since", "0"]);
    assert!(
        console.contains("chrome-cdp fixture: ping from the popup"),
        "{console}"
    );
    session.ok(&["--tab", &tab, "eval", "chrome.runtime.sendMessage('boom')"]);
    wait_until("the service worker's error", || {
        stdout(&session.run(&["errors"])).contains("chrome-cdp fixture: boom")
    });

    // Storage: what the popup wrote, then set, get, remove and clear.
    assert_eq!(
        session.json(&["extensions", "storage", &id])["items"],
        json!({"opened": 1})
    );
    let set = session.json(&[
        "extensions",
        "storage",
        "chrome-cdp fixture",
        "set",
        r#"{"theme": "dark", "count": 3}"#,
    ]);
    assert_eq!(
        set["items"],
        json!({"opened": 1, "theme": "dark", "count": 3})
    );
    assert_eq!(
        session.json(&["extensions", "storage", &id, "get", "theme", "count"])["items"],
        json!({"theme": "dark", "count": 3})
    );
    let removed = session.json(&["extensions", "storage", &id, "remove", "count"]);
    assert_eq!(removed["items"], json!({"opened": 1, "theme": "dark"}));
    // Other areas are apart.
    session.json(&[
        "extensions",
        "storage",
        &id,
        "set",
        r#"{"s": 1}"#,
        "--area",
        "session",
    ]);
    assert_eq!(
        session.json(&["extensions", "storage", &id, "--area", "session"])["items"],
        json!({"s": 1})
    );
    session.fails(
        &["extensions", "storage", &id, "clear", "--area", "managed"],
        2,
        "BadArgs",
    );

    // Reload after an edit: the change shows, and storage stays.
    let manifest = dir.join("manifest.json");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(&manifest, text.replace("\"1.2.3\"", "\"1.2.4\"")).unwrap();
    let popup_js = dir.join("popup.js");
    let text = std::fs::read_to_string(&popup_js).unwrap();
    std::fs::write(&popup_js, text.replace("opened ${", "shown ${")).unwrap();
    let reloaded = session.json(&["extensions", "reload", "chrome-cdp fixture"]);
    assert_eq!(reloaded["extension"]["id"], id.as_str());
    assert_eq!(reloaded["extension"]["version"], "1.2.4");
    let popup = session.json(&["extensions", "popup", &id]);
    let tab = popup["popup"].as_str().unwrap().to_owned();
    session.ok(&["--tab", &tab, "wait", "--text", "shown 2 times"]);

    let cleared = session.json(&["extensions", "storage", &id, "clear"]);
    assert_eq!(cleared["items"], json!({}));

    // Uninstall: it is gone, and naming it again says so.
    let gone = session.json(&["extensions", "uninstall", &id]);
    assert_eq!(gone["extension"]["version"], "1.2.4");
    assert_eq!(gone["extensions"], json!([]));
    let error = session.fails(&["extensions", "popup", &id], 1, "NotFound");
    assert_eq!(error["details"]["loaded"], json!([]), "{error}");
}

/// An extension with no service worker: its storage is reached through a
/// tab opened on it for the call, which does not stay.
#[test]
fn storage_of_an_extension_without_a_service_worker() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let session = sandbox.session();
    let dir = sandbox.tmp().join("plain");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        r#"{"manifest_version": 3, "name": "plain", "version": "1", "permissions": ["storage"]}"#,
    )
    .unwrap();
    session.ok(&["open", "--extension", dir.to_str().unwrap()]);
    let set = session.json(&["extensions", "storage", "plain", "set", r#"{"k": [1, 2]}"#]);
    assert_eq!(set["items"], json!({"k": [1, 2]}));
    assert_eq!(
        session.json(&["extensions", "storage", "plain"])["items"],
        json!({"k": [1, 2]})
    );
    assert_eq!(session.json(&["tabs"])["tabs"].as_array().unwrap().len(), 1);
    // It has no action popup: the action runs, and no tab opens.
    let popup = session.json(&["extensions", "popup", "plain"]);
    assert_eq!(popup.get("popup"), None, "{popup}");
}
