//! The scale suite (plan 22.1): `chrome-cdp test` on the generated suite
//! of `testdata/scale/` (`cargo xtask scale-suite`, 1,000 tests by
//! default) with `--jobs` 1, 2, 4, 8, 16 and 32 (each with the runner's
//! default `--jobs-isolation`, or one asked for). For each: the wall time,
//! the time per test, the runner's overhead per test by phase (from the
//! `setup`, `check` and `teardown` records of each test's `steps.jsonl`)
//! and the peak RSS of Chrome and the daemon.
//!
//! The suite's durations file (plan 22.5) is removed first, so the first
//! run starts the tests in the plan's order and the later ones longest
//! first, by what the earlier runs kept. With `shards`, each shard of the
//! suite then runs on its own (as on a CI machine of its own), split by
//! count and by time, each from the same durations file.

use std::collections::BTreeSet;
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::report::{self, Host};
use crate::sandbox::Sandbox;

/// The `--jobs` values run by default.
pub const JOBS: [usize; 6] = [1, 2, 4, 8, 16, 32];
/// The durations file of the suite, relative to it.
const DURATIONS: &str = "e2e/.durations.json";
/// How often the memory of Chrome and the daemon is read.
const SAMPLE_EVERY: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub struct Options<'a> {
    pub bin: &'a Path,
    pub chrome: &'a Path,
    /// The generated suite: `testdata/scale`.
    pub suite: PathBuf,
    pub jobs: Vec<Jobs>,
    /// Also run each of this many shards, with the last `jobs`.
    pub shards: Option<usize>,
}

/// A `--jobs` value to run, and its `--jobs-isolation` (`None` for the
/// runner's default): `16`, `16:context`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jobs {
    pub count: usize,
    pub isolation: Option<String>,
}

impl From<usize> for Jobs {
    fn from(count: usize) -> Self {
        Self {
            count,
            isolation: None,
        }
    }
}

impl std::str::FromStr for Jobs {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let (count, isolation) = match s.split_once(':') {
            Some((count, how)) if !how.is_empty() => (count, Some(how.to_owned())),
            Some(_) => return Err(format!("`{s}` has no --jobs-isolation after its colon")),
            None => (s, None),
        };
        let count = count
            .parse()
            .ok()
            .filter(|&n| n > 0)
            .ok_or_else(|| format!("`{s}` is not a number of jobs (or N:isolation)"))?;
        Ok(Self { count, isolation })
    }
}

impl fmt::Display for Jobs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.isolation {
            Some(how) => write!(f, "{}:{how}", self.count),
            None => write!(f, "{}", self.count),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScaleReport {
    pub suite: String,
    pub created: u64,
    pub host: Host,
    pub chrome: String,
    /// How many tests the suite has.
    pub tests: usize,
    pub runs: Vec<ScaleRun>,
    /// With `shards`: each shard's run, split by count and by time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shards: Vec<ShardRun>,
}

/// One shard of the suite, `chrome-cdp test --shard i/n`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShardRun {
    /// Split by the durations file rather than by count.
    pub by_time: bool,
    /// From 1.
    pub index: usize,
    pub total: usize,
    pub jobs: usize,
    pub tests: usize,
    /// What `test --list --shard` expected, when split by time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_ms: Option<u64>,
    pub wall_ms: u64,
    pub failed: usize,
    pub flaky: usize,
}

/// One `chrome-cdp test --jobs N` of the suite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScaleRun {
    pub jobs: usize,
    /// The `--jobs-isolation` it ran with; `None` for the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<String>,
    /// How many Chromes the tests ran in (0 when the runner did not say).
    #[serde(default)]
    pub browsers: usize,
    pub wall_ms: u64,
    pub passed: usize,
    /// Failed or flaky: either makes the run's numbers suspect.
    pub failed: usize,
    pub flaky: usize,
    /// The median of the tests' own durations.
    pub test_p50_ms: f64,
    /// The runner's overhead, per test on average.
    pub overhead: Phases,
    pub chrome_rss_peak: u64,
    pub daemon_rss_peak: u64,
    /// Whether it ran with the durations of earlier runs: the longest
    /// tests first.
    #[serde(default)]
    pub by_time: bool,
}

