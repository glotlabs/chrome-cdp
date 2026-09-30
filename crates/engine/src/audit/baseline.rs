//! Audit baselines (plan 17, milestone 17.4): a JSON file of accepted
//! findings, by key, committed with the project. An entry without
//! `count` accepts any number of its finding; with one, more than that
//! is new. Entries of the categories checked that match nothing are
//! stale; `--update` drops them, adds the new findings and keeps notes.

use std::collections::BTreeMap;
use std::path::Path;

use protocol::schema::{AcceptedFinding, AuditBaselineFile, Finding};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};

use super::rules::CATEGORIES;
use crate::expect::baseline::{read_file, write_file};

/// The format's version.
pub const VERSION: u32 = 1;

/// A problem with a baseline file: what, and where (1-based line and
/// column, when known).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineProblem {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

/// Reads a baseline's text, checking what `lint` checks: the JSON and its
/// fields, the version, keys of a known category with a rule and a
/// place, each key once.
pub fn parse_baseline(text: &str) -> Result<AuditBaselineFile, BaselineProblem> {
    let file: AuditBaselineFile = serde_json::from_str(text).map_err(|e| BaselineProblem {
        message: e.to_string(),
        line: e.line(),
        column: e.column(),
    })?;
    // Where a key is written, for the problem's place.
    let place = |key: &str| {
        let quoted = serde_json::to_string(key).unwrap_or_default();
        text.lines()
            .enumerate()
            .find_map(|(i, line)| line.find(&quoted).map(|at| (i + 1, at + 1)))
            .unwrap_or((1, 1))
    };
    if file.version != VERSION {
        return Err(BaselineProblem {
            message: format!(
                "version {} is not one this chrome-cdp reads; write \"version\": {VERSION}",
                file.version
            ),
            line: 1,
            column: 1,
        });
    }
    let mut seen = std::collections::HashSet::new();
    for entry in &file.accepted {
        let (line, column) = place(&entry.key);
        let mut parts = entry.key.splitn(3, '/');
        let (category, rule, rest) = (parts.next(), parts.next(), parts.next());
        if !category.is_some_and(|c| CATEGORIES.contains(&c))
            || rule.is_none_or(str::is_empty)
            || rest.is_none_or(str::is_empty)
        {
            return Err(BaselineProblem {
                message: format!(
                    "key {:?} is not <category>/<rule>/<where> with a category of {}",
                    entry.key,
                    CATEGORIES.join(", ")
                ),
                line,
                column,
            });
        }
        if !seen.insert(entry.key.as_str()) {
            return Err(BaselineProblem {
                message: format!("key {:?} is accepted twice", entry.key),
                line,
                column,
            });
        }
        if entry.count == Some(0) {
            return Err(BaselineProblem {
                message: format!(
                    "key {:?} has count 0; leave count out to accept any number",
                    entry.key
                ),
                line,
                column,
            });
        }
    }
    Ok(file)
}

/// The baseline at `path`, `None` if there is none yet.
pub(crate) fn read(path: &Path) -> Result<Option<AuditBaselineFile>, ErrorInfo> {
    let Some(bytes) = read_file(path)? else {
        return Ok(None);
    };
    parse_baseline(&String::from_utf8_lossy(&bytes))
        .map(Some)
        .map_err(|p| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!(
                    "the audit baseline {}:{}:{} is not valid: {}",
                    path.display(),
                    p.line,
                    p.column,
                    p.message
                ),
            )
            .hint(Hint::run(Cmd::new("schema").raw("audit-baseline")).why("its format"))
        })
}

/// Writes `file` to `path`, formatted, with a line break at the end.
pub(crate) fn write(path: &Path, file: &AuditBaselineFile) -> Result<(), ErrorInfo> {
    let mut text = serde_json::to_string_pretty(file).unwrap_or_default();
    text.push('\n');
    write_file(path, text.as_bytes())
}

/// Each key's count over `findings`.
fn totals(findings: &[Finding]) -> BTreeMap<&str, u32> {
    let mut totals = BTreeMap::new();
    for f in findings {
        *totals.entry(f.key.as_str()).or_insert(0) += f.count;
    }
    totals
}

/// Whether `key` is of a category in `checked`.
fn checked_key(key: &str, checked: &[&str]) -> bool {
    key.split('/').next().is_some_and(|c| checked.contains(&c))
}

/// Marks the findings `baseline` accepts, and returns its stale keys:
/// entries of the categories `checked` that no finding has (at any
/// severity: `all` are every finding found).
pub(crate) fn apply(
    baseline: &AuditBaselineFile,
    all: &[Finding],
    shown: &mut [Finding],
    checked: &[&str],
) -> Vec<String> {
    let totals = totals(all);
    for f in shown.iter_mut() {
        let total = totals.get(f.key.as_str()).copied().unwrap_or(0);
        f.accepted = baseline
            .accepted
            .iter()
            .any(|a| a.key == f.key && a.count.is_none_or(|n| total <= n));
    }
    baseline
        .accepted
        .iter()
        .filter(|a| checked_key(&a.key, checked) && !totals.contains_key(a.key.as_str()))
        .map(|a| a.key.clone())
        .collect()
}

