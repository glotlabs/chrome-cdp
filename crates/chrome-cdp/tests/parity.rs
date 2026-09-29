//! `docs/playwright-parity.md` stays honest (plan 10, milestone 10.3):
//! every row has a status and an agent-critical mark, no agent-critical
//! row is ❌ (the v1 gate), the summary line counts the rows, and every
//! command the chrome-cdp column names exists.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::collections::BTreeSet;
use std::path::Path;

use common::{Sandbox, stdout};

const STATUSES: [&str; 4] = ["✅", "🟡", "❌", "➖"];

/// A table row's cells, split on `|` but not on `\|`.
fn cells(line: &str) -> Vec<String> {
    let inner = line.trim().trim_start_matches('|').trim_end_matches('|');
    let mut out = vec![String::new()];
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                out.last_mut().unwrap().push('|');
                chars.next();
            }
            '|' => out.push(String::new()),
            _ => out.last_mut().unwrap().push(c),
        }
    }
    out.into_iter().map(|c| c.trim().to_owned()).collect()
}

/// `(line number, cells)` of each row of the parity tables: those whose
/// header is `Playwright | chrome-cdp | Status | Agent-critical`.
fn rows(markdown: &str) -> Vec<(usize, Vec<String>)> {
    let mut found = Vec::new();
    let mut in_table = false;
    for (n, line) in markdown.lines().enumerate() {
        if !line.starts_with('|') {
            in_table = false;
            continue;
        }
        let row = cells(line);
        if row == ["Playwright", "chrome-cdp", "Status", "Agent-critical"] {
            in_table = true;
        } else if in_table && !row.iter().all(|c| c.chars().all(|c| c == '-')) {
            found.push((n + 1, row));
        }
    }
    found
}

/// The top-level commands, from `help commands`.
fn commands() -> BTreeSet<String> {
    let sandbox = Sandbox::new();
    let out = common::run(sandbox.cli().args(["help", "commands"]));
    assert_eq!(out.status.code(), Some(0));
    stdout(&out)
        .lines()
        .filter_map(|l| l.strip_prefix("  "))
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_parity_matrix_passes_the_v1_gate() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/playwright-parity.md");
    let markdown = std::fs::read_to_string(path).unwrap();
    let rows = rows(&markdown);
    assert!(rows.len() >= 100, "{} rows", rows.len());
    let commands = commands();
    let mut counts = [0usize; 4];
    for (line, row) in &rows {
        assert_eq!(row.len(), 4, "line {line}: {row:?}");
        let status = STATUSES.iter().position(|s| *s == row[2]);
        let Some(status) = status else {
            panic!(
                "line {line}: status {:?} is not one of {STATUSES:?}",
                row[2]
            );
        };
        counts[status] += 1;
        assert!(
            matches!(row[3].as_str(), "yes" | "no"),
            "line {line}: agent-critical {:?} is not yes or no",
            row[3]
        );
        assert!(
            !(row[2] == "❌" && row[3] == "yes"),
            "line {line}: an agent-critical row is ❌, which fails the v1 gate: {}",
            row[0]
        );
        // A code span whose first word is a plain lowercase word names a command.
        for span in row[1].split('`').skip(1).step_by(2) {
            let word = span.split_whitespace().next().unwrap_or("");
            if word.len() > 1
                && word.starts_with(|c: char| c.is_ascii_lowercase())
                && word.chars().all(|c| c.is_ascii_lowercase() || c == '-')
            {
                assert!(
                    commands.contains(word),
                    "line {line}: `{span}` names no command"
                );
            }
        }
    }
    let summary = format!(
        "Today: {} rows; {} ✅, {} 🟡, {} ❌, {} ➖.",
        rows.len(),
        counts[0],
        counts[1],
        counts[2],
        counts[3]
    );
    assert!(
        markdown.contains(&summary),
        "the summary line should read: {summary}"
    );
}