impl ScaleRun {
    /// `16`, `16 context`, with the Chromes when more than one: `16 (4
    /// Chromes)`.
    pub fn label(&self) -> String {
        let mut label = self.jobs.to_string();
        if let Some(how) = &self.isolation {
            let _ = write!(label, " {how}");
        }
        if self.browsers > 1 {
            let _ = write!(label, " ({} Chromes)", self.browsers);
        }
        if self.by_time {
            label.push_str(", longest first");
        }
        label
    }

    /// The wall time per test: what a test costs the suite.
    #[allow(clippy::cast_precision_loss)]
    pub fn per_test_ms(&self, tests: usize) -> f64 {
        self.wall_ms as f64 / tests.max(1) as f64
    }
}

/// What the runner does around a test's steps, in ms.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Phases {
    /// `context new`, the mocks, the viewport, the determinism settings.
    pub setup: f64,
    /// `expect --no-errors` after the steps.
    pub check: f64,
    /// Reading the logs, `context close`.
    pub teardown: f64,
    /// The rest of the test's time that no step or record covers.
    pub other: f64,
}

impl Phases {
    pub fn total(&self) -> f64 {
        self.setup + self.check + self.teardown + self.other
    }
}

pub fn run(opts: &Options<'_>) -> Result<ScaleReport, String> {
    if !opts.suite.join("chrome-cdp.toml").is_file() {
        return Err(format!(
            "no scale suite in {}; write it with `cargo xtask scale-suite`",
            opts.suite.display()
        ));
    }
    let durations = opts.suite.join(DURATIONS);
    remove(&durations)?;
    let mut runs = Vec::new();
    let mut tests = 0;
    for jobs in &opts.jobs {
        crate::note(&format!("scale: --jobs {jobs}"));
        let (run, n) = one_run(opts, jobs, &[])?;
        crate::note(&format!(
            "scale: --jobs {}: {} s, {} passed, {} failed, {} flaky",
            run.label(),
            report::fmt_num(ms_f64(run.wall_ms) / 1000.0),
            run.passed,
            run.failed,
            run.flaky
        ));
        tests = n;
        runs.push(run);
    }
    let shards = match (opts.shards, opts.jobs.last()) {
        (Some(total), Some(jobs)) => shard_runs(opts, jobs, total)?,
        _ => Vec::new(),
    };
    Ok(ScaleReport {
        suite: "scale".to_owned(),
        created: report::now(),
        host: Host::this(),
        chrome: crate::micro::chrome_version(opts.chrome),
        tests,
        runs,
        shards,
    })
}

/// Each of `total` shards with `jobs`, split by count (no durations file)
/// and by time (the file the runs before left), each shard from the same
/// file, as CI machines restore the same cache; the file is as it was
/// after.
fn shard_runs(opts: &Options<'_>, jobs: &Jobs, total: usize) -> Result<Vec<ShardRun>, String> {
    let path = opts.suite.join(DURATIONS);
    let kept = std::fs::read_to_string(&path)
        .map_err(|e| format!("no durations to shard by ({}): {e}", path.display()))?;
    let restore =
        || std::fs::write(&path, &kept).map_err(|e| format!("write {}: {e}", path.display()));
    let mut shards = Vec::new();
    for by_time in [false, true] {
        for index in 1..=total {
            let shard = format!("{index}/{total}");
            if by_time {
                restore()?;
            } else {
                remove(&path)?;
            }
            let expected_ms = if by_time {
                expected(opts, &shard)?
            } else {
                None
            };
            let how = if by_time { "by time" } else { "by count" };
            crate::note(&format!("scale: --shard {shard} {how}, --jobs {jobs}"));
            let (run, tests) = one_run(opts, jobs, &["--shard".to_owned(), shard.clone()])?;
            crate::note(&format!(
                "scale: --shard {shard} {how}: {tests} tests, {} s",
                report::fmt_num(ms_f64(run.wall_ms) / 1000.0)
            ));
            shards.push(ShardRun {
                by_time,
                index,
                total,
                jobs: jobs.count,
                tests,
                expected_ms,
                wall_ms: run.wall_ms,
                failed: run.failed,
                flaky: run.flaky,
            });
        }
    }
    restore()?;
    Ok(shards)
}

