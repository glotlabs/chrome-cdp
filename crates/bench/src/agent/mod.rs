//! The agent task benchmark (plan 10.4): Claude Code headless on about 25
//! scripted tasks with only `chrome-cdp skill`, N times each. A verifier decides success; tokens, turns
//! and wall time come from Claude Code's result.
//!
//! `--check` runs no agent: for each task it checks that the verifier
//! fails on the untouched task and passes after the task's known solution,
//! so a broken task shows up before anyone pays for a run.

pub mod dev;
pub mod run;
pub mod task;
pub mod verify;
pub mod workdir;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use fixtures::Fixtures;
use serde::{Deserialize, Serialize};

use crate::report::{self, Host};
use crate::sandbox::Sandbox;
use crate::stats;
use run::{AgentOptions, AgentRun};
use task::{Task, Vars};
use verify::Check;

#[derive(Debug)]
pub struct Options {
    /// Task ids to run (all when empty).
    pub tasks: Vec<String>,
    pub reps: usize,
    /// Skip the tasks on public sites.
    pub offline: bool,
    pub agent: AgentOptions,
    /// The fixtures binary (`{fixtures}`: the e2e-shop's real backend).
    pub fixtures_bin: PathBuf,
    /// Where runs keep their logs and results.
    pub out: PathBuf,
}

/// One task done once.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    pub task: String,
    pub rep: usize,
    pub success: bool,
    pub checks: Vec<CheckRecord>,
    pub run: AgentRun,
    /// Setting up or running failed (not the agent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckRecord {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

impl From<Check> for CheckRecord {
    fn from(c: Check) -> Self {
        Self {
            name: c.name,
            ok: c.ok,
            detail: c.detail,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentReport {
    pub suite: String,
    pub created: u64,
    pub host: Host,
    pub chrome: String,
    pub model: String,
    pub reps: usize,
    pub episodes: Vec<Episode>,
}

/// The selected tasks.
pub fn select(ids: &[String], offline: bool) -> Result<Vec<Task>, String> {
    let all = task::load_all()?;
    for id in ids {
        if !all.iter().any(|t| &t.id == id) {
            let known: Vec<&str> = all.iter().map(|t| t.id.as_str()).collect();
            return Err(format!("no task `{id}`; tasks: {}", known.join(", ")));
        }
    }
    Ok(all
        .into_iter()
        .filter(|t| ids.is_empty() || ids.contains(&t.id))
        .filter(|t| !(offline && t.network))
        .collect())
}

/// A free local port.
fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| format!("find a free port: {e}"))
}

/// A task's working dir, servers and placeholders, ready for an agent
/// or a solution.
struct Setup {
    /// `[setup] dev`'s server. Fields drop in order: it stops before its
    /// files go.
    _dev_server: Option<dev::DevServer>,
    workdir: tempfile::TempDir,
    vars: Vars,
    before: workdir::Files,
    dev: Option<Fixtures>,
    /// Records the shared site had before this episode.
    records_from: usize,
}

impl Setup {
    fn new(task: &Task, site: &Fixtures, fixtures_bin: &Path) -> Result<Self, String> {
        let workdir = tempfile::Builder::new()
            .prefix(&format!("bench-{}-", task.id))
            .tempdir()
            .map_err(|e| format!("create a working dir: {e}"))?;
        let mut vars = Vars::default();
        vars.set("site", site.a());
        vars.set("site_b", site.b());
        vars.set("workdir", workdir.path().display().to_string());
        let port = free_port()?;
        vars.set("port", port.to_string());
        vars.set("port2", free_port()?.to_string());
        vars.set("fixtures", fixtures_bin.display().to_string());
        let before = workdir::prepare(task, workdir.path(), &vars)?;
        let dev = match &task.setup.serve {
            Some(dir) => {
                let dev = Fixtures::start_in(workdir.path().join(dir), 0, 0)
                    .map_err(|e| format!("serve {dir}: {e}"))?;
                vars.set("dev", dev.a());
                Some(dev)
            }
            None => None,
        };
        let dev_server = match &task.setup.dev {
            Some(_) if dev.is_some() => {
                return Err("a task has `serve` or `dev`, not both".to_owned());
            }
            Some(command) => {
                let server = dev::DevServer::start(&vars.expand(command), workdir.path(), port)?;
                vars.set("dev", format!("http://127.0.0.1:{port}"));
                Some(server)
            }
            None => None,
        };
        Ok(Self {
            workdir,
            vars,
            before,
            dev,
            _dev_server: dev_server,
            records_from: site.records().len(),
        })
    }

