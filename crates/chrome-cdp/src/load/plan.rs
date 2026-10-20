//! What a load run does, worked out before a browser starts (plans 23.1
//! and 23.3): the profile (or the flags of a run without one), the test
//! runner's plan of the journeys (config, backend, scenarios, variables),
//! a real backend, the journeys' data, and every step of every journey and
//! hook checked as a load step with its variables, all problems at once.
//!
//! A step whose words do not change between iterations (no `${test.id}`,
//! `${saved.…}`, `${vu.id}`, `${iteration.id}` or `${row.…}`) gets its
//! engine request once, here; the others are filled in again for each
//! iteration.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use protocol::script::{Resolve, Span, Unresolved, Var};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};

use super::data::Data;
use super::profile::{self, Model, OnFailure, Profile, TestPick, UserStage, Users};
use super::step::{self, Call};
use super::think::Think;
use super::threshold::{self, Threshold};
use crate::cli::{Cli, Command, GlobalArgs, JobsIsolation, LoadArgs, TestArgs};
use crate::script::{self as lines, Problem};
use crate::test::plan::{Plan, Planned, Prepared};
use crate::test::vars::{LoadVars, Resolver, Rows};

/// A run without a profile: `--users`' default.
pub(crate) const USERS: u32 = 10;
/// A run without a profile: `--duration`'s default.
pub(crate) const DURATION: Duration = Duration::from_secs(60);

/// A step of a journey or hook, ready to run.
#[derive(Debug)]
pub(super) struct Step {
    /// `file:line:col`.
    pub(super) at: String,
    /// The line as written.
    pub(super) text: String,
    pub(super) how: How,
    /// What kind of step it is, for `think_between_steps`.
    pub(super) kind: Kind,
}

/// What kind of step a step is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    /// A navigation or an input action: what a person pauses after.
    Action,
    Think,
    Other,
}

/// How a step gets its call.
#[derive(Debug)]
pub(super) enum How {
    /// Made once: its words are the same in every iteration.
    Fixed(Call),
    /// Made in each iteration, from its words with that iteration's
    /// variables.
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
    pub(super) timeout: Duration,
    /// The test's `@device` or `@viewport`, made in each iteration's
    /// context before its steps.
    pub(super) emulate: Option<engine::EmulateRequest>,
    /// How often it is picked, against the other journeys' weights.
    pub(super) weight: u32,
    /// Its rows, one per iteration (`data`).
    pub(super) data: Option<Arc<Data>>,
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
    /// What Chrome fails at once in every user's context (`[load] block`).
    pub(super) blocked: Vec<engine::BlockedUrl>,
    /// The profile's name (`e2e/load/shop.toml`), if the run has one.
    pub(super) profile: Option<String>,
    pub(super) model: Model,
    /// A bare `think`'s time.
    pub(super) think: Think,
    /// Without a profile, each user takes the journeys in turn instead of
    /// by weight (plan 23.1).
    pub(super) in_turn: bool,
    pub(super) on_failure: OnFailure,
    pub(super) max_failures: u64,
    /// The profile's `[thresholds]`, their groups known.
    pub(super) thresholds: Vec<Threshold>,
}