/// What `test --list --shard` expects of `shard`.
fn expected(opts: &Options<'_>, shard: &str) -> Result<Option<u64>, String> {
    let out = Command::new(opts.bin)
        .current_dir(&opts.suite)
        .args([
            "test",
            "--list",
            "--json",
            "--max-chars",
            "0",
            "--shard",
            shard,
        ])
        .output()
        .map_err(|e| format!("run chrome-cdp test --list: {e}"))?;
    let value: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("chrome-cdp test --list --json gave no JSON: {e}"))?;
    Ok(value["data"]["shards"]
        .as_array()
        .and_then(|shards| shards.iter().find(|s| s["listed"] == true))
        .and_then(|s| s["expected_ms"].as_u64()))
}

fn remove(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("remove {}: {e}", path.display()))
        }
        _ => Ok(()),
    }
}

/// The suite with `--jobs jobs` (and its `--jobs-isolation`) and `more`
/// arguments, in a fresh sandbox (new daemons and Chromes); also returns
/// how many tests ran.
fn one_run(opts: &Options<'_>, jobs: &Jobs, more: &[String]) -> Result<(ScaleRun, usize), String> {
    let by_time = opts.suite.join(DURATIONS).is_file();
    let mut args = vec![
        "test".to_owned(),
        "--jobs".to_owned(),
        jobs.count.to_string(),
    ];
    args.extend_from_slice(more);
    if let Some(how) = &jobs.isolation {
        args.extend(["--jobs-isolation".to_owned(), how.clone()]);
    }
    let sandbox = Sandbox::new(opts.bin, opts.chrome)?;
    let before: BTreeSet<u32> = processes().iter().map(|p| p.pid).collect();
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = {
        let stop = Arc::clone(&stop);
        let bin = opts.bin.display().to_string();
        let dir = sandbox.dir.path().display().to_string();
        std::thread::spawn(move || {
            let mut peak = (0, 0);
            while !stop.load(Ordering::Relaxed) {
                let (chrome, daemon) = rss(&processes(), &before, &bin, &dir);
                peak = (peak.0.max(chrome), peak.1.max(daemon));
                std::thread::sleep(SAMPLE_EVERY);
            }
            peak
        })
    };
    let start = Instant::now();
    let out = sandbox
        .cli()
        .current_dir(&opts.suite)
        // Every test in the JSON: by default it is cut to fit --max-chars.
        .args(&args)
        .args(["--json", "--max-chars", "0"])
        .output()
        .map_err(|e| format!("run chrome-cdp test: {e}"));
    let wall = start.elapsed();
    stop.store(true, Ordering::Relaxed);
    let (chrome_rss_peak, daemon_rss_peak) = sampler.join().unwrap_or_default();
    let out = out?;
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| {
        format!(
            "chrome-cdp test --json gave no JSON ({e}): {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )
    })?;
    if value["truncated"].as_array().is_some_and(|t| !t.is_empty()) {
        return Err(format!(
            "chrome-cdp test --json cut its output: {}",
            value["truncated"]
        ));
    }
    let tests = value["data"]["tests"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut durations = Vec::with_capacity(tests.len());
    let mut sum = Phases::default();
    for t in &tests {
        let total = ms_f64(t["duration_ms"].as_u64().unwrap_or(0));
        durations.push(total);
        let Some(dir) = t["artifacts"].as_str() else {
            continue;
        };
        let steps = std::fs::read_to_string(Path::new(dir).join("steps.jsonl")).unwrap_or_default();
        let phases = phases(&steps, total);
        sum.setup += phases.setup;
        sum.check += phases.check;
        sum.teardown += phases.teardown;
        sum.other += phases.other;
    }
    #[allow(clippy::cast_precision_loss)]
    let n = tests.len().max(1) as f64;
    let count =
        |key: &str| usize::try_from(value["data"][key].as_u64().unwrap_or(0)).unwrap_or(usize::MAX);
    let run = ScaleRun {
        jobs: jobs.count,
        isolation: jobs.isolation.clone(),
        browsers: usize::try_from(value["data"]["jobs"]["browsers"].as_u64().unwrap_or(0))
            .unwrap_or(0),
        wall_ms: u64::try_from(wall.as_millis()).unwrap_or(u64::MAX),
        passed: count("passed"),
        failed: count("failed"),
        flaky: count("flaky"),
        test_p50_ms: crate::stats::percentile(&durations, 50.0),
        overhead: Phases {
            setup: sum.setup / n,
            check: sum.check / n,
            teardown: sum.teardown / n,
            other: sum.other / n,
        },
        chrome_rss_peak,
        daemon_rss_peak,
        by_time,
    };
    Ok((run, tests.len()))
}

/// A test's overhead by phase, from its `steps.jsonl` and its duration
/// (`total`, in ms): what is not a step of its own or of its hooks.
pub fn phases(steps: &str, total: f64) -> Phases {
    let mut p = Phases::default();
    let mut page = 0.0;
    for step in steps
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
    {
        let took = ms_f64(step["duration_ms"].as_u64().unwrap_or(0));
        match step["hook"].as_str() {
            Some("setup") => p.setup += took,
            Some("check") => p.check += took,
            Some("teardown") => p.teardown += took,
            _ => page += took,
        }
    }
    p.other = (total - page - p.setup - p.check - p.teardown).max(0.0);
    p
}

#[allow(clippy::cast_precision_loss)]
fn ms_f64(ms: u64) -> f64 {
    ms as f64
}

/// A process, as `ps` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    /// Resident memory, in bytes.
    pub rss: u64,
    pub command: String,
}