/// The baseline `--update` writes: every key of `wanted` (the findings at
/// `--min-severity` or above), with the old entry's note, and its count
/// raised to what was found; old entries of categories not `checked`, or
/// whose findings are still there (`all`) but below the severity, stay.
pub(crate) fn updated(
    old: Option<&AuditBaselineFile>,
    all: &[Finding],
    wanted: &[Finding],
    checked: &[&str],
) -> AuditBaselineFile {
    let totals = totals(all);
    let mut entries: BTreeMap<String, AcceptedFinding> = BTreeMap::new();
    if let Some(old) = old {
        for a in &old.accepted {
            if !checked_key(&a.key, checked) || totals.contains_key(a.key.as_str()) {
                entries.insert(a.key.clone(), a.clone());
            }
        }
    }
    for f in wanted {
        let total = totals.get(f.key.as_str()).copied().unwrap_or(f.count);
        entries
            .entry(f.key.clone())
            .and_modify(|a| {
                if let Some(n) = &mut a.count {
                    *n = (*n).max(total);
                }
            })
            .or_insert_with(|| AcceptedFinding {
                key: f.key.clone(),
                count: None,
                note: None,
            });
    }
    AuditBaselineFile {
        version: VERSION,
        accepted: entries.into_values().collect(),
    }
}

#[cfg(test)]
mod tests {
    use protocol::schema::{FindingTarget, Severity};

    use super::*;

    fn found(key: &str, count: u32) -> Finding {
        Finding {
            category: key.split('/').next().unwrap_or_default().into(),
            rule: "r".into(),
            severity: Severity::Warning,
            message: String::new(),
            target: FindingTarget::default(),
            key: key.into(),
            count,
            docs: None,
            wcag: None,
            tab: None,
            accepted: false,
        }
    }

    #[test]
    fn parsing_checks_keys_and_version() {
        let file = parse_baseline(
            r#"{"version": 1, "accepted": [
                {"key": "security/no-csp//checkout", "note": "CDN move"},
                {"key": "issues/deprecation-unload-handler/self/app.js:3", "count": 1}
            ]}"#,
        )
        .unwrap();
        assert_eq!(file.accepted.len(), 2);
        assert_eq!(file.accepted[1].count, Some(1));
        let bad = parse_baseline("{\"version\": 1,\n \"accepted\": [{\"key\": \"nope/x\"}]}")
            .unwrap_err();
        assert_eq!((bad.line, bad.column), (2, 23));
        assert!(bad.message.contains("<category>/<rule>/<where>"));
        let twice =
            r#"{"version": 1, "accepted": [{"key": "meta/no-lang//"}, {"key": "meta/no-lang//"}]}"#;
        assert!(parse_baseline(twice).unwrap_err().message.contains("twice"));
        let unknown =
            parse_baseline(r#"{"version": 1, "accepted": [{"key": "meta/no-lang//", "why": 1}]}"#)
                .unwrap_err();
        assert!(
            unknown.message.contains("unknown field"),
            "{}",
            unknown.message
        );
        assert!(
            parse_baseline(r#"{"version": 2}"#)
                .unwrap_err()
                .message
                .contains("version 2")
        );
    }

    #[test]
    fn accepting_counts_and_stale_entries() {
        let baseline = AuditBaselineFile {
            version: 1,
            accepted: vec![
                AcceptedFinding {
                    key: "console/console-error/a".into(),
                    count: Some(2),
                    note: None,
                },
                AcceptedFinding {
                    key: "meta/no-lang//".into(),
                    count: None,
                    note: Some("later".into()),
                },
                AcceptedFinding {
                    key: "meta/no-title//".into(),
                    count: None,
                    note: None,
                },
                AcceptedFinding {
                    key: "a11y/image-alt/#logo".into(),
                    count: None,
                    note: None,
                },
            ],
        };
        let all = vec![
            found("console/console-error/a", 3),
            found("meta/no-lang//", 7),
        ];
        let mut shown = all.clone();
        let stale = apply(&baseline, &all, &mut shown, &["console", "meta"]);
        // 3 errors where 2 are accepted: new.
        assert!(!shown[0].accepted);
        assert!(shown[1].accepted);
        // a11y was not checked: not stale.
        assert_eq!(stale, ["meta/no-title//"]);
        let new = updated(Some(&baseline), &all, &all, &["console", "meta"]);
        let keys: Vec<(&str, Option<u32>, Option<&str>)> = new
            .accepted
            .iter()
            .map(|a| (a.key.as_str(), a.count, a.note.as_deref()))
            .collect();
        assert_eq!(
            keys,
            [
                ("a11y/image-alt/#logo", None, None),
                ("console/console-error/a", Some(3), None),
                ("meta/no-lang//", None, Some("later")),
            ]
        );
    }
}
