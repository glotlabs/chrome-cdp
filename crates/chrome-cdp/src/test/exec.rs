//! Running the tests of a [`Plan`] (plan 11, milestone 11.4).
//!
//! `jobs` worker threads take the tests in order. Each test runs in a
//! fresh browser context of the worker's session (`context new`), which
//! gets the test's mock sets, its viewport and the settings that make it
//! behave the same in every run ([`super::determinism`], 11.6) before its
//! `@before` steps, its own steps and its `@after` steps run; the context
//! is closed after.
//! Every step goes through the same code as a CLI command
//! ([`crate::run`]), with `--session` and `--context` set. A failed test
//! runs again up to its retries; one that then passes is flaky.
//!
//! A file's `@before-all` runs once, before its first test (a worker that
//! needs it meanwhile waits), in a context of its own; its `@after-all`
//! runs after its last test. A failed `@before-all` fails every test of
//! the file.
//!
//! After each step the runner reads the context's mock log (milestone
//! 11.5): a request no mock answered, in the scope of a set whose
//! `unmatched = "fail"`, fails the test at that step, naming the mocks
//! that came closest. A step that failed on its own gets them as notes,
//! since a 501 is the likely cause. After a test's own steps pass,
//! `expect --no-errors --all-tabs` runs unless `allow_console_errors`
//! (config) or `@allow-console-errors` (test) allows them.
//!
//! Against a real backend (milestone 11.11) a test's context gets only
//! the backend's own mock sets (hybrid mode), if any. Each step's
//! variables are filled in as it runs ([`super::vars`]): `${test.id}` is
//! the try's, and `${saved.NAME…}` reads what an earlier `http … --save
//! NAME` of the test (or of the file's `@before-all`) got. Mock-only
//! commands fail in real mode, and on a protected backend `http` writes
//! fail outside hooks unless the run has `--allow-writes`.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use protocol::schema::{MockLogData, RoutedRequest, TestFailure};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use serde_json::Value;

use e2e::scenario::Settings;

use super::artifacts::{self, Capture, Logs, Recorder, StepRecord};
use super::determinism;
use super::plan::{Plan, Planned, Prepared};
use super::report_md::{self, Subject};
use super::vars::{self, Resolver};
use super::verify::Seen;
use crate::cli::{Command, GlobalArgs, WAIT_TIMEOUT};
use crate::script;

/// How a test ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Status {
    Passed,
    Failed,
    /// Failed, then passed on a retry.
    Flaky,
    Skipped,
    NotRun,
}

/// A test's result.
#[derive(Debug, Clone)]
pub(super) struct Outcome {
    pub(super) status: Status,
    /// Its last try's.
    pub(super) duration: Duration,
    pub(super) tries: u32,
    /// The last failure (a flaky test's first).
    pub(super) failure: Option<Box<TestFailure>>,
    /// Why it was skipped or not run.
    pub(super) reason: Option<String>,
    /// `--repeat`: how many of its runs failed.
    pub(super) failed_runs: Option<u32>,
    /// What its steps warned about that the report shows (a screenshot
    /// baseline of another OS only).
    pub(super) warnings: Vec<String>,
    /// Its artifacts (its last try's; with `--repeat`, the first failed
    /// run's, else the first run's).
    pub(super) artifacts: Option<PathBuf>,
}

/// How a run's steps are made.
pub(super) struct Options {
    /// The `test` command's flags, which steps inherit.
    pub(super) global: GlobalArgs,
    /// The browser sessions: one, or one per worker
    /// (`--jobs-isolation browser`).
    pub(super) sessions: Vec<String>,
    pub(super) update_snapshots: bool,
    pub(super) bail: bool,
    /// Whether the run named the sessions itself (and removes them).
    pub(super) own_sessions: bool,
    /// `--trace`: a screenshot after every step.
    pub(super) trace: bool,
    /// `--debug`: the first failed test's context is left open, here.
    pub(super) debug: Option<Mutex<Option<Kept>>>,
    /// `--verify-mocks`: what each test's real exchanges showed.
    pub(super) verify: Option<super::verify::Verifier>,
    /// `--record-mocks`: each passed test's (by index) exchanges, and the
    /// scope they were recorded in.
    pub(super) record: Option<Mutex<super::record::Recorded>>,
    /// The `mock_server` of the web servers (plan 11.9), once started:
    /// each test gets a partition of its own, with its own sets.
    pub(super) mock_server: std::sync::OnceLock<e2e::mock_server::MockServer>,
}

/// The browser context `--debug` kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Kept {
    pub(super) session: String,
    pub(super) context: String,
    /// The test's `file:line` (a hook's file's `file:1`).
    pub(super) place: String,
}

/// Where a file's `@before-all` is.
#[derive(Debug, Clone)]
enum Once {
    NotRun,
    /// With what its `http --save` steps saved.
    Passed(BTreeMap<String, Value>),
    Failed(Box<TestFailure>),
}

