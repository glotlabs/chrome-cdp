//! What a `chrome-cdp test` run does, worked out before a browser starts:
//! the project config, the scenario files, the tests selected, and every
//! step checked against the CLI's commands (plan 11, milestone 11.4).

use std::path::{Path, PathBuf};
use std::time::Duration;

use e2e::Config;
use e2e::config::Fixed;
use e2e::scenario::select::{self, Filter, Selected};
use e2e::scenario::{self, Scenario, Settings, Step};
use protocol::script::{ScriptError, Span};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};

use crate::cli::{GlobalArgs, TestArgs};
use crate::script::{self as lines, Problem};

/// Where `expect-snapshot` and `expect-screenshot` baselines go, relative
/// to the project root.
pub(super) const SNAPSHOTS: &str = "e2e/snapshots";

/// A checked step.
#[derive(Debug, Clone)]
pub(super) struct Prepared {
    /// The words, variables filled in.
    pub(super) words: Vec<String>,
    pub(super) at: Span,
    /// The line as written.
    pub(super) text: String,
    /// The `@include` lines it came through.
    pub(super) via: Vec<Span>,
}

/// A file's hooks, checked.
#[derive(Debug, Default)]
pub(super) struct Hooks {
    pub(super) before_all: Vec<Prepared>,
    pub(super) after_all: Vec<Prepared>,
    pub(super) before: Vec<Prepared>,
    pub(super) after: Vec<Prepared>,
}

/// A test of the run.
#[derive(Debug)]
pub(super) struct Planned {
    /// Index into [`Plan::scenarios`].
    pub(super) file: usize,
    pub(super) name: String,
    pub(super) line: u32,
    /// The file's settings with the test's on top.
    pub(super) settings: Settings,
    pub(super) steps: Vec<Prepared>,
}

impl Planned {
    pub(super) fn skipped(&self) -> Option<&str> {
        self.settings.skip.as_deref()
    }
}

/// Everything a run needs to know.
#[derive(Debug)]
pub(super) struct Plan {
    pub(super) config: Config,
    pub(super) scenarios: Vec<Scenario>,
    /// By file, like [`Plan::scenarios`].
    pub(super) hooks: Vec<Hooks>,
    pub(super) tests: Vec<Planned>,
    /// How many tests run at once.
    pub(super) jobs: u32,
    /// Retries of a test without its own `@retries`.
    pub(super) retries: u32,
    /// `expect-snapshot` baselines' directory.
    pub(super) snapshots: PathBuf,
    /// What every test's context gets before its steps (11.6).
    pub(super) determinism: Fixed,
}