impl LoadPlan {
    pub(super) fn make(args: &LoadArgs, global: &GlobalArgs) -> Result<Self, ErrorInfo> {
        let cwd = std::env::current_dir().map_err(|e| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("cannot read the current directory: {e}"),
            )
        })?;
        let root = crate::test::plan::project(&cwd)?.root;
        let profile = match args.paths.as_slice() {
            [one] => match profile::find(one, &root, &cwd) {
                Some(path) => Some(profile::read(&path, &root)?),
                None if !one.contains(['.', '/', '\\', ':']) && !cwd.join(one).exists() => {
                    return Err(no_profile(one, &root));
                }
                None => None,
            },
            _ => None,
        };
        let mut problems = Vec::new();
        let test_args = match &profile {
            Some(p) => {
                flags_without_profile(args, p)?;
                profile_args(args, p, &mut problems)
            }
            None => test_args(args),
        };
        if test_args.paths.is_empty() {
            return Err(lines::invalid(problems));
        }
        let plan = Plan::make_for(&test_args, global, None, true)?;
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
        let blocked = blocked(&plan.config.load, plan.backend.base_url.as_deref())?;
        let mut global = global.clone();
        if let Some(url) = &plan.backend.base_url {
            global.base_url = Some(url.clone());
        }
        global.tab = None;
        global.context = None;
        global.json = false;
        global.quiet = false;
        // The journeys' tests and data.
        let picked: Vec<Picked<'_>> = match &profile {
            Some(p) => pick_journeys(&plan, p, &mut problems),
            None => plan
                .tests
                .iter()
                .filter(|t| t.skipped().is_none())
                .map(|test| Picked {
                    test,
                    weight: 1,
                    data: None,
                })
                .collect(),
        };
        if plan.backend.protected && !args.allow_writes {
            crate::test::plan::protect(
                &plan.backend.name,
                &plan.scenarios,
                picked.iter().map(|p| p.test),
            )?;
        }
        let checker = Checker {
            plan: &plan,
            global: &global,
            writes: !plan.backend.protected || args.allow_writes,
        };
        let mut hooks: Vec<Hooks> = plan.scenarios.iter().map(|_| Hooks::default()).collect();
        let mut hooked = vec![false; plan.scenarios.len()];
        let mut journeys = Vec::new();
        for (i, p) in picked.iter().enumerate() {
            let test = p.test;
            let scenario = &plan.scenarios[test.file];
            let saved = crate::test::vars::saved_names(scenario);
            let ready =
                |steps: &[Prepared], hook: bool, rows: Rows<'_>, problems: &mut Vec<Problem>| {
                    steps
                        .iter()
                        .filter_map(|s| checker.step(s, &saved, hook, rows, problems))
                        .collect::<Vec<_>>()
                };
            if !hooked[test.file] {
                hooked[test.file] = true;
                let file = &plan.hooks[test.file];
                // `@before` and `@after` run in each journey's iterations
                // of the file, with its row: the columns they all have.
                let (columns, files) = shared_columns(&picked, test.file);
                let rows = match &columns {
                    Some(columns) => Rows::Columns(columns, &files),
                    None => Rows::None,
                };
                hooks[test.file] = Hooks {
                    before_all: ready(&file.before_all, true, Rows::None, &mut problems),
                    after_all: ready(&file.after_all, true, Rows::None, &mut problems),
                    before: ready(&file.before, true, rows, &mut problems),
                    after: ready(&file.after, true, rows, &mut problems),
                };
            }
            let rows = match &p.data {
                Some((data, written)) => Rows::Columns(&data.columns, written),
                None => Rows::None,
            };
            let mut steps = ready(&test.steps, false, rows, &mut problems);
            if profile.as_ref().is_some_and(|p| p.think_between_steps) {
                steps = think_between(steps);
            }
            // The test runner's plan checked the device's name already.
            let emulate = emulation(&test.settings, &global)?;
            journeys.push(Journey {
                file: test.file,
                name: test.name.clone(),
                place: format!("{}:{}", scenario.name, test.line),
                steps,
                timeout: plan.timeout(&test.settings),
                emulate,
                weight: p.weight,
                data: picked[i].data.as_ref().map(|(data, _)| data.clone()),
            });
        }
        let thresholds = match &profile {
            Some(p) => thresholds(&p.thresholds, &journeys, &hooks, &mut problems),
            None => Vec::new(),
        };
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
        let (model, think, on_failure, max_failures) = match &profile {
            Some(p) => (p.model.clone(), p.think, p.on_failure, p.max_failures),
            None => (
                Model::Closed(closed(args)),
                args.think.unwrap_or_else(default_think),
                OnFailure::Continue,
                profile::MAX_FAILURES,
            ),
        };
        Ok(Self {
            plan,
            journeys,
            hooks,
            global,
            blocked,
            in_turn: profile.is_none(),
            profile: profile.map(|p| p.name),
            model,
            think,
            on_failure,
            max_failures,
            thresholds,
        })
    }
}

