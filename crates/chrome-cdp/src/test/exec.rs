//! Running the tests of a [`Plan`] (plan 11, milestone 11.4).
//!
//! `jobs` worker threads take the tests in order. Each test runs in a
//! fresh browser context of the worker's session (`context new`), which
//! gets the test's mock sets and viewport before its `@before` steps, its
//! own steps and its `@after` steps run; the context is closed after.
//! Every step goes through the same code as a CLI command
//! ([`crate::run`]), with `--session` and `--context` set. A failed test
//! runs again up to its retries; one that then passes is flaky.
//!
//! A file's `@before-all` runs once, before its first test (a worker that
//! needs it meanwhile waits), in a context of its own; its `@after-all`
//! runs after its last test. A failed `@before-all` fails every test of
//! the file.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use protocol::schema::TestFailure;
use protocol::{ErrorInfo, ErrorKind, Hint, Output};

use e2e::scenario::Settings;

use super::plan::{Plan, Planned, Prepared};
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
}

/// Where a file's `@before-all` is.
#[derive(Debug, Clone)]
enum Once {
    NotRun,
    Passed,
    Failed(Box<TestFailure>),
}

/// Runs `plan`'s tests; `report` gets each result in the plan's order as
/// soon as it and those before it are known. `stop` ends the run early
/// (the tests not started are not run).
pub(super) fn execute(
    plan: &Plan,
    options: &Options,
    stop: &AtomicBool,
    report: &(dyn Fn(usize, &Outcome) + Sync),
) -> Vec<Outcome> {
    let count = plan.tests.len();
    let next = AtomicUsize::new(0);
    let results: Mutex<(Vec<Option<Outcome>>, usize)> = Mutex::new((vec![None; count], 0));
    let before_all: Vec<Mutex<Once>> = plan
        .scenarios
        .iter()
        .map(|_| Mutex::new(Once::NotRun))
        .collect();
    // Tests of each file still to run, for its @after-all.
    let left: Vec<AtomicUsize> = plan
        .scenarios
        .iter()
        .enumerate()
        .map(|(f, _)| {
            AtomicUsize::new(
                plan.tests
                    .iter()
                    .filter(|t| t.file == f && t.skipped().is_none())
                    .count(),
            )
        })
        .collect();
    let workers = usize::try_from(plan.jobs).unwrap_or(1).min(count.max(1));
    std::thread::scope(|scope| {
        for worker in 0..workers {
            let session = &options.sessions[worker % options.sessions.len()];
            let (next, results, before_all, left) = (&next, &results, &before_all, &left);
            scope.spawn(move || {
                loop {
                    let index = next.fetch_add(1, Ordering::SeqCst);
                    if index >= count {
                        break;
                    }
                    let test = &plan.tests[index];
                    let mut outcome = if stop.load(Ordering::SeqCst) {
                        not_run(options)
                    } else if let Some(reason) = test.skipped() {
                        Outcome {
                            status: Status::Skipped,
                            duration: Duration::ZERO,
                            tries: 0,
                            failure: None,
                            reason: Some(reason.to_owned()),
                        }
                    } else {
                        let runner = Runner {
                            plan,
                            options,
                            session,
                        };
                        let outcome = match runner.before_all(test, &before_all[test.file]) {
                            Err(failure) => Outcome {
                                status: Status::Failed,
                                duration: Duration::ZERO,
                                tries: 0,
                                failure: Some(failure),
                                reason: None,
                            },
                            Ok(()) => runner.test(test),
                        };
                        let last = left[test.file].fetch_sub(1, Ordering::SeqCst) == 1;
                        if last {
                            runner.after_all(test, outcome)
                        } else {
                            outcome
                        }
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
                    done[index] = Some(outcome);
                    while *printed < count {
                        let Some(outcome) = &done[*printed] else {
                            break;
                        };
                        report(*printed, outcome);
                        *printed += 1;
                    }
                }
            });
        }
    });
    let (done, _) = results.into_inner().unwrap_or_else(PoisonError::into_inner);
    done.into_iter()
        .map(|o| o.unwrap_or_else(|| not_run(options)))
        .collect()
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
    }
}

/// Runs steps in one session.
struct Runner<'a> {
    plan: &'a Plan,
    options: &'a Options,
    session: &'a str,
}