/// Every process of the machine (`ps`; empty where there is none).
fn processes() -> Vec<Process> {
    Command::new("ps")
        .args(["-axo", "pid=,ppid=,rss=,command="])
        .output()
        .map(|out| parse_ps(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

/// `ps -o pid=,ppid=,rss=,command=`'s lines (RSS in KiB).
pub fn parse_ps(text: &str) -> Vec<Process> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let ppid = parts.next()?.parse().ok()?;
            let rss: u64 = parts.next()?.parse().ok()?;
            Some(Process {
                pid,
                ppid,
                rss: rss * 1024,
                command: parts.collect::<Vec<_>>().join(" "),
            })
        })
        .collect()
}

/// The resident memory of the run's Chrome (every process under the one
/// whose profile is in the sandbox `dir`) and its daemons (`bin
/// __daemon` processes that were not running `before`), in bytes.
pub fn rss(processes: &[Process], before: &BTreeSet<u32>, bin: &str, dir: &str) -> (u64, u64) {
    let daemons: BTreeSet<u32> = processes
        .iter()
        .filter(|p| {
            !before.contains(&p.pid) && p.command.starts_with(bin) && p.command.contains("__daemon")
        })
        .map(|p| p.pid)
        .collect();
    let mut chrome: BTreeSet<u32> = processes
        .iter()
        .filter(|p| !daemons.contains(&p.pid) && p.command.contains(dir))
        .map(|p| p.pid)
        .collect();
    // Chrome's helpers and renderers have no profile flag: its children.
    loop {
        let more: Vec<u32> = processes
            .iter()
            .filter(|p| chrome.contains(&p.ppid) && !chrome.contains(&p.pid))
            .map(|p| p.pid)
            .collect();
        if more.is_empty() {
            break;
        }
        chrome.extend(more);
    }
    let sum = |set: &BTreeSet<u32>| {
        processes
            .iter()
            .filter(|p| set.contains(&p.pid))
            .map(|p| p.rss)
            .sum()
    };
    (sum(&chrome), sum(&daemons))
}

/// `1.2 GB`, `340 MB`.
#[allow(clippy::cast_precision_loss)]
fn bytes(n: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = n as f64 / MB;
    if n == 0 {
        "–".to_owned()
    } else if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{mb:.0} MB")
    }
}