/// Runs `plan`'s tests, each `plan.repeat` times; `report` gets each
/// test's result in the plan's order as soon as it and those before it
/// are known. `stop` ends the run early (the tests not started are not
/// run).
pub(super) fn execute(
    plan: &Plan,
    options: &Options,
    stop: &AtomicBool,
    report: &(dyn Fn(usize, &Outcome) + Sync),
) -> Vec<Outcome> {
    let count = plan.tests.len();
    let repeat = usize::try_from(plan.repeat).unwrap_or(1).max(1);
    // A run of a test is an item; the runs of a test are next to each
    // other, so they spread over the workers.
    let items = count * repeat;
    let next = AtomicUsize::new(0);
    let results: Mutex<(Vec<Option<Outcome>>, usize)> = Mutex::new((vec![None; items], 0));
    let before_all: Vec<Mutex<Once>> = plan
        .scenarios
        .iter()
        .map(|_| Mutex::new(Once::NotRun))
        .collect();
    // Runs of each file still to go, for its @after-all.
    let left: Vec<AtomicUsize> = plan
        .scenarios
        .iter()
        .enumerate()
        .map(|(f, _)| {
            let tests = plan
                .tests
                .iter()
                .filter(|t| t.file == f && t.skipped().is_none())
                .count();
            AtomicUsize::new(tests * repeat)
        })
        .collect();
    // What an earlier run left is gone, also for the tests that do not
    // run now.
    for test in &plan.tests {
        if let Err(e) = std::fs::remove_dir_all(&test.dir)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("cannot remove {}: {e}", test.dir.display());
        }
    }
    let workers = usize::try_from(plan.jobs).unwrap_or(1).min(items.max(1));
    std::thread::scope(|scope| {
        for worker in 0..workers {
            let session = &options.sessions[worker % options.sessions.len()];
            let (next, results, before_all, left) = (&next, &results, &before_all, &left);
            scope.spawn(move || {
                loop {
                    let item = next.fetch_add(1, Ordering::SeqCst);
                    if item >= items {
                        break;
                    }
                    let test = &plan.tests[item / repeat];
                    let mut outcome = if stop.load(Ordering::SeqCst) {
                        not_run(options)
                    } else if let Some(reason) = test.skipped() {
                        Outcome {
                            status: Status::Skipped,
                            duration: Duration::ZERO,
                            tries: 0,
                            failure: None,
                            reason: Some(reason.to_owned()),
                            failed_runs: None,
                            warnings: Vec::new(),
                            artifacts: None,
                        }
                    } else {
                        let runner = Runner {
                            plan,
                            options,
                            session,
                            seen: Cell::new(0),
                            seen_served: Cell::new(0),
                            warnings: RefCell::new(Vec::new()),
                            recorder: RefCell::new(None),
                            test_id: RefCell::new(String::new()),
                            saved: RefCell::new(BTreeMap::new()),
                            file_saved: RefCell::new(BTreeMap::new()),
                            http_log: RefCell::new(Vec::new()),
                            reads: RefCell::new(Vec::new()),
                        };
                        let index = item / repeat;
                        let outcome = match runner.before_all(test, &before_all[test.file]) {
                            Err(failure) => Outcome {
                                status: Status::Failed,
                                duration: Duration::ZERO,
                                tries: 0,
                                failure: Some(failure),
                                reason: None,
                                failed_runs: None,
                                warnings: Vec::new(),
                                artifacts: None,
                            },
                            Ok(()) => {
                                runner.test(test, index, (repeat > 1).then_some(item % repeat + 1))
                            }
                        };
                        let last = left[test.file].fetch_sub(1, Ordering::SeqCst) == 1;
                        let mut outcome = if last {
                            runner.after_all(test, outcome)
                        } else {
                            outcome
                        };
                        outcome.warnings = runner.warnings.take();
                        outcome
                    };
                    if outcome.status == Status::Failed && options.bail {
                        stop.store(true, Ordering::SeqCst);
                    }
                    if outcome.status == Status::NotRun && test.skipped().is_some() {
                        outcome.status = Status::Skipped;
                        outcome.reason = test.skipped().map(str::to_owned);
                    }
                    let mut guard = results.lock().unwrap_or_else(PoisonError::into_inner);
                    let (done, printed) = &mut *guard;
                    done[item] = Some(outcome);
                    while *printed < count {
                        let runs = &done[*printed * repeat..(*printed + 1) * repeat];
                        let Some(runs) = runs.iter().cloned().collect::<Option<Vec<_>>>() else {
                            break;
                        };
                        report(*printed, &merge(runs, repeat));
                        *printed += 1;
                    }
                }
            });
        }
    });
    let (done, _) = results.into_inner().unwrap_or_else(PoisonError::into_inner);
    done.chunks(repeat)
        .map(|runs| {
            let runs = runs
                .iter()
                .map(|o| o.clone().unwrap_or_else(|| not_run(options)))
                .collect();
            merge(runs, repeat)
        })
        .collect()
}

/// A test's result from its runs (one without `--repeat`): failed if one
/// failed, then not run if one was not, then skipped if all were; the
/// slowest run's duration, and the first failure.
fn merge(mut runs: Vec<Outcome>, repeat: usize) -> Outcome {
    if repeat == 1 && runs.len() == 1 {
        return runs.remove(0);
    }
    let failed = runs.iter().filter(|o| o.status == Status::Failed).count();
    let any = |status: Status| runs.iter().any(|o| o.status == status);
    let status = if failed > 0 {
        Status::Failed
    } else if any(Status::NotRun) {
        Status::NotRun
    } else if runs.iter().all(|o| o.status == Status::Skipped) {
        Status::Skipped
    } else if any(Status::Flaky) {
        Status::Flaky
    } else {
        Status::Passed
    };
    let artifacts = runs
        .iter()
        .find(|o| o.status == Status::Failed)
        .or(runs.first())
        .and_then(|o| o.artifacts.clone());
    Outcome {
        status,
        artifacts,
        duration: runs.iter().map(|o| o.duration).max().unwrap_or_default(),
        tries: runs.iter().map(|o| o.tries).sum(),
        failure: runs.iter().find_map(|o| o.failure.clone()),
        reason: runs
            .iter()
            .find(|o| o.status == status)
            .and_then(|o| o.reason.clone()),
        failed_runs: Some(u32::try_from(failed).unwrap_or(u32::MAX)),
        warnings: {
            let mut warnings: Vec<String> = Vec::new();
            for warning in runs.iter().flat_map(|o| &o.warnings) {
                if !warnings.contains(warning) {
                    warnings.push(warning.clone());
                }
            }
            warnings
        },
    }
}

fn not_run(options: &Options) -> Outcome {
    Outcome {
        status: Status::NotRun,
        duration: Duration::ZERO,
        tries: 0,
        failure: None,
        reason: Some(if options.bail {
            "--bail stopped the run after a failed test".into()
        } else {
            "the run was stopped".into()
        }),
        failed_runs: None,
        warnings: Vec::new(),
        artifacts: None,
    }
}

