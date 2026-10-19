//! `chrome-cdp load` (plan 23, milestone 23.1): many users at once,
//! each a real browser running the app's own JavaScript, through the
//! project's scenario tests against a real backend.
//!
//! The run works out its plan as `test` does ([`plan`]: the config, the
//! backend, the scenarios and their variables), checks every step as a
//! load step before any browser starts, and refuses the mock backend.
//! Then it drives its Chromes in-process, without the daemon: a [`pool`]
//! of `--browsers` Chromes, each an [`engine::Engine`] of its own over
//! one CDP connection, and one tokio task per virtual user ([`user`]) on
//! a multi-threaded runtime sized to the CPUs. Each user's iterations run
//! in fresh browser contexts of its Chrome, their steps call the engine
//! directly ([`step`]), leanly, and every step's timing and result goes
//! to the [`record`]s.

mod plan;
mod pool;
mod record;
mod step;
pub(crate) mod think;
mod user;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use engine::OpenRequest;
use protocol::schema::{LoadData, LoadJourney};
use protocol::{ErrorInfo, ErrorKind, Hint, Mode, Output};
use tokio::time::Instant;

use crate::cli::{DEFAULT_TIMEOUT, GlobalArgs, LoadArgs};
use crate::output;
use plan::LoadPlan;
use pool::Pool;
use record::Records;
use user::Shared;

/// How many users share a Chrome by default (`--users-per-browser`),
/// until plan 23.2 measures it.
pub(crate) const USERS_PER_BROWSER: u32 = 25;

/// The error kind of a step whose Chrome crashed.
pub(crate) const BROWSER_CRASH: &str = "browser_crash";

/// The stack of the runtime's threads.
const STACK_SIZE: usize = 8 * 1024 * 1024;

/// Where load runs keep their files, in the artifacts directory.
const RUNS_DIR: &str = "load";

pub(crate) fn run(args: &LoadArgs, global: &GlobalArgs) -> u8 {
    let mode = global.mode();
    let result = load(args, global, mode).and_then(|data| {
        let text = text(&data);
        if data.iterations > 0 && data.failed == data.iterations {
            return Err(ErrorInfo::new(
                ErrorKind::ExpectFailed,
                format!(
                    "every iteration failed ({} of {})",
                    data.failed, data.iterations
                ),
            )
            .note("errors", errors_line(&data.errors))
            .hint(Hint::text(match &data.failures_dir {
                Some(dir) => format!("see what the users saw in {dir}"),
                None => "run the journey as a test first: chrome-cdp test <path>".to_owned(),
            }))
            .details(serde_json::to_value(&data).unwrap_or_default()));
        }
        Ok(Output::typed(&data, text).limit(args.max_chars, "--max-chars 0 for all of it"))
    });
    output::print(mode, result)
}

/// The run, from its plan to its counts.
fn load(args: &LoadArgs, global: &GlobalArgs, mode: Mode) -> Result<LoadData, ErrorInfo> {
    let plan = Arc::new(LoadPlan::make(args, global)?);
    let linted = crate::lint::for_run(
        &plan.plan.config,
        &plan.plan.scenarios,
        &plan.plan.backend.name,
        global,
    )?;
    if linted.errors > 0 {
        return Err(crate::lint::failed(&linted, "lint"));
    }
    let browsers = browsers(args)?;
    let timeout = global.timeout_or(DEFAULT_TIMEOUT).max(DEFAULT_TIMEOUT);
    let chrome = crate::install::find_chrome(None, false, timeout)?;
    if mode == Mode::Text {
        for warning in &chrome.warnings {
            output::stderr(&format!("warning: {warning}\n"));
        }
    }
    let open = OpenRequest {
        chrome: Some(chrome),
        headed: Some(args.headed),
        timeout_ms: millis(timeout),
        ..OpenRequest::default()
    };
    let run_id = plan.plan.run.run_id.clone();
    let dir = plan.plan.config.artifacts.join(RUNS_DIR).join(&run_id);
    let records =
        Records::new(&dir, plan.journeys.len(), args.raw, args.keep_failures).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("cannot write the run's files in {}: {e}", dir.display()),
            )
        })?;
    if mode == Mode::Text {
        output::stdout(&starting(&plan, args, browsers));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let servers = std::sync::OnceLock::new();
    let servers_started = AtomicBool::new(false);
    let test_plan = &plan.plan;
    let ran = crate::test::with_servers(
        test_plan,
        &servers,
        &stop,
        mode,
        &servers_started,
        false,
        || {
            crate::test::with_backend(test_plan, mode, || {
                drive(Drive {
                    plan: plan.clone(),
                    args,
                    browsers,
                    open: open.clone(),
                    records,
                    stop: stop.clone(),
                })
            })
        },
    );
    let (records, pool_restarts, took) = ran??;
    records.flush();
    let counts = records.counts();
    Ok(LoadData {
        run: run_id,
        dir: dir.display().to_string(),
        backend: plan.plan.backend.name.clone(),
        journeys: plan
            .journeys
            .iter()
            .zip(&counts.journeys)
            .map(|(j, (iterations, failed))| LoadJourney {
                place: j.place.clone(),
                name: j.name.clone(),
                iterations: *iterations,
                failed: *failed,
            })
            .collect(),
        users: args.users,
        browsers,
        users_per_browser: args.users.div_ceil(browsers),
        duration_ms: millis(took),
        iterations: counts.iterations,
        failed: counts.failed,
        steps: counts.steps,
        errors: counts.errors,
        browser_crashes: pool_restarts,
        failures_kept: records.kept(),
        failures_dir: (records.kept() > 0).then(|| records.failures_dir.display().to_string()),
        raw: records.raw_path.as_ref().map(|p| p.display().to_string()),
    })
}

