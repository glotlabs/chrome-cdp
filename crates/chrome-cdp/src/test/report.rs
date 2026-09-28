//! What `chrome-cdp test` prints (plan 11, milestone 11.4): a line per
//! test as it ends, a failure's step and error under its line, and a
//! closing summary; `--json` gives the same as one envelope.

use std::fmt::Write as _;
use std::time::Duration;

use protocol::schema::{TestFailure, TestResult, TestRunData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Mode, Output, Style, format};
use serde_json::json;

use super::exec::{Outcome, Status};
use super::plan::{Plan, Planned};

/// How many failed tests the closing error names.
const LISTED: usize = 10;

/// `e2e/scenarios/checkout.cdp:12`.
fn place(plan: &Plan, test: &Planned) -> String {
    format!("{}:{}", plan.scenarios[test.file].name, test.line)
}

/// The text of one test's result, as it ends.
pub(super) fn line(plan: &Plan, test: &Planned, outcome: &Outcome) -> String {
    let place = place(plan, test);
    let mut took = format::duration(outcome.duration);
    if plan.repeat > 1 && matches!(outcome.status, Status::Passed | Status::Failed) {
        took = match outcome.failed_runs.unwrap_or(0) {
            0 => format!("{} runs, slowest {took}", plan.repeat),
            failed => format!("failed {failed} of {} runs, slowest {took}", plan.repeat),
        };
    }
    let mut text = match outcome.status {
        Status::Passed => format!("✓ {place} {} ({took})\n", test.name),
        Status::Flaky => format!(
            "~ {place} {} ({took}; flaky: passed on try {})\n",
            test.name, outcome.tries
        ),
        Status::Failed => format!("✗ {place} {} ({took})\n", test.name),
        Status::Skipped => format!(
            "- {place} {} (skipped{})\n",
            test.name,
            outcome
                .reason
                .as_deref()
                .filter(|r| !r.is_empty())
                .map(|r| format!(": {r}"))
                .unwrap_or_default()
        ),
        Status::NotRun => format!("- {place} {} (not run)\n", test.name),
    };
    if let Some(failure) = &outcome.failure {
        let label = if outcome.status == Status::Flaky {
            "first try failed"
        } else if plan.repeat > 1 {
            "first failure"
        } else {
            "failed"
        };
        text.push_str(&failure_text(failure, label));
    }
    text
}

/// A failure under its test's line, indented.
fn failure_text(failure: &TestFailure, label: &str) -> String {
    let mut text = String::new();
    let hook = match failure.hook.as_deref() {
        Some("setup") => " (setting up the test)".to_owned(),
        Some("check") => " (checked after the test's steps)".to_owned(),
        Some(hook) => format!(" (in @{hook})"),
        None => String::new(),
    };
    let _ = writeln!(
        text,
        "    {label} at {}: {}{hook}",
        failure.at, failure.step
    );
    for via in &failure.via {
        let _ = writeln!(text, "      included by {via}");
    }
    let rendered = protocol::render(Mode::Text, Err(failure.error.clone()), Style::new(false));
    for line in rendered.stderr.lines() {
        let _ = writeln!(text, "    {line}");
    }
    text
}

/// What `--json` and the error's details say.
pub(super) fn data(
    plan: &Plan,
    outcomes: &[Outcome],
    duration: Duration,
    shard: Option<&str>,
) -> TestRunData {
    let count = |status: Status| outcomes.iter().filter(|o| o.status == status).count();
    TestRunData {
        tests: plan
            .tests
            .iter()
            .zip(outcomes)
            .map(|(test, outcome)| result(plan, test, outcome))
            .collect(),
        passed: count(Status::Passed),
        failed: count(Status::Failed),
        flaky: count(Status::Flaky),
        skipped: count(Status::Skipped),
        not_run: count(Status::NotRun),
        duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
        shard: shard.map(str::to_owned),
        listed: false,
        repeat: (plan.repeat > 1).then_some(plan.repeat),
    }
}

fn result(plan: &Plan, test: &Planned, outcome: &Outcome) -> TestResult {
    TestResult {
        file: plan.scenarios[test.file].name.clone(),
        line: test.line,
        name: test.name.clone(),
        status: match outcome.status {
            Status::Passed => "passed",
            Status::Failed => "failed",
            Status::Flaky => "flaky",
            Status::Skipped => "skipped",
            Status::NotRun => "not-run",
        }
        .to_owned(),
        tags: test.settings.tags.clone(),
        duration_ms: u64::try_from(outcome.duration.as_millis()).unwrap_or(u64::MAX),
        tries: outcome.tries,
        reason: outcome.reason.clone(),
        failed_runs: outcome.failed_runs,
        error: outcome.failure.as_deref().cloned(),
    }
}