impl Plan {
    /// Reads the config and the scenarios, selects the tests, and checks
    /// them. Every problem of the scenarios comes back at once.
    pub(super) fn make(args: &TestArgs, global: &GlobalArgs) -> Result<Self, ErrorInfo> {
        let cwd = std::env::current_dir().map_err(|e| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("cannot read the current directory: {e}"),
            )
        })?;
        let config = project(&cwd)?;
        let files = if args.paths.is_empty() {
            let found = select::files_matching(&config.root, &config.tests);
            if found.is_empty() {
                return Err(ErrorInfo::new(
                    ErrorKind::NotFound,
                    format!(
                        "no scenario files match {} in {}",
                        config.tests.join(", "),
                        config.root.display()
                    ),
                )
                .hint(
                    Hint::run(Cmd::new("init").arg("e2e"))
                        .why("writes an example")
                        .or("name the .cdp files: chrome-cdp test <path>…"),
                ));
            }
            found.into_iter().map(|f| (f, Vec::new())).collect()
        } else {
            let targets: Vec<_> = args
                .paths
                .iter()
                .map(|p| select::parse_target(p, &cwd))
                .collect();
            select::files_of(&targets)?
        };
        let mut scenarios = Vec::new();
        let mut lines_asked = Vec::new();
        let mut problems: Vec<Problem> = Vec::new();
        for (path, lines) in files {
            let name = scenario::display_name(&path);
            match scenario::load(&path, &name) {
                Ok(s) => {
                    scenarios.push(s);
                    lines_asked.push(lines);
                }
                Err(errors) => problems.extend(errors.into_iter().map(problem)),
            }
        }
        // The other files' steps are checked too, so every problem comes
        // back at once.
        let mut filter = Filter::default();
        if let Some(grep) = &args.grep {
            filter.set_grep(grep)?;
        }
        if let Some(tags) = &args.tags {
            filter.set_tags(tags);
        }
        let mut selected = select::select(&scenarios, &lines_asked, &filter)?;
        if let Some(shard) = &args.shard {
            let (index, total) = select::parse_shard(shard)?;
            selected = select::shard(selected, index, total);
        }
        let mut hooks: Vec<Hooks> = scenarios.iter().map(|_| Hooks::default()).collect();
        let mut hooked = vec![false; scenarios.len()];
        let mut tests = Vec::new();
        for Selected {
            file,
            test,
            settings,
            ..
        } in selected
        {
            let scenario = &scenarios[file];
            if let Some(device) = &settings.device
                && let Err(info) = engine::find_device(device, &global.session)
            {
                problems.push(Problem {
                    span: scenario.tests[test].span.clone(),
                    info,
                });
            }
            let skipped = settings.skip.is_some();
            let steps = if skipped {
                Vec::new()
            } else {
                prepare(&scenario.tests[test].steps, global, &mut problems)
            };
            if !skipped && !hooked[file] {
                hooked[file] = true;
                hooks[file] = Hooks {
                    before_all: prepare(&scenario.before_all, global, &mut problems),
                    after_all: prepare(&scenario.after_all, global, &mut problems),
                    before: prepare(&scenario.before, global, &mut problems),
                    after: prepare(&scenario.after, global, &mut problems),
                };
            }
            let t = &scenario.tests[test];
            tests.push(Planned {
                file,
                name: t.name.clone(),
                line: t.span.line,
                settings,
                steps,
            });
        }
        if !problems.is_empty() {
            problems.sort_by(|a, b| {
                (a.span.file.as_ref(), a.span.start).cmp(&(b.span.file.as_ref(), b.span.start))
            });
            return Err(lines::invalid(problems));
        }
        let jobs = args.jobs.unwrap_or_else(|| config.jobs.count()).max(1);
        let retries = args.retries.unwrap_or(config.retries);
        let snapshots = config.root.join(SNAPSHOTS);
        let determinism = config.determinism.resolve();
        Ok(Self {
            config,
            scenarios,
            hooks,
            tests,
            jobs,
            retries,
            snapshots,
            determinism,
        })
    }

    /// The time limit of a test (or hook) with `settings`.
    pub(super) fn timeout(&self, settings: &Settings) -> Duration {
        settings.timeout.unwrap_or(self.config.timeout)
    }

    /// Test `test`'s retries.
    pub(super) fn retries(&self, test: &Planned) -> u32 {
        test.settings.retries.unwrap_or(self.retries)
    }
}

/// The project's `[e2e]` config; without one, the defaults, rooted at
/// `cwd` (or at the config file that has no `[e2e]`).
fn project(cwd: &Path) -> Result<Config, ErrorInfo> {
    let file = e2e::find_project_file(cwd)?;
    let (text, file) = match file {
        Some(file) => {
            let text = std::fs::read_to_string(&file).map_err(|e| {
                ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!("cannot read {}: {e}", file.display()),
                )
            })?;
            (text, file)
        }
        None => (String::new(), cwd.join(e2e::config::PROJECT_FILES[0])),
    };
    if let Some(config) = Config::parse(&text, &file)? {
        return Ok(config);
    }
    Config::parse("[e2e]\n", &file)?
        .ok_or_else(|| ErrorInfo::new(ErrorKind::BadArgs, "the default [e2e] config did not parse"))
}

fn problem(e: ScriptError) -> Problem {
    let span = e.span.clone();
    let info = ErrorInfo::new(ErrorKind::BadArgs, e.message);
    Problem {
        span,
        info: match e.hint {
            Some(hint) => info.hint(hint),
            None => info,
        },
    }
}

/// Checks `steps` as `run` checks a script's lines; problems go into
/// `problems`.
fn prepare(steps: &[Step], global: &GlobalArgs, problems: &mut Vec<Problem>) -> Vec<Prepared> {
    let mut prepared = Vec::new();
    for step in steps {
        match lines::step(&step.line, step.vars.as_ref(), global) {
            Ok(checked) => prepared.push(Prepared {
                words: checked.words,
                at: checked.span,
                text: checked.text,
                via: step.via.clone(),
            }),
            Err(p) => problems.push(p),
        }
    }
    prepared
}
