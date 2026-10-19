//! The load suite (plan 23.1): `chrome-cdp load` on the reference journey
//! (`testdata/load/scenarios/shop.cdp`: browse, open a product, add it to
//! the cart, 2 s of think time on each page) against the load server
//! (`testdata/load/server`, in this process), at 25, 50, 100, 200 and 400
//! users, each with one Chrome per 10, 25 and 50 users.
//!
//! For each run: the iterations and how many failed, the steps' p50 and
//! p95 (think steps left out), Chrome's peak memory and CPU per user, and
//! chrome-cdp's own CPU and peak RSS. `ps` is read every 500 ms; CPU is
//! the sum over the run of each process's CPU time growth, so a process
//! that exits between two reads loses at most its last half second.
//! Chrome's memory is the sum of its processes' PSS on Linux (shared
//! pages split between the processes that share them), and of their RSS
//! elsewhere, which counts Chrome's shared framework once per process and
//! so reads several times too high on macOS.
//!
//! `--cache returning` runs the users with `[load] cache = "returning"`
//! (plan 23.2), with an `@before` that clears the user's cookies, as the
//! journey expects an empty cart: only the HTTP cache (and the context)
//! carries over. Each run also reports the browser contexts made and the
//! mean time one took, which context reuse saves.
//!
//! **Users per machine** is the most users of a run that held: under 1%
//! of its iterations failed, chrome-cdp and its Chromes used under 90% of
//! the machine's CPUs, and its step p95 stayed within twice the smallest
//! run's (of the same Chromes per user). Plan 23.5's capacity check will
//! replace this stand-in.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::report::{self, Host};
use crate::sandbox::Sandbox;

/// The users of each run, by default.
pub const USERS: [u32; 5] = [25, 50, 100, 200, 400];
/// The users per Chrome of each run, by default.
pub const PER_BROWSER: [u32; 3] = [10, 25, 50];
/// The journey, relative to the repository.
const JOURNEY: &str = "testdata/load/scenarios/shop.cdp";
/// How often `ps` is read.
const SAMPLE_EVERY: Duration = Duration::from_millis(500);
/// A run held if fewer of its iterations failed.
const MAX_FAILED: f64 = 0.01;
/// … and chrome-cdp and its Chromes used less of the machine's CPUs.
const MAX_CPU: f64 = 0.9;
/// … and its step p95 was at most this many times the smallest run's.
const MAX_SLOWDOWN: f64 = 2.0;

#[derive(Debug)]
pub struct Options<'a> {
    pub bin: &'a Path,
    pub chrome: &'a Path,
    pub users: Vec<u32>,
    pub per_browser: Vec<u32>,
    /// How long users start iterations, after the ramp-up.
    pub duration: Duration,
    pub ramp_up: Duration,
    /// `[load] cache`: `new` or `returning`.
    pub cache: &'a str,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoadReport {
    pub created: u64,
    pub host: Host,
    pub chrome: String,
    /// `chrome-headless-shell` or `chrome`, as `load` reported it.
    #[serde(default)]
    pub browser: String,
    /// `[load] cache`.
    #[serde(default)]
    pub cache: String,
    pub journey: String,
    pub duration_s: u64,
    pub ramp_up_s: u64,
    pub runs: Vec<LoadRun>,
    /// The most users of a run that held.
    pub users_per_machine: Option<u32>,
}

/// One `chrome-cdp load --users N --users-per-browser M`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoadRun {
    pub users: u32,
    pub per_browser: u32,
    pub browsers: u32,
    pub wall_ms: u64,
    pub iterations: u64,
    pub failed: u64,
    /// The steps' times (think steps left out), in ms.
    pub step_p50_ms: f64,
    pub step_p95_ms: f64,
    /// Every Chrome process of the run, at its peak: PSS on Linux, else
    /// RSS (see `memory`).
    pub chrome_rss_peak: u64,
    /// `pss` or `rss`: what `chrome_rss_peak` sums.
    #[serde(default)]
    pub memory: String,
    /// CPU seconds the Chromes used.
    pub chrome_cpu_s: f64,
    pub cdp_rss_peak: u64,
    pub cdp_cpu_s: f64,
    /// The browser contexts made, and the mean time one took, in ms.
    #[serde(default)]
    pub contexts: u64,
    #[serde(default)]
    pub context_setup_ms: f64,
    /// `chrome-headless-shell` or `chrome`.
    #[serde(default)]
    pub browser: String,
    /// Whether the run held (see the module's docs).
    #[serde(default)]
    pub held: bool,
}