/// Runs steps in one session.
struct Runner<'a> {
    plan: &'a Plan,
    options: &'a Options,
    session: &'a str,
    /// The last mock log entry looked at for unmatched requests.
    seen: Cell<u64>,
    /// The same, of the mock server's journal.
    seen_served: Cell<u64>,
    /// The warnings of steps the report shows.
    warnings: RefCell<Vec<String>>,
    /// The steps of the test (or hook) running now, and its artifacts.
    recorder: RefCell<Option<Recorder>>,
    /// `${test.id}` of the try running now.
    test_id: RefCell<String>,
    /// What the try's `http --save` steps saved.
    saved: RefCell<BTreeMap<String, Value>>,
    /// What the file's `@before-all` saved: every test starts with it.
    file_saved: RefCell<BTreeMap<String, Value>>,
    /// `--verify-mocks`: the try's `http` exchanges.
    http_log: RefCell<Vec<Seen>>,
    /// Where a `state load` of the try looks for files earlier steps
    /// wrote, after its own artifacts folder ([`super::outputs`]).
    reads: RefCell<Vec<PathBuf>>,
}

/// A step's place in a test, for its failure.
#[derive(Clone, Copy)]
enum Part {
    Hook(&'static str),
    Test,
}

impl Runner<'_> {
    /// `test` (the plan's test `index`)'s tries until it passes or its
    /// retries are used up; `run` is its run's number with `--repeat`.
    fn test(&self, test: &Planned, index: usize, run: Option<usize>) -> Outcome {
        let tries = self.plan.retries(test) + 1;
        let mut first_failure = None;
        for attempt in 1..=tries {
            let started = Instant::now();
            *self.test_id.borrow_mut() = vars::test_id(&self.plan.run, index, attempt, run);
            *self.saved.borrow_mut() = self.file_saved.borrow().clone();
            // The first try's artifacts are the test's; a retry's go below.
            let dir = match run {
                Some(run) => test.dir.join(format!("run-{run}")),
                None if attempt == 1 => test.dir.clone(),
                None => test.dir.join(format!("try-{attempt}")),
            };
            let label = match run {
                Some(run) => Some(format!("run {run} of {}", self.plan.repeat)),
                None if tries > 1 => Some(format!("try {attempt} of {tries}")),
                None => None,
            };
            let result = self.attempt(test, index, &dir, label);
            let duration = started.elapsed();
            match result {
                Ok(()) => {
                    let status = if attempt == 1 {
                        Status::Passed
                    } else {
                        Status::Flaky
                    };
                    return Outcome {
                        status,
                        duration,
                        tries: attempt,
                        failure: first_failure,
                        reason: None,
                        failed_runs: None,
                        warnings: Vec::new(),
                        artifacts: Some(dir),
                    };
                }
                Err(failure) if attempt == tries => {
                    return Outcome {
                        status: Status::Failed,
                        duration,
                        tries: attempt,
                        failure: Some(failure),
                        reason: None,
                        failed_runs: None,
                        warnings: Vec::new(),
                        artifacts: Some(dir),
                    };
                }
                Err(failure) => {
                    first_failure.get_or_insert(failure);
                }
            }
        }
        unreachable!("a test runs at least once")
    }

    /// One try of `test` in a fresh context, its artifacts in `dir`;
    /// `label` says which try or run it is, when there are several.
    fn attempt(
        &self,
        test: &Planned,
        index: usize,
        dir: &Path,
        label: Option<String>,
    ) -> Result<(), Box<TestFailure>> {
        let deadline = Instant::now() + self.plan.timeout(&test.settings);
        let hooks = &self.plan.hooks[test.file];
        let span = self.test_span(test);
        let subject = Subject {
            title: &test.name,
            place: &span,
            attempt: label,
            rerun: Cmd::new("test").arg(&span).to_string(),
        };
        self.http_log.borrow_mut().clear();
        // A failed try's `@after` may have left a file for the next, and
        // `@before-all` hands state to the tests through files.
        let file_dir = super::plan::file_dir(&self.plan.config, &self.plan.scenarios[test.file]);
        *self.reads.borrow_mut() = vec![test.dir.clone(), file_dir.join("@before-all")];
        let verify = self
            .options
            .verify
            .is_some()
            .then(|| self.plan.mock_mode_sets(&test.settings));
        self.in_context(
            dir,
            &subject,
            &test.settings,
            deadline,
            Some(&hooks.after),
            verify.as_deref(),
            |context| {
                self.steps(&hooks.before, context, deadline, Part::Hook("before"))?;
                self.steps(&test.steps, context, deadline, Part::Test)?;
                self.no_errors(test, context, &span)?;
                self.record_mocks(test, index, context);
                Ok(())
            },
        )
    }

