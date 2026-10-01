//! Repro bundles (plan 18, milestone 18.5) end to end against a real
//! Chrome (`CHROME_PATH`): a session on the fixture site that ends in a
//! failed `expect`, `repro save`, then `test <dir>` with the fixture server
//! stopped fails at the same step with the same error, and passes once the
//! bundle's mock gives the fixed answer.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Output;

use common::{Sandbox, chrome_path, stderr, stdout};
use fixtures::Fixtures;

/// A sandbox with a working directory that is no project.
struct Session {
    sandbox: Sandbox,
    work: PathBuf,
}

impl Session {
    fn new(chrome: &Path) -> Self {
        let sandbox = Sandbox::with_chrome(chrome);
        let work = sandbox.dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        Self { sandbox, work }
    }

    fn run(&self, args: &[&str]) -> Output {
        common::run(self.sandbox.cli().current_dir(&self.work).args(args))
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", stderr(&out));
        stdout(&out)
    }
}

/// Every file under `dir`, as (relative path, bytes).
fn files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(dir).unwrap().display().to_string();
                out.push((rel, std::fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn a_bundle_replays_offline_and_passes_with_the_fix_in_its_mocks() {
    let Some(chrome) = chrome_path() else { return };
    // A server of this test's own, to stop.
    let server = Fixtures::start(0, 0).unwrap();
    let site = server.a().to_owned();
    let session = Session::new(&chrome);

    session.ok(&["open", &format!("{site}/record/shop.html")]);
    session.ok(&["wait", "text=Monitor"]);
    let started = session.ok(&["repro", "start"]);
    assert!(started.contains("started a repro at "), "{started}");
    session.ok(&[
        "click",
        "role=listitem >> has-text=Mouse >> role=button[name=\"Add\"]",
    ]);
    session.ok(&["fill", "role=textbox[name=\"Quantity\"]", "2"]);
    session.ok(&["click", "role=button[name=\"Buy\"]"]);
    session.ok(&["expect", "role=status", "text", "ordered"]);
    // The bug: the order should be shipped.
    let failed = session.run(&[
        "expect",
        "role=status",
        "text",
        "Order o-17: shipped, 2 items",
        "--timeout",
        "1s",
        "--json",
    ]);
    assert_eq!(failed.status.code(), Some(1));
    let error = common::json_out(&failed)["error"]["message"]
        .as_str()
        .unwrap()
        .to_owned();
    session.ok(&["snapshot", "-i"]);

    let bundle = session.work.join("bugs/shipped");
    let saved = session.run(&[
        "repro",
        "save",
        "bugs/shipped",
        "--name",
        "orders a mouse",
        "--json",
    ]);
    assert_eq!(saved.status.code(), Some(0), "{}", stderr(&saved));
    let data = common::json_out(&saved)["data"].clone();
    assert_eq!(data["path"], bundle.display().to_string());
    assert_eq!(data["started"], true);
    assert_eq!(data["failed"]["kind"], "ExpectFailed");
    assert!(data["mocks"].as_u64().unwrap() >= 3, "{data}");
    for file in [
        "chrome-cdp.toml",
        "repro.cdp",
        "state.json",
        "README.md",
        "mocks/repro.toml",
        "evidence/snapshot.txt",
        "evidence/screenshot.png",
        "evidence/console.txt",
        "evidence/errors.txt",
        "evidence/requests.txt",
    ] {
        assert!(bundle.join(file).is_file(), "{file} in {data}");
    }
    let test = std::fs::read_to_string(bundle.join("repro.cdp")).unwrap();
    assert!(
        test.contains(&format!(
            "@before\n  state load state.json\n\n@test \"orders a mouse\"\n  goto {site}/record/shop.html\n  click 'role=listitem >> has-text=Mouse >> role=button[name=\"Add\"]'\n  fill 'role=textbox[name=\"Quantity\"]' 2\n  click 'role=button[name=\"Buy\"]'\n  expect role=status text ordered\n  # failed in the session (ExpectFailed: "
        )),
        "{test}"
    );
    assert!(
        test.trim_end()
            .ends_with("expect role=status text 'Order o-17: shipped, 2 items' --timeout 1s"),
        "{test}"
    );
    let readme = std::fs::read_to_string(bundle.join("README.md")).unwrap();
    assert!(readme.contains(&error), "{readme}");
    let mocks = std::fs::read_to_string(bundle.join("mocks/repro.toml")).unwrap();
    assert!(mocks.contains("unmatched = \"fail\""), "{mocks}");
    assert!(mocks.contains("path = \"/record/shop.html\""), "{mocks}");
    assert!(
        mocks.contains("respond = { json = { id = \"o-17\", status = \"ordered\" } }"),
        "{mocks}"
    );
    session.ok(&["close"]);

    // Offline: the server is gone, every request comes from the mocks.
    server.stop();
    let out = session.run(&["test", "bugs/shipped", "--json"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let run = common::json_out(&out);
    let failure = &run["error"]["details"]["tests"][0];
    assert_eq!(failure["status"], "failed", "{run}");
    // The same step (the test's last line), the same error.
    assert_eq!(
        failure["error"]["at"],
        format!("repro.cdp:{}:3", test.lines().count()),
        "{failure}"
    );
    assert_eq!(failure["error"]["error"]["message"], error.as_str());
    assert_eq!(failure["error"]["error"]["kind"], "ExpectFailed");

    // The fix, in the mocks: now it passes, a regression test.
    std::fs::write(
        bundle.join("mocks/repro.toml"),
        mocks.replace("status = \"ordered\"", "status = \"shipped\""),
    )
    .unwrap();
    std::fs::write(
        bundle.join("repro.cdp"),
        test.replace("expect role=status text ordered\n", ""),
    )
    .unwrap();
    let out = session.run(&["test", "bugs/shipped"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let requests =
        std::fs::read_to_string(bundle.join("e2e/.results/repro.cdp/orders-a-mouse/requests.txt"))
            .unwrap();
    assert!(
        requests.contains("← mock \"POST /record/api/order.json\""),
        "{requests}"
    );
    assert!(!requests.contains("unmatched"), "{requests}");
}

#[test]
fn a_bundle_keeps_no_password() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared().a();
    let session = Session::new(&chrome);
    let password = "hunter2-repro-secret";

    session.ok(&["open", &format!("{site}/script/login.html")]);
    session.ok(&["repro", "start"]);
    session.ok(&["fill", "label=Email", "ada@example.com"]);
    session.ok(&["fill", "label=Password", password]);
    let form = format!("{{\"Email\":\"ada@example.com\",\"Password\":\"{password}\"}}");
    session.ok(&["fill-form", "--values", &form]);
    session.ok(&["click", "role=button[name=\"Sign in\"]"]);
    session.ok(&["expect", "#welcome", "text", "Welcome, ada@example.com"]);
    let saved = session.ok(&["repro", "save", "login"]);
    assert!(
        saved.contains("the replay needs secrets: PASSWORD"),
        "{saved}"
    );

    let bundle = session.work.join("login");
    for (file, bytes) in files(&bundle) {
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains(password), "{file} holds the password");
    }
    let test = std::fs::read_to_string(bundle.join("repro.cdp")).unwrap();
    assert!(
        test.contains("  fill label=Password \"${secret:PASSWORD}\"\n"),
        "{test}"
    );
    assert!(
        test.contains(
            "  fill-form --values \"{\\\"Email\\\":\\\"ada@example.com\\\",\\\"Password\\\":\\\"${secret:PASSWORD}\\\"}\"\n"
        ),
        "{test}"
    );
    let readme = std::fs::read_to_string(bundle.join("README.md")).unwrap();
    assert!(
        readme.contains("- `PASSWORD` (typed into a password field"),
        "{readme}"
    );
    session.ok(&["close"]);
    // The replay types the secret, not its name.
    std::fs::write(
        bundle.join("repro.cdp"),
        format!("{test}  expect label=Password value \"${{secret:PASSWORD}}\"\n"),
    )
    .unwrap();

    // The replay reads it from the environment.
    let out = common::run(
        session
            .sandbox
            .cli()
            .current_dir(&session.work)
            .env("PASSWORD", password)
            .args(["test", "login"]),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(!stdout(&out).contains(password));
}

#[test]
fn save_without_start_warns_and_refuses_a_full_directory() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared().a();
    let session = Session::new(&chrome);
    session.ok(&["open", &format!("{site}/record/shop.html")]);
    session.ok(&["wait", "text=Monitor"]);
    let out = session.run(&["repro", "save", "plain"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stderr(&out).contains("no `repro start` in this session"));
    let test = std::fs::read_to_string(session.work.join("plain/repro.cdp")).unwrap();
    assert!(
        test.contains(&format!(
            "@test \"plain\"\n  goto {site}/record/shop.html\n  wait text=Monitor\n"
        )),
        "{test}"
    );

    let again = session.run(&["repro", "save", "plain"]);
    assert_eq!(again.status.code(), Some(2));
    assert!(
        stderr(&again).contains("has files already"),
        "{}",
        stderr(&again)
    );
    std::fs::write(session.work.join("plain/notes.txt"), "mine").unwrap();
    session.ok(&["repro", "save", "plain", "--force"]);
    assert!(session.work.join("plain/notes.txt").is_file());
}