impl LoadRun {
    /// The share of iterations that failed.
    #[allow(clippy::cast_precision_loss)]
    pub fn failed_share(&self) -> f64 {
        self.failed as f64 / self.iterations.max(1) as f64
    }

    fn wall_s(&self) -> f64 {
        Duration::from_millis(self.wall_ms).as_secs_f64().max(0.001)
    }

    /// The CPUs chrome-cdp and its Chromes kept busy, on average.
    pub fn cores(&self) -> f64 {
        (self.chrome_cpu_s + self.cdp_cpu_s) / self.wall_s()
    }

    /// chrome-cdp's own CPUs, on average.
    pub fn cdp_cores(&self) -> f64 {
        self.cdp_cpu_s / self.wall_s()
    }

    /// Chrome's CPU per user, in % of a core.
    pub fn chrome_cpu_per_user(&self) -> f64 {
        self.chrome_cpu_s / self.wall_s() / f64::from(self.users.max(1)) * 100.0
    }

    /// Chrome's peak RSS per user.
    pub fn chrome_rss_per_user(&self) -> u64 {
        self.chrome_rss_peak / u64::from(self.users.max(1))
    }
}

pub fn run(opts: &Options<'_>) -> Result<LoadReport, String> {
    let root = report::workspace_root();
    let journey =
        std::fs::read_to_string(root.join(JOURNEY)).map_err(|e| format!("read {JOURNEY}: {e}"))?;
    let server = load_server::LoadServer::start(load_server::Options {
        threads: 4,
        ..load_server::Options::default()
    })
    .map_err(|e| format!("start the load server: {e}"))?;
    let mut runs = Vec::new();
    for &per_browser in &opts.per_browser {
        for &users in &opts.users {
            crate::note(&format!(
                "load: {users} users, {per_browser} per Chrome, {} s",
                opts.duration.as_secs()
            ));
            let run = one_run(opts, &journey, server.url(), users, per_browser)?;
            crate::note(&format!(
                "load: {users} users, {per_browser} per Chrome: {} iterations, {} failed, step p95 {} ms, {} CPUs, chrome-cdp {} CPUs and {}",
                run.iterations,
                run.failed,
                report::fmt_num(run.step_p95_ms),
                report::fmt_num(run.cores()),
                report::fmt_num(run.cdp_cores()),
                bytes(run.cdp_rss_peak)
            ));
            runs.push(run);
        }
    }
    let cpus = Host::this().cpus;
    judge(&mut runs, cpus);
    Ok(LoadReport {
        created: report::now(),
        host: Host::this(),
        chrome: crate::micro::chrome_version(opts.chrome),
        browser: runs.first().map(|r| r.browser.clone()).unwrap_or_default(),
        cache: opts.cache.to_owned(),
        journey: JOURNEY.to_owned(),
        duration_s: opts.duration.as_secs(),
        ramp_up_s: opts.ramp_up.as_secs(),
        users_per_machine: runs.iter().filter(|r| r.held).map(|r| r.users).max(),
        runs,
    })
}

/// Marks the runs that held: few failures, CPU to spare on a machine of
/// `cpus`, and steps no slower than twice the smallest run's of the same
/// users per Chrome.
#[allow(clippy::cast_precision_loss)]
pub fn judge(runs: &mut [LoadRun], cpus: usize) {
    let mut smallest: BTreeMap<u32, f64> = BTreeMap::new();
    for run in runs.iter() {
        let p95 = smallest.entry(run.per_browser).or_insert(f64::MAX);
        if run.iterations > 0 {
            *p95 = p95.min(run.step_p95_ms);
        }
    }
    for run in runs.iter_mut() {
        let base = smallest.get(&run.per_browser).copied().unwrap_or(f64::MAX);
        run.held = run.iterations > 0
            && run.failed_share() < MAX_FAILED
            && run.cores() < MAX_CPU * cpus as f64
            && run.step_p95_ms <= base * MAX_SLOWDOWN;
    }
}

