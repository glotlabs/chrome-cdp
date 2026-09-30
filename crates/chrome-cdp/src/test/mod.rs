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

mod all;
mod artifacts;
mod backend;
mod determinism;
mod exec;
pub(crate) mod plan;
mod record;
mod report;
mod report_md;
pub(crate) mod reporters;
mod vars;
mod verify;
mod watch;

use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use e2e::web_server::{Stopped, WebServers};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Mode};

use crate::cli::{GlobalArgs, JobsIsolation, TestArgs};
use crate::{output, script};
use exec::{Kept, Options, Outcome};
use plan::Plan;

pub(crate) use vars::saved_names as vars_saved_names;

/// The command `cli` runs when it needs the mocks (it fails against a
/// real backend): `mock state`, `mock seed`, `mock ws send` and `expect
/// mock-state`.
pub(crate) fn mock_only(cli: &crate::cli::Cli) -> Option<&'static str> {
    use crate::cli::{Command, MockCommand};
    match &cli.command {
        Some(Command::Mock(args)) => match &args.command {
            MockCommand::State(_) => Some("mock state"),
            MockCommand::Seed(_) => Some("mock seed"),
            MockCommand::Ws(_) => Some("mock ws send"),
            _ => None,
        },
        Some(Command::Expect(args)) if args.target.as_deref() == Some("mock-state") => {
            Some("expect mock-state")
        }
        _ => None,
    }
}

/// Points the baseline commands of a step of project `root` at the
/// project's E2E folders (`e2e/snapshots`, and `e2e/audit` for `expect
/// --audit NAME`) unless the step gives `--dir`; `update` is `test
/// --update-snapshots`. `test` and `run file.cdp` (of a file inside a
/// project with an `[e2e]` section) share them.
pub(crate) fn project_baselines(cli: &mut crate::cli::Cli, root: &std::path::Path, update: bool) {
    use crate::cli::Command;
    let snapshots = || root.join(plan::SNAPSHOTS);
    match &mut cli.command {
        Some(Command::ExpectSnapshot(args)) => {
            args.update |= update;
            args.dir.get_or_insert_with(snapshots);
        }
        Some(Command::ExpectScreenshot(args)) => {
            args.update |= update;
            args.dir.get_or_insert_with(snapshots);
        }
        Some(Command::Expect(args)) if args.layout => {
            args.update |= update;
            args.dir.get_or_insert_with(snapshots);
        }
        Some(Command::Expect(args)) if args.audit && args.baseline.is_some() => {
            args.update |= update;
            args.dir.get_or_insert_with(|| root.join(plan::AUDITS));
        }
        _ => {}
    }
}

/// Runs `test` and returns the exit code; `explicit_session` says whether
/// `--session` (or its env var) named the session to use.
pub(crate) fn run(args: &TestArgs, global: &GlobalArgs, explicit_session: bool) -> u8 {
    let mode = global.mode();
    if args.backend.as_deref() == Some(e2e::backend::ALL) {
        return all::run(args, global, explicit_session);
    }
    let plan = match Plan::make(args, global, None) {
        Ok(plan) => plan,
        Err(info) => return output::print(mode, Err(info)),
    };
    if args.list {
        let listed = report::listed(&plan, args.shard.as_deref(), args.max_chars);
        return output::print(mode, Ok(listed));
    }
    if let Err(info) = ready(&plan, args, global, mode) {
        return output::print(mode, Err(info));
    }
    let options = options(&plan, args, global, explicit_session);
    let stop = AtomicBool::new(false);
    let servers_started = AtomicBool::new(false);
    let ran = with_servers(
        &plan,
        &options.mock_server,
        &stop,
        mode,
        &servers_started,
        args.watch,
        || {
            with_backend(&plan, mode, || {
                run_tests(&options, args.headed, || {
                    let started = Instant::now();
                    starting(&plan, mode);
                    let outcomes = execute(&plan, &options, &stop, mode);
                    if !args.watch {
                        return Ran::Once(outcomes, started.elapsed());
                    }
                    let kept = None;
                    let servers = servers_started.load(Ordering::SeqCst);
                    let code = conclude(
                        &plan,
                        args,
                        &outcomes,
                        started.elapsed(),
                        kept,
                        servers,
                        &options,
                        mode,
                    );
                    let rerun = |plan: &Plan| {
                        let started = Instant::now();
                        starting(plan, mode);
                        let outcomes = execute(plan, &options, &stop, mode);
                        conclude(
                            plan,
                            args,
                            &outcomes,
                            started.elapsed(),
                            None,
                            servers,
                            &options,
                            mode,
                        )
                    };
                    let make = || Plan::make(args, global, None);
                    let explicit = !args.paths.is_empty();
                    Ran::Watched(watch::run(&plan, code, &stop, mode, rerun, make, explicit))
                })
            })
        },
    );
    match ran.and_then(|r| r) {
        Ok((Ran::Once(outcomes, duration), kept)) => conclude(
            &plan,
            args,
            &outcomes,
            duration,
            kept,
            servers_started.load(Ordering::SeqCst),
            &options,
            mode,
        ),
        Ok((Ran::Watched(code), _)) => code,
        Err(info) => output::print(mode, Err(info)),
    }
}

