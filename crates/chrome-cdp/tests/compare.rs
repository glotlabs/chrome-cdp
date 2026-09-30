//! `compare` (plan 16, milestone 16.5) end to end, against a real Chrome
//! (`CHROME_PATH`) and the fixture site's `compare/` pages, served on
//! both fixture origins: a.html and b.html differ in exactly a heading's
//! text, an added button, a removed link, a moved block and a colour only
//! `--visual` sees.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use common::{Sandbox, chrome_path, code, json_out, stderr, stdout};
use fixtures::Fixtures;
use serde_json::Value;

fn a_url() -> String {
    format!("{}/compare/a.html", Fixtures::shared().a())
}

fn b_url() -> String {
    format!("{}/compare/b.html", Fixtures::shared().b())
}

/// The changes of a `compare --json` answer as `(mark, role, name)`, in
/// order, the topmost of each added subtree only.
fn changes(data: &Value) -> Vec<(String, String, String)> {
    fn walk(nodes: &Value, out: &mut Vec<(String, String, String)>) {
        for node in nodes.as_array().into_iter().flatten() {
            if let Some(change) = node["change"].as_str() {
                out.push((
                    change.to_owned(),
                    node["role"].as_str().unwrap_or_default().to_owned(),
                    node["name"].as_str().unwrap_or_default().to_owned(),
                ));
                if change == "added" {
                    continue;
                }
            }
            walk(&node["children"], out);
        }
    }
    let mut out = Vec::new();
    walk(&data["snapshot"]["diff"], &mut out);
    out
}

/// The known differences of a.html and b.html.
fn expected() -> Vec<(String, String, String)> {
    [
        ("changed", "heading", "Your cart (3)"),
        ("removed", "region", "Offers"),
        ("removed", "link", "Continue shopping"),
        ("added", "button", "Apply coupon"),
        ("added", "region", "Offers"),
    ]
    .iter()
    .map(|(c, r, n)| ((*c).to_owned(), (*r).to_owned(), (*n).to_owned()))
    .collect()
}

/// A PNG file's width and height, from its header.
fn png_size(path: &str) -> (u32, u32) {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[1..4], b"PNG", "{path}");
    let at = |i: usize| u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap());
    (at(16), at(20))
}

/// Whether a region of `visual` holds the point.
fn covered(visual: &Value, (x, y): (u64, u64)) -> bool {
    visual["regions"].as_array().unwrap().iter().any(|r| {
        let (rx, ry) = (r["x"].as_u64().unwrap(), r["y"].as_u64().unwrap());
        let (w, h) = (r["width"].as_u64().unwrap(), r["height"].as_u64().unwrap());
        (rx..rx + w).contains(&x) && (ry..ry + h).contains(&y)
    })
}

