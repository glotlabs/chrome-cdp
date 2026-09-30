//! `explore` (plan 16, milestone 16.6) end to end, against a real Chrome
//! (`CHROME_PATH`) and the fixture site's `explore/` app, whose map is
//! known (`testdata/site/explore/expected.txt`, the text output with and
//! without `--click-buttons`; `UPDATE_GOLDEN=1` rewrites it), and
//! against `examples/e2e-shop` with its mock backend.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::path::{Path, PathBuf};

use common::{Sandbox, chrome_path, code, stderr, stdout};
use fixtures::Fixtures;
use serde_json::Value;

fn start() -> String {
    format!("{}/explore/", Fixtures::shared().a())
}

fn expected_file() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/site/explore/expected.txt")
}

/// The output with what differs from run to run replaced: the origins
/// and how long it took.
fn normalize(text: &str) -> String {
    let site = Fixtures::shared();
    let host_b = site.b().trim_start_matches("http://");
    let text = text.replace(site.a(), "<site>").replace(host_b, "<host-b>");
    text.lines()
        .map(|line| match line.find(" actions, ") {
            Some(at) if line.starts_with("explore ") => {
                let rest = &line[at + " actions, ".len()..];
                let end = rest.find(' ').unwrap_or(rest.len());
                format!("{}<time>{}", &line[..at + " actions, ".len()], &rest[end..])
            }
            _ => line.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every state's (url path, title, dialogs), in order.
fn states(data: &Value) -> Vec<(String, String, Vec<String>)> {
    data["states"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let url = s["url"].as_str().unwrap();
            let path = url::Url::parse(url).unwrap();
            let mut shown = path.path().to_owned();
            if let Some(q) = path.query() {
                shown = format!("{shown}?{q}");
            }
            let dialogs = s["dialogs"]
                .as_array()
                .map(|d| d.iter().map(|x| x.as_str().unwrap().to_owned()).collect())
                .unwrap_or_default();
            (shown, s["title"].as_str().unwrap().to_owned(), dialogs)
        })
        .collect()
}

/// The names of every element a step of the walk clicked.
fn clicked(data: &Value) -> Vec<String> {
    let mut names = Vec::new();
    let steps = data["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| &e["step"])
        .chain(
            data["broken"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| &b["step"]),
        );
    for step in steps {
        if let Some(name) = step["element"]["name"].as_str() {
            names.push(name.to_owned());
        }
    }
    names
}

#[test]
fn maps_the_fixture_and_leaves_the_agents_tabs_alone() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let session = sandbox.session();
    let home = format!("{}/index.html", Fixtures::shared().a());
    session.ok(&["open", &home]);
    let tabs_before = session.json(&["tabs"]);

    let url = start();
    let mut outputs = String::new();
    for extra in [&[][..], &["--click-buttons"][..]] {
        let args = [&["explore", url.as_str()][..], extra].concat();
        let out = session.run(&args);
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        outputs.push_str(&format!(
            "$ chrome-cdp {}\n{}\n",
            normalize(&args.join(" ")),
            normalize(&stdout(&out))
        ));
    }
    let file = expected_file();
    if std::env::var_os("UPDATE_GOLDEN").is_some_and(|v| !v.is_empty() && v != "0") {
        std::fs::write(&file, &outputs).unwrap();
    }
    let expected = std::fs::read_to_string(&file).unwrap();
    assert_eq!(outputs, expected, "UPDATE_GOLDEN=1 accepts it");

    // The agent's tab is as it was, and explore's context is gone.
    assert_eq!(session.json(&["tabs"]), tabs_before);
    let contexts = session.json(&["context", "list"]);
    assert_eq!(contexts["contexts"], Value::Array(Vec::new()), "{contexts}");
}

#[test]
fn never_clicks_the_destructive_button_or_submits_without_the_flags() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let session = sandbox.session();
    session.ok(&["open"]);

    let data = session.json(&["explore", &start(), "--click-buttons"]);
    let names = clicked(&data);
    assert!(!names.iter().any(|n| n == "Delete account"), "{names:?}");
    assert!(!names.iter().any(|n| n == "Send"), "{names:?}");
    assert_eq!(
        data["skipped"]["destructive"][0]["element"]["name"],
        "Delete account"
    );
    assert_eq!(data["skipped"]["forms"][0]["element"]["name"], "Send");
    assert_eq!(data["skipped"]["forms"][0]["from"], "s4");
    // Without --click-buttons, buttons are not candidates at all.
    let data = session.json(&["explore", &start()]);
    assert_eq!(data["skipped"]["destructive"], Value::Array(Vec::new()));
    assert!(!clicked(&data).iter().any(|n| n == "Help"));

    // --submit-forms sends the form; --allow-destructive deletes.
    let data = session.json(&[
        "explore",
        &start(),
        "--click-buttons",
        "--submit-forms",
        "--allow-destructive",
    ]);
    let found = states(&data);
    assert!(
        found
            .iter()
            .any(|(url, title, _)| url == "/explore/sent.html?message=" && title == "Sent"),
        "{found:?}"
    );
    let names = clicked(&data);
    assert!(names.iter().any(|n| n == "Delete account"), "{names:?}");
    assert!(names.iter().any(|n| n == "Send"), "{names:?}");
    assert_eq!(data["skipped"]["forms"], Value::Array(Vec::new()));
}

#[test]
fn each_budget_limit_stops_it() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let session = sandbox.session();
    session.ok(&["open"]);

    let data = session.json(&["explore", &start(), "--max-states", "3"]);
    assert_eq!(data["stopped_by"], "max-states");
    assert_eq!(data["states"].as_array().unwrap().len(), 3);
    assert!(!data["unvisited"].as_array().unwrap().is_empty(), "{data}");

    let data = session.json(&["explore", &start(), "--max-actions", "3"]);
    assert_eq!(data["stopped_by"], "max-actions");
    assert!(data["actions"].as_u64().unwrap() <= 3, "{data}");

    let data = session.json(&["explore", &start(), "--depth", "1"]);
    assert_eq!(data["stopped_by"], "depth");
    let depths: Vec<u64> = data["states"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["depth"].as_u64().unwrap())
        .collect();
    assert_eq!(depths, [0, 1, 1, 1, 1], "{data}");
    // Mug and the team page are two steps away.
    assert!(
        data["unvisited"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["step"]["element"]["name"] == "Mug"),
        "{data}"
    );

    let data = session.json(&["explore", &start(), "--time", "1ms"]);
    assert_eq!(data["stopped_by"], "time");
    assert_eq!(data["states"].as_array().unwrap().len(), 1);

    let data = session.json(&["explore", &start()]);
    assert_eq!(data["stopped_by"], "done");
    assert_eq!(data["unvisited"], Value::Null);
}