/// One run in a fresh sandbox: a project whose backend is the server.
fn one_run(
    opts: &Options<'_>,
    journey: &str,
    url: &str,
    users: u32,
    per_browser: u32,
) -> Result<LoadRun, String> {
    let sandbox = Sandbox::new(opts.bin, opts.chrome)?;
    let project = sandbox.dir.path().join("project");
    std::fs::create_dir_all(project.join("scenarios"))
        .map_err(|e| format!("create the project: {e}"))?;
    std::fs::write(
        project.join("chrome-cdp.toml"),
        format!(
            "[e2e]\ntests = [\"scenarios/*.cdp\"]\ndefault_backend = \"load\"\n\n\
             [e2e.backends.load]\nmode = \"real\"\nbase_url = \"{url}\"\n\n\
             [load]\ncache = \"{}\"\n",
            opts.cache
        ),
    )
    .map_err(|e| format!("write the project: {e}"))?;
    // A returning user keeps the shop's cart cookie, and the journey
    // expects an empty cart: the user clears its cookies (a new session)
    // and keeps its HTTP cache.
    let journey = if opts.cache == "returning" {
        format!("@before\n  cookies clear\n\n{journey}")
    } else {
        journey.to_owned()
    };
    std::fs::write(project.join("scenarios/shop.cdp"), journey)
        .map_err(|e| format!("write the journey: {e}"))?;
    let child = sandbox
        .cli()
        .current_dir(&project)
        .args([
            "load",
            "scenarios/shop.cdp",
            "--users",
            &users.to_string(),
            "--users-per-browser",
            &per_browser.to_string(),
            "--duration",
            &format!("{}s", opts.duration.as_secs()),
            "--ramp-up",
            &format!("{}s", opts.ramp_up.as_secs()),
            "--raw",
            "--keep-failures",
            "3",
            "--json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("run chrome-cdp load: {e}"))?;
    let root = child.id();
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut usage = Usage::default();
            while !stop.load(Ordering::Relaxed) {
                usage.sample(&processes(), root);
                std::thread::sleep(SAMPLE_EVERY);
            }
            usage
        })
    };
    let start = Instant::now();
    let out = child
        .wait_with_output()
        .map_err(|e| format!("run chrome-cdp load: {e}"));
    let wall = start.elapsed();
    stop.store(true, Ordering::Relaxed);
    let usage = sampler.join().unwrap_or_default();
    let out = out?;
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| {
        format!(
            "chrome-cdp load --json gave no JSON ({e}): {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )
    })?;
    let data = if value["ok"] == true {
        &value["data"]
    } else {
        &value["error"]["details"]
    };
    if data.is_null() {
        return Err(format!("chrome-cdp load failed: {}", value["error"]));
    }
    let steps = data["raw"]
        .as_str()
        .map(|p| std::fs::read_to_string(p).unwrap_or_default())
        .unwrap_or_default();
    let times = step_times(&steps);
    Ok(LoadRun {
        users,
        per_browser,
        browsers: u32::try_from(data["browsers"].as_u64().unwrap_or(0)).unwrap_or(0),
        wall_ms: u64::try_from(wall.as_millis()).unwrap_or(u64::MAX),
        iterations: data["iterations"].as_u64().unwrap_or(0),
        failed: data["failed"].as_u64().unwrap_or(0),
        step_p50_ms: crate::stats::percentile(&times, 50.0),
        step_p95_ms: crate::stats::percentile(&times, 95.0),
        chrome_rss_peak: usage.chrome_rss_peak,
        memory: memory_kind().to_owned(),
        chrome_cpu_s: usage.chrome_cpu_s,
        cdp_rss_peak: usage.cdp_rss_peak,
        cdp_cpu_s: usage.cdp_cpu_s,
        contexts: data["contexts"].as_u64().unwrap_or(0),
        context_setup_ms: data["context_setup_ms"].as_f64().unwrap_or(0.0),
        browser: data["browser"].as_str().unwrap_or_default().to_owned(),
        held: false,
    })
}

/// The durations of the steps of `steps.jsonl` that are not `think`
/// steps or the runner's own (`setup`, `memory`), in ms.
pub fn step_times(steps: &str) -> Vec<f64> {
    steps
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|s| {
            s["hook"] != "setup"
                && s["hook"] != "memory"
                && !s["command"]
                    .as_str()
                    .is_some_and(|c| c == "think" || c.starts_with("think "))
        })
        .filter_map(|s| Some(s["end_ms"].as_f64()? - s["start_ms"].as_f64()?))
        .collect()
}

/// A process, as `ps` shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    /// Resident memory, in bytes.
    pub rss: u64,
    /// CPU time so far, in seconds.
    pub cpu: f64,
}

