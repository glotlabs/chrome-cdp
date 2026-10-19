//! The benchmark harness behind `cargo xtask bench` (plan 10.4). Five
//! suites: `micro` (per-command timings), `tokens` (snapshot sizes),
//! `agent` (Claude Code on scripted tasks with chrome-cdp), `scale`
//! (the test runner on a suite of 1,000 tests, plan 22.1) and `load`
//! (`chrome-cdp load` at hundreds of users, plan 23.1).
//! docs/benchmarks.md says how to run each, and what they measure.

// The results table goes to stdout.
#![allow(clippy::print_stdout)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bench::{agent, load, micro, note, report, scale, tokens};
use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "bench",
    about = "chrome-cdp's benchmarks (run through `cargo xtask bench`)"
)]
struct Cli {
    #[command(subcommand)]
    suite: Suite,
}

#[derive(Debug, Subcommand)]
enum Suite {
    /// Per-command timings: cold start, warm round trip, snapshot by page
    /// size, locators, click-to-report, route overhead, batch vs single
    Micro {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        gate: Gate,
        /// Cold starts to time
        #[arg(long, default_value_t = 5)]
        cold: usize,
        /// Warm round trips to time
        #[arg(long, default_value_t = 50)]
        warm: usize,
        /// Samples of every other measurement
        #[arg(long, default_value_t = 10)]
        samples: usize,
    },
    /// Snapshot tokens per corpus page
    Tokens {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        gate: Gate,
        /// auto (anthropic when ANTHROPIC_API_KEY or ANTHROPIC_AUTH_TOKEN is
        /// set), anthropic (the token counting API) or o200k (offline)
        #[arg(long, default_value = "auto")]
        tokenizer: String,
    },
    /// Claude Code on the tasks of bench/agent/tasks
    Agent {
        #[command(flatten)]
        common: Common,
        /// Only these tasks (repeatable)
        #[arg(long = "task")]
        tasks: Vec<String>,
        /// Runs per task
        #[arg(long, default_value_t = 5)]
        reps: usize,
        /// Claude Code's --model (its default when unset)
        #[arg(long)]
        model: Option<String>,
        /// The claude executable
        #[arg(long, default_value = "claude")]
        claude: PathBuf,
        /// Skip the tasks on public sites
        #[arg(long)]
        offline: bool,
        /// Run no agent: check that each task's verifier fails untouched and
        /// passes after the task's known solution
        #[arg(long)]
        check: bool,
        /// List the tasks and exit
        #[arg(long)]
        list: bool,
    },
    /// `chrome-cdp test` on the scale suite (cargo xtask scale-suite) with
    /// several --jobs: wall time, overhead by phase, peak RSS
    Scale {
        #[command(flatten)]
        common: Common,
        /// The suite's directory (cargo xtask bench scale writes it first)
        #[arg(long, default_value = "testdata/scale")]
        suite: PathBuf,
        /// The --jobs values to run, each with the runner's default
        /// --jobs-isolation or the one after a colon, e.g. `--jobs
        /// 1,4,16,16:context`
        #[arg(long, value_delimiter = ',', default_values_t = scale::JOBS.map(scale::Jobs::from))]
        jobs: Vec<scale::Jobs>,
        /// Then run each of N shards with the last --jobs, split by count
        /// and by the durations file, e.g. `--shards 4`
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
        shards: Option<u64>,
        /// Also publish the results as the baseline later milestones
        /// compare with (the `scale-baseline` section)
        #[arg(long)]
        baseline: bool,
    },
    /// `chrome-cdp load` on the reference journey against the load server
    /// at several user counts and Chromes per user: users per machine,
    /// Chrome's RSS and CPU per user, chrome-cdp's own CPU and RSS
    Load {
        #[command(flatten)]
        common: Common,
        /// The users of each run, e.g. `--users 25,50`
        #[arg(long, value_delimiter = ',', default_values_t = load::USERS)]
        users: Vec<u32>,
        /// The users per Chrome of each run, e.g. `--per-browser 25`
        #[arg(long, value_delimiter = ',', default_values_t = load::PER_BROWSER)]
        per_browser: Vec<u32>,
        /// Seconds each run's users start iterations, after the ramp-up
        #[arg(long, default_value_t = 60)]
        duration: u64,
        /// Seconds over which each run's users start
        #[arg(long, default_value_t = 10)]
        ramp_up: u64,
        /// `[load] cache` of the runs: `new` or `returning`
        #[arg(long, default_value = "new", value_parser = ["new", "returning"])]
        cache: String,
    },
}

#[derive(Debug, Args)]
struct Common {
    /// Where the chrome-cdp and fixtures binaries are
    #[arg(long, default_value = "target/release")]
    bin_dir: PathBuf,
    /// The Chrome both tools drive
    #[arg(long, env = "CHROME_PATH")]
    chrome: PathBuf,
    /// Do not write the results into docs/benchmarks.md
    #[arg(long)]
    no_publish: bool,
}

#[derive(Debug, Args)]
struct Gate {
    /// Fail if a metric is more than 15% worse than its committed baseline
    /// (bench/baselines/)
    #[arg(long)]
    check: bool,
    /// Record the results as the new baseline
    #[arg(long, conflicts_with = "check")]
    update_baseline: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.suite) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error(&e);
            ExitCode::FAILURE
        }
    }
}

/// The binary `name` in `dir`, which must exist.
fn binary(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let dir = if dir.is_relative() {
        report::workspace_root().join(dir)
    } else {
        dir.to_owned()
    };
    let path = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "no {} yet; build it (cargo xtask bench does)",
            path.display()
        ))
    }
}

