//! `load --compare <run|path>` (plan 23.4): an earlier run's numbers next
//! to this one's in the reports, with the changes above 10% marked. The
//! earlier run is a run's id in `e2e/.results/load/`, `last` (the latest
//! run with a summary), or a path to a run's folder or its
//! `summary.json`; it is read before the users start. A time that moved
//! by less than 1 ms is not marked, whatever the share.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use protocol::schema::{LoadChange, LoadCompare, LoadData, LoadMetric};
use protocol::{ErrorInfo, ErrorKind, Hint};

/// A change above this share is marked.
pub(super) const MARKED: f64 = 0.10;

/// … if it is a time that changed by this many ms at least: 1.2 ms from
/// 0.9 ms is noise, not a slower server.
const NOISE_MS: f64 = 1.0;

/// The earlier run: its id, its summary's path, and its data.
#[derive(Debug, Clone)]
pub(super) struct Earlier {
    pub(super) run: String,
    pub(super) summary: PathBuf,
    pub(super) data: LoadData,
}

/// Reads the run `arg` names, from the runs in `runs` (or a path from
/// `cwd`).
pub(super) fn read(arg: &str, runs: &Path, cwd: &Path) -> Result<Earlier, ErrorInfo> {
    let summary = find(arg, runs, cwd)?;
    let text = std::fs::read_to_string(&summary).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::NotFound,
            format!("--compare: cannot read {}: {e}", summary.display()),
        )
    })?;
    let data: LoadData = serde_json::from_str(&text).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!(
                "--compare: {} is no load run's summary: {e}",
                summary.display()
            ),
        )
    })?;
    Ok(Earlier {
        run: data.run.clone(),
        summary,
        data,
    })
}

/// The `summary.json` `arg` names.
fn find(arg: &str, runs: &Path, cwd: &Path) -> Result<PathBuf, ErrorInfo> {
    if arg == "last" {
        return latest(runs).ok_or_else(|| {
            ErrorInfo::new(
                ErrorKind::NotFound,
                format!(
                    "--compare last: no run in {} has a summary.json",
                    runs.display()
                ),
            )
            .hint("run one first; --compare takes it next time")
        });
    }
    let path = cwd.join(arg);
    let in_runs = runs.join(arg);
    for candidate in [&path, &in_runs] {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
        let summary = candidate.join(super::SUMMARY);
        if candidate.is_dir() && summary.is_file() {
            return Ok(summary);
        }
    }
    let names = run_names(runs);
    let close = protocol::suggest::similar(arg, names.iter().map(String::as_str), 1);
    let hint = match close.first() {
        Some(name) => Hint::text(format!("did you mean --compare {name}?")),
        None if names.is_empty() => Hint::text("no run has a summary yet; run one first"),
        None => Hint::text(format!(
            "--compare takes a run's id in {}, last, or a run's folder or summary.json",
            runs.display()
        )),
    };
    Err(ErrorInfo::new(
        ErrorKind::NotFound,
        format!("--compare: no run {arg} (nor a file or folder by that name)"),
    )
    .hint(hint))
}

/// The runs in `runs` with a summary, newest first.
fn summaries(runs: &Path) -> Vec<(std::time::SystemTime, String, PathBuf)> {
    let mut found: Vec<_> = std::fs::read_dir(runs)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| {
            let summary = e.path().join(super::SUMMARY);
            let modified = summary.metadata().ok()?.modified().ok()?;
            Some((
                modified,
                e.file_name().to_string_lossy().into_owned(),
                summary,
            ))
        })
        .collect();
    found.sort_by_key(|a| std::cmp::Reverse(a.0));
    found
}

fn latest(runs: &Path) -> Option<PathBuf> {
    summaries(runs).into_iter().next().map(|(_, _, path)| path)
}

fn run_names(runs: &Path) -> Vec<String> {
    summaries(runs)
        .into_iter()
        .map(|(_, name, _)| name)
        .collect()
}

