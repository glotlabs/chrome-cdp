//! What a `chrome-cdp test` run does, worked out before a browser starts:
//! the project config, the backend (plan 11.11), the scenario files, the
//! tests selected, and every step checked against the CLI's commands
//! (plan 11, milestone 11.4).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use e2e::Config;
use e2e::backend::{Mode, Selected as Backend};
use e2e::config::Fixed;
use e2e::scenario::select::{self, Filter, Selected};
use e2e::scenario::{self, Scenario, Settings, Step, Vars};
use protocol::script::{Line, ScriptError, Span};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};

use super::vars::{Resolver, RunVars};
use crate::cli::{GlobalArgs, TestArgs};
use crate::script::{self as lines, Problem};

/// Where `expect-snapshot` and `expect-screenshot` baselines go, relative
/// to the project root.
pub(super) const SNAPSHOTS: &str = "e2e/snapshots";

/// A checked step.
#[derive(Debug, Clone)]
pub(super) struct Prepared {
    /// The line; the runner fills its variables in as it runs
    /// ([`super::vars`]), since `${test.id}` and `${saved.…}` change.
    pub(super) line: Line,
    pub(super) vars: Arc<Vars>,
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
    /// Its artifacts' directory: `<artifacts>/<file>/<name as a slug>`.
    pub(super) dir: PathBuf,
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
    /// How many times each test runs (`--repeat`; 1 without).
    pub(super) repeat: u32,
    /// `--debug`: a failure stops the run, so retries are off.
    pub(super) debug: bool,
    /// `expect-snapshot` baselines' directory.
    pub(super) snapshots: PathBuf,
    /// What every test's context gets before its steps (11.6).
    pub(super) determinism: Fixed,
    /// The backend the run uses (11.11).
    pub(super) backend: Backend,
    /// `${var:…}`, `${run.id}` and the backend's name.
    pub(super) run: RunVars,
    /// `--allow-writes`: `http` writes on a protected backend.
    pub(super) allow_writes: bool,
}

impl Plan {
    /// Reads the config and the scenarios, selects the tests for backend
    /// `backend` (`None`: `--backend`, else the config's default), and
    /// checks them. Every problem of the scenarios comes back at once.
    pub(super) fn make(
        args: &TestArgs,
        global: &GlobalArgs,
        backend: Option<&str>,
    ) -> Result<Self, ErrorInfo> {
        let cwd = std::env::current_dir().map_err(|e| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("cannot read the current directory: {e}"),
            )
        })?;
        let mut config = project(&cwd)?;
        let name = backend
            .or(args.backend.as_deref())
            .unwrap_or(&config.default_backend)
            .to_owned();
        let backend = config.backend(&name)?;
        if backend.mode == Mode::Real && args.update_snapshots {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("--update-snapshots with backend {name}: baselines come from mock runs"),
            )
            .hint(Hint::text(
                "update them against the mocks (--backend mock), whose data does not change",
            )));
        }
        if backend.name != e2e::config::MOCK_BACKEND
            && args.backend.as_deref() == Some(e2e::backend::ALL)
        {
            config.artifacts = config.artifacts.join(&backend.name);
        }
        let run = RunVars::new(&name, e2e::backend::Vars::load(&backend.vars)?);
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
        let saved: Vec<BTreeSet<String>> = scenarios.iter().map(super::vars::saved_names).collect();
        let known = config.backend_names();
        let mut tests = Vec::new();
        for Selected {
            file,
            test,
            mut settings,
            ..
        } in selected
        {
            let scenario = &scenarios[file];
            if let Some(word) = settings
                .backend
                .iter()
                .find(|b| !matches!(b.as_str(), "mock" | "real") && !known.contains(b))
            {
                problems.push(Problem {
                    span: scenario.tests[test].span.clone(),
                    info: ErrorInfo::new(
                        ErrorKind::BadArgs,
                        format!(
                            "@backend {word}: no such backend; the backends are {}",
                            known.join(", ")
                        ),
                    )
                    .hint("write @backend mock, @backend real, or a backend's name"),
                });
            }
            if settings.skip.is_none() {
                settings.skip = settings.skipped_on(&backend.name, backend.mode.as_str());
            }
            let check = |steps: &[Step], problems: &mut Vec<Problem>| {
                prepare(steps, global, problems, &run, &saved[file])
            };
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
                check(&scenario.tests[test].steps, &mut problems)
            };
            if !skipped && !hooked[file] {
                hooked[file] = true;
                hooks[file] = Hooks {
                    before_all: check(&scenario.before_all, &mut problems),
                    after_all: check(&scenario.after_all, &mut problems),
                    before: check(&scenario.before, &mut problems),
                    after: check(&scenario.after, &mut problems),
                };
            }
            let t = &scenario.tests[test];
            let dir = test_dir(&config, scenario, test);
            tests.push(Planned {
                file,
                name: t.name.clone(),
                line: t.span.line,
                settings,
                steps,
                dir,
            });
        }
        if !problems.is_empty() {
            problems.sort_by(|a, b| {
                (a.span.file.as_ref(), a.span.start).cmp(&(b.span.file.as_ref(), b.span.start))
            });
            return Err(lines::invalid(problems));
        }
        if backend.protected && !args.allow_writes {
            protect(&backend.name, &scenarios, &tests)?;
        }
        let jobs = args.jobs.unwrap_or_else(|| backend.jobs.count()).max(1);
        let retries = args.retries.unwrap_or(backend.retries);
        let repeat = args.repeat.unwrap_or(1);
        let snapshots = config.root.join(SNAPSHOTS);
        let determinism = backend.determinism.resolve();
        Ok(Self {
            config,
            scenarios,
            hooks,
            tests,
            jobs,
            retries,
            repeat,
            debug: args.debug,
            snapshots,
            determinism,
            backend,
            run,
            allow_writes: args.allow_writes,
        })
    }

    /// The time limit of a test (or hook) with `settings`.
    pub(super) fn timeout(&self, settings: &Settings) -> Duration {
        settings.timeout.unwrap_or(self.backend.timeout)
    }

    /// Whether the run is against a real backend.
    pub(super) fn real(&self) -> bool {
        self.backend.mode == Mode::Real
    }

    /// The mock sets a test with `settings` gets in mock mode: the
    /// defaults, then its `@mocks` (a set named again moves last).
    pub(super) fn mock_mode_sets(&self, settings: &Settings) -> Vec<String> {
        let mut sets = self.config.default_mocks.clone();
        for name in &settings.mocks {
            sets.retain(|s| s != name);
            sets.push(name.clone());
        }
        sets
    }

    /// Test `test`'s retries: none with `--repeat`, whose point is to see
    /// the failures retries would hide, nor with `--debug`, which keeps
    /// the first failure.
    pub(super) fn retries(&self, test: &Planned) -> u32 {
        if self.repeat > 1 || self.debug {
            return 0;
        }
        test.settings.retries.unwrap_or(self.retries)
    }
}