/// How many Chromes the run uses: `--browsers`, else enough for
/// `--users-per-browser` each, never more than there are users.
fn browsers(args: &LoadArgs) -> Result<u32, ErrorInfo> {
    let needed = args.users.div_ceil(args.users_per_browser);
    match args.browsers {
        None => Ok(needed),
        Some(n) if n.saturating_mul(args.users_per_browser) < args.users => Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!(
                "--browsers {n} with --users-per-browser {} hold {} users, not {}",
                args.users_per_browser,
                n * args.users_per_browser,
                args.users
            ),
        )
        .hint(Hint::text(format!(
            "use --browsers {needed}, or a larger --users-per-browser"
        )))),
        Some(n) => Ok(n.min(args.users)),
    }
}

/// What the run is about to do: `load: 50 users in 2 Chromes, 1 journey,
/// 1m against backend local (http://localhost:4174)`.
fn starting(plan: &LoadPlan, args: &LoadArgs, browsers: u32) -> String {
    let backend = &plan.plan.backend;
    let mut text = format!(
        "load: {} {} in {} {}, {} {}, {}",
        args.users,
        if args.users == 1 { "user" } else { "users" },
        browsers,
        if browsers == 1 { "Chrome" } else { "Chromes" },
        plan.journeys.len(),
        if plan.journeys.len() == 1 {
            "journey"
        } else {
            "journeys"
        },
        protocol::format::duration(args.duration)
    );
    if let Some(n) = args.iterations {
        let _ = write!(text, " (at most {n} iterations each)");
    }
    let _ = write!(text, " against backend {}", backend.name);
    if let Some(url) = &backend.base_url {
        let _ = write!(text, " ({url})");
    }
    text.push('\n');
    text
}

/// What [`drive`] needs.
struct Drive<'a> {
    plan: Arc<LoadPlan>,
    args: &'a LoadArgs,
    browsers: u32,
    open: OpenRequest,
    records: Records,
    stop: Arc<AtomicBool>,
}

/// Runs the users on a runtime of the run's own: starts the Chromes,
/// runs the `@before-all` hooks, the users, then the `@after-all` hooks.
/// The records, the Chromes started again, and how long the users ran.
fn drive(drive: Drive<'_>) -> Result<(Records, u64, Duration), ErrorInfo> {
    let threads = std::thread::available_parallelism().map_or(2, std::num::NonZero::get);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        // Page commands nest deep futures; the daemon runs them on a main
        // thread's stack.
        .thread_stack_size(STACK_SIZE)
        .thread_name("load")
        .enable_all()
        .build()
        .map_err(|e| {
            ErrorInfo::new(
                ErrorKind::LaunchFailed,
                format!("cannot start the runtime: {e}"),
            )
        })?;
    runtime.block_on(users(drive))
}

