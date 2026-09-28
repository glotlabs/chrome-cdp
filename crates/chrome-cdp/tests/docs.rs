//! The E2E docs' examples are valid (plan 11, milestone 11.10): every
//! mock file and scenario block of `docs/e2e/recipes.md` and
//! `docs/e2e/migrating.md` whose first line names its file (`# e2e/…`)
//! is written into a project, and `fmt --check` and `lint` pass on it.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::path::Path;

use common::{Sandbox, stderr, stdout};

/// `(file, text)` of each ```` ```toml ```` or ```` ```cdp ```` block whose
/// first line is `# e2e/<path>`.
fn blocks(markdown: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut rest = markdown;
    while let Some(start) = rest.find("\n```") {
        let after = &rest[start + 4..];
        let Some((lang, body)) = after.split_once('\n') else {
            break;
        };
        let Some(end) = body.find("\n```") else {
            break;
        };
        let text = &body[..=end];
        if matches!(lang, "toml" | "cdp")
            && let Some(first) = text.lines().next()
            && let Some(path) = first.strip_prefix("# ")
            && path.starts_with("e2e/")
        {
            found.push((path.to_owned(), text.to_owned()));
        }
        rest = &body[end + 4..];
    }
    found
}

/// Data files the mock files name (`json_file`, `seed`), made empty, so
/// `lint` finds them.
fn data_files(root: &Path, toml: &str) {
    for key in ["json_file = \"", "seed = \""] {
        for part in toml.split(key).skip(1) {
            let file = part.split('"').next().unwrap();
            if file.contains("{{") {
                continue;
            }
            let path = root.join("e2e/mocks").join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "[]").unwrap();
        }
    }
}

fn check(doc: &str, at_least: usize) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/e2e")
        .join(doc);
    let markdown = std::fs::read_to_string(&path).unwrap();
    let sandbox = Sandbox::new();
    let root = sandbox.dir.path().join("app");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("chrome-cdp.toml"),
        "[e2e]\nbase_url = \"http://localhost:3000\"\ndefault_mocks = []\n",
    )
    .unwrap();
    let found = blocks(&markdown);
    assert!(found.len() >= at_least, "{doc}: {found:?}");
    for (file, text) in &found {
        let target = root.join(file);
        assert!(!target.exists(), "{doc}: {file} twice");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, text).unwrap();
        if Path::new(file).extension().is_some_and(|e| e == "toml") {
            data_files(&root, text);
        }
    }
    let out = common::run(sandbox.cli().current_dir(&root).args(["fmt", "--check"]));
    assert_eq!(out.status.code(), Some(0), "{doc}: {}", stderr(&out));
    let out = common::run(sandbox.cli().current_dir(&root).arg("lint"));
    assert!(
        out.status.code() == Some(0) && stdout(&out).ends_with(" 0 errors, 0 warnings\n"),
        "{doc}: {}{}",
        stdout(&out),
        stderr(&out)
    );
}

#[test]
fn the_recipes_are_valid() {
    check("recipes.md", 15);
}

#[test]
fn the_migration_examples_are_valid() {
    check("migrating.md", 3);
}