pub fn markdown(r: &ScaleReport) -> String {
    let mut md = format!(
        "Last run: {} on {} ({} CPUs), {}. {} tests (`cargo xtask scale-suite --tests {}`).\n\n",
        report::date(r.created),
        r.host.key(),
        r.host.cpus,
        r.chrome,
        r.tests,
        r.tests
    );
    md.push_str(
        "| `--jobs` | Wall time | Per test | Test p50 | Overhead | setup | check | teardown | other | Chrome RSS | Daemon RSS | Failed |\n\
         |---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n",
    );
    let ms = |v: f64| format!("{} ms", report::fmt_num(v));
    for run in &r.runs {
        let o = &run.overhead;
        let _ = writeln!(
            md,
            "| {} | {} s | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            run.label(),
            report::fmt_num(ms_f64(run.wall_ms) / 1000.0),
            ms(run.per_test_ms(r.tests)),
            ms(run.test_p50_ms),
            ms(o.total()),
            ms(o.setup),
            ms(o.check),
            ms(o.teardown),
            ms(o.other),
            bytes(run.chrome_rss_peak),
            bytes(run.daemon_rss_peak),
            if run.flaky > 0 {
                format!("{} (+{} flaky)", run.failed, run.flaky)
            } else {
                run.failed.to_string()
            }
        );
    }
    md
}