    /// Runs `body` in a fresh context set up with `settings`, then
    /// `after` (`@after`, which runs whatever happened, while there is
    /// time), recording the steps and artifacts in `dir`. A failure gets
    /// the page as it was (before `@after`) and a `report.md`. With
    /// `--debug`, the first failure's context stays open, without its
    /// `@after`.
    #[allow(clippy::too_many_arguments)]
    fn in_context(
        &self,
        dir: &Path,
        subject: &Subject<'_>,
        settings: &Settings,
        deadline: Instant,
        after: Option<&[Prepared]>,
        verify: Option<&[String]>,
        body: impl FnOnce(&str) -> Result<(), Box<TestFailure>>,
    ) -> Result<(), Box<TestFailure>> {
        *self.recorder.borrow_mut() = Some(Recorder::new(dir.to_owned(), self.options.trace));
        let mut capture = None;
        let mut logs = Logs::default();
        let mut kept = false;
        let result = match self.new_context(subject.place) {
            Err(failure) => Err(failure),
            Ok(context) => {
                let mut result = self
                    .settle(&context, settings, subject.place, deadline)
                    .and_then(|()| body(&context));
                if result.is_err() {
                    capture = Some(self.capture(dir, &context));
                }
                kept = result.is_err() && self.keep(&context, subject.place);
                if !kept && let Some(after) = after {
                    let ran =
                        self.steps(after, &context, deadline + AFTER_GRACE, Part::Hook("after"));
                    if result.is_ok() && ran.is_err() {
                        capture = Some(self.capture(dir, &context));
                    }
                    result = result.and(ran);
                }
                if let Some(sets) = verify {
                    self.verify(&context, sets);
                }
                let id = self.test_id.borrow().clone();
                let served = self
                    .served()
                    .map(|server| (server.url(), server.journal(Some(&id))));
                logs = artifacts::logs(dir, &context, served, &|words| {
                    self.invoke(words, Some(&context), None)
                });
                if let Some(server) = self.served() {
                    server.close_partition(&id);
                }
                if !kept {
                    self.close_context(&context);
                }
                result
            }
        };
        let recorder = self.recorder.borrow_mut().take();
        result.map_err(|mut failure| {
            if !kept {
                artifacts::point_hint_at_files(&mut failure.error, dir);
            }
            let steps = recorder.as_ref().map(Recorder::steps).unwrap_or_default();
            let mut text =
                report_md::render(subject, &failure, steps, capture.as_ref(), &logs, dir);
            if let Some(kept) = kept.then(|| self.kept_context()).flatten() {
                let _ = writeln!(
                    text,
                    "\nThe test's browser context is still open (--debug): `{}`",
                    Cmd::new("snapshot")
                        .session(&kept.session)
                        .flag("context", &kept.context)
                );
            }
            let path = dir.join(artifacts::REPORT);
            artifacts::write(&path, &text);
            failure.report = Some(path.display().to_string());
            failure
        })
    }

    /// `failure.png` and `snapshot.txt` of `context`'s page.
    fn capture(&self, dir: &Path, context: &str) -> Capture {
        artifacts::capture(dir, &|words| self.invoke(words, Some(context), None))
    }

    /// The context `--debug` kept, if one was.
    fn kept_context(&self) -> Option<Kept> {
        self.options
            .debug
            .as_ref()
            .and_then(|k| k.lock().unwrap_or_else(PoisonError::into_inner).clone())
    }

    /// With `--debug`, whether `context` is the first to fail, and stays.
    fn keep(&self, context: &str, place: &str) -> bool {
        let Some(debug) = &self.options.debug else {
            return false;
        };
        let mut kept = debug.lock().unwrap_or_else(PoisonError::into_inner);
        if kept.is_some() {
            return false;
        }
        *kept = Some(Kept {
            session: self.session.to_owned(),
            context: context.to_owned(),
            place: place.to_owned(),
        });
        true
    }

    /// Adds a step that started at `started` to the recording; with
    /// `--trace`, a screenshot of `context`'s page after it too.
    fn record(&self, mut step: StepRecord, started: Instant, context: Option<&str>) {
        let shot = {
            let recorder = self.recorder.borrow();
            match (recorder.as_ref(), context) {
                (Some(r), Some(context)) if r.trace() => Some((r.next_screenshot(), context)),
                _ => None,
            }
        };
        let elapsed = started.elapsed();
        if let Some(((path, name), context)) = shot {
            let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
            let file = path.display().to_string();
            let (taken, masked) = artifacts::screenshot(&file, "5s", &|words| {
                self.invoke(words, Some(context), None)
            });
            if taken.is_ok() {
                step.screenshot = Some(name);
                step.masked = masked;
            }
        }
        if let Some(recorder) = self.recorder.borrow_mut().as_mut() {
            recorder.record(step, started, elapsed);
        }
    }

    /// The span of `test`'s `@test` line.
    fn test_span(&self, test: &Planned) -> String {
        format!("{}:{}", self.plan.scenarios[test.file].name, test.line)
    }