#[test]
fn include_and_exclude_choose_the_links_followed() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let session = sandbox.session();
    session.ok(&["open"]);

    let data = session.json(&["explore", &start(), "--exclude", "**/about.html"]);
    let found = states(&data);
    assert!(!found.iter().any(|(url, ..)| url == "/explore/about.html"));
    assert!(
        data["skipped"]["external"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["element"]["name"] == "About"),
        "{data}"
    );

    let data = session.json(&["explore", &start(), "--include", "**/products.html"]);
    let urls: Vec<String> = states(&data).into_iter().map(|(url, ..)| url).collect();
    assert_eq!(urls, ["/explore/", "/explore/products.html"]);
}

#[test]
fn output_dir_holds_the_map_and_a_scenario_that_replays_each_state() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let session = sandbox.session();
    session.ok(&["open"]);
    let dir = sandbox.dir.path().join("map");
    let dir_arg = dir.display().to_string();

    let data = session.json(&[
        "explore",
        &start(),
        "--click-buttons",
        "-o",
        &dir_arg,
        "--screenshots",
    ]);
    let map: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("map.json")).unwrap()).unwrap();
    assert_eq!(map["states"], data["states"]);
    let found = states(&data);
    assert!(found.iter().any(|(_, _, dialogs)| dialogs == &["Help"]));
    for (state, (url, title, dialogs)) in data["states"].as_array().unwrap().iter().zip(&found) {
        let id = state["id"].as_str().unwrap();
        let scenario = dir.join("states").join(format!("{id}.cdp"));
        assert_eq!(state["scenario"], scenario.display().to_string());
        let png = dir.join("states").join(format!("{id}.png"));
        assert_eq!(&std::fs::read(&png).unwrap()[1..4], b"PNG", "{id}");
        session.ok(&["run", &scenario.display().to_string()]);
        let now = session.ok(&[
            "eval",
            "location.pathname + location.search + '|' + document.title + '|' + \
             [...document.querySelectorAll('dialog[open]')].map(d => d.getAttribute('aria-label')).join(',')",
        ]);
        assert_eq!(
            now.trim(),
            format!("{url}|{title}|{}", dialogs.join(",")),
            "{id} replayed from {}",
            std::fs::read_to_string(&scenario).unwrap()
        );
    }

    // The directory must be empty or new.
    let error = session.fails(&["explore", &start(), "-o", &dir_arg], 2, "BadArgs");
    assert!(
        error["message"].as_str().unwrap().contains("not empty"),
        "{error}"
    );
}

#[test]
fn it_sees_the_routes_of_the_current_context_and_exits_1_on_problems() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let session = sandbox.session();
    session.ok(&["open"]);
    // A route of the agent's context: explore's copy answers the same.
    session.ok(&[
        "route",
        "**/explore/team.html",
        "--body",
        "<title>Mocked team</title><h1>Mocked</h1>",
        "--header",
        "Content-Type: text/html",
    ]);
    let data = session.json(&["explore", &start()]);
    let found = states(&data);
    assert!(
        found
            .iter()
            .any(|(url, title, _)| url == "/explore/team.html" && title == "Mocked team"),
        "{found:?}"
    );
    // The broken link and the error: --exit-code exits 1, -q says 2.
    let out = session.run(&["explore", &start(), "--exit-code", "-q"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "2");
    // Without problems it exits 0.
    let out = session.run(&[
        "explore",
        &start(),
        "--exit-code",
        "--exclude",
        "**/missing.html",
        "--exclude",
        "**/products.html",
    ]);
    assert_eq!(code(&out), 0, "{}", stdout(&out));
}

#[test]
fn maps_the_example_shop_with_its_mocks_within_the_default_budget() {
    let Some(chrome) = chrome_path() else { return };
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/e2e-shop");
    // The built app alone: every /api request must be the mocks'.
    let site = Fixtures::start_in(example.join("dist"), 0, 0).unwrap();
    let sandbox = Sandbox::with_chrome(&chrome);
    let session = sandbox.session();
    session.ok(&["open"]);
    let mocks = example.join("e2e/mocks").display().to_string();
    session.ok(&["mock", "use", "--dir", &mocks, "base"]);
    let data = session.json(&["explore", &format!("{}/", site.a())]);
    assert_eq!(data["stopped_by"], "done", "{data}");
    assert_eq!(data["problems"], 0, "{data}");
    assert_eq!(data["failed"], Value::Null, "{data}");
    let urls: Vec<String> = states(&data).into_iter().map(|(url, ..)| url).collect();
    for want in ["/", "/cart", "/login", "/products/1", "/products/2"] {
        assert!(urls.iter().any(|u| u == want), "{want} in {urls:?}");
    }
}
