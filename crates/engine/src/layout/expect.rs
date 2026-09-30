//! `expect --layout --baseline NAME` (plan 16, milestone 16.2): fails
//! when `layout` finds error findings that the baseline does not list.
//!
//! The baseline is a text file of finding keys, one per line (`#` starts
//! a comment), at `<dir>/layout/<NAME>.txt`: the project's `.chrome-cdp/`
//! or, under the test runner, `e2e/snapshots/`. Like the other matchers
//! it checks the page again until it passes or the timeout ends, so a
//! page still settling does not fail it. `--update` writes the current
//! findings instead. Keys the baseline lists that are no longer found
//! are named in a warning, so the file can be cleaned up.

use std::collections::BTreeSet;
use std::time::Duration;

use protocol::schema::{BaselineResult, ExpectData, ExpectLayout, LayoutSeverity};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde_json::json;
use tokio::time::Instant;

use super::Linted;
use crate::Engine;
use crate::context::Scoped as _;
use crate::expect::baseline::{check_name as check_baseline_name, read_file, write_file};
use crate::request::{LayoutBaseline, LayoutRequest, millis};
use crate::tabs::tab_name;

/// How long to wait before checking again.
const RETRY_DELAY: Duration = Duration::from_millis(100);

/// The first lines of a baseline `--update` writes.
const HEADER: &str = "# expect --layout baseline: one finding key per line, \
                      `<kind> <viewport> <locator>`.\n\
                      # Findings listed here are accepted; `--update` rewrites the list.\n";

/// Checks a baseline's name.
pub(super) fn check_name(name: &str) -> Result<(), ErrorInfo> {
    check_baseline_name(
        name,
        Cmd::new("expect")
            .switch("layout")
            .flag("baseline", "checkout"),
    )
}

/// The keys a baseline lists, and its comment lines (kept by `--update`).
fn parse(text: &str) -> (BTreeSet<String>, Vec<String>) {
    let mut keys = BTreeSet::new();
    let mut comments = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            comments.push(line.to_owned());
        } else if !line.is_empty() {
            keys.insert(line.to_owned());
        }
    }
    (keys, comments)
}

/// A baseline's text: its comments (or the header), then the keys.
fn render(comments: &[String], keys: &BTreeSet<String>) -> String {
    let mut out = if comments.is_empty() {
        HEADER.to_owned()
    } else {
        comments.iter().fold(String::new(), |mut out, c| {
            out.push_str(c);
            out.push('\n');
            out
        })
    };
    for key in keys {
        out.push_str(key);
        out.push('\n');
    }
    out
}

/// The command again, for hints.
fn command(req: &LayoutRequest, baseline: &LayoutBaseline) -> Cmd {
    let mut cmd = Cmd::new("expect").switch("layout");
    if let Some(locator) = &req.locator {
        cmd = cmd.arg(locator);
    }
    cmd = cmd.flag("baseline", &baseline.name);
    if !req.viewports.is_empty() {
        cmd = cmd.flag("viewports", req.viewports.join(","));
    }
    if !req.only.is_empty() {
        cmd = cmd.flag("only", req.only.join(","));
    }
    if req.full {
        cmd = cmd.switch("full");
    }
    if let Some(min) = req.min_target {
        cmd = cmd.flag("min-target", min.to_string());
    }
    if let Some(pct) = req.stretch_text {
        cmd = cmd.flag("stretch-text", pct.to_string());
    }
    if let Some(dir) = &baseline.dir {
        cmd = cmd.flag("dir", dir);
    }
    cmd
}

/// The error findings of a run: key, and the line that shows it.
fn errors(linted: &Linted) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = linted
        .runs
        .iter()
        .flat_map(|run| &run.findings)
        .filter(|f| f.severity == LayoutSeverity::Error)
        .map(|f| {
            let viewport = f.key.split(' ').nth(1).unwrap_or_default();
            (
                f.key.clone(),
                format!("{} at {viewport}: {}", f.kind, f.message),
            )
        })
        .collect();
    out.sort();
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