fn run(suite: Suite) -> Result<(), String> {
    match suite {
        Suite::Micro {
            common,
            gate,
            cold,
            warm,
            samples,
        } => {
            let bin = binary(&common.bin_dir, "chrome-cdp")?;
            let r = micro::run(&micro::Options {
                bin: &bin,
                chrome: &common.chrome,
                samples: micro::Samples {
                    cold,
                    warm,
                    each: samples,
                },
            })?;
            let path = report::results_dir().join("micro.json");
            report::write_json(&path, &r)?;
            print!("{}", micro::markdown(&r));
            note(&format!("wrote {}", path.display()));
            if !common.no_publish {
                report::publish("micro", &micro::markdown(&r))?;
            }
            if gate.check || gate.update_baseline {
                let name = format!("micro-{}", r.host.key());
                report::gate(&name, &r.tools["chrome-cdp"], gate.update_baseline)?;
            }
            Ok(())
        }
        Suite::Tokens {
            common,
            gate,
            tokenizer,
        } => {
            let bin = binary(&common.bin_dir, "chrome-cdp")?;
            binary(&common.bin_dir, "fixtures")?;
            let dir = bin.parent().unwrap_or(Path::new("."));
            let r = tokens::run(dir, &common.chrome, &tokenizer)?;
            let path = report::results_dir().join("tokens.json");
            report::write_json(&path, &r)?;
            print!("{}", tokens::markdown(&r));
            note(&format!("wrote {}", path.display()));
            if !common.no_publish {
                report::publish("tokens", &tokens::markdown(&r))?;
            }
            if gate.check || gate.update_baseline {
                let name = tokens::baseline_name(&r.counts.tokenizer);
                report::gate(&name, &tokens::metrics(&r.counts), gate.update_baseline)?;
            }
            Ok(())
        }
        Suite::Scale {
            common,
            suite,
            jobs,
            shards,
            baseline,
        } => {
            let bin = binary(&common.bin_dir, "chrome-cdp")?;
            let suite = if suite.is_relative() {
                report::workspace_root().join(suite)
            } else {
                suite
            };
            let r = scale::run(&scale::Options {
                bin: &bin,
                chrome: &common.chrome,
                suite,
                jobs,
                shards: shards.and_then(|n| usize::try_from(n).ok()),
            })?;
            let path = report::results_dir().join("scale.json");
            report::write_json(&path, &r)?;
            let md = scale::markdown(&r);
            let shards_md = scale::shards_markdown(&r);
            print!("{md}");
            print!("{shards_md}");
            note(&format!("wrote {}", path.display()));
            if !common.no_publish {
                report::publish("scale", &md)?;
                if !shards_md.is_empty() {
                    report::publish("scale-shards", &shards_md)?;
                }
                if baseline {
                    report::publish("scale-baseline", &md)?;
                }
            }
            // Every number first, then whether the runs were clean.
            let failed: Vec<String> = r
                .runs
                .iter()
                .filter(|run| run.failed > 0 || run.flaky > 0)
                .map(|run| {
                    format!(
                        "--jobs {}: {} failed, {} flaky",
                        run.label(),
                        run.failed,
                        run.flaky
                    )
                })
                .collect();
            if failed.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "the scale suite was not clean: {}",
                    failed.join("; ")
                ))
            }
        }
        Suite::Load {
            common,
            users,
            per_browser,
            duration,
            ramp_up,
            cache,
        } => {
            let bin = binary(&common.bin_dir, "chrome-cdp")?;
            let r = load::run(&load::Options {
                bin: &bin,
                chrome: &common.chrome,
                users,
                per_browser,
                duration: std::time::Duration::from_secs(duration),
                ramp_up: std::time::Duration::from_secs(ramp_up),
                cache: &cache,
            })?;
            let path = report::results_dir().join("load.json");
            report::write_json(&path, &r)?;
            let md = load::markdown(&r);
            print!("{md}");
            note(&format!("wrote {}", path.display()));
            if !common.no_publish {
                report::publish("load", &md)?;
            }
            Ok(())
        }
        Suite::Agent {
            common,
            tasks,
            reps,
            model,
            claude,
            offline,
            check,
            list,
        } => {
            if list {
                for t in agent::select(&tasks, offline)? {
                    println!(
                        "{:<22} {}{}",
                        t.id,
                        t.title,
                        if t.network { " (internet)" } else { "" }
                    );
                }
                return Ok(());
            }
            let opts = agent::Options {
                tasks: tasks.clone(),
                reps,
                offline,
                agent: agent::run::AgentOptions {
                    claude,
                    model,
                    bin: binary(&common.bin_dir, "chrome-cdp")?,
                    chrome: common.chrome.clone(),
                },
                fixtures_bin: binary(&common.bin_dir, "fixtures")?,
                out: report::results_dir()
                    .join("agent")
                    .join(report::now().to_string()),
            };
            if check {
                return agent::check(&opts);
            }
            let r = agent::run(&opts)?;
            report::write_json(&opts.out.join("report.json"), &r)?;
            let md = agent::markdown(&r);
            std::fs::write(opts.out.join("summary.md"), &md).map_err(|e| e.to_string())?;
            print!("{md}");
            note(&format!("wrote {}", opts.out.display()));
            let whole = tasks.is_empty() && !offline;
            if !common.no_publish && whole {
                report::publish("agent", &md)?;
            } else if !common.no_publish {
                note("a partial run (--task or --offline): docs/benchmarks.md is left as it is");
            }
            Ok(())
        }
    }
}

#[allow(clippy::print_stderr)]
fn error(message: &str) {
    eprintln!("error: {message}");
}
