//! The benchmark harness behind `cargo xtask bench` (plan 10.4). Three
//! suites: `micro` (per-command timings), `tokens` (snapshot sizes) and
//! `agent` (Claude Code on scripted tasks with chrome-cdp).
//! docs/benchmarks.md says how to run each, and what they measure.

// The results table goes to stdout.
#![allow(clippy::print_stdout)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bench::{agent, micro, note, report, tokens};
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