    fn verify(
        &self,
        task: &Task,
        site: &Fixtures,
        reply: &str,
        bin: &Path,
        chrome: &Path,
    ) -> Vec<Check> {
        let mut records: Vec<_> = site.records().split_off(self.records_from);
        if let Some(dev) = &self.dev {
            records.extend(dev.records());
        }
        verify::verify(
            task,
            &verify::Context {
                workdir: self.workdir.path(),
                reply,
                records: &records,
                vars: &self.vars,
                before: &self.before,
                bin,
                chrome,
            },
        )
    }
}

fn start_site() -> Result<Fixtures, String> {
    Fixtures::start(0, 0).map_err(|e| format!("start the fixture server: {e}"))
}

/// `--check`: every selected task's verifier fails untouched and passes
/// after its solution.
pub fn check(opts: &Options) -> Result<(), String> {
    let tasks = select(&opts.tasks, opts.offline)?;
    let site = start_site()?;
    let mut broken = Vec::new();
    let mut skipped = 0;
    for task in &tasks {
        if let Some(why) = missing_link(task) {
            crate::note(&format!("skip  {}: {why}", task.id));
            skipped += 1;
            continue;
        }
        let outcome = check_one(task, &site, opts).unwrap_or_else(|e| Err(format!("error: {e}")));
        match outcome {
            Ok(()) => crate::note(&format!("ok    {}", task.id)),
            Err(why) => {
                crate::note(&format!("FAIL  {}: {why}", task.id));
                broken.push(task.id.clone());
            }
        }
    }
    if broken.is_empty() {
        crate::note(&format!(
            "all {} tasks check out{}",
            tasks.len() - skipped,
            if skipped > 0 {
                format!(" ({skipped} skipped)")
            } else {
                String::new()
            }
        ));
        Ok(())
    } else {
        Err(format!(
            "{} of {} tasks are broken: {}",
            broken.len(),
            tasks.len(),
            broken.join(", ")
        ))
    }
}

/// Why `task` cannot be set up here: a `link` whose source is missing (a
/// fixture app's `node_modules` on a machine without Node).
fn missing_link(task: &Task) -> Option<String> {
    let root = report::workspace_root();
    task.setup
        .link
        .iter()
        .find(|l| !root.join(&l.from).exists())
        .map(|l| {
            let hint = l
                .hint
                .as_deref()
                .map(|h| format!("; {h}"))
                .unwrap_or_default();
            format!("{} is missing{hint}", l.from)
        })
}

/// `Ok(Ok(()))` when the task checks out, `Ok(Err(why))` when not.
fn check_one(task: &Task, site: &Fixtures, opts: &Options) -> Result<Result<(), String>, String> {
    let (bin, chrome) = (&opts.agent.bin, &opts.agent.chrome);
    let summary = |checks: &[Check]| {
        checks
            .iter()
            .map(|c| {
                format!(
                    "{} {}{}",
                    if c.ok { "✓" } else { "✗" },
                    c.name,
                    if c.detail.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", c.detail.replace('\n', " / "))
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    // Untouched, the task must not pass.
    let untouched = Setup::new(task, site, &opts.fixtures_bin)?;
    let checks = untouched.verify(task, site, "", bin, chrome);
    if checks.iter().all(|c| c.ok) {
        return Ok(Err(format!(
            "passes with nothing done: {}",
            summary(&checks)
        )));
    }
    drop(untouched);

    let solved = Setup::new(task, site, &opts.fixtures_bin)?;
    let sandbox = Sandbox::new(bin, chrome)?;
    workdir::apply_solution(task, solved.workdir.path(), &solved.vars)?;
    if let Some(script) = &task.solution.batch {
        batch(
            &sandbox,
            solved.workdir.path(),
            &solved.vars.expand(script),
            task.solution.continue_on_error,
        )?;
    }
    let reply = task
        .solution
        .answer
        .as_deref()
        .map(|a| format!("ANSWER: {a}"))
        .unwrap_or_default();
    let checks = solved.verify(task, site, &reply, bin, chrome);
    if checks.iter().all(|c| c.ok) {
        Ok(Ok(()))
    } else {
        Ok(Err(format!(
            "the solution does not pass: {}",
            summary(&checks)
        )))
    }
}

/// Runs a `chrome-cdp batch` script in `dir`.
fn batch(sandbox: &Sandbox, dir: &Path, script: &str, keep_going: bool) -> Result<(), String> {
    let mut cmd = sandbox.cli();
    cmd.arg("batch");
    if keep_going {
        cmd.arg("--continue");
    }
    let mut child = cmd
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("run batch: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(script.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    // With --continue a step fails on purpose; the verifier judges.
    if out.status.success() || keep_going {
        Ok(())
    } else {
        Err(format!(
            "the solution's batch failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// Runs the benchmark: every selected task, `reps` times, one at a
/// time (tasks use fixed ports and a real browser each).
pub fn run(opts: &Options) -> Result<AgentReport, String> {
    let tasks = select(&opts.tasks, opts.offline)?;
    let site = start_site()?;
    std::fs::create_dir_all(&opts.out)
        .map_err(|e| format!("create {}: {e}", opts.out.display()))?;
    let log_path = opts.out.join("episodes.jsonl");
    let mut log = std::fs::File::create(&log_path)
        .map_err(|e| format!("create {}: {e}", log_path.display()))?;
    let mut episodes = Vec::new();
    let total = tasks.len() * opts.reps;
    for rep in 1..=opts.reps {
        for task in &tasks {
            crate::note(&format!(
                "[{}/{total}] {} · run {rep}",
                episodes.len() + 1,
                task.id
            ));
            let e = episode(task, rep, &site, opts);
            crate::note(&format!(
                "  {} in {} turns, {} tokens, {} file reads (+{} in the shell), {:.0}s{}",
                if e.success { "passed" } else { "FAILED" },
                e.run.turns,
                e.run.tokens.total(),
                e.run.file_reads,
                e.run.shell_reads,
                Duration::from_millis(e.run.wall_ms).as_secs_f64(),
                e.error
                    .as_deref()
                    .map(|x| format!(" ({x})"))
                    .unwrap_or_default()
            ));
            let line = serde_json::to_string(&e).map_err(|x| x.to_string())?;
            writeln!(log, "{line}").map_err(|x| x.to_string())?;
            episodes.push(e);
        }
    }
    Ok(AgentReport {
        suite: "agent".to_owned(),
        created: report::now(),
        host: Host::this(),
        chrome: crate::micro::chrome_version(&opts.agent.chrome),
        model: opts
            .agent
            .model
            .clone()
            .unwrap_or_else(|| "Claude Code's default".to_owned()),
        reps: opts.reps,
        episodes,
    })
}

fn episode(task: &Task, rep: usize, site: &Fixtures, opts: &Options) -> Episode {
    let mut e = Episode {
        task: task.id.clone(),
        rep,
        success: false,
        checks: Vec::new(),
        run: AgentRun::default(),
        error: None,
    };
    let result = (|| -> Result<(AgentRun, Vec<Check>), String> {
        let setup = Setup::new(task, site, &opts.fixtures_bin)?;
        let sandbox = Sandbox::new(&opts.agent.bin, &opts.agent.chrome)?;
        let prompt = setup.vars.expand(&task.prompt);
        let log = opts.out.join(format!("{}.{rep}", task.id));
        let run = run::run_agent(
            &prompt,
            setup.workdir.path(),
            &sandbox,
            &opts.agent,
            Duration::from_secs(task.timeout_secs),
            &log,
        )?;
        // The agent's browser goes before the checks start their own.
        drop(sandbox);
        let checks = setup.verify(task, site, &run.reply, &opts.agent.bin, &opts.agent.chrome);
        Ok((run, checks))
    })();
    match result {
        Ok((run, checks)) => {
            e.success = !run.timed_out && !checks.is_empty() && checks.iter().all(|c| c.ok);
            if run.timed_out {
                e.error = Some(format!("timed out after {}s", task.timeout_secs));
            }
            e.run = run;
            e.checks = checks.into_iter().map(CheckRecord::from).collect();
        }
        Err(why) => e.error = Some(why),
    }
    e
}

/// Aggregates over all episodes.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub episodes: usize,
    pub passed: usize,
    pub tokens_median: f64,
    pub tokens_mean: f64,
    pub turns_median: f64,
    /// Read, Grep and Glob calls.
    pub file_reads_median: f64,
    /// Bash commands that read or list files.
    pub shell_reads_median: f64,
    pub wall_median_s: f64,
    pub cost_usd: f64,
}

#[allow(clippy::cast_precision_loss)]
pub fn summarize(episodes: &[Episode]) -> Option<Summary> {
    if episodes.is_empty() {
        return None;
    }
    let tokens: Vec<f64> = episodes
        .iter()
        .map(|e| e.run.tokens.total() as f64)
        .collect();
    let turns: Vec<f64> = episodes.iter().map(|e| e.run.turns as f64).collect();
    let reads: Vec<f64> = episodes.iter().map(|e| e.run.file_reads as f64).collect();
    let shell: Vec<f64> = episodes.iter().map(|e| e.run.shell_reads as f64).collect();
    let wall: Vec<f64> = episodes
        .iter()
        .map(|e| e.run.wall_ms as f64 / 1000.0)
        .collect();
    Some(Summary {
        episodes: episodes.len(),
        passed: episodes.iter().filter(|e| e.success).count(),
        tokens_median: stats::percentile(&tokens, 50.0),
        tokens_mean: stats::mean(&tokens),
        turns_median: stats::percentile(&turns, 50.0),
        file_reads_median: stats::percentile(&reads, 50.0),
        shell_reads_median: stats::percentile(&shell, 50.0),
        wall_median_s: stats::percentile(&wall, 50.0),
        cost_usd: episodes.iter().map(|e| e.run.cost_usd).sum(),
    })
}

/// The agent section of docs/benchmarks.md.
#[allow(clippy::cast_precision_loss)]
pub fn markdown(r: &AgentReport) -> String {
    let tasks: BTreeSet<&str> = r.episodes.iter().map(|e| e.task.as_str()).collect();
    let mut md = format!(
        "Last run: {} on {}, {}, model: {}; {} tasks, {} run(s) per task.\n\n",
        report::date(r.created),
        r.host.key(),
        r.chrome,
        r.model,
        tasks.len(),
        r.reps
    );
    md.push_str("| Success | Tokens (median) | Tokens (mean) | Turns (median) | Files read (median) | Shell reads (median) | Wall time (median) | Cost |\n|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    if let Some(s) = summarize(&r.episodes) {
        let _ = writeln!(
            md,
            "| {}/{} ({:.0}%) | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0}s | ${:.2} |",
            s.passed,
            s.episodes,
            s.passed as f64 * 100.0 / s.episodes.max(1) as f64,
            s.tokens_median,
            s.tokens_mean,
            s.turns_median,
            s.file_reads_median,
            s.shell_reads_median,
            s.wall_median_s,
            s.cost_usd
        );
    }
    md.push_str("\n| Task | Success | Tokens (median) |\n|---|---:|---:|\n");
    for task in &tasks {
        let mine: Vec<&Episode> = r.episodes.iter().filter(|e| e.task == *task).collect();
        let tokens: Vec<f64> = mine.iter().map(|e| e.run.tokens.total() as f64).collect();
        let _ = writeln!(
            md,
            "| `{task}` | {}/{} | {:.0} |",
            mine.iter().filter(|e| e.success).count(),
            mine.len(),
            stats::percentile(&tokens, 50.0)
        );
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use run::Tokens;

    fn episode(task: &str, success: bool, tokens: u64) -> Episode {
        Episode {
            task: task.into(),
            rep: 1,
            success,
            checks: vec![],
            run: AgentRun {
                turns: 4,
                file_reads: 6,
                shell_reads: 2,
                tokens: Tokens {
                    input: tokens,
                    ..Tokens::default()
                },
                wall_ms: 30_000,
                cost_usd: 0.5,
                ..AgentRun::default()
            },
            error: None,
        }
    }

    #[test]
    fn summaries_overall_and_per_task() {
        let episodes = [
            episode("login", true, 100),
            episode("login", false, 300),
            episode("search", true, 900),
        ];
        let s = summarize(&episodes[..2]).unwrap();
        assert_eq!((s.passed, s.episodes), (1, 2));
        assert!((s.tokens_mean - 200.0).abs() < 1e-9);
        let md = markdown(&AgentReport {
            suite: "agent".into(),
            created: 0,
            host: Host {
                os: "linux".into(),
                arch: "x86_64".into(),
                cpus: 2,
            },
            chrome: "Chrome".into(),
            model: "m".into(),
            reps: 2,
            episodes: episodes.to_vec(),
        });
        assert!(
            md.contains("| 2/3 (67%) | 300 | 433 | 4 | 6 | 2 | 30s | $1.50 |"),
            "{md}"
        );
        assert!(md.contains("| `login` | 1/2 | 100 |"), "{md}");
        assert!(md.contains("| `search` | 1/1 | 900 |"), "{md}");
    }

    #[test]
    fn unknown_tasks_are_named() {
        let err = select(&["nope".into()], false).unwrap_err();
        assert!(err.starts_with("no task `nope`; tasks: "), "{err}");
    }
}
