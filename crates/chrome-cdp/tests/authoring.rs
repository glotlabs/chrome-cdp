//! The authoring tools of plan 11, milestone 11.8, end to end against a
//! real Chrome (`CHROME_PATH`) and the fixture site: explore a page that
//! talks to a real backend with refs, `mock record` its API, save the
//! session with `history --as-scenario`, and run that test with `test`
//! on the recorded mocks.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Output;

use common::{Sandbox, chrome_path, stdout};
use fixtures::Fixtures;

/// A project in a sandbox: `app/` with a config whose base_url is the
/// fixture site.
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
                "[e2e]\nbase_url = \"{}\"\ntests = [\"scenarios/*.cdp\"]\nmocks = \"mocks\"\n",
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
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        stdout(&out)
    }
}

/// The ref on the `n`th (from 0) snapshot line that contains `what`.
fn nth_ref(snapshot: &str, what: &str, n: usize) -> String {
    let line = snapshot
        .lines()
        .filter(|l| l.contains(what))
        .nth(n)
        .unwrap_or_else(|| panic!("no {what} #{n} in {snapshot}"));
    let start = line.rfind("[e").unwrap() + 1;
    let end = start + line[start..].find(']').unwrap();
    line[start..end].to_owned()
}

#[test]
fn explore_record_save_and_run_the_test_on_the_recorded_mocks() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let site = Fixtures::shared().a();

    // Explore against the real backend, with refs.
    project.ok(&["open", &format!("{site}/record/shop.html")]);
    project.ok(&["wait", "text=Monitor"]);
    let snapshot = project.ok(&["snapshot", "-i"]);
    // Every product has an "Add" button: the second is the mouse's.
    let add = nth_ref(&snapshot, "button \"Add\"", 1);
    let quantity = nth_ref(&snapshot, "textbox \"Quantity\"", 0);
    let buy = nth_ref(&snapshot, "button \"Buy\"", 0);
    project.ok(&["click", &add]);
    project.ok(&["fill", &quantity, "2"]);
    project.ok(&["click", &buy]);
    project.ok(&[
        "expect",
        "role=status",
        "text",
        "Order o-17: ordered, 2 items",
    ]);
    // A mistake the test should not repeat.
    assert_eq!(project.run(&["click", "e999"]).status.code(), Some(1));

    // Record the API.
    let recorded = project.ok(&[
        "mock",
        "record",
        "--scope",
        "/record/api/**",
        "-o",
        "mocks/recorded.toml",
    ]);
    assert!(
        recorded.contains("mocks/recorded.toml: 2 mocks from 2 requests, 1 data file"),
        "{recorded}"
    );
    let toml = std::fs::read_to_string(project.root.join("mocks/recorded.toml")).unwrap();
    assert!(
        toml.contains("respond = { json_file = \"data/recorded/record/api/products.json\" }"),
        "{toml}"
    );
    assert!(
        toml.contains("# timestamps in the body ($[0].updated_at)"),
        "{toml}"
    );
    assert!(
        toml.contains("respond = { json = { id = \"o-17\", status = \"ordered\" } }"),
        "{toml}"
    );
    project.ok(&["mock", "check", "recorded"]);

    // Save the session as a test.
    let out = project.run(&["history", "--as-scenario", "orders a mouse"]);
    assert_eq!(out.status.code(), Some(0));
    let scenario = stdout(&out);
    assert_eq!(
        scenario,
        "@test \"orders a mouse\"\n  goto /record/shop.html\n  wait text=Monitor\n  click 'role=listitem >> has-text=Mouse >> role=button[name=\"Add\"]'\n  fill 'role=textbox[name=\"Quantity\"]' 2\n  click 'role=button[name=\"Buy\"]'\n  expect role=status text 'Order o-17: ordered, 2 items'\n"
    );
    let err = common::stderr(&out);
    assert!(err.contains("snapshot -i (only reads)"), "{err}");
    assert!(err.contains("click e999 (failed: NotFound)"), "{err}");
    std::fs::write(project.root.join("scenarios/order.cdp"), &scenario).unwrap();
    project.ok(&["close"]);

    // The test passes on the recorded mocks with the backend off: the
    // page comes from a static server of the run's own, which has no API.
    let page = project.root.join("site/record/shop.html");
    std::fs::create_dir_all(page.parent().unwrap()).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/site/record/shop.html"),
        &page,
    )
    .unwrap();
    std::fs::write(project.root.join("site/index.html"), "").unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = project.root.join("chrome-cdp.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        format!(
            "{}\n[e2e.web_server]\nstatic = \"site\"\nurl = \"http://127.0.0.1:{port}\"\n",
            text.replace(site, &format!("http://127.0.0.1:{port}"))
                .replace("[e2e]\n", "[e2e]\ndefault_mocks = [\"recorded\"]\n")
        ),
    )
    .unwrap();
    let out = project.run(&["test"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        common::stderr(&out)
    );
    let requests = std::fs::read_to_string(
        project
            .root
            .join("e2e/.results/order.cdp/orders-a-mouse/requests.txt"),
    )
    .unwrap();
    assert!(
        requests.contains("/record/api/products.json fetch")
            && requests.contains("← mock \"GET /record/api/products.json\" (recorded.toml:8)"),
        "{requests}"
    );
    assert!(
        requests.contains("← mock \"POST /record/api/order.json\" (recorded.toml:13)"),
        "{requests}"
    );
}