/// Before a browser starts: a plan with tests, and mocks that check
/// (plan 11.8): the sets the run applies, and with `--verify-mocks` the
/// sets the tests would have in mock mode.
fn ready(plan: &Plan, args: &TestArgs, global: &GlobalArgs, mode: Mode) -> Result<(), ErrorInfo> {
    if plan.tests.is_empty() {
        return Err(ErrorInfo::new(ErrorKind::NotFound, "no test matches").hint(
            Hint::run(Cmd::new("test").switch("list"))
                .why("lists the tests without --grep, --tags or --shard"),
        ));
    }
    if (args.verify_mocks || args.record_mocks) && !plan.real() {
        let flag = if args.verify_mocks {
            "--verify-mocks compares mocks with"
        } else {
            "--record-mocks records"
        };
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!(
                "{flag} a real backend, and {} is the mock one",
                plan.backend.name
            ),
        )
        .hint(Hint::text(
            "add --backend <name> of a real backend in [e2e.backends]",
        )));
    }
    let mut sets: Vec<String> = Vec::new();
    let mut add = |names: &[String]| {
        for name in names {
            if !sets.contains(name) {
                sets.push(name.clone());
            }
        }
    };
    if plan.real() {
        add(&plan.backend.mocks);
    }
    if !plan.real() || args.verify_mocks {
        add(&plan.config.default_mocks);
        for test in &plan.tests {
            add(&test.settings.mocks);
        }
    }
    // `lint`'s scenario rules (11.12): errors stop the run.
    let linted = crate::lint::for_run(&plan.config, &plan.scenarios, &plan.backend.name, global)?;
    if linted.errors > 0 {
        return Err(crate::lint::failed(&linted, "lint"));
    }
    if mode == Mode::Text && linted.warnings > 0 {
        output::stderr(&format!(
            "warning: {}; `chrome-cdp lint` lists them\n",
            if linted.warnings == 1 {
                "1 lint warning".to_owned()
            } else {
                format!("{} lint warnings", linted.warnings)
            }
        ));
    }
    let warnings = crate::mock::check_for_run(&plan.config, &sets)?;
    if mode == Mode::Text {
        const SHOWN: usize = 5;
        let mut text = String::new();
        for w in warnings.iter().take(SHOWN) {
            let _ = writeln!(text, "warning: {w}");
        }
        if warnings.len() > SHOWN {
            let _ = writeln!(
                text,
                "warning: {} more from the mock check; `chrome-cdp mock check` lists them",
                warnings.len() - SHOWN
            );
        }
        output::stderr(&text);
    }
    Ok(())
}

/// How the run's steps are made: the sessions, and the flags steps
/// inherit, with the backend's base_url.
fn options(plan: &Plan, args: &TestArgs, global: &GlobalArgs, explicit_session: bool) -> Options {
    let base = if explicit_session {
        global.session.clone()
    } else if args.debug {
        DEBUG_SESSION.to_owned()
    } else {
        format!("test-{}", std::process::id())
    };
    let sessions: Vec<String> = match args.jobs_isolation {
        JobsIsolation::Context => vec![base],
        JobsIsolation::Browser => (1..=plan.jobs).map(|i| format!("{base}-{i}")).collect(),
    };
    if args.debug && !explicit_session {
        // What an earlier --debug run left open.
        for session in &sessions {
            close(global, session, true);
        }
    }
    let mut global = global.clone();
    if plan.real()
        && let Some(url) = &plan.backend.base_url
    {
        global.base_url = Some(url.clone());
    }
    Options {
        global,
        sessions,
        update_snapshots: args.update_snapshots,
        bail: args.bail || args.debug,
        own_sessions: !explicit_session,
        trace: args.trace,
        debug: args.debug.then(|| std::sync::Mutex::new(None)),
        verify: args
            .verify_mocks
            .then(|| verify::Verifier::new(&plan.config)),
        record: args
            .record_mocks
            .then(|| std::sync::Mutex::new(std::collections::BTreeMap::new())),
        mock_server: std::sync::OnceLock::new(),
    }
}