#[test]
fn urls_on_two_origins_differ_in_exactly_the_known_ways() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let session = sandbox.session();
    session.ok(&["open"]);
    let data = session.json(&["compare", &a_url(), &b_url()]);
    assert_eq!(changes(&data), expected(), "{data}");
    assert_eq!(data["snapshot"]["changes"], 5);
    assert_eq!(data["differs"], true);
    assert!(data.get("visual").is_none());
    // The URLs' tabs are closed again.
    let tabs = session.json(&["tabs"]);
    assert_eq!(tabs["tabs"].as_array().unwrap().len(), 1, "{tabs}");

    // --exit-code: 1 when they differ, 0 when not, and the text either way.
    let out = session.run(&["compare", &a_url(), &b_url(), "--exit-code"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("snapshot: 5 changes"),
        "{}",
        stdout(&out)
    );
    let same = format!("{}/compare/a.html", Fixtures::shared().b());
    let out = session.run(&["compare", &a_url(), &same, "--exit-code"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("snapshot: no changes"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn visual_regions_cover_every_difference_and_name_them_on_b() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let session = sandbox.session();
    session.ok(&["open", &a_url()]);
    session.ok(&["tab", "new", &b_url()]);
    let data = session.json(&["compare", "tab:t1", "tab:t2", "--visual"]);
    let visual = &data["visual"];
    let count = visual["region_count"].as_u64().unwrap();
    assert!((1..=5).contains(&count), "{visual}");
    // The heading's digit, the block's two places, the total's colour,
    // and the link's place where the button now is.
    for point in [(160, 30), (100, 100), (100, 420), (40, 238), (40, 288)] {
        assert!(covered(visual, point), "{point:?} in {visual}");
    }
    // Named on B, with refs of this session.
    let named: Vec<String> = visual["regions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["element"]["name"].as_str().map(str::to_owned))
        .collect();
    assert!(named.iter().any(|n| n == "Apply coupon"), "{named:?}");
    assert!(named.iter().any(|n| n == "Your cart (3)"), "{named:?}");
    let button = visual["regions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["element"]["name"] == "Apply coupon")
        .unwrap();
    let reference = button["element"]["ref"].as_str().unwrap();
    assert!(
        session
            .ok(&["--tab", "t2", "attr", reference, "class"])
            .contains("next")
    );
    // The image is of the area compared.
    let image = visual["image"].as_str().unwrap();
    assert_eq!(
        png_size(image),
        (
            u32::try_from(visual["width"].as_u64().unwrap()).unwrap(),
            u32::try_from(visual["height"].as_u64().unwrap()).unwrap()
        )
    );
    // Only the colour: the snapshots agree, the pixels do not.
    session.ok(&["--tab", "t1", "goto", &b_url()]);
    session.ok(&[
        "--tab",
        "t2",
        "eval",
        "document.querySelector('.total').style.color = 'blue'",
    ]);
    let data = session.json(&["compare", "tab:t1", "tab:t2", "--visual"]);
    assert_eq!(data["snapshot"]["changes"], 0);
    assert_eq!(data["visual"]["region_count"], 1, "{data}");
    assert_eq!(
        data["visual"]["regions"][0]["element"]["name"],
        "Total: $12"
    );
    let out = session.run(&["compare", "tab:t1", "tab:t1", "--visual", "--exit-code"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("visual: no pixels differ"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn a_tab_a_url_interactive_and_a_scope() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let session = sandbox.session();
    session.ok(&["open", &a_url()]);
    let data = session.json(&["compare", "tab:t1", &b_url(), "-i"]);
    let only: Vec<_> = expected()
        .into_iter()
        .filter(|(_, role, _)| role == "link" || role == "button")
        .collect();
    assert_eq!(changes(&data), only, "{data}");
    assert_eq!(data["a"]["tab"], "t1");
    assert!(data["b"].get("tab").is_none(), "{data}");
    // --scope on both pages: the navigation is left out, the main kept.
    let data = session.json(&["compare", "tab:t1", &b_url(), "--scope", "role=main"]);
    assert_eq!(changes(&data), expected(), "{data}");
    let diff = data["snapshot"]["diff"].as_array().unwrap();
    assert_eq!(diff.len(), 1, "{data}");
    assert_eq!(diff[0]["role"], "main", "{data}");
    let error = session.fails(
        &["compare", "tab:t1", &b_url(), "--scope", "css=#nothing"],
        1,
        "NotFound",
    );
    assert!(
        error["message"].as_str().unwrap().starts_with("tab:t1: "),
        "{error}"
    );
}

#[test]
fn a_snapshot_file_and_another_session() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let session = sandbox.session();
    let other = sandbox.session();
    session.ok(&["open", &a_url()]);
    other.ok(&["open", &b_url()]);
    let file = sandbox.dir.path().join("a.json");
    std::fs::write(&file, session.ok(&["snapshot", "--json"])).unwrap();
    let file = file.display().to_string();

    // The file against the other session's page.
    let side = format!("session:{}", other.name);
    let data = session.json(&["compare", &file, &side]);
    assert_eq!(changes(&data), expected(), "{data}");
    assert_eq!(data["a"]["side"], file);
    assert_eq!(data["b"]["side"], side);
    // A file has no pixels.
    session.fails(&["compare", &file, &side, "--visual"], 2, "BadArgs");
    // Nor does it take --scope: a warning says so.
    let out = session.run(&["compare", &file, "tab:t1", "--scope", "role=main"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stderr(&out).contains("does not apply"), "{}", stderr(&out));

    // --visual with a session side: its regions have its refs.
    let data = session.json(&["compare", "tab:t1", &side, "--visual"]);
    let regions = data["visual"]["regions"].as_array().unwrap();
    let button = regions
        .iter()
        .find(|r| r["element"]["name"] == "Apply coupon")
        .unwrap_or_else(|| panic!("{data}"));
    let reference = button["element"]["ref"].as_str().unwrap();
    assert!(other.ok(&["attr", reference, "class"]).contains("next"));

    // A session that is not running.
    let out = session.run(&["compare", "tab:t1", "session:nobody", "--json"]);
    assert_eq!(code(&out), 4, "{}", stdout(&out));
    assert_eq!(json_out(&out)["error"]["kind"], "DaemonUnavailable");
}

#[test]
fn viewports_that_differ() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let session = sandbox.session();
    session.ok(&["open", &a_url()]);
    session.ok(&["viewport", "900x700"]);
    // A URL side gets A's viewport.
    let data = session.json(&["compare", "tab:t1", &b_url(), "--visual"]);
    assert_eq!(
        (&data["visual"]["width"], &data["visual"]["height"]),
        (&Value::from(900), &Value::from(700)),
        "{data}"
    );
    // A tab of another size: the area they share, with a warning.
    let out = session.run(&[
        "compare",
        "tab:t1",
        &b_url(),
        "--visual",
        "--viewport",
        "600x500",
        "--json",
    ]);
    assert_eq!(code(&out), 0, "{}", stdout(&out));
    let envelope = json_out(&out);
    let visual = &envelope["data"]["visual"];
    assert!(visual["width"].as_u64().unwrap() <= 600, "{visual}");
    assert!(visual["height"].as_u64().unwrap() <= 500, "{visual}");
    let warnings = envelope["warnings"].to_string();
    assert!(warnings.contains("the viewports differ"), "{warnings}");
    // --viewport needs --visual.
    session.fails(
        &["compare", "tab:t1", &b_url(), "--viewport", "600x500"],
        2,
        "BadArgs",
    );
}