/// The profile's thresholds with their groups, which must be the
/// journeys' steps, the journeys, or groups every run has (an endpoint is
/// only known when a request has it).
fn thresholds(
    written: &[(threshold::Written, Span)],
    journeys: &[Journey],
    hooks: &[Hooks],
    problems: &mut Vec<Problem>,
) -> Vec<Threshold> {
    let mut known = threshold::Known::default();
    let hook_steps = hooks
        .iter()
        .flat_map(|h| [&h.before_all, &h.before, &h.after, &h.after_all])
        .flatten();
    for step in journeys.iter().flat_map(|j| &j.steps).chain(hook_steps) {
        let group = super::record::step_group(&step.at);
        if !known.steps.contains(&group) {
            known.steps.push(group);
        }
    }
    known.journeys = journeys
        .iter()
        .map(|j| (j.place.clone(), j.name.clone()))
        .collect();
    let mut thresholds = Vec::new();
    for (w, at) in written {
        match known.resolve(&w.group) {
            Ok(target) => thresholds.push(Threshold {
                written: w.clone(),
                target,
            }),
            Err(e) => problems.push(problem(
                at,
                format!("{}: {e}", w.name),
                "a step is named by its file and line (step checkout.cdp:14), a journey by its \
                 test's name; docs/e2e/load.md#thresholds",
            )),
        }
    }
    thresholds
}

/// The error of `load NAME` when NAME is no profile, file or folder.
fn no_profile(name: &str, root: &Path) -> ErrorInfo {
    let names = profile::names(root);
    let info = ErrorInfo::new(
        ErrorKind::NotFound,
        format!(
            "no load profile {name}: {} is not there",
            Path::new(profile::DIR)
                .join(format!("{name}.toml"))
                .display()
        ),
    );
    let close = protocol::suggest::similar(name, names.iter().map(String::as_str), 3);
    if let Some(first) = close.first() {
        return info.hint(Hint::run(Cmd::new("load").arg(*first)).why("did you mean it?"));
    }
    if names.is_empty() {
        info.hint(Hint::run(Cmd::new("init").arg("load")).why("writes an example profile"))
    } else {
        info.hint(Hint::text(format!("the profiles are {}", names.join(", "))))
    }
}

/// The think time of a run without a profile and `--think`.
fn default_think() -> Think {
    Think::Between {
        min: Duration::from_secs(2),
        max: Duration::from_secs(8),
    }
}

/// A run without a profile: `--users` loop from the start (or over
/// `--ramp-up`) for `--duration`.
fn closed(args: &LoadArgs) -> Users {
    let users = args.users.unwrap_or(USERS);
    let ramp_up = args.ramp_up.unwrap_or_default();
    let mut stages = Vec::new();
    if !ramp_up.is_zero() {
        stages.push(UserStage {
            from: 0,
            to: users,
            duration: ramp_up,
        });
    }
    stages.push(UserStage {
        from: users,
        to: users,
        duration: args.duration.unwrap_or(DURATION),
    });
    Users {
        stages,
        iterations: args.iterations,
    }
}

/// With a profile, the flags that say what a run without one does are
/// errors: the profile says it.
fn flags_without_profile(args: &LoadArgs, profile: &Profile) -> Result<(), ErrorInfo> {
    let given = [
        ("--users", args.users.is_some()),
        ("--duration", args.duration.is_some()),
        ("--ramp-up", args.ramp_up.is_some()),
        ("--iterations", args.iterations.is_some()),
        ("--think", args.think.is_some()),
        ("--grep", args.grep.is_some()),
        ("--tags", args.tags.is_some()),
    ];
    match given.iter().find(|(_, given)| *given) {
        Some((flag, _)) => Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!(
                "{flag} is for a run without a profile; {} sets the run",
                profile.name
            ),
        )
        .hint(Hint::text(format!(
            "change it in {}: [arrivals] or [users], think, [[journeys]]",
            profile.name
        )))),
        None => Ok(()),
    }
}

