//! `chrome-cdp test` (plan 11, milestone 11.4): runs the project's E2E
//! scenarios.
//!
//! [`plan`] reads the `[e2e]` config and the scenario files, selects the
//! tests and checks every step before anything starts, so a typo fails at
//! once with its `file:line:col` (exit code 2). Then the run starts the
//! config's web servers (unless something answers on their URLs already),
//! opens a browser session of its own (`test-<pid>`, or `--session`), and
//! [`exec`] runs the tests in `jobs` workers, each test in a fresh browser
//! context. [`report`] prints a line per test as it ends (in the plan's
//! order), and the closing summary. The sessions it started are closed at
//! the end, and the web servers stopped, also when one exits early or on
//! Ctrl-C.

mod artifacts;
mod determinism;
mod exec;
mod plan;
mod report;
mod report_md;
pub(crate) mod reporters;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use e2e::web_server::{Stopped, WebServers};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Mode};

use crate::cli::{GlobalArgs, JobsIsolation, TestArgs};
use crate::{output, script};
use exec::{Options, Outcome};
use plan::Plan;

/// Runs `test` and returns the exit code; `explicit_session` says whether
/// `--session` (or its env var) named the session to use.
pub(crate) fn run(args: &TestArgs, global: &GlobalArgs, explicit_session: bool) -> u8 {
    let mode = global.mode();
    let plan = match Plan::make(args, global) {
        Ok(plan) => plan,
        Err(info) => return output::print(mode, Err(info)),
    };
    if args.list {
        let listed = report::listed(&plan, args.shard.as_deref(), args.max_chars);
        return output::print(mode, Ok(listed));
    }
    if plan.tests.is_empty() {
        let info = ErrorInfo::new(ErrorKind::NotFound, "no test matches").hint(
            Hint::run(Cmd::new("test").switch("list"))
                .why("lists the tests without --grep, --tags or --shard"),
        );
        return output::print(mode, Err(info));
    }
    let base = if explicit_session {
        global.session.clone()
    } else {
        format!("test-{}", std::process::id())
    };
    let sessions = match args.jobs_isolation {
        JobsIsolation::Context => vec![base],
        JobsIsolation::Browser => (1..=plan.jobs).map(|i| format!("{base}-{i}")).collect(),
    };
    let options = Options {
        global: global.clone(),
        sessions,
        update_snapshots: args.update_snapshots,
        bail: args.bail,
        own_sessions: !explicit_session,
        trace: args.trace,
        debug: None,
    };
    let started = Instant::now();
    if mode == Mode::Text {
        let files = plan
            .tests
            .iter()
            .map(|t| t.file)
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let runs = plan.tests.len() * usize::try_from(plan.repeat).unwrap_or(1);
        output::stdout(&format!(
            "running {} test{}{} from {files} file{}, {} at once\n",
            plan.tests.len(),
            if plan.tests.len() == 1 { "" } else { "s" },
            if plan.repeat > 1 {
                format!(" {} times each", plan.repeat)
            } else {
                String::new()
            },
            if files == 1 { "" } else { "s" },
            plan.jobs.min(u32::try_from(runs).unwrap_or(u32::MAX))
        ));
    }
    let stop = AtomicBool::new(false);
    let outcomes = with_servers(&plan, &stop, mode, || {
        run_tests(&plan, &options, args.headed, &stop, mode)
    });
    let outcomes = match outcomes.and_then(|r| r) {
        Ok(outcomes) => outcomes,
        Err(info) => return output::print(mode, Err(info)),
    };
    let duration = started.elapsed();
    let mut data = report::data(&plan, &outcomes, duration, args.shard.as_deref());
    let written = reporters::write(&args.reporter, &data, &plan.config.artifacts);
    let mut unwritten = None;
    match written {
        Ok(written) => data.reporters = written.into_iter().collect(),
        Err(info) => unwritten = Some(info.message),
    }
    let summary = report::summary(&data, duration);
    let mut result = report::finish(&plan, &data, duration, args.max_chars);
    if let Some(warning) = unwritten {
        result = match result {
            Ok(mut out) => {
                out.warnings.push(warning);
                Ok(out)
            }
            Err(info) => Err(info.note("warning", warning)),
        };
    }
    if mode == Mode::Text && result.is_err() {
        output::stdout(&format!("{summary}\n{}", report::reports(&data)));
    }
    output::print(mode, result)
}