#[test]
fn history_lists_commands_and_clears() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let site = Fixtures::shared().a();
    project.ok(&["open", &format!("{site}/record/shop.html")]);
    // Steps of a batch are kept like commands.
    let mut batch = project.sandbox.cli();
    batch
        .current_dir(&project.root)
        .arg("batch")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    let mut child = batch.spawn().unwrap();
    {
        use std::io::Write as _;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(b"wait text=Mouse\ntext css=h1\n").unwrap();
    }
    assert!(child.wait_with_output().unwrap().status.success());
    let listed = project.ok(&["history"]);
    assert!(listed.contains("   1 ✓ open "), "{listed}");
    assert!(
        listed.contains("   2 ✓ wait text=Mouse\n   3 ✓ text css=h1\n"),
        "{listed}"
    );
    let quiet = project.ok(&["history", "--clear", "-q"]);
    assert_eq!(quiet.trim(), "3");
    let listed = project.ok(&["history"]);
    assert!(listed.contains("no commands yet"), "{listed}");
    // The new numbers go on from the old.
    project.ok(&["text", "css=h1"]);
    let listed = project.ok(&["history"]);
    assert!(listed.contains("   4 ✓ text css=h1"), "{listed}");
}

/// Refs inside iframes (a same-site and a cross-site one) are written as
/// a `frame=` step, by the frame's name or URL, then the element's own
/// locator; the scenario replays.
#[test]
fn refs_in_frames_become_frame_locators_that_replay() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let site = Fixtures::shared().a();

    project.ok(&["open", &format!("{site}/read/framed.html")]);
    let snapshot = project.ok(&["snapshot", "-i"]);
    let same = nth_ref(&snapshot, "button \"Pay now\"", 0);
    let other = nth_ref(&snapshot, "button \"Pay now\"", 1);
    project.ok(&["click", &same]);
    project.ok(&["click", &other]);

    let out = project.run(&["history", "--as-scenario", "pays twice"]);
    assert_eq!(out.status.code(), Some(0));
    let scenario = stdout(&out);
    assert_eq!(
        scenario,
        "@test \"pays twice\"\n  goto /read/framed.html\n  click 'frame=same >> role=button[name=\"Pay now\"]'\n  click 'frame=/read/frame.html?from=framed >> role=button[name=\"Pay now\"]'\n"
    );
    std::fs::write(project.root.join("scenarios/pay.cdp"), &scenario).unwrap();
    project.ok(&["close"]);

    let out = project.run(&["test"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        common::stderr(&out)
    );
}

/// The project's `testid_attribute` reaches the daemon's stable locators:
/// two buttons alike but for their `data-test` are told apart by it
/// (written as `css=[data-test=…]`, which replays whatever the config
/// says), and the scenario replays.
#[test]
fn history_uses_the_configured_testid_attribute() {
    let Some(chrome) = chrome_path() else { return };
    let project = Project::new(&chrome);
    let config = project.root.join("chrome-cdp.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, format!("testid_attribute = \"data-test\"\n{text}")).unwrap();
    let site = Fixtures::shared().a();

    project.ok(&["open", &format!("{site}/record/testids.html")]);
    let snapshot = project.ok(&["snapshot", "-i"]);
    let mouse = nth_ref(&snapshot, "button \"Remove\"", 1);
    project.ok(&["click", &mouse]);
    project.ok(&["expect", &mouse, "text", "Removed"]);

    let out = project.run(&["history", "--as-scenario", "removes the mouse"]);
    assert_eq!(out.status.code(), Some(0));
    let scenario = stdout(&out);
    assert_eq!(
        scenario,
        "@test \"removes the mouse\"\n  goto /record/testids.html\n  click 'css=[data-test=\"remove-mouse\"]'\n  expect 'role=button[name=\"Removed\"]' text Removed\n"
    );
    std::fs::write(project.root.join("scenarios/remove.cdp"), &scenario).unwrap();
    project.ok(&["close"]);

    let out = project.run(&["test"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        common::stderr(&out)
    );
}