/// The earlier run's numbers next to `now`'s.
pub(super) fn compare(earlier: &Earlier, now: &LoadData) -> LoadCompare {
    let mut changes = Vec::new();
    let before = &earlier.data;
    if let (Some(b), Some(a)) = (&before.requests, &now.requests) {
        group(&mut changes, "requests", b, a, true);
    }
    for a in &now.endpoints {
        if let Some(b) = before.endpoints.iter().find(|b| b.name == a.name) {
            group(&mut changes, &a.name, b, a, true);
        }
    }
    for a in &now.journeys {
        if let Some(b) = before.journeys.iter().find(|b| b.place == a.place) {
            let name = format!("journey {}", a.name);
            for (stat, b, a) in [
                ("p50", b.iteration_ms.p50, a.iteration_ms.p50),
                ("p95", b.iteration_ms.p95, a.iteration_ms.p95),
                ("p99", b.iteration_ms.p99, a.iteration_ms.p99),
            ] {
                if b > 0.0 || a > 0.0 {
                    changes.push(change(format!("{stat}({name})"), b, a, "ms"));
                }
            }
        }
    }
    for a in &now.step_times {
        if let Some(b) = before.step_times.iter().find(|b| b.name == a.name) {
            group(&mut changes, &a.name, b, a, false);
        }
    }
    for a in &now.vitals {
        if let Some(b) = before.vitals.iter().find(|b| b.name == a.name) {
            changes.push(change(format!("p75({})", a.name), b.p75, a.p75, &a.unit));
        }
    }
    let marked = changes.iter().filter(|c| c.marked).count();
    LoadCompare {
        run: earlier.run.clone(),
        summary: earlier.summary.display().to_string(),
        changes,
        marked: u64::try_from(marked).unwrap_or(u64::MAX),
    }
}

/// A group's p50, p95 and p99, and with `rates` its error rate and rate.
fn group(changes: &mut Vec<LoadChange>, name: &str, b: &LoadMetric, a: &LoadMetric, rates: bool) {
    let timed = |m: &LoadMetric| m.count > m.cached;
    if timed(b) && timed(a) {
        for (stat, before, after) in [
            ("p50", b.p50, a.p50),
            ("p95", b.p95, a.p95),
            ("p99", b.p99, a.p99),
        ] {
            changes.push(change(format!("{stat}({name})"), before, after, &a.unit));
        }
    }
    if rates {
        changes.push(change(
            format!("error_rate({name})"),
            b.error_rate * 100.0,
            a.error_rate * 100.0,
            "%",
        ));
        changes.push(change(format!("rate({name})"), b.rate, a.rate, "/s"));
    }
}

fn change(name: String, before: f64, after: f64, unit: &str) -> LoadChange {
    let share = (before > 0.0).then(|| ((after - before) / before * 1000.0).round() / 1000.0);
    LoadChange {
        name,
        before,
        after,
        unit: unit.to_owned(),
        change: share,
        marked: match share {
            _ if unit == "ms" && (after - before).abs() < NOISE_MS => false,
            Some(s) => s.abs() > MARKED,
            None => after > 0.0,
        },
    }
}

/// `412 ms`, `0.4%`, `20/s`, `0.051`.
fn shown(v: f64, unit: &str) -> String {
    match unit {
        "ms" => super::status::ms_text(v),
        "" => format!("{v:.3}"),
        unit => format!("{}{unit}", (v * 100.0).round() / 100.0),
    }
}

/// `+35%`, `-12%`, `new`.
fn change_text(c: &LoadChange) -> String {
    match c.change {
        Some(s) => format!("{:+.0}%", s * 100.0),
        None if c.after > 0.0 => "from 0".to_owned(),
        None => "=".to_owned(),
    }
}