/// Opens the sessions, runs the tests, and closes the sessions it opened.
fn run_tests(
    plan: &Plan,
    options: &Options,
    headed: bool,
    stop: &AtomicBool,
    mode: Mode,
) -> Result<Vec<Outcome>, ErrorInfo> {
    let mut opened = Vec::new();
    let mut failed = None;
    for session in &options.sessions {
        match open(&options.global, session, headed) {
            Ok(started) => {
                if started {
                    opened.push(session.clone());
                }
            }
            Err(info) => {
                failed = Some(info);
                break;
            }
        }
    }
    let print = |index: usize, outcome: &Outcome| {
        if mode == Mode::Text {
            output::stdout(&report::line(plan, &plan.tests[index], outcome));
        }
    };
    let result = failed.map_or_else(|| Ok(exec::execute(plan, options, stop, &print)), Err);
    for session in &opened {
        close(&options.global, session, options.own_sessions);
    }
    result
}

/// `open` in `session`; whether it started the browser.
fn open(global: &GlobalArgs, session: &str, headed: bool) -> Result<bool, ErrorInfo> {
    let mut words = vec!["open".to_owned()];
    if headed {
        words.push("--headed".to_owned());
    }
    let mut global = global.clone();
    session.clone_into(&mut global.session);
    global.context = None;
    global.json = false;
    global.quiet = false;
    let out = crate::run(script::command(&words, &global)?)?;
    Ok(out.data["started"].as_bool().unwrap_or(true))
}

/// `close` in `session`; a session the run named itself (`test-<pid>`)
/// has its directory removed too, so runs leave none behind.
fn close(global: &GlobalArgs, session: &str, own: bool) {
    let mut global = global.clone();
    session.clone_into(&mut global.session);
    global.context = None;
    let result = script::command(&["close".to_owned()], &global).and_then(crate::run);
    if let Err(e) = result {
        tracing::warn!("cannot close session {session}: {}", e.message);
        return;
    }
    if own {
        let paths = daemon::paths::SessionPaths::new(&daemon::paths::Env::current(), session);
        for dir in [&paths.dir, &paths.home] {
            if let Err(e) = std::fs::remove_dir_all(dir)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!("cannot remove {}: {e}", dir.display());
            }
        }
    }
}

/// Runs `work` with the config's web servers up (unless there are none),
/// and stops them after. A server that exits, or Ctrl-C, stops the run:
/// `stop` is set, so the tests not started are not run.
fn with_servers<T: Send>(
    plan: &Plan,
    stop: &AtomicBool,
    mode: Mode,
    work: impl FnOnce() -> T + Send,
) -> Result<T, ErrorInfo> {
    let config = &plan.config;
    if config.web_servers.is_empty() {
        return Ok(work());
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| {
            ErrorInfo::new(
                ErrorKind::LaunchFailed,
                format!("cannot start the runtime: {e}"),
            )
        })?;
    std::fs::create_dir_all(&config.artifacts).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot create {}: {e}", config.artifacts.display()),
        )
    })?;
    let mut servers =
        runtime.block_on(WebServers::start(&config.web_servers, &config.artifacts))?;
    if mode == Mode::Text {
        for server in servers.servers() {
            let how = if server.reused {
                "already running".to_owned()
            } else {
                format!("started in {}", protocol::format::duration(server.ready_in))
            };
            output::stdout(&format!("web server {} ({how})\n", server.url));
        }
    }
    std::thread::scope(|scope| {
        let (tx, rx) = tokio::sync::oneshot::channel();
        scope.spawn(move || {
            let _ = tx.send(work());
        });
        let interrupt = async {
            let _ = tokio::signal::ctrl_c().await;
        };
        let ran = runtime.block_on(servers.run(rx, interrupt));
        let result = match ran {
            Ok(Ok(done)) => Ok(done),
            Ok(Err(_)) => Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                "the test run stopped unexpectedly",
            )),
            Err(Stopped::Exited(info)) => {
                stop.store(true, Ordering::SeqCst);
                Err(info)
            }
            Err(Stopped::Interrupted) => {
                stop.store(true, Ordering::SeqCst);
                Err(ErrorInfo::new(ErrorKind::Timeout, "interrupted (Ctrl-C)")
                    .hint(Hint::text("the tests that had started ran to their end")))
            }
        };
        runtime.block_on(servers.stop());
        result
    })
}