impl Engine {
    /// `expect --layout --baseline NAME`; see the module docs.
    pub(super) async fn expect_layout(&self, req: LayoutRequest) -> Result<Output, ErrorInfo> {
        let Some(baseline) = req.baseline.clone() else {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                "expect --layout needs --baseline NAME",
            ));
        };
        let assertion = format!("--layout --baseline {}", baseline.name);
        let path = baseline.path.display().to_string();
        let old = read_file(&baseline.path)?.map(|bytes| parse(&String::from_utf8_lossy(&bytes)));
        if old.is_none() && !baseline.update {
            return Err(ErrorInfo::new(
                ErrorKind::ExpectFailed,
                format!("expect {assertion} failed: there is no baseline yet"),
            )
            .note("baseline", &path)
            .hint(
                Hint::run(
                    command(&req, &baseline)
                        .switch("update")
                        .scoped(&self.session),
                )
                .why("stores the findings as the baseline; commit the file"),
            )
            .details(json!({ "baseline": path })));
        }
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let start = Instant::now();
        let deadline = start + timeout;
        let mut retries = 0u64;
        let (linted, found, new) = loop {
            let linted = {
                let _tab = self.tabs.lock(&page.target_id).await;
                let work = self.lint(&running, &page, &req, timeout);
                Box::pin(self.unless_blocked(&running, &page, mark, work)).await
            };
            let linted = match linted {
                Ok(linted) => linted,
                // The scope's element may come back (a re-render).
                Err(e)
                    if e.kind == ErrorKind::NotFound && Instant::now() + RETRY_DELAY < deadline =>
                {
                    retries += 1;
                    tokio::time::sleep(RETRY_DELAY).await;
                    continue;
                }
                Err(e) => return Err(e),
            };
            let found = errors(&linted);
            let new: Vec<(String, String)> = match &old {
                Some((listed, _)) if !baseline.update => found
                    .iter()
                    .filter(|(key, _)| !listed.contains(key))
                    .cloned()
                    .collect(),
                _ => Vec::new(),
            };
            if new.is_empty() || Instant::now() + RETRY_DELAY >= deadline {
                break (linted, found, new);
            }
            retries += 1;
            tokio::time::sleep(RETRY_DELAY).await;
        };
        let elapsed = start.elapsed();
        let keys: BTreeSet<String> = found.iter().map(|(k, _)| k.clone()).collect();
        if !new.is_empty() {
            let count = new.len();
            let mut info = ErrorInfo::new(
                ErrorKind::ExpectFailed,
                format!(
                    "expect {assertion} failed: {} the baseline does not list",
                    plural(count, "finding", "findings")
                ),
            )
            .note("baseline", &path);
            for (_, line) in new.iter().take(10) {
                info = info.note("new", line);
            }
            if count > 10 {
                info = info.note("more", format!("{} more in --json's details", count - 10));
            }
            let hint = Hint::run(
                command(&req, &baseline)
                    .switch("update")
                    .scoped(&self.session),
            )
            .why("accepts them into the baseline, if they are fine")
            .or(format!(
                "fix them, or see them with {}",
                Cmd::new("layout").scoped(&self.session)
            ));
            return Err(info.hint(hint).details(json!({
                "assertion": assertion,
                "baseline": path,
                "new": new.iter().map(|(k, _)| k).collect::<Vec<_>>(),
                "found": keys,
                "elapsed_ms": u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                "retries": retries,
            })));
        }
        let (result, stale, text) = match &old {
            Some((listed, comments)) if baseline.update => {
                if *listed == keys {
                    let text = format!(
                        "baseline {path} is up to date ({})",
                        plural(keys.len(), "finding", "findings")
                    );
                    (BaselineResult::Unchanged, Vec::new(), text)
                } else {
                    write_file(&baseline.path, render(comments, &keys).as_bytes())?;
                    let added = keys.difference(listed).count();
                    let removed = listed.difference(&keys).count();
                    let text = format!(
                        "updated baseline {path}: {} ({added} added, {removed} removed)",
                        plural(keys.len(), "finding", "findings")
                    );
                    (BaselineResult::Updated, Vec::new(), text)
                }
            }
            None => {
                write_file(&baseline.path, render(&[], &keys).as_bytes())?;
                let text = format!(
                    "created baseline {path} ({})",
                    plural(keys.len(), "finding", "findings")
                );
                (BaselineResult::Created, Vec::new(), text)
            }
            Some((listed, _)) => {
                // Only what this run looked for can be missing.
                let kinds = super::kinds(&req)?;
                let viewports: Vec<&str> = linted.runs.iter().map(|r| r.name.as_str()).collect();
                let checked = |key: &str| {
                    let mut words = key.split(' ');
                    let (kind, viewport) = (
                        words.next().unwrap_or_default(),
                        words.next().unwrap_or_default(),
                    );
                    kinds.contains(&kind) && viewports.contains(&viewport)
                };
                let stale: Vec<String> = listed
                    .difference(&keys)
                    .filter(|k| checked(k))
                    .cloned()
                    .collect();
                let text = format!(
                    "passed: expect {assertion}: {}, all in the baseline ({})",
                    plural(keys.len(), "error finding", "error findings"),
                    format::duration(elapsed)
                );
                (BaselineResult::Passed, stale, text)
            }
        };
        let mut warnings = linted.warnings.clone();
        if !stale.is_empty() {
            let shown: Vec<&str> = stale.iter().take(5).map(String::as_str).collect();
            warnings.push(format!(
                "the baseline lists {} no longer found ({}{}); {} drops them",
                plural(stale.len(), "finding", "findings"),
                shown.join("; "),
                if stale.len() > 5 { "; …" } else { "" },
                command(&req, &baseline)
                    .switch("update")
                    .scoped(&self.session)
            ));
        }
        let data = ExpectData {
            tab: tab_name(page.id),
            assertion,
            expected: "no error findings the baseline does not list".to_owned(),
            actual: json!(0),
            count: Some(keys.len() as u64),
            elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            retries,
            layout: Some(ExpectLayout {
                baseline: path.clone(),
                result,
                findings: keys.into_iter().collect(),
                stale,
            }),
            perf: None,
            audit: None,
        };
        Ok(warnings
            .into_iter()
            .fold(Output::typed(&data, text).quiet(path), Output::warning))
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baselines_are_keys_and_comments() {
        let (keys, comments) = parse(
            "# accepted\n\nclipped desktop role=heading[name=\"A\"]\n  covered mobile #x #y  \n",
        );
        assert_eq!(comments, ["# accepted"]);
        assert_eq!(
            keys.into_iter().collect::<Vec<_>>(),
            [
                "clipped desktop role=heading[name=\"A\"]",
                "covered mobile #x #y"
            ]
        );
        let keys: BTreeSet<String> = ["b".to_owned(), "a".to_owned()].into();
        assert_eq!(render(&["# mine".to_owned()], &keys), "# mine\na\nb\n");
        assert!(render(&[], &keys).starts_with("# expect --layout baseline"));
    }
}