/// Runs `work` between the backend's setup and teardown commands; a
/// failed teardown is a warning on stderr.
fn with_backend<T>(
    plan: &Plan,
    mode: Mode,
    work: impl FnOnce() -> Result<T, ErrorInfo>,
) -> Result<T, ErrorInfo> {
    backend::setup(plan, mode)?;
    let result = work();
    for warning in backend::teardown(plan, mode) {
        if mode != Mode::Json {
            output::stderr(&format!("warning: {warning}\n"));
        }
    }
    result
}

/// Shows a command from the project config on stderr before it runs, in
/// every output mode: the config may come from a parent directory or a
/// hidden `.chrome-cdp.toml`, so nothing it runs goes unseen.
pub(super) fn running(command: &str, config: &std::path::Path) {
    output::stderr(&format!("running: {command} (from {})\n", config.display()));
}

/// What the tests did: one run's results, or `--watch`'s last exit code
/// (its runs print their own results).
enum Ran {
    Once(Vec<Outcome>, std::time::Duration),
    Watched(u8),
}

/// `running N tests from M files, J at once`, after what a real
/// backend changes.
fn starting(plan: &Plan, mode: Mode) {
    if mode != Mode::Text {
        return;
    }
    if plan.real() {
        let backend = &plan.backend;
        let mut line = format!("backend {} (real", backend.name);
        if let Some(url) = &backend.base_url {
            let _ = write!(line, ", {url}");
        }
        line.push(')');
        if !backend.mocks.is_empty() {
            let _ = write!(line, " with mocks {}", backend.mocks.join(", "));
        }
        let time = if plan.determinism.freeze_time.is_some() {
            "time frozen"
        } else {
            "time runs"
        };
        let _ = writeln!(line, ": {time}, so determinism is best-effort");
        output::stdout(&line);
    }
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
        plan.jobs
            .min(u32::try_from(runs).unwrap_or(u32::MAX))
            .max(1)
    ));
}

/// Runs `plan`'s tests, printing each line as it ends.
fn execute(plan: &Plan, options: &Options, stop: &AtomicBool, mode: Mode) -> Vec<Outcome> {
    let print = |index: usize, outcome: &Outcome| {
        if mode == Mode::Text {
            output::stdout(&report::line(plan, &plan.tests[index], outcome));
        }
    };
    exec::execute(plan, options, stop, &print)
}

/// Writes the reporters' files and prints the run's result; returns the
/// exit code.
#[allow(clippy::too_many_arguments)]
fn conclude(
    plan: &Plan,
    args: &TestArgs,
    outcomes: &[Outcome],
    duration: std::time::Duration,
    kept: Option<Kept>,
    servers_started: bool,
    options: &Options,
    mode: Mode,
) -> u8 {
    let mut data = report::data(plan, outcomes, duration, args.shard.as_deref());
    data.verify = options
        .verify
        .as_ref()
        .map(|v| v.finish(&plan.backend.name, args.update));
    let mut record_warnings = Vec::new();
    if let Some(record) = &options.record {
        let (recorded, warnings) = record::write(plan, record);
        data.recorded = recorded;
        record_warnings = warnings;
        if mode == Mode::Text {
            output::stdout(&record::text(&data.recorded));
        }
    }
    data.debug = kept.map(|kept| protocol::schema::TestDebug {
        session: kept.session,
        context: kept.context,
        test: kept.place,
    });
    let written = reporters::write(&args.reporter, &data, &plan.config.artifacts);
    let mut unwritten = None;
    match written {
        Ok(written) => data.reporters = written.into_iter().collect(),
        Err(info) => unwritten = Some(info.message),
    }
    let summary = report::summary(&data, duration);
    let mut result = report::finish(plan, &data, duration, args.max_chars);
    if let (Err(info), Some(debug)) = (&mut result, &data.debug) {
        *info = report::debugging(info.clone(), debug, servers_started);
    }
    for warning in record_warnings {
        result = match result {
            Ok(out) => Ok(out.warning(warning)),
            Err(info) => Err(info.note("warning", warning)),
        };
    }
    if let Some(warning) = unwritten {
        result = match result {
            Ok(mut out) => {
                out.warnings.push(warning);
                Ok(out)
            }
            Err(info) => Err(info.note("warning", warning)),
        };
    }
    if let Some(verify) = &data.verify {
        if mode == Mode::Text {
            output::stdout(&verify::text(verify));
        }
        if let Some(drift) = verify::error(verify) {
            result = match result {
                Ok(_) => Err(drift.details(serde_json::json!(data))),
                Err(info) => Err(info.note("mock drift", drift.message)),
            };
        }
    }
    if mode == Mode::Text && result.is_err() {
        output::stdout(&format!("{summary}\n{}", report::reports(&data)));
    }
    output::print(mode, result)
}