/// The test runner's arguments for `profile`'s journeys: their files, and
/// the profile's backend unless `--backend`. A file that is not there is
/// a problem.
fn profile_args(args: &LoadArgs, profile: &Profile, problems: &mut Vec<Problem>) -> TestArgs {
    let mut files: Vec<String> = Vec::new();
    for journey in &profile.journeys {
        if !journey.file.is_file() {
            problems.push(problem(
                &journey.at,
                format!(
                    "test = \"{}\": no such scenario file {}",
                    journey.test,
                    journey.file.display()
                ),
                "name a .cdp file relative to the project root, e.g. e2e/scenarios/shop.cdp:search",
            ));
            continue;
        }
        let file = journey.file.display().to_string();
        if !files.contains(&file) {
            files.push(file);
        }
    }
    TestArgs {
        paths: files,
        backend: args.backend.clone().or_else(|| profile.backend.clone()),
        grep: None,
        tags: None,
        ..test_args(args)
    }
}

/// A journey of the run: its test, weight and data (with the file as
/// written).
struct Picked<'a> {
    test: &'a Planned,
    weight: u32,
    data: Option<(Arc<Data>, String)>,
}

/// `profile`'s journeys in `plan`, their data read; what is missing is a
/// problem.
fn pick_journeys<'a>(
    plan: &'a Plan,
    profile: &Profile,
    problems: &mut Vec<Problem>,
) -> Vec<Picked<'a>> {
    let seed = super::seed(&plan.run.run_id);
    let mut picked = Vec::new();
    for (index, journey) in profile.journeys.iter().enumerate() {
        let Some(file) = plan
            .scenarios
            .iter()
            .position(|s| same_file(&s.path, &journey.file))
        else {
            // A file that is not there is a problem already.
            continue;
        };
        let scenario = &plan.scenarios[file];
        let found = match &journey.pick {
            TestPick::Name(name) => scenario.tests.iter().find(|t| &t.name == name),
            TestPick::Line(line) => scenario
                .tests
                .iter()
                .find(|t| (t.span.line..=t.end_line).contains(line)),
        };
        let Some(found) = found else {
            let names: Vec<&str> = scenario.tests.iter().map(|t| t.name.as_str()).collect();
            let hint = match &journey.pick {
                TestPick::Name(name) => {
                    let close = protocol::suggest::similar(name, names.iter().copied(), 3);
                    if close.is_empty() {
                        format!("its tests are {}", quoted(&names))
                    } else {
                        format!("did you mean {}?", quoted(&close))
                    }
                }
                TestPick::Line(_) => format!(
                    "its tests start at lines {}",
                    scenario
                        .tests
                        .iter()
                        .map(|t| t.span.line.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            let what = match &journey.pick {
                TestPick::Name(name) => format!("no test named \"{name}\""),
                TestPick::Line(line) => format!("line {line} is not in a test"),
            };
            problems.push(problem(
                &journey.at,
                format!("test = \"{}\": {what} in {}", journey.test, scenario.name),
                hint,
            ));
            continue;
        };
        let Some(test) = plan
            .tests
            .iter()
            .find(|t| t.file == file && t.line == found.span.line)
        else {
            continue;
        };
        if let Some(why) = test.skipped() {
            problems.push(problem(
                &journey.at,
                format!(
                    "test = \"{}\" is skipped on backend {}: {why}",
                    journey.test, plan.backend.name
                ),
                "a journey runs on the profile's backend; drop @skip or its @backend",
            ));
            continue;
        }
        let data = match &journey.data {
            None => None,
            Some(spec) => match Data::load(spec, seed ^ u64::try_from(index).unwrap_or(0)) {
                Ok(data) => Some((Arc::new(data), spec.written.clone())),
                Err(e) => {
                    problems.push(problem(
                        &spec.at,
                        e,
                        "data is a CSV file with a header line, or a JSON array of objects",
                    ));
                    continue;
                }
            },
        };
        picked.push(Picked {
            test,
            weight: journey.weight,
            data,
        });
    }
    picked
}

/// The columns every journey of file `file` has in its data, and their
/// files; `None` when one of them has no data.
fn shared_columns(picked: &[Picked<'_>], file: usize) -> (Option<Vec<String>>, String) {
    let mut columns: Option<Vec<String>> = None;
    let mut files: Vec<&str> = Vec::new();
    for p in picked.iter().filter(|p| p.test.file == file) {
        let Some((data, written)) = &p.data else {
            return (None, String::new());
        };
        if !files.contains(&written.as_str()) {
            files.push(written);
        }
        columns = Some(match columns {
            None => data.columns.clone(),
            Some(c) => c.into_iter().filter(|c| data.columns.contains(c)).collect(),
        });
    }
    (columns, files.join(" and "))
}

/// `steps` with a bare `think` after each navigation and action that is
/// not followed by one.
fn think_between(steps: Vec<Step>) -> Vec<Step> {
    let mut out: Vec<Step> = Vec::with_capacity(steps.len() * 2);
    let mut steps = steps.into_iter().peekable();
    while let Some(step) = steps.next() {
        let pause =
            step.kind == Kind::Action && steps.peek().is_none_or(|next| next.kind != Kind::Think);
        let at = step.at.clone();
        out.push(step);
        if pause {
            out.push(Step {
                at,
                text: "think (think_between_steps)".into(),
                how: How::Fixed(Call::Think(None)),
                kind: Kind::Think,
            });
        }
    }
    out
}

/// Whether `a` and `b` are the same file.
fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (a.canonicalize(), b.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        )
}

/// `"a", "b"`.
fn quoted(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A problem of the profile at `at`.
fn problem(at: &Span, message: impl Into<String>, hint: impl Into<String>) -> Problem {
    Problem {
        span: at.clone(),
        info: ErrorInfo::new(ErrorKind::BadArgs, message).hint(Hint::text(hint.into())),
    }
}

/// What every user's context blocks: `[load] block`'s own patterns, in
/// order, then the `third-party` preset, which lets the backend's origin
/// and `[load] allow_hosts` through and blocks every other host. Chrome
/// takes the first pattern that matches a request, so a pattern of your
/// own blocks same-origin requests too.
pub(super) fn blocked(
    load: &e2e::config::Load,
    base_url: Option<&str>,
) -> Result<Vec<engine::BlockedUrl>, ErrorInfo> {
    use e2e::config::Block;
    let mut blocked: Vec<engine::BlockedUrl> = load
        .block
        .iter()
        .filter_map(|b| match b {
            Block::Pattern(p) => Some(engine::BlockedUrl {
                pattern: p.clone(),
                block: true,
            }),
            Block::ThirdParty => None,
        })
        .collect();
    if !load.block.contains(&Block::ThirdParty) {
        return Ok(blocked);
    }
    let origin = base_url.and_then(origin).ok_or_else(|| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!(
                "[load] block = [\"{}\"] lets the backend's origin through, and the backend \
                 has no base_url",
                e2e::config::THIRD_PARTY
            ),
        )
        .hint("give the backend a base_url in [e2e.backends.NAME]")
    })?;
    let allow = |pattern: String| engine::BlockedUrl {
        pattern,
        block: false,
    };
    blocked.push(allow(format!("*://{origin}/*")));
    blocked.extend(
        load.allow_hosts
            .iter()
            .map(|host| allow(format!("*://{host}:*/*"))),
    );
    blocked.push(engine::BlockedUrl {
        pattern: "*://*:*/*".into(),
        block: true,
    });
    Ok(blocked)
}

/// `http://localhost:4176/app` → `localhost:4176`: the host and port of an
/// http(s) URL.
fn origin(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = &rest[..end];
    // Credentials are no part of the origin.
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    (!host.is_empty()).then_some(host)
}

/// The `emulate` request of a test's `@device` or `@viewport`, as `test`
/// makes it; `None` without either (Chrome's own viewport).
fn emulation(
    settings: &e2e::scenario::Settings,
    global: &GlobalArgs,
) -> Result<Option<engine::EmulateRequest>, ErrorInfo> {
    let words: Vec<String> = match (&settings.device, settings.viewport) {
        (Some(device), _) => vec!["emulate".into(), device.clone()],
        (None, Some(size)) => vec![
            "viewport".into(),
            size.to_string(),
            "--dpr".into(),
            "1".into(),
        ],
        (None, None) => return Ok(None),
    };
    let call = crate::session::setup_call(&lines::command(&words, global)?)?;
    serde_json::from_value(call.params)
        .map(Some)
        .map_err(|e| ErrorInfo::new(ErrorKind::ProtocolError, format!("bad emulation: {e}")))
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
        rows: Rows<'_>,
        problems: &mut Vec<Problem>,
    ) -> Option<Step> {
        let problem = |info: ErrorInfo| Problem {
            span: step.at.clone(),
            info,
        };
        let checking = Resolver::checking(&step.vars, &self.plan.run, saved).in_load(LoadVars {
            vu: "vu",
            iteration: "iteration",
            rows,
        });
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
        let kind = match &cli.command {
            Some(Command::Think(_)) => Kind::Think,
            Some(
                Command::Goto(_) | Command::Back(_) | Command::Forward(_) | Command::Reload(_),
            ) => Kind::Action,
            Some(command) if matches!(crate::action(command), Ok(Some(_))) => Kind::Action,
            _ => Kind::Other,
        };
        Some(Step {
            at: step.at.to_string(),
            text: step.text.clone(),
            how,
            kind,
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

/// The variables that are the same in every iteration: `${test.id}`,
/// `${saved.…}`, `${vu.id}`, `${iteration.id}` and `${row.…}` are not.
struct Fixed<'a>(Resolver<'a>);

impl Resolve for Fixed<'_> {
    fn resolve(&self, var: &Var) -> Result<String, Unresolved> {
        match var {
            Var::Path(path)
                if matches!(
                    path.first().map(String::as_str),
                    Some("test" | "saved" | "vu" | "iteration" | "row")
                ) =>
            {
                Err(Unresolved::new("changes with each iteration"))
            }
            _ => self.0.resolve(var),
        }
    }
}

#[cfg(test)]
mod tests {
    use e2e::config::{Block, Load};

    use super::{blocked, origin};

    fn patterns(load: &Load, base_url: Option<&str>) -> Vec<(String, bool)> {
        blocked(load, base_url)
            .unwrap()
            .into_iter()
            .map(|b| (b.pattern, b.block))
            .collect()
    }

    #[test]
    fn nothing_is_blocked_by_default() {
        assert!(patterns(&Load::default(), Some("http://localhost:4176")).is_empty());
    }

    #[test]
    fn own_patterns_come_before_the_third_party_preset() {
        let load = Load {
            block: vec![Block::ThirdParty, Block::Pattern("*://*:*/*.woff2".into())],
            allow_hosts: vec!["cdn.example.com".into(), "*.stripe.com".into()],
            ..Load::default()
        };
        assert_eq!(
            patterns(&load, Some("https://shop.example.com/app/")),
            [
                ("*://*:*/*.woff2".to_owned(), true),
                ("*://shop.example.com/*".to_owned(), false),
                ("*://cdn.example.com:*/*".to_owned(), false),
                ("*://*.stripe.com:*/*".to_owned(), false),
                ("*://*:*/*".to_owned(), true),
            ]
        );
    }

    #[test]
    fn the_preset_needs_a_base_url() {
        let load = Load {
            block: vec![Block::ThirdParty],
            ..Load::default()
        };
        let err = blocked(&load, None).unwrap_err();
        assert!(err.message.contains("has no base_url"), "{}", err.message);
    }

    #[test]
    fn origins_are_host_and_port() {
        assert_eq!(origin("http://localhost:4176"), Some("localhost:4176"));
        assert_eq!(origin("https://u:p@shop.com/a?b#c"), Some("shop.com"));
        assert_eq!(origin("ftp://x"), None);
        assert_eq!(origin("http:///x"), None);
    }
}
