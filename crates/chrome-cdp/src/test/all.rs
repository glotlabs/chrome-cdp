//! `test --backend all` (plan 11, milestone 11.11): the suite once per
//! backend, `mock` first, each with its own web servers, setup and
//! teardown, and artifacts below `<artifacts>/<backend>/` (the mock
//! run's where a plain run puts them). The text output has a heading per
//! backend; the result groups the tests by backend and fails if any run
//! did.

use std::fmt::Write as _;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use protocol::schema::{TestBackendRun, TestRunData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Mode, Output, format};
use serde_json::json;

use super::plan::Plan;
use super::{
    execute, options, output, ready, report, reporters, run_tests, starting, with_backend,
    with_servers,
};
use crate::cli::{GlobalArgs, TestArgs};

/// How many failed tests the closing error names.
const LISTED: usize = 10;

pub(super) fn run(args: &TestArgs, global: &GlobalArgs, explicit_session: bool) -> u8 {
    let mode = global.mode();
    match run_all(args, global, explicit_session, mode) {
        Ok(result) => output::print(mode, result),
        Err(info) => output::print(mode, Err(info)),
    }
}

fn run_all(
    args: &TestArgs,
    global: &GlobalArgs,
    explicit_session: bool,
    mode: Mode,
) -> Result<Result<Output, ErrorInfo>, ErrorInfo> {
    if args.watch || args.debug || args.verify_mocks || args.record_mocks {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            "--backend all runs each backend once; --watch, --debug, --verify-mocks and --record-mocks take one backend",
        )
        .hint(Hint::run(
            Cmd::new("test").flag("backend", "<name>"),
        )));
    }
    let first = Plan::make(args, global, Some(e2e::config::MOCK_BACKEND))?;
    let names = first.config.backend_names();
    let artifacts = first.config.artifacts.clone();
    let started = Instant::now();
    let mut runs: Vec<(Plan, TestRunData)> = Vec::new();
    let mut plan = Some(first);
    for name in &names {
        let plan = match plan.take() {
            Some(plan) => plan,
            None => Plan::make(args, global, Some(name))?,
        };
        if mode == Mode::Text && !args.list {
            output::stdout(&format!("== backend {name} ==\n"));
        }
        let data = if args.list {
            let listed = report::listed(&plan, args.shard.as_deref(), 0);
            serde_json::from_value(listed.data)
                .map_err(|e| ErrorInfo::new(ErrorKind::ProtocolError, format!("the list: {e}")))?
        } else {
            once(&plan, args, global, explicit_session, mode)
                .map_err(|info| info.note("backend", name.clone()))?
        };
        if mode == Mode::Text && !args.list {
            output::stdout(&format!(
                "{name}: {}\n",
                report::summary(&data, Duration::from_millis(data.duration_ms))
            ));
        }
        runs.push((plan, data));
    }
    let mut data = combine(&runs, started.elapsed(), args);
    if args.list {
        let text = listed_text(&runs, &data);
        return Ok(Ok(
            Output::typed(&data, text).limit(args.max_chars, "--max-chars 0 for all of it")
        ));
    }
    let mut warning = None;
    match reporters::write(&args.reporter, &data, &artifacts) {
        Ok(written) => data.reporters = written.into_iter().collect(),
        Err(info) => warning = Some(info.message),
    }
    let result = finish(&runs, &data, started.elapsed(), args.max_chars);
    if mode == Mode::Text && result.is_err() {
        output::stdout(&report::reports(&data));
    }
    Ok(match (result, warning) {
        (Ok(out), Some(w)) => Ok(out.warning(w)),
        (Err(info), Some(w)) => Err(info.note("warning", w)),
        (result, None) => result,
    })
}

/// One backend's run.
fn once(
    plan: &Plan,
    args: &TestArgs,
    global: &GlobalArgs,
    explicit_session: bool,
    mode: Mode,
) -> Result<TestRunData, ErrorInfo> {
    ready(plan, args, mode)?;
    let options = options(plan, args, global, explicit_session);
    let stop = AtomicBool::new(false);
    let servers = AtomicBool::new(false);
    let started = Instant::now();
    let ran = with_servers(plan, &stop, mode, &servers, false, || {
        with_backend(plan, mode, || {
            run_tests(&options, args.headed, || {
                starting(plan, mode);
                execute(plan, &options, &stop, mode)
            })
        })
    });
    let (outcomes, _) = ran.and_then(|r| r)?;
    let mut data = report::data(plan, &outcomes, started.elapsed(), args.shard.as_deref());
    for test in &mut data.tests {
        test.backend = Some(plan.backend.name.clone());
    }
    Ok(data)
}