/// The session `--debug` leaves a failed test's context open in.
const DEBUG_SESSION: &str = "test-debug";

/// Opens the sessions, runs `work`, and closes the sessions it opened
/// (but the one with the context `--debug` kept).
fn run_tests<T>(
    options: &Options,
    headed: bool,
    work: impl FnOnce() -> T,
) -> Result<(T, Option<Kept>), ErrorInfo> {
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
    let result = failed.map_or_else(|| Ok(work()), Err);
    let kept = options.debug.as_ref().and_then(|kept| {
        kept.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    });
    for session in &opened {
        if kept.as_ref().is_some_and(|k| &k.session == session) {
            continue;
        }
        close(&options.global, session, options.own_sessions);
    }
    result.map(|done| (done, kept))
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

/// The variable that gives web servers the mock server's URL.
const MOCK_URL_ENV: &str = "CHROME_CDP_MOCK_URL";

/// Starts the `mock_server` of `web_servers` (in mock mode), serving the
/// default mocks to requests without a test's header; returns its URL.
/// A run's second call (`--watch`) finds it running.
fn start_mock_server(
    plan: &Plan,
    web_servers: &[e2e::config::WebServer],
    started: &std::sync::OnceLock<e2e::mock_server::MockServer>,
    mode: Mode,
) -> Result<Option<String>, ErrorInfo> {
    if plan.real() {
        return Ok(None);
    }
    let Some(wanted) = web_servers.iter().find_map(|w| w.mock_server.as_ref()) else {
        return Ok(None);
    };
    if let Some(server) = started.get() {
        return Ok(Some(server.url()));
    }
    let config = &plan.config;
    let cwd = std::env::current_dir().unwrap_or_else(|_| config.root.clone());
    let sets = config
        .default_mocks
        .iter()
        .map(|name| mock::MockSet::load(&mock::Source::find(&config.mocks, &cwd, name)?))
        .collect::<Result<Vec<_>, _>>()?;
    let seed = plan.determinism.random_seed.unwrap_or_default();
    let addr = std::net::SocketAddr::new(wanted.host, wanted.port);
    let server = e2e::mock_server::MockServer::start(addr, sets, seed, None).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::LaunchFailed,
            format!("the mock_server cannot listen on {addr}: {e}"),
        )
        .hint(Hint::text(
            "stop what listens there (a `chrome-cdp mock serve`?), or give mock_server another port",
        ))
    })?;
    let url = server.url();
    if mode == Mode::Text {
        output::stdout(&format!("mock server {url}\n"));
    }
    let _ = started.set(server);
    Ok(Some(url))
}

/// Runs `work` with the config's web servers up (unless there are none),
/// and stops them after. A server that exits, or Ctrl-C, stops the run:
/// `stop` is set, so the tests not started are not run.
#[allow(clippy::too_many_arguments)]
fn with_servers<T: Send>(
    plan: &Plan,
    mock_server: &std::sync::OnceLock<e2e::mock_server::MockServer>,
    stop: &AtomicBool,
    mode: Mode,
    started: &AtomicBool,
    watch: bool,
    work: impl FnOnce() -> T + Send,
) -> Result<T, ErrorInfo> {
    let config = &plan.config;
    let web_servers = &plan.backend.web_servers;
    // --watch runs until Ctrl-C, which stops it cleanly.
    if web_servers.is_empty() && !watch {
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
    let mut web_servers = web_servers.clone();
    if let Some(url) = start_mock_server(plan, &web_servers, mock_server, mode)? {
        for server in &mut web_servers {
            server
                .env
                .entry(MOCK_URL_ENV.to_owned())
                .or_insert_with(|| url.clone());
        }
    }
    let mut servers = runtime.block_on(WebServers::start_with(
        &web_servers,
        &config.artifacts,
        |server| running(&server.command, &config.file),
    ))?;
    started.store(
        servers.servers().iter().any(|s| !s.reused),
        Ordering::SeqCst,
    );
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
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        scope.spawn(move || {
            let _ = tx.send(work());
        });
        let interrupt = async {
            let _ = tokio::signal::ctrl_c().await;
        };
        let ran = runtime.block_on(servers.run(&mut rx, interrupt));
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
            Err(Stopped::Interrupted) if watch => {
                // The end of watching: the run in progress finishes.
                stop.store(true, Ordering::SeqCst);
                runtime.block_on(rx).map_err(|_| {
                    ErrorInfo::new(ErrorKind::BadArgs, "the test run stopped unexpectedly")
                })
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
