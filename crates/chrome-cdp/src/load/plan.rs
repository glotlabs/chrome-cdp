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
    /// The test's `@device` or `@viewport`, made in each iteration's
    /// context before its steps.
    pub(super) emulate: Option<engine::EmulateRequest>,
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
        let blocked = blocked(&plan.config.load, plan.backend.base_url.as_deref())?;
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
            // The test runner's plan checked the device's name already.
            let emulate = emulation(&test.settings, &global)?;
            journeys.push(Journey {
                file: test.file,
                name: test.name.clone(),
                place: format!("{}:{}", scenario.name, test.line),
                steps: ready(&test.steps, false, &mut problems),
                timeout: plan.timeout(&test.settings),
                emulate,
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
            blocked,
        })
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