/// Every process of the machine (`ps`; empty where there is none).
fn processes() -> Vec<Process> {
    Command::new("ps")
        .args(["-axo", "pid=,ppid=,rss=,time="])
        .output()
        .map(|out| parse_ps(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

/// `ps -o pid=,ppid=,rss=,time=`'s lines (RSS in KiB; time as
/// `[[dd-]hh:]mm:ss[.cc]`).
pub fn parse_ps(text: &str) -> Vec<Process> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let ppid = parts.next()?.parse().ok()?;
            let rss: u64 = parts.next()?.parse().ok()?;
            let cpu = cpu_time(parts.next()?)?;
            Some(Process {
                pid,
                ppid,
                rss: rss * 1024,
                cpu,
            })
        })
        .collect()
}

/// `1:02.50` → 62.5 s; `01:02:03` → 3723 s; `1-00:00:00` → a day.
pub fn cpu_time(text: &str) -> Option<f64> {
    let (days, rest) = match text.split_once('-') {
        Some((d, rest)) => (d.parse::<f64>().ok()?, rest),
        None => (0.0, text),
    };
    let mut seconds = 0.0;
    for part in rest.split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(days * 86_400.0 + seconds)
}

/// What a run's processes used: chrome-cdp (`root`) and every process
/// under it (its Chromes).
#[derive(Debug, Default)]
pub struct Usage {
    pub chrome_rss_peak: u64,
    pub chrome_cpu_s: f64,
    pub cdp_rss_peak: u64,
    pub cdp_cpu_s: f64,
    /// Each process's CPU time at the last read.
    seen: BTreeMap<u32, f64>,
}

impl Usage {
    /// Adds a read of `processes`: each one's CPU time since the last
    /// read (all of it for a new one), and the peaks.
    pub fn sample(&mut self, processes: &[Process], root: u32) {
        self.sample_with(processes, root, pss);
    }

    /// [`Usage::sample`], with a Chrome process's memory from `memory`
    /// when it knows it (else its RSS).
    pub fn sample_with(
        &mut self,
        processes: &[Process],
        root: u32,
        memory: impl Fn(u32) -> Option<u64>,
    ) {
        let mut tree = BTreeSet::from([root]);
        loop {
            let more: Vec<u32> = processes
                .iter()
                .filter(|p| tree.contains(&p.ppid) && !tree.contains(&p.pid))
                .map(|p| p.pid)
                .collect();
            if more.is_empty() {
                break;
            }
            tree.extend(more);
        }
        let mut chrome_rss = 0;
        for p in processes.iter().filter(|p| tree.contains(&p.pid)) {
            let memory = if p.pid == root {
                p.rss
            } else {
                memory(p.pid).unwrap_or(p.rss)
            };
            let grew = (p.cpu - self.seen.get(&p.pid).copied().unwrap_or(0.0)).max(0.0);
            self.seen.insert(p.pid, p.cpu);
            if p.pid == root {
                self.cdp_cpu_s += grew;
                self.cdp_rss_peak = self.cdp_rss_peak.max(p.rss);
            } else {
                self.chrome_cpu_s += grew;
                chrome_rss += memory;
            }
        }
        self.chrome_rss_peak = self.chrome_rss_peak.max(chrome_rss);
    }
}

/// What Chrome's memory is summed from here: `pss` on Linux, `rss`
/// elsewhere.
fn memory_kind() -> &'static str {
    if cfg!(target_os = "linux") {
        "pss"
    } else {
        "rss"
    }
}