/// A step's place in a test, for its failure.
#[derive(Clone, Copy)]
enum Part {
    Hook(&'static str),
    Test,
}

impl Runner<'_> {
    /// `test`'s tries until it passes or its retries are used up.
    fn test(&self, test: &Planned) -> Outcome {
        let tries = self.plan.retries(test) + 1;
        let mut first_failure = None;
        for attempt in 1..=tries {
            let started = Instant::now();
            let result = self.attempt(test);
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
                    };
                }
                Err(failure) if attempt == tries => {
                    return Outcome {
                        status: Status::Failed,
                        duration,
                        tries: attempt,
                        failure: Some(failure),
                        reason: None,
                    };
                }
                Err(failure) => {
                    first_failure.get_or_insert(failure);
                }
            }
        }
        unreachable!("a test runs at least once")
    }

    /// One try of `test` in a fresh context.
    fn attempt(&self, test: &Planned) -> Result<(), Box<TestFailure>> {
        let deadline = Instant::now() + self.plan.timeout(&test.settings);
        let hooks = &self.plan.hooks[test.file];
        let span = self.test_span(test);
        let context = self.new_context(&span)?;
        let result = (|| {
            self.settle(&context, &test.settings, &span, deadline)?;
            self.steps(&hooks.before, &context, deadline, Part::Hook("before"))?;
            self.steps(&test.steps, &context, deadline, Part::Test)
        })();
        // `@after` runs whatever happened, while there is time.
        let after = self.steps(
            &hooks.after,
            &context,
            deadline + AFTER_GRACE,
            Part::Hook("after"),
        );
        self.close_context(&context);
        result.and(after)
    }

    /// The span of `test`'s `@test` line.
    fn test_span(&self, test: &Planned) -> String {
        format!("{}:{}", self.plan.scenarios[test.file].name, test.line)
    }

    /// Makes the context a test (or hook) runs in.
    fn new_context(&self, span: &str) -> Result<String, Box<TestFailure>> {
        let out = self
            .invoke(&["context", "new"], None, None)
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

    /// Gives a fresh context its mock sets and viewport.
    fn settle(
        &self,
        context: &str,
        settings: &Settings,
        span: &str,
        deadline: Instant,
    ) -> Result<(), Box<TestFailure>> {
        if !settings.mocks.is_empty() || !self.plan.config.default_mocks.is_empty() {
            let mut words = vec!["mock", "use"];
            words.extend(settings.mocks.iter().map(String::as_str));
            let text = words.join(" ");
            self.invoke(&words, Some(context), Some(deadline))
                .map_err(|e| setup_failure(span, &text, e))?;
        }
        let size = settings
            .viewport
            .unwrap_or(self.plan.config.viewport)
            .to_string();
        let words = settings
            .device
            .as_deref()
            .map_or(["viewport", size.as_str()], |device| ["emulate", device]);
        self.invoke(&words, Some(context), Some(deadline))
            .map_err(|e| setup_failure(span, &words.join(" "), e))?;
        Ok(())
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
            self.step(step, context, deadline)
                .map_err(|error| failure(step, part, self.forget_session(error)))?;
        }
        Ok(())
    }

    /// Runs one step, within the test's time.
    fn step(&self, step: &Prepared, context: &str, deadline: Instant) -> Result<(), ErrorInfo> {
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
        let mut cli = script::command(&step.words, &global)?;
        match &mut cli.command {
            Some(Command::ExpectSnapshot(args)) => {
                args.update |= self.options.update_snapshots;
                args.dir.get_or_insert_with(|| self.plan.snapshots.clone());
            }
            Some(Command::ExpectScreenshot(args)) => {
                args.update |= self.options.update_snapshots;
                args.dir.get_or_insert_with(|| self.plan.snapshots.clone());
            }
            _ => {}
        }
        crate::run(cli)?;
        if Instant::now() > deadline {
            return Err(over());
        }
        Ok(())
    }

    /// `error` without the `--session` of the run's own session in its
    /// hint and notes: that session is closed when anyone reads them.
    fn forget_session(&self, mut error: ErrorInfo) -> ErrorInfo {
        let flag = format!(" --session {}", self.session);
        if self.session == self.options.global.session {
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
            *state = match self.hook(test, &hooks.before_all, "before-all") {
                Ok(()) => Once::Passed,
                Err(failure) => Once::Failed(failure),
            };
        }
        match &*state {
            Once::Failed(failure) => Err(failure.clone()),
            Once::NotRun | Once::Passed => Ok(()),
        }
    }

    /// Runs the file's `@after-all` after its last test (`test`); its
    /// failure fails that test.
    fn after_all(&self, test: &Planned, outcome: Outcome) -> Outcome {
        let hooks = &self.plan.hooks[test.file];
        if hooks.after_all.is_empty() {
            return outcome;
        }
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
        let context = self.new_context(&span)?;
        let result = self
            .settle(&context, &file.settings, &span, deadline)
            .and_then(|()| self.steps(steps, &context, deadline, Part::Hook(name)));
        self.close_context(&context);
        result
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
    })
}

/// A failure of what the runner does around a test's steps.
fn setup_failure(at: &str, step: &str, error: ErrorInfo) -> Box<TestFailure> {
    Box::new(TestFailure {
        at: at.to_owned(),
        step: step.to_owned(),
        hook: Some("setup".to_owned()),
        via: Vec::new(),
        error,
    })
}
