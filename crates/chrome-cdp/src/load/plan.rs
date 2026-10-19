//! What a load run does, worked out before a browser starts (plan 23.1):
//! the test runner's plan of the journeys (config, backend, scenarios,
//! variables), a real backend, and every step of every journey and hook
//! checked as a load step, all problems at once.
//!
//! A step whose words do not change between iterations (no `${test.id}`,
//! no `${saved.…}`) gets its engine request once, here; the others are
//! filled in again for each iteration.

use std::collections::BTreeMap;

use protocol::script::{Resolve, Unresolved, Var};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};

use super::step::{self, Call};
use crate::cli::{Cli, Command, GlobalArgs, JobsIsolation, LoadArgs, TestArgs};
use crate::script::{self as lines, Problem};
use crate::test::plan::{Plan, Prepared};
use crate::test::vars::Resolver;

/// A step of a journey or hook, ready to run.
#[derive(Debug)]
pub(super) struct Step {
    /// `file:line:col`.
    pub(super) at: String,
    /// The line as written.
    pub(super) text: String,
    pub(super) how: How,
}

/// How a step gets its call.
#[derive(Debug)]
pub(super) enum How {
    /// Made once: its words are the same in every iteration.
    Fixed(Call),
    /// Made in each iteration, from its words with that iteration's
    /// `${test.id}` and `${saved.…}`.
    Each(Prepared),
}

/// A file's hooks, ready to run.
#[derive(Debug, Default)]
pub(super) struct Hooks {
    pub(super) before_all: Vec<Step>,
    pub(super) after_all: Vec<Step>,
    pub(super) before: Vec<Step>,
    pub(super) after: Vec<Step>,
}

/// A test the users run.
#[derive(Debug)]
pub(super) struct Journey {
    /// Index into [`Plan::scenarios`].
    pub(super) file: usize,
    pub(super) name: String,
    /// `file:line`.
    pub(super) place: String,
    pub(super) steps: Vec<Step>,
    /// How long one iteration may take (the test's `@timeout`, else the
    /// config's).
    pub(super) timeout: std::time::Duration,
}

/// Everything a load run needs to know.
#[derive(Debug)]
pub(super) struct LoadPlan {
    /// The test runner's plan of the same files.
    pub(super) plan: Plan,
    pub(super) journeys: Vec<Journey>,
    /// By file, like [`Plan::scenarios`]; only the files of journeys have
    /// any.
    pub(super) hooks: Vec<Hooks>,
    /// The flags steps get: the backend's base_url.
    pub(super) global: GlobalArgs,
}

impl LoadPlan {
    pub(super) fn make(args: &LoadArgs, global: &GlobalArgs) -> Result<Self, ErrorInfo> {
        let plan = Plan::make(&test_args(args), global, None)?;
        if !plan.real() {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!(
                    "load needs a real backend, and {} is the mock one",
                    plan.backend.name
                ),
            )
            .note(
                "why",
                "load testing the mocks measures chrome-cdp, not the site",
            )
            .hint(
                Hint::text("add --backend <name> of a real backend in [e2e.backends]").or(format!(
                    "e.g. {}",
                    args.paths
                        .iter()
                        .fold(Cmd::new("load"), Cmd::arg)
                        .flag("backend", "staging")
                )),
            ));
        }
        let mut global = global.clone();
        if let Some(url) = &plan.backend.base_url {
            global.base_url = Some(url.clone());
        }
        global.tab = None;
        global.context = None;
        global.json = false;
        global.quiet = false;
        let mut problems = Vec::new();
        let mut journeys = Vec::new();
        let mut hooks: Vec<Hooks> = plan.scenarios.iter().map(|_| Hooks::default()).collect();
        let mut hooked = vec![false; plan.scenarios.len()];
        let checker = Checker {
            plan: &plan,
            global: &global,
            writes: !plan.backend.protected || args.allow_writes,
        };
        for test in plan.tests.iter().filter(|t| t.skipped().is_none()) {
            let scenario = &plan.scenarios[test.file];
            let saved = crate::test::vars::saved_names(scenario);
            let ready = |steps: &[Prepared], hook: bool, problems: &mut Vec<Problem>| {
                steps
                    .iter()
                    .filter_map(|s| checker.step(s, &saved, hook, problems))
                    .collect::<Vec<_>>()
            };
            if !hooked[test.file] {
                hooked[test.file] = true;
                let file = &plan.hooks[test.file];
                hooks[test.file] = Hooks {
                    before_all: ready(&file.before_all, true, &mut problems),
                    after_all: ready(&file.after_all, true, &mut problems),
                    before: ready(&file.before, true, &mut problems),
                    after: ready(&file.after, true, &mut problems),
                };
            }
            journeys.push(Journey {
                file: test.file,
                name: test.name.clone(),
                place: format!("{}:{}", scenario.name, test.line),
                steps: ready(&test.steps, false, &mut problems),
                timeout: plan.timeout(&test.settings),
            });
        }
        if !problems.is_empty() {
            problems.sort_by(|a, b| {
                (a.span.file.as_ref(), a.span.start).cmp(&(b.span.file.as_ref(), b.span.start))
            });
            return Err(lines::invalid(problems));
        }
        if journeys.is_empty() {
            let skipped = plan.tests.len();
            return Err(if skipped == 0 {
                ErrorInfo::new(ErrorKind::NotFound, "no test matches").hint(
                    Hint::run(
                        args.paths
                            .iter()
                            .fold(Cmd::new("test"), Cmd::arg)
                            .switch("list"),
                    )
                    .why("lists the tests the paths hold"),
                )
            } else {
                ErrorInfo::new(
                    ErrorKind::NotFound,
                    format!(
                        "every test matched is skipped on backend {}",
                        plan.backend.name
                    ),
                )
                .hint("pick tests without @skip or @backend mock")
            });
        }
        Ok(Self {
            plan,
            journeys,
            hooks,
            global,
        })
    }
}

