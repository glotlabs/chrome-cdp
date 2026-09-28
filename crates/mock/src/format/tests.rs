use std::path::{Path, PathBuf};

use super::format;

/// Formats `src`, checking that the values stay and that formatting again
/// changes nothing.
fn check(src: &str) -> String {
    let out = format(src).unwrap_or_else(|e| panic!("{e}\n{src}"));
    let before: toml::Table = toml::from_str(src).unwrap();
    let after: toml::Table = toml::from_str(&out).unwrap();
    assert_eq!(before, after, "{out}");
    assert_eq!(format(&out).unwrap(), out, "not idempotent:\n{out}");
    out
}

const MESSY: &str = r#"#:schema ./schema.json
# The shop's API.


[settings]
  scope = ["/api/**"]
unmatched="fail"
[[mock]]
respond = {status=200,json={ price = 49 }}   # the price
  # which request
match = { method = "GET", path = "/api/price" }
name = "price"

[[mock]]
sequence = [
  { status = 503 },   # busy first
  { json = { ok = true } },
]
match = { method = "POST", path = "/api/orders" }
when = { logged_in = true }



[[collection]]
seed = "data/todos.json"
path = "/api/todos"
name = "todos"
"#;

const TIDY: &str = r#"#:schema ./schema.json
# The shop's API.

[settings]
scope = ["/api/**"]
unmatched = "fail"

[[mock]]
name = "price"
# which request
match = { method = "GET", path = "/api/price" }
respond = { status = 200, json = { price = 49 } }   # the price

[[mock]]
match = { method = "POST", path = "/api/orders" }
sequence = [
  { status = 503 },   # busy first
  { json = { ok = true } },
]
when = { logged_in = true }

[[collection]]
name = "todos"
path = "/api/todos"
seed = "data/todos.json"
"#;

#[test]
fn formats_a_messy_mock_file() {
    assert_eq!(check(MESSY), TIDY);
}

#[test]
fn keeps_comments() {
    let out = check(MESSY);
    for comment in [
        "#:schema",
        "# The shop's API.",
        "# which request",
        "# the price",
        "# busy first",
    ] {
        assert!(out.contains(comment), "{comment} lost:\n{out}");
    }
}

#[test]
fn keeps_sub_tables_and_dotted_keys() {
    let src =
        "[[mock]]\nrespond.status = 201\nmatch.path = \"/a\"\n[mock.respond.headers]\nx = \"1\"\n";
    let out = check(src);
    assert!(
        out.find("match.path").unwrap() < out.find("respond.status").unwrap(),
        "{out}"
    );
}

#[test]
fn reports_bad_toml() {
    let e = format("[[mock]\n").unwrap_err();
    assert!(e.contains("line 1"), "{e}");
}

#[test]
fn empty_files_stay_empty() {
    assert_eq!(check(""), "");
    assert_eq!(check("\n\n"), "");
}

/// Every mock file of the repository's fixtures and docs.
#[test]
fn every_fixture_formats_and_keeps_its_values() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for dir in ["testdata", "examples", "crates/e2e/src/init"] {
        collect(&root.join(dir), &mut files);
    }
    assert!(!files.is_empty());
    for file in files {
        let src = std::fs::read_to_string(&file).unwrap();
        if toml::from_str::<toml::Table>(&src).is_err() {
            continue;
        }
        check(&src);
    }
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "toml") {
            out.push(path);
        }
    }
}