/// With shards: each shard's tests and wall time, split by count and by
/// time, and how much longer than the mean the slowest shard took.
pub fn shards_markdown(r: &ScaleReport) -> String {
    let Some(first) = r.shards.first() else {
        return String::new();
    };
    let mut md = format!(
        "Each of {} shards on its own with `--jobs {}`, by count (no durations file) and by time (the file the runs above left):\n\n\
         | Shard | By count: tests | Wall time | By time: tests | Expected | Wall time |\n\
         |---:|---:|---:|---:|---:|---:|\n",
        first.total, first.jobs
    );
    let secs = |ms: u64| format!("{} s", report::fmt_num(ms_f64(ms) / 1000.0));
    let split = |by_time: bool| -> Vec<&ShardRun> {
        r.shards.iter().filter(|s| s.by_time == by_time).collect()
    };
    let (count, time) = (split(false), split(true));
    for (c, t) in count.iter().zip(&time) {
        let _ = writeln!(
            md,
            "| {}/{} | {} | {} | {} | {} | {} |",
            c.index,
            c.total,
            c.tests,
            secs(c.wall_ms),
            t.tests,
            t.expected_ms.map_or_else(|| "-".to_owned(), secs),
            secs(t.wall_ms)
        );
    }
    let spread = |shards: &[&ShardRun]| {
        let walls: Vec<f64> = shards.iter().map(|s| ms_f64(s.wall_ms)).collect();
        #[allow(clippy::cast_precision_loss)]
        let mean = walls.iter().sum::<f64>() / walls.len().max(1) as f64;
        let max = walls.iter().copied().fold(0.0, f64::max);
        (max, (max / mean - 1.0) * 100.0)
    };
    let ((cmax, cpct), (tmax, tpct)) = (spread(&count), spread(&time));
    let _ = writeln!(
        md,
        "\nThe slowest shard: by count {} s, {cpct:.1}% above the mean; by time {} s, {tpct:.1}% above the mean.",
        report::fmt_num(cmax / 1000.0),
        report::fmt_num(tmax / 1000.0),
    );
    let unclean: usize = r.shards.iter().map(|s| s.failed + s.flaky).sum();
    if unclean > 0 {
        let _ = writeln!(md, "\n{unclean} tests failed or were flaky in the shards.");
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overhead_is_split_by_phase() {
        let steps = r#"{"n":1,"duration_ms":40,"hook":"setup"}
{"n":2,"duration_ms":10,"hook":"setup"}
{"n":3,"duration_ms":300,"hook":"before"}
{"n":4,"duration_ms":500}
{"n":5,"duration_ms":5,"hook":"check"}
{"n":6,"duration_ms":20,"hook":"teardown"}
{"n":7,"duration_ms":7,"hook":"teardown"}
"#;
        let p = phases(steps, 900.0);
        assert_eq!(
            p,
            Phases {
                setup: 50.0,
                check: 5.0,
                teardown: 27.0,
                other: 18.0
            }
        );
        assert!((p.total() - 100.0).abs() < 1e-9);
        // Times rounded to ms can add up to more than the test's.
        assert!(phases(steps, 800.0).other.abs() < 1e-9);
    }

    #[test]
    fn rss_counts_the_runs_chrome_and_daemons() {
        let ps = parse_ps(
            "    1     0   100 /sbin/launchd
   10     1  2000 /t/release/chrome-cdp __daemon --session test-1
   11    10 50000 /chrome --user-data-dir=/sandbox/tmp/p --headless
   12    11 30000 /chrome-helper --type=renderer
   13    12  1000 /chrome-helper --type=utility
   20     1  3000 /t/release/chrome-cdp __daemon --session mine
   30     1  9000 /chrome --user-data-dir=/home/me/p
",
        );
        assert_eq!(ps.len(), 7);
        assert_eq!(ps[1].rss, 2000 * 1024);
        let before = BTreeSet::from([1, 20, 30]);
        assert_eq!(
            rss(&ps, &before, "/t/release/chrome-cdp", "/sandbox"),
            (81_000 * 1024, 2000 * 1024)
        );
    }

    #[test]
    fn the_table_has_a_row_per_run() {
        let run = |jobs, wall_ms| ScaleRun {
            jobs,
            isolation: None,
            browsers: 1,
            wall_ms,
            passed: 10,
            failed: 0,
            flaky: 0,
            test_p50_ms: 1200.0,
            overhead: Phases {
                setup: 100.0,
                check: 1.0,
                teardown: 30.0,
                other: 12.0,
            },
            chrome_rss_peak: 512 * 1024 * 1024,
            daemon_rss_peak: 40 * 1024 * 1024,
            by_time: false,
        };
        let r = ScaleReport {
            suite: "scale".to_owned(),
            created: 1_790_640_000,
            host: Host {
                os: "linux".to_owned(),
                arch: "x86_64".to_owned(),
                cpus: 4,
            },
            chrome: "Chrome 155".to_owned(),
            tests: 10,
            runs: vec![run(1, 15_000), run(4, 4_000)],
            shards: Vec::new(),
        };
        let md = markdown(&r);
        assert!(md.starts_with("Last run: 2026-09-29 on linux-x86_64 (4 CPUs)"));
        assert!(
            md.contains(
                "| 1 | 15 s | 1500 ms | 1200 ms | 143 ms | 100 ms | 1 ms | 30 ms | 12 ms | 512 MB | 40 MB | 0 |"
            ),
            "{md}"
        );
        assert!(md.contains("| 4 | 4 s | 400 ms |"), "{md}");
        let mut r = r;
        r.runs[1].isolation = Some("browser".to_owned());
        r.runs[1].browsers = 4;
        assert!(markdown(&r).contains("| 4 browser (4 Chromes) | 4 s | 400 ms |"));
        r.runs[1].by_time = true;
        assert!(markdown(&r).contains("| 4 browser (4 Chromes), longest first | 4 s |"));
        assert_eq!(shards_markdown(&r), "");

        // Shards: by count, then by time, side by side.
        let shard = |by_time, index, tests, wall_ms| ShardRun {
            by_time,
            index,
            total: 2,
            jobs: 4,
            tests,
            expected_ms: by_time.then_some(wall_ms - 500),
            wall_ms,
            failed: 0,
            flaky: 0,
        };
        r.shards = vec![
            shard(false, 1, 5, 12_000),
            shard(false, 2, 5, 8_000),
            shard(true, 1, 4, 10_500),
            shard(true, 2, 6, 9_500),
        ];
        let md = shards_markdown(&r);
        assert!(
            md.contains("| 1/2 | 5 | 12 s | 4 | 10 s | 10.5 s |"),
            "{md}"
        );
        assert!(
            md.contains(
                "by count 12 s, 20.0% above the mean; by time 10.5 s, 5.0% above the mean."
            ),
            "{md}"
        );
    }

    #[test]
    fn jobs_name_an_isolation_after_a_colon() {
        let jobs = |s: &str| s.parse::<Jobs>();
        assert_eq!(jobs("16"), Ok(Jobs::from(16)));
        let context = jobs("16:context").unwrap();
        assert_eq!(
            (context.count, context.isolation.as_deref()),
            (16, Some("context"))
        );
        assert_eq!(context.to_string(), "16:context");
        for bad in ["0", "x", "16:", ":context"] {
            assert!(jobs(bad).is_err(), "{bad}");
        }
    }
}
