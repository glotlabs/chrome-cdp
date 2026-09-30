//! Healing broken locators (plan 18, milestone 18.2).
//!
//! When a step that has a fingerprint ([`super::fingerprints`]) fails and
//! its locator now finds no element (or several), the runner asks the
//! daemon for the elements of the page that best match the fingerprint
//! (`heal_match`). Without `--heal` the failure only names the best one;
//! with it, a `high` match (or a `medium` one with `--heal=medium`) runs
//! the step again with that element's stable locator, and the test goes
//! on. The heals of the tests that passed become `heal.patch`, a unified
//! diff of the scenario and step files ([`plan`]); `--write` applies it.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use protocol::schema::{HealCandidate, TestHeal};
use protocol::{ErrorInfo, Hint};

use super::fingerprints::{normalize, relative};

/// The artifacts' file the patch goes in.
pub(super) const PATCH: &str = "heal.patch";

/// The lowest level `--heal` uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    High,
    Medium,
}

impl Level {
    /// Whether a candidate of level `level` (`high`, `medium`, `low`) is
    /// good enough.
    pub(super) fn accepts(self, level: &str) -> bool {
        match self {
            Self::High => level == "high",
            Self::Medium => level == "high" || level == "medium",
        }
    }
}

/// A step the runner ran with another locator.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Heal {
    /// The file that holds the step, as spans name it (relative to the
    /// current directory).
    pub(super) file: String,
    pub(super) line: u32,
    /// Where the locator's word is in the file, in bytes.
    pub(super) start: usize,
    pub(super) end: usize,
    /// The word as written.
    pub(super) raw: String,
    /// Why the file cannot be patched: the word holds variables, whose
    /// values come from elsewhere.
    pub(super) fixed: Option<String>,
    /// The locator the step has (variables filled in), and the one it
    /// ran with.
    pub(super) from: String,
    pub(super) to: String,
    /// The step's fingerprint key, and the one it has once patched.
    pub(super) key: String,
    pub(super) new_key: String,
    pub(super) confidence: f64,
    pub(super) reasons: Vec<String>,
}

impl Heal {
    /// As `--json` and `steps.jsonl` show it; `unpatched` says why the
    /// patch leaves it out.
    pub(super) fn schema(&self, unpatched: Option<String>) -> TestHeal {
        TestHeal {
            file: self.file.clone(),
            line: self.line,
            from: self.from.clone(),
            to: self.to.clone(),
            confidence: self.confidence,
            reasons: self.reasons.clone(),
            unpatched,
        }
    }

    /// `cart.cdp:5: role=… -> role=… (0.84: same testid and role; similar name)`.
    pub(super) fn line(&self) -> String {
        format!(
            "{}:{}: {} -> {} ({:.2}: {})",
            self.file,
            self.line,
            self.from,
            self.to,
            self.confidence,
            reasons_text(&self.reasons)
        )
    }
}

/// The reasons as a phrase: `same testid, role and container; similar
/// name; text differs`.
pub(super) fn reasons_text(reasons: &[String]) -> String {
    let pick = |prefix: &str, suffix: &str| -> Vec<String> {
        reasons
            .iter()
            .filter_map(|r| {
                r.strip_prefix(prefix)?
                    .strip_suffix(suffix)
                    .map(str::to_owned)
            })
            .collect()
    };
    let list = |items: &[String]| match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    };
    let mut parts = Vec::new();
    let same = pick("same ", "");
    if !same.is_empty() {
        parts.push(format!("same {}", list(&same)));
    }
    let similar = pick("similar ", "");
    if !similar.is_empty() {
        parts.push(format!("similar {}", list(&similar)));
    }
    let differ = pick("", " differs");
    match differ.len() {
        0 => {}
        1 => parts.push(format!("{} differs", differ[0])),
        _ => parts.push(format!("{} differ", list(&differ))),
    }
    for r in reasons {
        if !r.starts_with("same ") && !r.starts_with("similar ") && !r.ends_with(" differs") {
            parts.push(r.clone());
        }
    }
    parts.join("; ")
}

/// `role=button[name="Add to basket"] (confidence 0.84: same testid; …)`,
/// or the element when it has no locator.
pub(super) fn candidate_text(c: &HealCandidate) -> String {
    format!(
        "{} (confidence {:.2}: {})",
        c.locator.as_deref().unwrap_or(&c.element),
        c.confidence,
        reasons_text(&c.reasons)
    )
}