/// A scenario's artifacts directory: `<artifacts>/<file>`, the file
/// relative to the folder its `tests` glob starts in (`checkout.cdp` for
/// `e2e/scenarios/checkout.cdp` and the default glob), else to the
/// project root.
pub(super) fn file_dir(config: &Config, scenario: &Scenario) -> PathBuf {
    config.artifacts.join(file_rel(config, scenario))
}

/// A scenario's path relative to the folder its `tests` glob starts in,
/// else to the project root.
pub(super) fn file_rel(config: &Config, scenario: &Scenario) -> PathBuf {
    let path = scenario
        .path
        .strip_prefix(&config.root)
        .unwrap_or(&scenario.path);
    for glob in &config.tests {
        let base: PathBuf = Path::new(glob)
            .components()
            .take_while(|c| {
                !c.as_os_str()
                    .to_string_lossy()
                    .contains(['*', '?', '[', '{'])
            })
            .collect();
        if base != Path::new(glob)
            && let Ok(rest) = path.strip_prefix(&base)
        {
            return rest.to_path_buf();
        }
    }
    path.to_path_buf()
}

/// Test `index`'s artifacts directory: its name as a slug, with its line
/// when another test of the file has the same slug.
fn test_dir(config: &Config, scenario: &Scenario, index: usize) -> PathBuf {
    let test = &scenario.tests[index];
    let slug = super::artifacts::slug(&test.name);
    let shared = scenario
        .tests
        .iter()
        .enumerate()
        .any(|(i, t)| i != index && super::artifacts::slug(&t.name) == slug);
    let name = if slug.is_empty() || shared {
        format!(
            "{}{}{}",
            slug,
            if slug.is_empty() { "test-" } else { "-" },
            test.span.line
        )
    } else {
        slug
    };
    file_dir(config, scenario).join(name)
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

/// Checks `steps` as `run` checks a script's lines, with the run's
/// variables and the names the file saves; problems go into `problems`.
fn prepare(
    steps: &[Step],
    global: &GlobalArgs,
    problems: &mut Vec<Problem>,
    run: &RunVars,
    saved: &BTreeSet<String>,
) -> Vec<Prepared> {
    let mut prepared = Vec::new();
    for step in steps {
        let resolver = Resolver::checking(&step.vars, run, saved);
        match lines::step(&step.line, &resolver, global) {
            Ok(checked) => prepared.push(Prepared {
                line: step.line.clone(),
                vars: step.vars.clone(),
                at: checked.span,
                text: checked.text,
                via: step.via.clone(),
            }),
            Err(p) => problems.push(p),
        }
    }
    prepared
}

/// A protected backend runs only `@safe` tests, unless `--allow-writes`.
fn protect(name: &str, scenarios: &[Scenario], tests: &[Planned]) -> Result<(), ErrorInfo> {
    let unsafe_tests: Vec<String> = tests
        .iter()
        .filter(|t| t.skipped().is_none() && !t.settings.safe)
        .map(|t| format!("{}:{} {}", scenarios[t.file].name, t.line, t.name))
        .collect();
    let Some(first) = unsafe_tests.first() else {
        return Ok(());
    };
    let mut info = ErrorInfo::new(
        ErrorKind::BadArgs,
        format!(
            "backend {name} is protected, and {} of the run {} not marked @safe",
            unsafe_tests.len(),
            if unsafe_tests.len() == 1 {
                "test is"
            } else {
                "tests are"
            }
        ),
    )
    .note("first", first.clone());
    if unsafe_tests.len() > 1 {
        info = info.note("and", format!("{} more", unsafe_tests.len() - 1));
    }
    Err(info
        .hint(Hint::text(
            "mark read-only tests @safe (or pick them with --tags); --allow-writes runs the rest too",
        ))
        .details(serde_json::json!({ "unsafe": unsafe_tests })))
}