/// The text line: `compared with <run>: 2 of 40 numbers changed by more
/// than 10%: p95(GET /api/cart) +35%, …`.
pub(super) fn line(compare: &LoadCompare) -> String {
    let mut text = format!(
        "compared with {}: {} of {} numbers changed by more than 10%",
        compare.run,
        compare.marked,
        compare.changes.len()
    );
    let marked: Vec<String> = compare
        .changes
        .iter()
        .filter(|c| c.marked)
        .take(5)
        .map(|c| format!("{} {}", c.name, change_text(c)))
        .collect();
    if !marked.is_empty() {
        let _ = write!(text, ": {}", marked.join(", "));
        if compare.marked > 5 {
            text.push_str(", …");
        }
    }
    text
}

/// `report.md`'s section.
pub(super) fn markdown(compare: &LoadCompare) -> String {
    let mut md = format!(
        "\n## Compared with {}\n\n{} of {} numbers changed by more than 10%.\n",
        compare.run,
        compare.marked,
        compare.changes.len()
    );
    if compare.marked > 0 {
        md.push_str("\n| | Before | Now | Change |\n|---|--:|--:|--:|\n");
        for c in compare.changes.iter().filter(|c| c.marked) {
            let _ = writeln!(
                md,
                "| `{}` | {} | {} | {} |",
                c.name.replace('|', "\\|"),
                shown(c.before, &c.unit),
                shown(c.after, &c.unit),
                change_text(c)
            );
        }
    }
    md
}

/// `report.html`'s section: every number, the marked ones colored (up
/// is worse for times and errors, but not for rates).
pub(super) fn html(compare: &LoadCompare) -> String {
    let escape = |t: &str| {
        t.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let mut html = format!(
        "<h2>Compared with {}</h2><p>{} of {} numbers changed by more than 10%.</p>\
         <table><tr><th></th><th class=n>Before</th><th class=n>Now</th><th class=n>Change</th></tr>",
        escape(&compare.run),
        compare.marked,
        compare.changes.len()
    );
    for c in &compare.changes {
        let worse = if c.unit == "/s" {
            c.after < c.before
        } else {
            c.after > c.before
        };
        let class = match (c.marked, worse) {
            (false, _) => "",
            (true, true) => " class=up",
            (true, false) => " class=down",
        };
        let _ = write!(
            html,
            "<tr><td><code>{}</code></td><td class=n>{}</td><td class=n>{}</td><td class=n><span{class}>{}{}</span></td></tr>",
            escape(&c.name),
            shown(c.before, &c.unit),
            shown(c.after, &c.unit),
            if c.marked {
                if worse { "▲ " } else { "▼ " }
            } else {
                ""
            },
            change_text(c)
        );
    }
    html.push_str("</table>\n");
    html
}

#[cfg(test)]
mod tests {
    use super::{change, line};
    use protocol::schema::LoadCompare;

    #[test]
    fn changes_above_ten_percent_are_marked() {
        let up = change("p95(GET /api/cart)".into(), 200.0, 270.0, "ms");
        assert_eq!(up.change, Some(0.35));
        assert!(up.marked);
        let same = change("p95(steps)".into(), 200.0, 219.0, "ms");
        assert!(!same.marked);
        let noise = change("p95(static js)".into(), 0.9, 1.4, "ms");
        assert!(!noise.marked);
        let down = change("p95(steps)".into(), 200.0, 170.0, "ms");
        assert!(down.marked);
        let new = change("error_rate(requests)".into(), 0.0, 0.5, "%");
        assert_eq!(new.change, None);
        assert!(new.marked);
        let none = change("error_rate(requests)".into(), 0.0, 0.0, "%");
        assert!(!none.marked);
        let compare = LoadCompare {
            run: "r1".into(),
            summary: "x".into(),
            marked: 2,
            changes: vec![up, same, new],
        };
        assert_eq!(
            line(&compare),
            "compared with r1: 2 of 3 numbers changed by more than 10%: \
             p95(GET /api/cart) +35%, error_rate(requests) from 0"
        );
    }
}
