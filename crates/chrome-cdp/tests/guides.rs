//! The user docs stay honest (plan 10, milestone 10.6): every relative
//! link in `README.md` and `docs/` (plans aside) reaches a file and, with a
//! `#fragment`, one of its headings; every command a code block runs
//! exists; and the troubleshooting guide has a section per error kind.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use common::{Sandbox, stdout};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `README.md` and every Markdown file under `docs/` but the plans and the
/// generated command reference.
fn pages() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if path.is_dir() {
                if name != "plans" && name != "commands" {
                    walk(&path, out);
                }
            } else if path.extension().is_some_and(|e| e == "md") {
                out.push(path);
            }
        }
    }
    let root = root();
    let mut pages = vec![root.join("README.md")];
    walk(&root.join("docs"), &mut pages);
    pages.sort();
    pages
}

/// The text outside fenced code blocks, and the lines inside them.
fn split(markdown: &str) -> (String, Vec<String>) {
    let mut prose = String::new();
    let mut code = Vec::new();
    let mut fenced = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            prose.push('\n');
        } else if fenced {
            code.push(line.to_owned());
            prose.push('\n');
        } else {
            prose.push_str(line);
            prose.push('\n');
        }
    }
    (prose, code)
}

/// The targets of `[text](target)` links in `prose`, with their line.
fn links(prose: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (n, line) in prose.lines().enumerate() {
        let mut rest = line;
        while let Some(i) = rest.find("](") {
            let after = &rest[i + 2..];
            let Some(end) = after.find(')') else { break };
            out.push((n + 1, after[..end].to_owned()));
            rest = &after[end..];
        }
    }
    out
}

/// GitHub's anchor for a heading.
fn slug(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

/// The headings' anchors, and the ids of `<a id="…">` tags.
fn anchors(markdown: &str) -> BTreeSet<String> {
    let prose = split(markdown).0;
    let mut out: BTreeSet<String> = prose
        .lines()
        .filter(|l| l.starts_with('#'))
        .map(|l| slug(l.trim_start_matches('#')))
        .collect();
    for part in prose.split("<a id=\"").skip(1) {
        if let Some((id, _)) = part.split_once('"') {
            out.insert(id.to_owned());
        }
    }
    out
}

#[test]
fn relative_links_reach_files_and_headings() {
    let mut broken = Vec::new();
    for page in pages() {
        let text = fs::read_to_string(&page).unwrap();
        let dir = page.parent().unwrap();
        for (line, target) in links(&split(&text).0) {
            if target.contains("://") || target.starts_with("mailto:") {
                continue;
            }
            let (file, fragment) = target.split_once('#').unwrap_or((&target, ""));
            let path = if file.is_empty() {
                page.clone()
            } else {
                dir.join(file)
            };
            let at = format!("{}:{line}: {target}", page.display());
            if !path.exists() {
                broken.push(format!("{at} (no such file)"));
            } else if !fragment.is_empty() && path.extension().is_some_and(|e| e == "md") {
                let heads = anchors(&fs::read_to_string(&path).unwrap());
                if !heads.contains(fragment) {
                    broken.push(format!("{at} (no such heading)"));
                }
            }
        }
    }
    assert!(broken.is_empty(), "broken links:\n{}", broken.join("\n"));
}

/// Top-level commands, from `help commands`.
fn commands() -> BTreeSet<String> {
    let sandbox = Sandbox::new();
    let out = common::run(sandbox.cli().args(["help", "commands"]));
    assert_eq!(out.status.code(), Some(0));
    stdout(&out)
        .lines()
        .filter(|l| l.starts_with("  "))
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

#[test]
fn code_blocks_run_real_commands() {
    let commands = commands();
    let mut unknown = Vec::new();
    for page in pages() {
        let text = fs::read_to_string(&page).unwrap();
        for line in split(&text).1 {
            let line = line.trim_start().trim_start_matches("$ ");
            let Some(rest) = line.strip_prefix("chrome-cdp ") else {
                continue;
            };
            // Global flags may come first: `chrome-cdp --session s open`.
            let word = rest
                .split_whitespace()
                .find(|w| !w.starts_with('-'))
                .unwrap_or("");
            if rest.trim_start().starts_with('-') || commands.contains(word) {
                continue;
            }
            unknown.push(format!("{}: {line}", page.display()));
        }
    }
    assert!(
        unknown.is_empty(),
        "no such command:\n{}",
        unknown.join("\n")
    );
}

/// Every `chrome-cdp …` example of the command reference (the commands'
/// `--help`) parses the way `lint` parses a `.cdp` line: its flags and
/// locators are valid. Lines with shell syntax are skipped, and so are
/// the problems that only concern scripts (refs, `--json`, commands a
/// script cannot run).
#[test]
fn help_examples_parse() {
    let mut examples = Vec::new();
    let mut dir: Vec<PathBuf> = fs::read_dir(root().join("docs/commands"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    dir.sort();
    for page in dir {
        let text = fs::read_to_string(&page).unwrap();
        for line in split(&text).1 {
            // Examples are indented; `Usage:` lines are not.
            let Some(rest) = line.strip_prefix("  chrome-cdp ") else {
                continue;
            };
            let command = rest.split("  #").next().unwrap().trim();
            if command.is_empty() || command.contains(['|', '<', '>', ';', '&', '$', '`']) {
                continue;
            }
            examples.push((page.file_name().unwrap().to_owned(), command.to_owned()));
        }
    }
    assert!(examples.len() > 300, "{} examples", examples.len());

    let sandbox = Sandbox::new();
    let script = sandbox.tmp().join("examples.cdp");
    let lines: Vec<&str> = examples.iter().map(|(_, c)| c.as_str()).collect();
    fs::write(&script, lines.join("\n")).unwrap();
    let out = common::run(
        sandbox
            .cli()
            .args(["lint", "--json", "--max-chars", "0"])
            .arg(&script),
    );
    let json: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let problems = json["error"]["details"]["problems"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let script_only = |p: &serde_json::Value| {
        let message = p["message"].as_str().unwrap_or("");
        matches!(p["rule"].as_str(), Some("ref" | "unknown-command"))
            || message.contains("cannot run inside a script")
            || message.contains("--json on a line")
    };
    let bad: Vec<String> = problems
        .iter()
        .filter(|p| !script_only(p))
        .map(|p| {
            let line = usize::try_from(p["line"].as_u64().unwrap()).unwrap();
            let (page, command) = &examples[line - 1];
            format!(
                "{}: chrome-cdp {command}\n  {}",
                page.display(),
                p["message"]
            )
        })
        .collect();
    assert!(
        bad.is_empty(),
        "help examples that do not parse:\n{}",
        bad.join("\n")
    );
}

#[test]
fn troubleshooting_covers_every_error_kind() {
    let text = fs::read_to_string(root().join("docs/troubleshooting.md")).unwrap();
    for kind in protocol::ErrorKind::ALL {
        let name = format!("{kind:?}");
        assert!(
            text.contains(&format!("\n### {name}\n")),
            "docs/troubleshooting.md has no section for {name}"
        );
    }
}