/// `7 tests: 3 passed, 1 failed, 1 flaky, 1 skipped, 1 not run (12.3s)`.
pub(super) fn summary(data: &TestRunData, duration: Duration) -> String {
    let mut parts = vec![format!("{} passed", data.passed)];
    for (n, what) in [
        (data.failed, "failed"),
        (data.flaky, "flaky"),
        (data.skipped, "skipped"),
        (data.not_run, "not run"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {what}"));
        }
    }
    format!(
        "{} test{}: {} ({})",
        data.tests.len(),
        if data.tests.len() == 1 { "" } else { "s" },
        parts.join(", "),
        format::duration(duration)
    )
}

/// The run's result: an [`Output`] when no test failed, else an error
/// naming the failed tests, with the data in its details.
pub(super) fn finish(
    plan: &Plan,
    data: &TestRunData,
    duration: Duration,
    max_chars: usize,
) -> Result<Output, ErrorInfo> {
    let summary = summary(data, duration);
    let failed: Vec<(&Planned, &TestResult)> = plan
        .tests
        .iter()
        .zip(&data.tests)
        .filter(|(_, r)| r.status == "failed")
        .collect();
    if failed.is_empty() {
        let quiet = format!("{} passed", data.passed + data.flaky);
        return Ok(Output::typed(data, summary)
            .quiet(quiet)
            .limit(max_chars, "--max-chars 0 for all of it"));
    }
    let (first, _) = failed[0];
    let mut info = ErrorInfo::new(
        ErrorKind::ExpectFailed,
        format!("{} of {} tests failed", failed.len(), data.tests.len()),
    );
    for (test, _) in failed.iter().take(LISTED) {
        info = info.note("failed", format!("{} {}", place(plan, test), test.name));
    }
    if failed.len() > LISTED {
        info = info.note(
            "failed",
            format!("{} more (see --json)", failed.len() - LISTED),
        );
    }
    let mut again = Cmd::new("test").arg(place(plan, first));
    if plan.repeat > 1 {
        again = again.flag("repeat", plan.repeat.to_string());
    }
    Err(info
        .hint(Hint::run(again).why("runs the first failed test again"))
        .details(json!(data)))
}

/// `--list`: the tests the run would have.
pub(super) fn listed(plan: &Plan, shard: Option<&str>, max_chars: usize) -> Output {
    let mut text = String::new();
    let mut tests = Vec::new();
    for test in &plan.tests {
        let mut line = format!("{} {}", place(plan, test), test.name);
        if !test.settings.tags.is_empty() {
            let _ = write!(line, " [{}]", test.settings.tags.join(" "));
        }
        if let Some(reason) = test.skipped() {
            line.push_str(" (skipped");
            if !reason.is_empty() {
                let _ = write!(line, ": {reason}");
            }
            line.push(')');
        }
        let _ = writeln!(text, "{line}");
        tests.push(TestResult {
            file: plan.scenarios[test.file].name.clone(),
            line: test.line,
            name: test.name.clone(),
            status: if test.skipped().is_some() {
                "skipped"
            } else {
                "listed"
            }
            .to_owned(),
            tags: test.settings.tags.clone(),
            duration_ms: 0,
            tries: 0,
            reason: test.skipped().map(str::to_owned),
            failed_runs: None,
            error: None,
        });
    }
    let skipped = tests.iter().filter(|t| t.status == "skipped").count();
    let _ = write!(
        text,
        "{} test{}{}",
        tests.len(),
        if tests.len() == 1 { "" } else { "s" },
        if skipped > 0 {
            format!(" ({skipped} skipped)")
        } else {
            String::new()
        }
    );
    let quiet: Vec<String> = plan.tests.iter().map(|t| place(plan, t)).collect();
    let data = TestRunData {
        tests,
        passed: 0,
        failed: 0,
        flaky: 0,
        skipped,
        not_run: 0,
        duration_ms: 0,
        shard: shard.map(str::to_owned),
        listed: true,
        repeat: None,
    };
    Output::typed(&data, text)
        .quiet(quiet.join("\n"))
        .limit(max_chars, "--max-chars 0 for all of it")
}