async fn users(drive: Drive<'_>) -> Result<(Records, u64, Duration), ErrorInfo> {
    let Drive {
        plan,
        args,
        browsers,
        open,
        records,
        stop,
    } = drive;
    // Ctrl-C stops new iterations; the running ones finish.
    let interrupted = stop.clone();
    let listener = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            interrupted.store(true, Ordering::SeqCst);
        }
    });
    let name = format!("load-{}", plan.plan.run.run_id);
    let pool = Pool::start(&name, browsers, open).await?;
    let mut records = records;
    records.start();
    let started = records.started;
    let shared = Arc::new(Shared {
        plan: plan.clone(),
        pool,
        records,
        think: args.think,
        stop: stop.clone(),
        ends: started + args.ramp_up + args.duration,
        iterations: args.iterations,
        file_saved: std::sync::OnceLock::new(),
        seed: seed(&plan.plan.run.run_id),
    });
    let result = run_users(shared.clone(), args, started).await;
    listener.abort();
    let Ok(shared) = Arc::try_unwrap(shared) else {
        return Err(ErrorInfo::new(
            ErrorKind::ProtocolError,
            "a load run's user outlived the run",
        ));
    };
    shared.pool.close().await;
    let took = result?;
    Ok((shared.records, shared.pool.restarts(), took))
}

/// The hooks and the users; how long the users ran.
async fn run_users(
    shared: Arc<Shared>,
    args: &LoadArgs,
    started: Instant,
) -> Result<Duration, ErrorInfo> {
    let files: Vec<usize> = {
        let mut files: Vec<usize> = shared.plan.journeys.iter().map(|j| j.file).collect();
        files.dedup();
        files.sort_unstable();
        files.dedup();
        files
    };
    let mut saved = vec![BTreeMap::new(); shared.plan.plan.scenarios.len()];
    for &file in &files {
        let hooks = &shared.plan.hooks[file];
        saved[file] = user::hook_once(
            &shared,
            file,
            &hooks.before_all,
            "before-all",
            BTreeMap::new(),
        )
        .await?;
    }
    let _ = shared.file_saved.set(saved.clone());
    let browsers = shared.pool.len();
    let users: Vec<_> = (1..=args.users)
        .map(|vu| {
            let start = started + user::start_offset(args.ramp_up, vu, args.users);
            let browser = usize::try_from(vu - 1).unwrap_or(0) % browsers;
            tokio::spawn(user::run(shared.clone(), vu, browser, start))
        })
        .collect();
    let mut panicked = None;
    for user in users {
        if let Err(e) = user.await
            && e.is_panic()
        {
            panicked = Some(e);
        }
    }
    if let Some(e) = panicked {
        return Err(ErrorInfo::new(
            ErrorKind::ProtocolError,
            format!("a load run's user stopped unexpectedly: {e}"),
        ));
    }
    let took = started.elapsed();
    let mut warnings = Vec::new();
    for &file in &files {
        let hooks = &shared.plan.hooks[file];
        if let Err(e) = user::hook_once(
            &shared,
            file,
            &hooks.after_all,
            "after-all",
            saved[file].clone(),
        )
        .await
        {
            warnings.push(e.message);
        }
    }
    for warning in warnings {
        output::stderr(&format!("warning: {warning}\n"));
    }
    Ok(took)
}

/// A seed for the users' random numbers from the run's id.
fn seed(run_id: &str) -> u64 {
    run_id.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    }) ^ SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| {
        u64::try_from(d.as_nanos() & u128::from(u64::MAX)).unwrap_or(0)
    })
}

/// The run in a few lines.
fn text(data: &LoadData) -> String {
    let mut text = format!(
        "{} {} in {}: {} failed\n",
        data.iterations,
        if data.iterations == 1 {
            "iteration"
        } else {
            "iterations"
        },
        protocol::format::duration(Duration::from_millis(data.duration_ms)),
        data.failed
    );
    for journey in &data.journeys {
        let _ = writeln!(
            text,
            "  {} {}: {} iterations, {} failed",
            journey.place, journey.name, journey.iterations, journey.failed
        );
    }
    let s = &data.steps;
    let _ = writeln!(
        text,
        "steps: {} ok, {} failed, {} timeout, {} error",
        s.ok, s.failed, s.timeout, s.error
    );
    if !data.errors.is_empty() {
        let _ = writeln!(text, "errors: {}", errors_line(&data.errors));
    }
    if data.browser_crashes > 0 {
        let _ = writeln!(
            text,
            "Chrome crashed {} {} and was started again",
            data.browser_crashes,
            if data.browser_crashes == 1 {
                "time"
            } else {
                "times"
            }
        );
    }
    if let Some(dir) = &data.failures_dir {
        let _ = writeln!(text, "failures kept: {} in {dir}", data.failures_kept);
    }
    if let Some(raw) = &data.raw {
        let _ = writeln!(text, "step timings: {raw}");
    }
    text.trim_end().to_owned()
}

/// `ExpectFailed 3, Timeout 1`.
fn errors_line(errors: &BTreeMap<String, u64>) -> String {
    errors
        .iter()
        .map(|(kind, n)| format!("{kind} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}