    /// Makes the context a test (or hook) runs in.
    fn new_context(&self, span: &str) -> Result<String, Box<TestFailure>> {
        let out = self
            .setup(&["context", "new"], None, None, span)
            .map_err(|e| setup_failure(span, "context new", e))?;
        out.data["context"]["id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| {
                setup_failure(
                    span,
                    "context new",
                    ErrorInfo::new(ErrorKind::ProtocolError, "context new gave no context id"),
                )
            })
    }

    fn close_context(&self, context: &str) {
        if let Err(e) = self.invoke(&["context", "close", context], None, None) {
            tracing::warn!("cannot close context {context}: {}", e.message);
        }
    }

    /// Gives a fresh context its mock sets, viewport and the settings
    /// that make it behave the same in every run.
    fn settle(
        &self,
        context: &str,
        settings: &Settings,
        span: &str,
        deadline: Instant,
    ) -> Result<(), Box<TestFailure>> {
        // Real mode: only the backend's own sets (hybrid mode), without
        // the defaults or the tests' @mocks.
        let words: Option<Vec<&str>> = if self.plan.real() {
            (!self.plan.backend.mocks.is_empty()).then(|| {
                ["mock", "use", "--no-defaults"]
                    .into_iter()
                    .chain(self.plan.backend.mocks.iter().map(String::as_str))
                    .collect()
            })
        } else {
            (!settings.mocks.is_empty() || !self.plan.config.default_mocks.is_empty()).then(|| {
                ["mock", "use"]
                    .into_iter()
                    .chain(settings.mocks.iter().map(String::as_str))
                    .collect()
            })
        };
        let served = self.served();
        let id = self.test_id.borrow().clone();
        let mut tagged: Vec<String> = Vec::new();
        if let (Some(server), Some(_)) = (served, &words) {
            // The server's partition for the test, with the test's sets;
            // the context's requests to the app and to the server say
            // which it is.
            let sets = self.plan.mock_mode_sets(settings);
            let loaded = self
                .load_sets(&sets)
                .map_err(|e| setup_failure(span, "mock serve", e))?;
            let frozen = self
                .plan
                .determinism
                .freeze_time
                .as_deref()
                .and_then(|t| protocol::format::parse_time(t).ok());
            server.partition(&id, loaded, frozen);
            tagged.extend(["--test-id".to_owned(), id.clone()]);
            let origins = self
                .plan
                .backend
                .web_servers
                .iter()
                .map(|w| w.url.clone())
                .chain(self.plan.backend.base_url.clone())
                .chain(std::iter::once(server.url()));
            for origin in origins {
                tagged.extend(["--test-origin".to_owned(), origin]);
            }
        }
        if let Some(mut words) = words {
            let text = words.join(" ");
            words.extend(tagged.iter().map(String::as_str));
            self.setup(&words, Some(context), Some(deadline), span)
                .map_err(|e| setup_failure(span, &text, e))?;
        }
        let size = settings
            .viewport
            .unwrap_or(self.plan.config.viewport)
            .to_string();
        let words: &[&str] = match settings.device.as_deref() {
            Some(device) => &["emulate", device],
            None => &["viewport", size.as_str(), "--dpr", "1"],
        };
        self.setup(words, Some(context), Some(deadline), span)
            .map_err(|e| setup_failure(span, &words.join(" "), e))?;
        for words in determinism::commands(&self.plan.determinism) {
            let words: Vec<&str> = words.iter().map(String::as_str).collect();
            self.setup(&words, Some(context), Some(deadline), span)
                .map_err(|e| setup_failure(span, &setup_text(&words), e))?;
        }
        Ok(())
    }

    /// Runs a command of the runner's setup, and records it.
    fn setup(
        &self,
        words: &[&str],
        context: Option<&str>,
        deadline: Option<Instant>,
        span: &str,
    ) -> Result<Output, ErrorInfo> {
        let started = Instant::now();
        let result = self.invoke(words, context, deadline);
        let mut record = StepRecord::new(
            span.to_owned(),
            setup_text(words),
            Some("setup".to_owned()),
            Vec::new(),
            result.as_ref(),
        );
        // Ids and session names that change from run to run.
        record.output.clear();
        self.record(record, started, None);
        result
    }

    /// Runs `steps` in `context` until one fails.
    fn steps(
        &self,
        steps: &[Prepared],
        context: &str,
        deadline: Instant,
        part: Part,
    ) -> Result<(), Box<TestFailure>> {
        for step in steps {
            let started = Instant::now();
            let result = self.step(step, context, deadline, part);
            let unmatched = self.unmatched(context);
            let result = match result {
                Err(error) => Err(self.forget_session(with_unmatched(error, &unmatched))),
                Ok(_) if !unmatched.is_empty() => {
                    Err(self.forget_session(unmatched_error(&unmatched, &self.mock_sets(context))))
                }
                Ok(out) => Ok(out),
            };
            let mut record = StepRecord::new(
                step.at.to_string(),
                step.text.clone(),
                match part {
                    Part::Hook(name) => Some(name.to_owned()),
                    Part::Test => None,
                },
                step.via.iter().map(ToString::to_string).collect(),
                result.as_ref(),
            );
            record.output = self.forget_session_in(&record.output);
            self.record(record, started, Some(context));
            if let Err(error) = result {
                return Err(failure(step, part, error));
            }
        }
        Ok(())
    }

    /// The mock server, in mock mode.
    fn served(&self) -> Option<&e2e::mock_server::MockServer> {
        self.options.mock_server.get().filter(|_| !self.plan.real())
    }

    /// Mock sets `names` of the project, read (`mock check` already
    /// checked them).
    fn load_sets(&self, names: &[String]) -> Result<Vec<mock::MockSet>, ErrorInfo> {
        let config = &self.plan.config;
        let cwd = std::env::current_dir().unwrap_or_else(|_| config.root.clone());
        names
            .iter()
            .map(|name| mock::MockSet::load(&mock::Source::find(&config.mocks, &cwd, name)?))
            .collect()
    }

    /// The mock server's requests of the running try that went
    /// unmatched in a `fail` set's scope, since the last look.
    fn unmatched_served(&self) -> Vec<RoutedRequest> {
        let Some(server) = self.served() else {
            return Vec::new();
        };
        let seen = self.seen_served.get();
        let fresh: Vec<RoutedRequest> = server
            .journal(Some(&self.test_id.borrow()))
            .into_iter()
            .filter(|e| e.seq > seen && e.policy.as_deref() == Some("fail"))
            .collect();
        if let Some(last) = fresh.iter().map(|e| e.seq).max() {
            self.seen_served.set(last);
        }
        fresh
    }

    /// The requests of `context` that went unmatched in the scope of a
    /// set whose `unmatched = "fail"`, since the last look.
    fn unmatched(&self, context: &str) -> Vec<RoutedRequest> {
        let mut fresh = self.unmatched_in(context);
        fresh.extend(self.unmatched_served());
        fresh
    }

    /// [`Self::unmatched`] of the browser's requests.
    fn unmatched_in(&self, context: &str) -> Vec<RoutedRequest> {
        let Ok(out) = self.invoke(
            &["mock", "log", "--unmatched", "--limit", "0"],
            Some(context),
            None,
        ) else {
            return Vec::new();
        };
        let Ok(data) = serde_json::from_value::<MockLogData>(out.data) else {
            return Vec::new();
        };
        let seen = self.seen.get();
        let fresh: Vec<RoutedRequest> = data
            .log
            .into_iter()
            .filter(|e| e.seq > seen && e.policy.as_deref() == Some("fail"))
            .collect();
        if let Some(last) = fresh.iter().map(|e| e.seq).max() {
            self.seen.set(last);
        }
        fresh
    }

    /// The names of the mock sets `context` uses, for a `mock explain`
    /// hint.
    fn mock_sets(&self, context: &str) -> Vec<String> {
        self.invoke(&["mock", "list"], Some(context), None)
            .ok()
            .and_then(|out| {
                out.data["sets"].as_array().map(|sets| {
                    // Listed in the order they are tried: the last applied
                    // first.
                    sets.iter()
                        .rev()
                        .filter_map(|s| s["name"].as_str().map(str::to_owned))
                        .collect()
                })
            })
            .unwrap_or_default()
    }

    /// `expect --no-errors --all-tabs` after a test's own steps, unless
    /// console errors are allowed.
    fn no_errors(&self, test: &Planned, context: &str, span: &str) -> Result<(), Box<TestFailure>> {
        if self.plan.config.allow_console_errors || test.settings.allow_console_errors {
            return Ok(());
        }
        let words = ["expect", "--no-errors", "--all-tabs"];
        let started = Instant::now();
        let result = self.invoke(&words, Some(context), None);
        let record = StepRecord::new(
            span.to_owned(),
            words.join(" "),
            Some("check".to_owned()),
            Vec::new(),
            result.as_ref(),
        );
        self.record(record, started, None);
        result.map(drop).map_err(|error| {
            let error = self.forget_session(error).hint(Hint::text(
                "fix the error; if the test expects it, add @allow-console-errors to the test (or allow_console_errors = true to [e2e])",
            ));
            Box::new(TestFailure {
                at: span.to_owned(),
                step: words.join(" "),
                hook: Some("check".to_owned()),
                via: Vec::new(),
                error,
                report: None,
            })
        })
    }

    /// Runs one step, within the test's time.
    fn step(
        &self,
        step: &Prepared,
        context: &str,
        deadline: Instant,
        part: Part,
    ) -> Result<Output, ErrorInfo> {
        let over = || {
            ErrorInfo::new(
                ErrorKind::Timeout,
                "the test ran out of time before this step ended",
            )
            .hint(Hint::text(
                "give it longer: @timeout 60s in the test, or timeout = \"60s\" in [e2e]",
            ))
        };
        if Instant::now() >= deadline {
            return Err(over());
        }
        let mut global = self.global(Some(context), Some(deadline));
        global.tab = None;
        let words = {
            let test_id = self.test_id.borrow();
            let saved = self.saved.borrow();
            let resolver = Resolver::running(&step.vars, &self.plan.run, &test_id, &saved);
            step.line
                .words
                .iter()
                .map(|w| w.expand(&resolver))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| {
                    let info = ErrorInfo::new(ErrorKind::BadArgs, e.message);
                    match e.hint {
                        Some(hint) => info.hint(hint),
                        None => info,
                    }
                })?
        };
        let mut cli = script::command(&words, &global)?;
        self.allowed(&cli, part)?;
        let save = match &cli.command {
            Some(Command::Http(args)) => args.save.clone(),
            _ => None,
        };
        let sent = match &cli.command {
            Some(Command::Http(args)) if self.options.verify.is_some() => Some((
                now_ms(),
                args.json_body.clone().or_else(|| args.body.clone()),
                args.headers.clone(),
            )),
            _ => None,
        };
        super::project_baselines(
            &mut cli,
            &self.plan.config.root,
            self.options.update_snapshots,
        );
        // Files the step writes go in the test's artifacts folder.
        let dir = self.recorder.borrow().as_ref().map(|r| r.dir.clone());
        if let Some(dir) = dir {
            super::outputs::resolve(&mut cli, &dir, &self.reads.borrow());
        }
        let screenshot = matches!(cli.command, Some(Command::ExpectScreenshot(_)));
        let exit_on_finding = crate::exits_on_finding(&cli);
        let out = crate::run(cli)?;
        if exit_on_finding && crate::found(&out) {
            let message = if out.data["differs"] == true {
                "the pages differ (--exit-code)"
            } else {
                "explore found a problem (--exit-code)"
            };
            return Err(ErrorInfo::new(ErrorKind::ExpectFailed, message).details(out.data));
        }
        if let Some(name) = save {
            self.saved
                .borrow_mut()
                .insert(name, out.data["body"].clone());
        }
        if let Some((at_ms, body, headers)) = sent {
            self.http_log.borrow_mut().push(Seen {
                at_ms,
                exchange: http_exchange(&out.data, body, &headers),
            });
        }
        if screenshot {
            let mut warnings = self.warnings.borrow_mut();
            for warning in &out.warnings {
                if !warnings.contains(warning) {
                    warnings.push(warning.clone());
                }
            }
        }
        if Instant::now() > deadline {
            return Err(over());
        }
        Ok(out)
    }

    /// `--verify-mocks`: gives the verifier the real exchanges of the
    /// test's context: its pages' (from the HAR export) and its `http`
    /// steps'. A failure to read them is a warning of the test.
    fn verify(&self, context: &str, sets: &[String]) {
        let Some(verifier) = &self.options.verify else {
            return;
        };
        let mut seen: Vec<Seen> = self.http_log.take();
        let exported = self.exchanges(context, verifier.scope(sets));
        let result = exported.and_then(|pages| {
            seen.extend(pages);
            verifier.compare(sets, seen)
        });
        if let Err(e) = result {
            self.warnings
                .borrow_mut()
                .push(format!("--verify-mocks could not compare this test: {e}"));
        }
    }

    /// The requests of `context`'s pages in `scope` (their fetch and XHR
    /// ones when empty), with their answers, from the HAR export.
    fn exchanges(&self, context: &str, scope: Vec<String>) -> Result<Vec<Seen>, String> {
        let request = engine::HarExportRequest {
            tab: None,
            scope,
            timeout_ms: 10_000,
        };
        let global = self.global(Some(context), None);
        let out = crate::session::call_running(&global, daemon::rpc::HAR_EXPORT, &request)
            .map_err(|e| e.message)?;
        let exchanges = mock::record::har_exchanges(&out.data["har"])?;
        let times: Vec<i64> = out.data["har"]["log"]["entries"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|e| {
                        e["startedDateTime"]
                            .as_str()
                            .and_then(|t| protocol::format::parse_time(t).ok())
                            .unwrap_or(0)
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(exchanges
            .into_iter()
            .enumerate()
            .map(|(i, exchange)| Seen {
                at_ms: times.get(i).copied().unwrap_or(0),
                exchange,
            })
            .collect())
    }

    /// `--record-mocks`: keeps test `index`'s exchanges (its last try's,
    /// which passed), in the scope of the sets it has in mock mode (else
    /// its fetch and XHR requests). A failure to read them is a warning
    /// of the test.
    fn record_mocks(&self, test: &Planned, index: usize, context: &str) {
        let Some(record) = &self.options.record else {
            return;
        };
        let sets = self.plan.mock_mode_sets(&test.settings);
        let scope = super::verify::scope_of_sets(&self.plan.config, &sets);
        match self.exchanges(context, scope.clone()) {
            Ok(seen) => {
                let exchanges = seen.into_iter().map(|s| s.exchange).collect();
                record
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(index, (scope, exchanges));
            }
            Err(e) => self.warnings.borrow_mut().push(format!(
                "--record-mocks could not read this test's requests: {e}"
            )),
        }
    }

    /// Whether the run lets step `cli` run: mock-only commands need mock
    /// mode, and `http` writes to a protected backend need a hook or
    /// `--allow-writes`.
    fn allowed(&self, cli: &crate::cli::Cli, part: Part) -> Result<(), ErrorInfo> {
        let plan = self.plan;
        let mock_only = super::mock_only(cli);
        if let Some(command) = mock_only
            && plan.real()
        {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!(
                    "{command} needs the mocks, and backend {} is real",
                    plan.backend.name
                ),
            )
            .hint(Hint::text(
                "mark the test @backend mock, so real backends skip it",
            )));
        }
        if let Some(Command::Http(args)) = &cli.command
            && plan.backend.protected
            && !plan.allow_writes
            && matches!(part, Part::Test)
            && !matches!(
                args.method.to_ascii_uppercase().as_str(),
                "GET" | "HEAD" | "OPTIONS"
            )
        {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!(
                    "http {} on protected backend {}: a @safe test only reads",
                    args.method.to_ascii_uppercase(),
                    plan.backend.name
                ),
            )
            .hint(Hint::text(
                "arrange and clean up in @before/@after, or run with --allow-writes",
            )));
        }
        Ok(())
    }

    /// `error` without the `--session` of the run's own session in its
    /// hint and notes: that session is closed when anyone reads them.
    fn forget_session(&self, mut error: ErrorInfo) -> ErrorInfo {
        let flag = format!(" --session {}", self.session);
        if self.session == self.options.global.session || self.options.debug.is_some() {
            return error;
        }
        if let Some(hint) = &mut error.hint {
            *hint = hint.replace(&flag, "");
        }
        for note in &mut error.notes {
            note.text = note.text.replace(&flag, "");
        }
        error
    }

    /// `text` without the `--session` of the run's own session.
    fn forget_session_in(&self, text: &str) -> String {
        if self.session == self.options.global.session || self.options.debug.is_some() {
            return text.to_owned();
        }
        text.replace(&format!(" --session {}", self.session), "")
    }

    /// Runs a runner command (`context new`, `mock use`, …).
    fn invoke(
        &self,
        words: &[&str],
        context: Option<&str>,
        deadline: Option<Instant>,
    ) -> Result<Output, ErrorInfo> {
        let words: Vec<String> = words.iter().map(|&w| w.to_owned()).collect();
        let global = self.global(context, deadline);
        crate::run(script::command(&words, &global)?)
    }

    /// The flags of a step: the `test` command's, in the worker's session
    /// and `context`, its timeout cut to the test's time left.
    fn global(&self, context: Option<&str>, deadline: Option<Instant>) -> GlobalArgs {
        let mut global = self.options.global.clone();
        self.session.clone_into(&mut global.session);
        global.context = context.map(str::to_owned);
        global.json = false;
        global.quiet = false;
        if let Some(deadline) = deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            global.timeout = match global.timeout {
                Some(t) => Some(t.min(left)),
                None if left < WAIT_TIMEOUT => Some(left),
                None => None,
            };
        }
        global
    }

    /// Runs the file's `@before-all` unless it ran; waits while another
    /// worker runs it. Its failure fails `test`.
    fn before_all(&self, test: &Planned, once: &Mutex<Once>) -> Result<(), Box<TestFailure>> {
        let hooks = &self.plan.hooks[test.file];
        if hooks.before_all.is_empty() {
            return Ok(());
        }
        let mut state = once.lock().unwrap_or_else(PoisonError::into_inner);
        if matches!(*state, Once::NotRun) {
            self.saved.borrow_mut().clear();
            *state = match self.hook(test, &hooks.before_all, "before-all") {
                Ok(()) => Once::Passed(self.saved.take()),
                Err(failure) => Once::Failed(failure),
            };
        }
        match &*state {
            Once::Failed(failure) => Err(failure.clone()),
            Once::Passed(saved) => {
                self.file_saved.borrow_mut().clone_from(saved);
                Ok(())
            }
            Once::NotRun => Ok(()),
        }
    }

    /// Runs the file's `@after-all` after its last test (`test`); its
    /// failure fails that test.
    fn after_all(&self, test: &Planned, outcome: Outcome) -> Outcome {
        let hooks = &self.plan.hooks[test.file];
        if hooks.after_all.is_empty() {
            return outcome;
        }
        *self.saved.borrow_mut() = self.file_saved.borrow().clone();
        match self.hook(test, &hooks.after_all, "after-all") {
            Err(failure) if outcome.failure.is_none() || outcome.status != Status::Failed => {
                Outcome {
                    status: Status::Failed,
                    failure: Some(failure),
                    ..outcome
                }
            }
            _ => outcome,
        }
    }

    /// A file-wide hook, in a context of its own with the file's settings.
    fn hook(
        &self,
        test: &Planned,
        steps: &[Prepared],
        name: &'static str,
    ) -> Result<(), Box<TestFailure>> {
        let file = &self.plan.scenarios[test.file];
        let deadline = Instant::now() + self.plan.timeout(&file.settings);
        let span = format!("{}:1", file.name);
        let title = format!("@{name} of {}", file.name);
        let subject = Subject {
            title: &title,
            place: &span,
            attempt: None,
            rerun: Cmd::new("test").arg(&file.name).to_string(),
        };
        let file_dir = super::plan::file_dir(&self.plan.config, file);
        let dir = file_dir.join(format!("@{name}"));
        *self.reads.borrow_mut() = vec![file_dir.join("@before-all")];
        *self.test_id.borrow_mut() = format!("{}-f{}", self.plan.run.run_id, test.file + 1);
        self.in_context(
            &dir,
            &subject,
            &file.settings,
            deadline,
            None,
            None,
            |context| self.steps(steps, context, deadline, Part::Hook(name)),
        )
    }
}