/// The runs as one result: every test (with its backend), the counts
/// added up, and each backend's.
fn combine(runs: &[(Plan, TestRunData)], duration: Duration, args: &TestArgs) -> TestRunData {
    let sum = |f: fn(&TestRunData) -> usize| runs.iter().map(|(_, d)| f(d)).sum();
    TestRunData {
        tests: runs
            .iter()
            .flat_map(|(plan, d)| {
                d.tests.iter().cloned().map(|mut t| {
                    t.backend = Some(plan.backend.name.clone());
                    t
                })
            })
            .collect(),
        passed: sum(|d| d.passed),
        failed: sum(|d| d.failed),
        flaky: sum(|d| d.flaky),
        skipped: sum(|d| d.skipped),
        not_run: sum(|d| d.not_run),
        duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
        shard: args.shard.clone(),
        listed: args.list,
        repeat: args.repeat.filter(|&r| r > 1),
        reporters: std::collections::BTreeMap::new(),
        debug: None,
        backend: Some(e2e::backend::ALL.to_owned()),
        backends: runs
            .iter()
            .map(|(plan, d)| TestBackendRun {
                name: plan.backend.name.clone(),
                mode: plan.backend.mode.as_str().to_owned(),
                passed: d.passed,
                failed: d.failed,
                flaky: d.flaky,
                skipped: d.skipped,
                not_run: d.not_run,
                duration_ms: d.duration_ms,
            })
            .collect(),
        verify: None,
        recorded: Vec::new(),
    }
}

/// `--list`: each backend's tests under its name.
fn listed_text(runs: &[(Plan, TestRunData)], data: &TestRunData) -> String {
    let mut text = String::new();
    for (plan, d) in runs {
        let _ = writeln!(text, "== backend {} ==", plan.backend.name);
        for t in &d.tests {
            let mut line = format!("{}:{} {}", t.file, t.line, t.name);
            if let Some(reason) = &t.reason {
                let _ = write!(line, " (skipped: {reason})");
            }
            text.push_str(&line);
            text.push('\n');
        }
    }
    let _ = write!(
        text,
        "{} test runs on {} backends ({} skipped)",
        data.tests.len(),
        runs.len(),
        data.skipped
    );
    text
}

/// The result: the combined data, or an error naming the failed tests.
fn finish(
    runs: &[(Plan, TestRunData)],
    data: &TestRunData,
    duration: Duration,
    max_chars: usize,
) -> Result<Output, ErrorInfo> {
    let per_backend: Vec<String> = runs
        .iter()
        .map(|(plan, d)| {
            let mut parts = vec![format!("{} passed", d.passed)];
            if d.failed > 0 {
                parts.push(format!("{} failed", d.failed));
            }
            if d.skipped > 0 {
                parts.push(format!("{} skipped", d.skipped));
            }
            format!("{} {}", plan.backend.name, parts.join(", "))
        })
        .collect();
    let summary = format!(
        "{} on {} backends: {} ({})",
        report::summary(data, duration),
        runs.len(),
        per_backend.join("; "),
        format::duration(duration)
    );
    let failed: Vec<(String, String)> = data
        .tests
        .iter()
        .filter(|t| t.status == "failed")
        .map(|t| {
            (
                t.backend.clone().unwrap_or_default(),
                format!("{}:{}", t.file, t.line),
            )
        })
        .collect();
    let Some((backend, place)) = failed.first() else {
        let quiet = format!("{} passed", data.passed + data.flaky);
        return Ok(Output::typed(data, summary)
            .quiet(quiet)
            .limit(max_chars, "--max-chars 0 for all of it"));
    };
    let mut info = ErrorInfo::new(
        ErrorKind::ExpectFailed,
        format!("{} of {} test runs failed", failed.len(), data.tests.len()),
    );
    for (b, p) in failed.iter().take(LISTED) {
        let name = data
            .tests
            .iter()
            .find(|t| t.backend.as_deref() == Some(b) && format!("{}:{}", t.file, t.line) == *p)
            .map(|t| t.name.clone())
            .unwrap_or_default();
        info = info.note("failed", format!("{p} {name} (backend {b})"));
    }
    if failed.len() > LISTED {
        info = info.note(
            "failed",
            format!("{} more (see --json)", failed.len() - LISTED),
        );
    }
    Err(info
        .note("backends", per_backend.join("; "))
        .hint(
            Hint::run(Cmd::new("test").arg(place).flag("backend", backend))
                .why("runs the first failed test again"),
        )
        .details(json!(data)))
}