/// `error` of a step whose locator found nothing, with what its
/// fingerprint matches now; `rerun` runs the test again, and `heal` is
/// the run's `--heal` (`None` without it).
pub(super) fn explain(
    error: ErrorInfo,
    best: &HealCandidate,
    rerun: &str,
    heal: Option<Level>,
) -> ErrorInfo {
    match best.level.as_str() {
        "high" | "medium" => {
            let flag = if best.level == "high" {
                "--heal"
            } else {
                "--heal=medium"
            };
            let error = error.note("likely the same element now", candidate_text(best));
            if heal.is_some_and(|h| h.accepts(&best.level)) {
                // --heal tried it; the note says what, the hint stays.
                return error;
            }
            error.hint(Hint::text(format!(
                "run it with {flag} to use it ({rerun} {flag}), or change the locator"
            )))
        }
        _ => error.note(
            "closest element now",
            format!(
                "{} (confidence {:.2}, too low to heal: {})",
                best.element,
                best.confidence,
                reasons_text(&best.reasons)
            ),
        ),
    }
}

/// Why the heal of locator `word` cannot go into the patch: where its
/// value comes from (`include`: the `@include` line of its parameters).
pub(super) fn fixed_reason(word: &protocol::script::Word, include: Option<&str>) -> String {
    let vars: Vec<String> = word
        .vars()
        .map(|v| match (&v.var, include) {
            (protocol::script::Var::Name(_), Some(at)) => {
                format!("{} (set by the @include at {at})", v.var)
            }
            (var, _) => var.to_string(),
        })
        .collect();
    format!(
        "the locator comes from {}, which the patch cannot change",
        vars.join(" and ")
    )
}

/// One file's change of a locator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Change {
    /// The file (absolute), and as the patch names it (relative to the
    /// project root).
    pub(super) path: PathBuf,
    pub(super) name: String,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) old: String,
    pub(super) new: String,
    /// The fingerprint key before and after.
    pub(super) key: String,
    pub(super) new_key: String,
}

/// What goes in the patch, and why the rest does not.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Planned {
    pub(super) changes: Vec<Change>,
    /// By test (the plan's index), then heal: why it is left out.
    pub(super) left_out: BTreeMap<(usize, usize), String>,
}

