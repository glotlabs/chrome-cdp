//! What every suite shares: the repository's paths, the host, baselines
//! (a regression of more than 15% fails) and the generated sections of
//! docs/benchmarks.md.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::stats::{Better, Metric};

/// How much worse than its baseline a metric may get (plan 10.4).
pub const REGRESSION_LIMIT: f64 = 0.15;

/// The published results.
pub const BENCHMARKS_DOC: &str = "docs/benchmarks.md";

pub fn workspace_root() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    dir.ancestors().nth(2).unwrap_or(dir).to_path_buf()
}

/// Where results go: `target/bench/`.
pub fn results_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| workspace_root().join("target"), PathBuf::from)
        .join("bench")
}

/// The machine a suite ran on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub os: String,
    pub arch: String,
    pub cpus: usize,
}

impl Host {
    pub fn this() -> Self {
        Self {
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            cpus: std::thread::available_parallelism().map_or(1, usize::from),
        }
    }

    /// `linux-x86_64`: timing baselines are per OS and architecture.
    pub fn key(&self) -> String {
        format!("{}-{}", self.os, self.arch)
    }
}

/// Seconds since the Unix epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, text + "\n").map_err(|e| format!("write {}: {e}", path.display()))
}

/// A committed baseline: `bench/baselines/<name>.json`.
pub fn baseline_path(name: &str) -> PathBuf {
    workspace_root()
        .join("bench/baselines")
        .join(format!("{name}.json"))
}

/// A metric more than [`REGRESSION_LIMIT`] worse than its baseline.
#[derive(Debug, Clone, PartialEq)]
pub struct Regression {
    pub name: String,
    pub baseline: f64,
    pub now: f64,
    pub unit: String,
}

impl std::fmt::Display for Regression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let change = if self.baseline == 0.0 {
            0.0
        } else {
            (self.now - self.baseline) / self.baseline * 100.0
        };
        write!(
            f,
            "{}: {} {} (baseline {} {}, {change:+.0}%)",
            self.name,
            fmt_num(self.now),
            self.unit,
            fmt_num(self.baseline),
            self.unit
        )
    }
}

/// The metrics of `now` more than 15% worse (by p50) than the metric of
/// the same name in `baseline`. Metrics without a baseline are new, not
/// regressions.
pub fn regressions(now: &[Metric], baseline: &[Metric]) -> Vec<Regression> {
    now.iter()
        .filter_map(|m| {
            let base = baseline.iter().find(|b| b.name == m.name)?;
            // Relative change, worse as positive; exactly 15% is fine.
            let change = match m.better {
                Better::Lower => m.p50 - base.p50,
                Better::Higher => base.p50 - m.p50,
            } / base.p50.abs().max(f64::MIN_POSITIVE);
            let worse = change > REGRESSION_LIMIT + 1e-9;
            worse.then(|| Regression {
                name: m.name.clone(),
                baseline: base.p50,
                now: m.p50,
                unit: m.unit.clone(),
            })
        })
        .collect()
}

/// `--check` / `--update-baseline` for a suite's metrics: checking fails
/// on a regression and warns without a baseline.
pub fn gate(name: &str, metrics: &[Metric], update: bool) -> Result<(), String> {
    let path = baseline_path(name);
    if update {
        write_json(&path, &metrics)?;
        crate::note(&format!("recorded the baseline {}", path.display()));
        return Ok(());
    }
    if !path.is_file() {
        crate::warn(&format!(
            "no baseline {} yet; record one with --update-baseline on the machine that \
             checks it",
            path.display()
        ));
        return Ok(());
    }
    let baseline: Vec<Metric> = read_json(&path)?;
    let found = regressions(metrics, &baseline);
    if found.is_empty() {
        crate::note(&format!(
            "no metric is more than 15% worse than {}",
            path.display()
        ));
        return Ok(());
    }
    let mut message = format!(
        "{} metric(s) regressed more than 15% against {}:",
        found.len(),
        path.display()
    );
    for r in &found {
        let _ = write!(message, "\n  {r}");
    }
    message
        .push_str("\nIf the change is wanted, rerun with --update-baseline and commit the file.");
    Err(message)
}

/// `text` with the part between `<!-- bench:<name> -->` and
/// `<!-- /bench:<name> -->` replaced by `content`.
pub fn replace_section(text: &str, name: &str, content: &str) -> Result<String, String> {
    let start = format!("<!-- bench:{name} -->");
    let end = format!("<!-- /bench:{name} -->");
    let (Some(a), Some(b)) = (text.find(&start), text.find(&end)) else {
        return Err(format!(
            "{BENCHMARKS_DOC} has no `{start}` … `{end}` section"
        ));
    };
    if b < a {
        return Err(format!("{BENCHMARKS_DOC}: `{end}` comes before `{start}`"));
    }
    Ok(format!(
        "{}{start}\n{}\n{end}{}",
        &text[..a],
        content.trim_end(),
        &text[b + end.len()..]
    ))
}

/// Writes `content` into section `name` of docs/benchmarks.md.
pub fn publish(name: &str, content: &str) -> Result<(), String> {
    let path = workspace_root().join(BENCHMARKS_DOC);
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let text = replace_section(&text, name, content)?;
    std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    crate::note(&format!("updated the `{name}` section of {BENCHMARKS_DOC}"));
    Ok(())
}

/// `12`, `3.4`, `0.25`: as short as the value allows.
pub fn fmt_num(value: f64) -> String {
    if value.abs() >= 100.0 || value.fract() == 0.0 {
        format!("{value:.0}")
    } else if value.abs() >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

/// The date of a Unix time, `2026-09-29` (UTC).
pub fn date(unix: u64) -> String {
    // Days to a civil date (Howard Hinnant's algorithm).
    let days = i64::try_from(unix / 86_400).unwrap_or(0);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metric(name: &str, better: Better, p50: f64) -> Metric {
        Metric::value(name, "ms", better, p50)
    }

    #[test]
    fn regressions_are_over_fifteen_percent_worse() {
        let baseline = [
            metric("a", Better::Lower, 100.0),
            metric("b", Better::Higher, 10.0),
            metric("c", Better::Lower, 100.0),
        ];
        let now = [
            metric("a", Better::Lower, 115.0),
            metric("b", Better::Higher, 8.4),
            metric("c", Better::Lower, 116.0),
            metric("new", Better::Lower, 1.0),
        ];
        let found = regressions(&now, &baseline);
        let names: Vec<&str> = found.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["b", "c"]);
        assert_eq!(found[1].to_string(), "c: 116 ms (baseline 100 ms, +16%)");
    }

    #[test]
    fn sections_are_replaced_between_markers() {
        let doc = "# T\n<!-- bench:x -->\nold\n<!-- /bench:x -->\nafter\n";
        assert_eq!(
            replace_section(doc, "x", "new\n").unwrap(),
            "# T\n<!-- bench:x -->\nnew\n<!-- /bench:x -->\nafter\n"
        );
        assert!(replace_section(doc, "y", "z").is_err());
    }

    #[test]
    fn the_published_doc_has_every_section() {
        let doc = std::fs::read_to_string(workspace_root().join(BENCHMARKS_DOC)).unwrap();
        for name in ["micro", "tokens", "agent"] {
            replace_section(&doc, name, "x").unwrap();
        }
    }

    #[test]
    fn dates_and_numbers() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_790_640_000), "2026-09-29");
        assert_eq!(fmt_num(1234.5), "1234");
        assert_eq!(fmt_num(12.34), "12.3");
        assert_eq!(fmt_num(1.234), "1.23");
        assert_eq!(fmt_num(3.0), "3");
    }
}