/// `load`'s arguments as the test runner's, for its plan.
fn test_args(args: &LoadArgs) -> TestArgs {
    TestArgs {
        paths: args.paths.clone(),
        grep: args.grep.clone(),
        tags: args.tags.clone(),
        jobs: None,
        jobs_isolation: JobsIsolation::Auto,
        shard: None,
        retries: None,
        repeat: None,
        bail: false,
        failed_first: false,
        headed: args.headed,
        update_snapshots: false,
        list: false,
        trace: false,
        debug: false,
        watch: false,
        reporter: Vec::new(),
        backend: args.backend.clone(),
        verify_mocks: false,
        update: false,
        allow_writes: args.allow_writes,
        heal: None,
        write: false,
        record_mocks: false,
        match_bodies: false,
        ui_coverage: false,
        max_chars: 0,
    }
}

/// Checks steps as load steps.
struct Checker<'a> {
    plan: &'a Plan,
    global: &'a GlobalArgs,
    /// Whether `http` writes may run in a journey's own steps (a protected
    /// backend needs `--allow-writes`).
    writes: bool,
}

impl Checker<'_> {
    /// `step` ready to run, or `None` with its problem in `problems`.
    fn step(
        &self,
        step: &Prepared,
        saved: &std::collections::BTreeSet<String>,
        hook: bool,
        problems: &mut Vec<Problem>,
    ) -> Option<Step> {
        let problem = |info: ErrorInfo| Problem {
            span: step.at.clone(),
            info,
        };
        let checking = Resolver::checking(&step.vars, &self.plan.run, saved);
        let words = match expand(step, &checking) {
            Ok(words) => words,
            Err(info) => {
                problems.push(problem(info));
                return None;
            }
        };
        let checked = lines::command(&words, self.global)
            .and_then(|cli| step::check(&cli, &words).map(|()| cli))
            .and_then(|cli| self.writes_allowed(&cli, hook).map(|()| cli));
        let cli = match checked {
            Ok(cli) => cli,
            Err(info) => {
                problems.push(problem(info));
                return None;
            }
        };
        let empty = BTreeMap::new();
        let fixed = Fixed(Resolver::running(&step.vars, &self.plan.run, "", &empty));
        let how = match expand(step, &fixed) {
            Ok(_) => match step::call(&cli) {
                Ok(call) => How::Fixed(call),
                Err(info) => {
                    problems.push(problem(info));
                    return None;
                }
            },
            Err(_) => How::Each(step.clone()),
        };
        Some(Step {
            at: step.at.to_string(),
            text: step.text.clone(),
            how,
        })
    }

    /// On a protected backend without `--allow-writes`, a journey's own
    /// steps only read: `http` writes belong in hooks.
    fn writes_allowed(&self, cli: &Cli, hook: bool) -> Result<(), ErrorInfo> {
        if self.writes || hook {
            return Ok(());
        }
        match &cli.command {
            Some(Command::Http(args))
                if !matches!(
                    args.method.to_ascii_uppercase().as_str(),
                    "GET" | "HEAD" | "OPTIONS"
                ) =>
            {
                Err(ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!(
                        "http {} on protected backend {}: a @safe journey only reads",
                        args.method.to_ascii_uppercase(),
                        self.plan.backend.name
                    ),
                )
                .hint(Hint::text(
                    "arrange and clean up in @before/@after, or run with --allow-writes",
                )))
            }
            _ => Ok(()),
        }
    }
}

/// A step's words with its variables filled in by `vars`.
pub(super) fn expand(step: &Prepared, vars: &dyn Resolve) -> Result<Vec<String>, ErrorInfo> {
    step.line
        .words
        .iter()
        .map(|w| w.expand(vars))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| {
            let info = ErrorInfo::new(ErrorKind::BadArgs, e.message);
            match e.hint {
                Some(hint) => info.hint(hint),
                None => info,
            }
        })
}

/// The variables that are the same in every iteration: `${test.id}` and
/// `${saved.…}` are not.
struct Fixed<'a>(Resolver<'a>);

impl Resolve for Fixed<'_> {
    fn resolve(&self, var: &Var) -> Result<String, Unresolved> {
        match var {
            Var::Path(path)
                if matches!(path.first().map(String::as_str), Some("test" | "saved")) =>
            {
                Err(Unresolved::new("changes with each iteration"))
            }
            _ => self.0.resolve(var),
        }
    }
}