/// Now, in ms since 1970.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// An `http` step's exchange, from its output's data and the body and
/// headers it sent.
fn http_exchange(data: &Value, body: Option<String>, headers: &[String]) -> mock::record::Exchange {
    let text = |key: &str| data[key].as_str().unwrap_or_default().to_owned();
    let mut request_headers: Vec<(String, String)> = headers
        .iter()
        .filter_map(|h| {
            let (n, v) = h.split_once(':')?;
            Some((n.trim().to_owned(), v.trim().to_owned()))
        })
        .collect();
    let body = body.map(|b| match serde_json::from_str::<Value>(&b) {
        Ok(json) => {
            request_headers.push(("Content-Type".into(), "application/json".into()));
            json.to_string()
        }
        Err(_) => b,
    });
    let response_body = match &data["body"] {
        Value::Null => Vec::new(),
        Value::String(s) if data["json"] != Value::Bool(true) => s.clone().into_bytes(),
        other => other.to_string().into_bytes(),
    };
    mock::record::Exchange {
        method: text("method"),
        url: text("url"),
        request_headers,
        request_body: body,
        status: u16::try_from(data["status"].as_u64().unwrap_or(0)).unwrap_or(0),
        response_headers: Vec::new(),
        response_body: Some(response_body),
        mime: data["headers"]["content-type"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        resource_type: "other".into(),
    }
}

/// How long `@after` steps may run past the test's time limit, so a test
/// that ran out of time still cleans up.
const AFTER_GRACE: Duration = Duration::from_secs(10);

fn failure(step: &Prepared, part: Part, error: ErrorInfo) -> Box<TestFailure> {
    Box::new(TestFailure {
        at: step.at.to_string(),
        step: step.text.clone(),
        hook: match part {
            Part::Hook(name) => Some(name.to_owned()),
            Part::Test => None,
        },
        via: step.via.iter().map(ToString::to_string).collect(),
        error,
        report: None,
    })
}

/// The failure of a step after which requests went unmatched.
fn unmatched_error(unmatched: &[RoutedRequest], sets: &[String]) -> ErrorInfo {
    let first = &unmatched[0];
    let message = match unmatched.len() {
        1 => format!("unmatched request: {} {}", first.method, first.url),
        n => format!(
            "{n} unmatched requests; the first: {} {}",
            first.method, first.url
        ),
    };
    let mut info = ErrorInfo::new(ErrorKind::ExpectFailed, message);
    if let Some(set) = &first.unmatched {
        info = info.note(
            "scope",
            format!("mock set {set}, whose unmatched = \"fail\" answered it 501"),
        );
    }
    if first.closest.is_empty() {
        info = info.note("closest mock", "none has a path like it");
    }
    for check in &first.closest {
        info = info.note("closest mock", mock::text::closest_line(check));
    }
    for other in unmatched.iter().skip(1).take(UNMATCHED_LISTED) {
        info = info.note("unmatched", format!("{} {}", other.method, other.url));
    }
    let mut explain = Cmd::new("mock")
        .arg("explain")
        .arg(&first.method)
        .arg(&first.url);
    if !sets.is_empty() {
        explain = explain.flag("mocks", sets.join(","));
    }
    info.hint(format!("add a mock for it, or run {explain}"))
        .details(serde_json::json!({ "unmatched": unmatched }))
}

/// `error` of a step that failed, with the requests that went unmatched
/// meanwhile (a 501 is the likely cause).
fn with_unmatched(mut error: ErrorInfo, unmatched: &[RoutedRequest]) -> ErrorInfo {
    for entry in unmatched.iter().take(UNMATCHED_LISTED) {
        let mut text = format!("{} {} (no mock answered it", entry.method, entry.url);
        if let Some(check) = entry.closest.first() {
            text.push_str("; closest mock: ");
            text.push_str(&mock::text::closest_line(check));
        }
        text.push(')');
        error = error.note("unmatched", text);
    }
    if !unmatched.is_empty()
        && let Some(serde_json::Value::Object(details)) = &mut error.details
    {
        details.insert("unmatched".into(), serde_json::json!(unmatched));
    }
    error
}

/// How many unmatched requests a failure lists.
const UNMATCHED_LISTED: usize = 3;

/// A setup command as a failure names it: an init script by its first
/// line, not its whole code.
fn setup_text(words: &[&str]) -> String {
    match words {
        ["init-script", "add", "--code", code] => format!(
            "init-script add --code '{}…'",
            code.lines().next().unwrap_or_default()
        ),
        _ => words.join(" "),
    }
}

/// A failure of what the runner does around a test's steps.
fn setup_failure(at: &str, step: &str, error: ErrorInfo) -> Box<TestFailure> {
    Box::new(TestFailure {
        at: at.to_owned(),
        step: step.to_owned(),
        hook: Some("setup".to_owned()),
        via: Vec::new(),
        error,
        report: None,
    })
}