/// The patch of the heals of each test (by the plan's index, with
/// whether the test passed). A heal is left out when its test failed,
/// its locator comes from a variable, or another test healed the same
/// word differently (then neither goes in). `root` is the project root.
pub(super) fn plan(tests: &[(usize, bool, &[Heal])], root: &Path) -> Planned {
    let mut planned = Planned::default();
    // By file and word: the heals of it.
    let mut words: BTreeMap<(String, usize), Vec<(usize, usize, &Heal)>> = BTreeMap::new();
    for &(test, passed, heals) in tests {
        for (n, heal) in heals.iter().enumerate() {
            if !passed {
                planned.left_out.insert(
                    (test, n),
                    "the test failed after this heal, so the heal is suspect".into(),
                );
            } else if let Some(why) = &heal.fixed {
                planned.left_out.insert((test, n), why.clone());
            } else {
                words
                    .entry((heal.file.clone(), heal.start))
                    .or_default()
                    .push((test, n, heal));
            }
        }
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    for ((file, _), heals) in words {
        let first = heals[0].2;
        if let Some(other) = heals.iter().find(|(_, _, h)| h.to != first.to) {
            for (test, n, heal) in &heals {
                let to = if heal.to == first.to {
                    &other.2.to
                } else {
                    &first.to
                };
                planned.left_out.insert(
                    (*test, *n),
                    format!("another test healed this locator differently, to {to}"),
                );
            }
            continue;
        }
        planned.changes.push(Change {
            path: normalize(&cwd.join(&file)),
            name: relative(root, &file),
            start: first.start,
            end: first.end,
            old: first.raw.clone(),
            new: protocol::format::shell_quote(&first.to).into_owned(),
            key: first.key.clone(),
            new_key: first.new_key.clone(),
        });
    }
    planned
}

/// The new text of each changed file, from the files as they are now:
/// `Err` names a change that no longer applies (the file changed).
pub(super) fn apply(changes: &[Change]) -> Result<Vec<(PathBuf, String, String, String)>, String> {
    let mut by_file: BTreeMap<&Path, Vec<&Change>> = BTreeMap::new();
    for c in changes {
        by_file.entry(&c.path).or_default().push(c);
    }
    let mut out = Vec::new();
    for (path, mut changes) in by_file {
        let old = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let mut new = old.clone();
        changes.sort_by_key(|c| std::cmp::Reverse(c.start));
        for c in &changes {
            if new.get(c.start..c.end) != Some(c.old.as_str()) {
                return Err(format!(
                    "{} changed during the run: `{}` is not where it was",
                    path.display(),
                    c.old
                ));
            }
            new.replace_range(c.start..c.end, &c.new);
        }
        out.push((path.to_path_buf(), changes[0].name.clone(), old, new));
    }
    Ok(out)
}

/// A unified diff of `files` (path, name, old text, new text).
pub(super) fn diff(files: &[(PathBuf, String, String, String)]) -> String {
    let mut text = String::new();
    for (_, name, old, new) in files {
        let diff = similar::TextDiff::from_lines(old, new);
        let _ = write!(
            text,
            "{}",
            diff.unified_diff()
                .context_radius(3)
                .header(&format!("a/{name}"), &format!("b/{name}"))
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heal(file: &str, start: usize, raw: &str, to: &str) -> Heal {
        Heal {
            file: file.into(),
            line: 2,
            start,
            end: start + raw.len(),
            raw: raw.into(),
            fixed: None,
            from: raw.trim_matches('\'').into(),
            to: to.into(),
            key: format!("2:{}", raw.trim_matches('\'')),
            new_key: format!("2:{to}"),
            confidence: 0.9,
            reasons: vec!["same testid".into()],
        }
    }

    #[test]
    fn reasons_read_as_a_phrase() {
        let r = |v: &[&str]| reasons_text(&v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
        assert_eq!(
            r(&["same testid", "same role", "same container", "name differs"]),
            "same testid, role and container; name differs"
        );
        assert_eq!(
            r(&[
                "testid differs",
                "same role",
                "similar name",
                "text differs"
            ]),
            "same role; similar name; testid and text differ"
        );
        assert_eq!(r(&["role of the same family"]), "role of the same family");
    }

    #[test]
    fn heals_of_passed_tests_make_the_patch() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.cdp");
        std::fs::write(
            &file,
            "@test \"t\"\n  click 'text=Add'\n  expect #x visible\n",
        )
        .unwrap();
        let file = file.display().to_string();
        let a = [heal(
            &file,
            18,
            "'text=Add'",
            "role=button[name=\"Add it\"]",
        )];
        let b = [heal(
            &file,
            18,
            "'text=Add'",
            "role=button[name=\"Add it\"]",
        )];
        let mut fixed = heal(&file, 38, "#x", "#y");
        fixed.fixed = Some("the locator comes from ${var:x}".into());
        let c = [fixed];
        let failed = [heal(&file, 38, "#x", "#z")];
        let planned = plan(
            &[
                (0, true, &a),
                (1, true, &b),
                (2, true, &c),
                (3, false, &failed),
            ],
            dir.path(),
        );
        assert_eq!(planned.changes.len(), 1, "one change for both tests");
        assert_eq!(planned.changes[0].name, "a.cdp");
        assert_eq!(planned.changes[0].new, "'role=button[name=\"Add it\"]'");
        assert_eq!(planned.left_out[&(2, 0)], "the locator comes from ${var:x}");
        assert!(planned.left_out[&(3, 0)].starts_with("the test failed"));
        let files = apply(&planned.changes).unwrap();
        assert_eq!(
            files[0].3,
            "@test \"t\"\n  click 'role=button[name=\"Add it\"]'\n  expect #x visible\n"
        );
        let patch = diff(&files);
        assert!(
            patch.starts_with("--- a/a.cdp\n+++ b/a.cdp\n@@ -1,3 +1,3 @@\n"),
            "{patch}"
        );
        assert!(patch.contains("\n-  click 'text=Add'\n+  click 'role=button[name=\"Add it\"]'\n"));
        // The file changed since: nothing applies.
        std::fs::write(dir.path().join("a.cdp"), "@test \"t\"\n  goto /\n").unwrap();
        assert!(
            apply(&planned.changes)
                .unwrap_err()
                .contains("changed during the run")
        );
    }

    #[test]
    fn two_tests_that_heal_a_word_differently_conflict() {
        let a = [heal("s.cdp", 10, "#old", "#new")];
        let b = [heal("s.cdp", 10, "#old", "#other")];
        let planned = plan(&[(0, true, &a), (1, true, &b)], Path::new("/"));
        assert!(planned.changes.is_empty());
        assert_eq!(
            planned.left_out[&(0, 0)],
            "another test healed this locator differently, to #other"
        );
        assert_eq!(
            planned.left_out[&(1, 0)],
            "another test healed this locator differently, to #new"
        );
    }

    #[test]
    fn a_likely_element_is_named_with_a_hint() {
        let best = HealCandidate {
            locator: Some("role=button[name=\"Add to basket\"]".into()),
            element: "button \"Add to basket\"".into(),
            confidence: 0.84,
            level: "high".into(),
            reasons: vec!["same testid".into(), "similar name".into()],
        };
        let error = explain(
            ErrorInfo::new(protocol::ErrorKind::NotFound, "no element"),
            &best,
            "chrome-cdp test cart.cdp:3",
            None,
        );
        assert_eq!(
            error.notes[0].text,
            "role=button[name=\"Add to basket\"] (confidence 0.84: same testid; similar name)"
        );
        assert_eq!(
            error.hint.as_deref(),
            Some(
                "run it with --heal to use it (chrome-cdp test cart.cdp:3 --heal), or change the locator"
            )
        );
        let low = HealCandidate {
            level: "low".into(),
            confidence: 0.4,
            ..best
        };
        let error = explain(
            ErrorInfo::new(protocol::ErrorKind::NotFound, "no element").hint("x"),
            &low,
            "r",
            Some(Level::High),
        );
        assert_eq!(error.notes[0].label, "closest element now");
        assert_eq!(error.hint.as_deref(), Some("x"));
    }
}