/// A process's proportional set size on Linux (`/proc/<pid>/smaps_rollup`),
/// in bytes; `None` elsewhere or when it cannot be read.
fn pss(pid: u32) -> Option<u64> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let text = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok()?;
    let line = text.lines().find(|l| l.starts_with("Pss:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
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

pub fn markdown(r: &LoadReport) -> String {
    let mut md = format!(
        "Last run: {} on {} ({} CPUs), {}{}. `{}`, {} s per run after a {} s ramp-up, \
         `cache = \"{}\"`.\n\n",
        report::date(r.created),
        r.host.key(),
        r.host.cpus,
        r.chrome,
        if r.browser.is_empty() {
            String::new()
        } else {
            format!(" as {}", r.browser)
        },
        r.journey,
        r.duration_s,
        r.ramp_up_s,
        if r.cache.is_empty() { "new" } else { &r.cache }
    );
    let _ = writeln!(
        md,
        "**Users per machine: {}** (the most users of a run that held).\n",
        r.users_per_machine
            .map_or_else(|| "none held".to_owned(), |n| n.to_string())
    );
    let memory = r
        .runs
        .first()
        .map_or("RSS", |run| if run.memory == "pss" { "PSS" } else { "RSS" });
    let _ = writeln!(
        md,
        "| Users | Per Chrome | Chromes | Iterations | Failed | Step p50 | Step p95 | Chrome {memory} / user | Chrome CPU / user | chrome-cdp CPU | chrome-cdp RSS | All CPUs | Contexts | Context setup | Held |",
    );
    md.push_str("|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|:---:|\n");
    for run in &r.runs {
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {}% | {} ms | {} ms | {} | {}% | {} | {} | {} | {} | {} ms | {} |",
            run.users,
            run.per_browser,
            run.browsers,
            run.iterations,
            report::fmt_num(run.failed_share() * 100.0),
            report::fmt_num(run.step_p50_ms),
            report::fmt_num(run.step_p95_ms),
            bytes(run.chrome_rss_per_user()),
            report::fmt_num(run.chrome_cpu_per_user()),
            report::fmt_num(run.cdp_cores()),
            bytes(run.cdp_rss_peak),
            report::fmt_num(run.cores()),
            run.contexts,
            report::fmt_num(run.context_setup_ms),
            if run.held { "yes" } else { "no" }
        );
    }
    md
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{LoadRun, Usage, cpu_time, judge, parse_ps, step_times};

    #[test]
    fn cpu_times_parse_in_both_forms() {
        assert_eq!(cpu_time("1:02.50"), Some(62.5));
        assert_eq!(cpu_time("01:02:03"), Some(3723.0));
        assert_eq!(cpu_time("1-00:00:01"), Some(86_401.0));
        assert_eq!(cpu_time("x"), None);
    }

    #[test]
    fn usage_adds_what_each_process_used_since_the_last_read() {
        let mut usage = Usage::default();
        usage.sample_with(
            &parse_ps(
                "  10   1  2048  0:01.00\n  11  10  1024  0:02.00\n  12  99  9999  9:00.00\n",
            ),
            10,
            |_| None,
        );
        // The renderer 13 is new; 11 used one more second.
        usage.sample_with(
            &parse_ps(
                "  10   1  4096  0:01.50\n  11  10  1024  0:03.00\n  13  11  1024  0:00.50\n",
            ),
            10,
            |_| None,
        );
        assert!((usage.cdp_cpu_s - 1.5).abs() < 1e-9);
        assert!((usage.chrome_cpu_s - 3.5).abs() < 1e-9);
        assert_eq!(usage.cdp_rss_peak, 4096 * 1024);
        assert_eq!(usage.chrome_rss_peak, 2048 * 1024);
    }

    #[test]
    fn think_and_setup_are_not_step_times() {
        let steps = r#"{"command":"goto /","start_ms":0.0,"end_ms":120.0,"result":"ok"}
{"command":"think 2s","start_ms":120.0,"end_ms":2120.0,"result":"ok"}
{"command":"context new","hook":"setup","start_ms":0.0,"end_ms":5.0,"result":"error"}
{"command":"renderer memory","hook":"memory","start_ms":0.0,"end_ms":5.0,"result":"error"}
{"command":"click x","start_ms":2120.0,"end_ms":2150.0,"result":"ok"}"#;
        assert_eq!(step_times(steps), vec![120.0, 30.0]);
    }

    fn run(users: u32, failed: u64, p95: f64, cpu_s: f64) -> LoadRun {
        LoadRun {
            users,
            per_browser: 25,
            browsers: users.div_ceil(25),
            wall_ms: 10_000,
            iterations: 100,
            failed,
            step_p50_ms: p95 / 2.0,
            step_p95_ms: p95,
            chrome_rss_peak: 0,
            memory: "rss".to_owned(),
            chrome_cpu_s: cpu_s,
            cdp_rss_peak: 0,
            cdp_cpu_s: 0.0,
            contexts: 0,
            context_setup_ms: 0.0,
            browser: "chrome".to_owned(),
            held: false,
        }
    }

    #[test]
    fn a_run_holds_without_failures_saturation_or_slow_steps() {
        let mut runs = vec![
            run(25, 0, 100.0, 10.0),
            run(50, 0, 180.0, 20.0),
            run(100, 2, 150.0, 40.0),
            run(200, 0, 250.0, 60.0),
            run(400, 0, 150.0, 150.0),
        ];
        judge(&mut runs, 16);
        let held: Vec<bool> = runs.iter().map(|r| r.held).collect();
        // 100: 2% failed; 200: 2.5x slower; 400: 15 of 16 CPUs.
        assert_eq!(held, [true, true, false, false, false]);
    }
}
