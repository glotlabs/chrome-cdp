//! The scale suite (plan 22.1): `chrome-cdp test` on the generated suite
//! of `testdata/scale/` (`cargo xtask scale-suite`, 1,000 tests by
//! default) with `--jobs` 1, 2, 4, 8, 16 and 32. For each: the wall time,
//! the time per test, the runner's overhead per test by phase (from the
//! `setup`, `check` and `teardown` records of each test's `steps.jsonl`)
//! and the peak RSS of Chrome and the daemon.

use std::collections::BTreeSet;
use std::fmt::Write as _;
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
/// How often the memory of Chrome and the daemon is read.
const SAMPLE_EVERY: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub struct Options<'a> {
    pub bin: &'a Path,
    pub chrome: &'a Path,
    /// The generated suite: `testdata/scale`.
    pub suite: PathBuf,
    pub jobs: Vec<usize>,
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
}

/// One `chrome-cdp test --jobs N` of the suite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScaleRun {
    pub jobs: usize,
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
}

impl ScaleRun {
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
    let mut runs = Vec::new();
    let mut tests = 0;
    for &jobs in &opts.jobs {
        crate::note(&format!("scale: --jobs {jobs}"));
        let (run, n) = one_run(opts, jobs)?;
        crate::note(&format!(
            "scale: --jobs {jobs}: {} s, {} passed, {} failed, {} flaky",
            report::fmt_num(ms_f64(run.wall_ms) / 1000.0),
            run.passed,
            run.failed,
            run.flaky
        ));
        tests = n;
        runs.push(run);
    }
    Ok(ScaleReport {
        suite: "scale".to_owned(),
        created: report::now(),
        host: Host::this(),
        chrome: crate::micro::chrome_version(opts.chrome),
        tests,
        runs,
    })
}

/// The suite with `--jobs jobs`, in a fresh sandbox (a new daemon and
/// Chrome); also returns how many tests ran.
fn one_run(opts: &Options<'_>, jobs: usize) -> Result<(ScaleRun, usize), String> {
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
        .args(["test", "--jobs", &jobs.to_string(), "--json"])
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
        jobs,
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
            run.jobs,
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
    }
}
