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

use super::durations::{self, Durations};
use super::vars::{LoadVars, Resolver, Rows, RunVars};
use crate::cli::{GlobalArgs, TestArgs};
use crate::script::{self as lines, Problem};

/// Where `expect-snapshot` and `expect-screenshot` baselines go, relative
/// to the project root.
pub(crate) const SNAPSHOTS: &str = "e2e/snapshots";

/// The folder whose `audit/` holds `expect --audit` baselines
/// (`e2e/audit/<name>.json`), relative to the project root.
pub(crate) const AUDITS: &str = "e2e";

/// A checked step.
#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    /// The line; the runner fills its variables in as it runs
    /// ([`super::vars`]), since `${test.id}` and `${saved.…}` change.
    pub(crate) line: Line,
    pub(crate) vars: Arc<Vars>,
    pub(crate) at: Span,
    /// The line as written.
    pub(crate) text: String,
    /// The `@include` lines it came through.
    pub(crate) via: Vec<Span>,
}

/// A file's hooks, checked.
#[derive(Debug, Default)]
pub(crate) struct Hooks {
    pub(crate) before_all: Vec<Prepared>,
    pub(crate) after_all: Vec<Prepared>,
    pub(crate) before: Vec<Prepared>,
    pub(crate) after: Vec<Prepared>,
}

/// A test of the run.
#[derive(Debug)]
pub(crate) struct Planned {
    /// Index into [`Plan::scenarios`].
    pub(crate) file: usize,
    pub(crate) name: String,
    pub(crate) line: u32,
    /// The file's settings with the test's on top.
    pub(crate) settings: Settings,
    pub(crate) steps: Vec<Prepared>,
    /// Its artifacts' directory: `<artifacts>/<file>/<name as a slug>`.
    pub(crate) dir: PathBuf,
}

impl Planned {
    pub(crate) fn skipped(&self) -> Option<&str> {
        self.settings.skip.as_deref()
    }
}

/// Everything a run needs to know.
#[derive(Debug)]
pub(crate) struct Plan {
    pub(crate) config: Config,
    pub(crate) scenarios: Vec<Scenario>,
    /// By file, like [`Plan::scenarios`].
    pub(crate) hooks: Vec<Hooks>,
    pub(crate) tests: Vec<Planned>,
    /// How many tests run at once.
    pub(crate) jobs: u32,
    /// Why that many, when the run chose it (`auto`): `16 CPUs, 30 GB
    /// free`.
    pub(crate) jobs_why: Option<String>,
    /// Retries of a test without its own `@retries`.
    pub(crate) retries: u32,
    /// How many times each test runs (`--repeat`; 1 without).
    pub(crate) repeat: u32,
    /// `--debug`: a failure stops the run, so retries are off.
    pub(crate) debug: bool,
    /// What every test's context gets before its steps (11.6).
    pub(crate) determinism: Fixed,
    /// The backend the run uses (11.11).
    pub(crate) backend: Backend,
    /// `${var:…}`, `${run.id}` and the backend's name.
    pub(crate) run: RunVars,
    /// `--allow-writes`: `http` writes on a protected backend.
    pub(crate) allow_writes: bool,
    /// The tests' durations and last statuses (plan 22.5).
    pub(super) durations: Durations,
    /// `--failed-first`: the tests that failed last time start first.
    pub(crate) failed_first: bool,
    /// `--shard` split by the durations: each shard's expected time.
    pub(crate) shards: Option<Shards>,
}

/// The shards of a `--shard` split by time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shards {
    /// The run's, from 1.
    pub(crate) index: usize,
    /// Each shard's tests and their expected time, in ms.
    pub(crate) expected: Vec<(usize, u64)>,
}

impl Plan {
    /// Reads the config and the scenarios, selects the tests for backend
    /// `backend` (`None`: `--backend`, else the config's default), and
    /// checks them. Every problem of the scenarios comes back at once.
    pub(crate) fn make(
        args: &TestArgs,
        global: &GlobalArgs,
        backend: Option<&str>,
    ) -> Result<Self, ErrorInfo> {
        Self::make_for(args, global, backend, false)
    }

    /// [`Plan::make`]; `load` for a load run's plan, which checks the
    /// steps' `${row.…}` against the journeys' data itself.
    pub(crate) fn make_for(
        args: &TestArgs,
        global: &GlobalArgs,
        backend: Option<&str>,
        load: bool,
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
        let durations = Durations::load(&config.durations, &backend.name);
        let mut shards = None;
        if let Some(shard) = &args.shard {
            let (index, total) = select::parse_shard(shard)?;
            (selected, shards) = shard_by_time(selected, index, total, |s| {
                let scenario = &scenarios[s.file];
                let file = durations::key(&config.root, &scenario.path);
                durations.ms(&file, &scenario.tests[s.test].name)
            });
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
                prepare(steps, global, problems, &run, &saved[file], load)
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
        let picked = args.jobs.map_or_else(
            || backend.jobs.pick(),
            |count| e2e::config::Picked { count, why: None },
        );
        let jobs = picked.count.max(1);
        let retries = args.retries.unwrap_or(backend.retries);
        let repeat = args.repeat.unwrap_or(1);
        let determinism = backend.determinism.resolve();
        Ok(Self {
            config,
            scenarios,
            hooks,
            tests,
            jobs,
            jobs_why: picked.why,
            retries,
            repeat,
            debug: args.debug,
            determinism,
            backend,
            run,
            allow_writes: args.allow_writes,
            durations,
            failed_first: args.failed_first,
            shards,
        })
    }

    /// The order the tests start in, as indexes into [`Plan::tests`]: with
    /// `--failed-first` those that failed last time first, then the
    /// longest first by the durations file, those it has no time for
    /// first of all (they may be long); skipped tests last. Ties keep the
    /// plan's order, so without durations it is the plan's order.
    pub(crate) fn order(&self) -> Vec<usize> {
        let root = &self.config.root;
        let tests: Vec<Known> = self
            .tests
            .iter()
            .map(|test| {
                let file = durations::key(root, &self.scenarios[test.file].path);
                Known {
                    skipped: test.skipped().is_some(),
                    failed: self.failed_first && self.durations.failed(&file, &test.name),
                    ms: self.durations.ms(&file, &test.name),
                }
            })
            .collect();
        order(&tests)
    }

    /// The time limit of a test (or hook) with `settings`.
    pub(crate) fn timeout(&self, settings: &Settings) -> Duration {
        settings.timeout.unwrap_or(self.backend.timeout)
    }

    /// Whether the run is against a real backend.
    pub(crate) fn real(&self) -> bool {
        self.backend.mode == Mode::Real
    }

    /// The mock sets a test with `settings` gets in mock mode: the
    /// defaults, then its `@mocks` (a set named again moves last).
    pub(crate) fn mock_mode_sets(&self, settings: &Settings) -> Vec<String> {
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
    pub(crate) fn retries(&self, test: &Planned) -> u32 {
        if self.repeat > 1 || self.debug {
            return 0;
        }
        test.settings.retries.unwrap_or(self.retries)
    }
}

/// What the run order goes by of a test.
#[derive(Debug, Clone, Copy, Default)]
struct Known {
    skipped: bool,
    /// It failed last time, and the run has `--failed-first`.
    failed: bool,
    /// Its average duration.
    ms: Option<u64>,
}

/// [`Plan::order`] of `tests`.
fn order(tests: &[Known]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..tests.len()).collect();
    order.sort_by_key(|&i| {
        let t = tests[i];
        (
            t.skipped,
            !t.failed,
            t.ms.is_some(),
            std::cmp::Reverse(t.ms.unwrap_or(0)),
        )
    });
    order
}

/// Shard `index` (from 1) of `total` of `selected`, split by time when
/// `ms` knows how long one of them takes ([`select::pack`]), else by
/// count as without durations. A test it has no time for counts as the
/// mean of those it has, a skipped one as nothing. The tests keep their
/// order.
fn shard_by_time(
    selected: Vec<Selected>,
    index: usize,
    total: usize,
    ms: impl Fn(&Selected) -> Option<u64>,
) -> (Vec<Selected>, Option<Shards>) {
    let known: Vec<Option<u64>> = selected
        .iter()
        .map(|s| {
            if s.skipped().is_some() {
                Some(0)
            } else {
                ms(s)
            }
        })
        .collect();
    let times: Vec<u64> = selected
        .iter()
        .zip(&known)
        .filter(|(s, _)| s.skipped().is_none())
        .filter_map(|(_, ms)| *ms)
        .collect();
    if times.is_empty() {
        return (select::shard(selected, index, total), None);
    }
    let mean = times.iter().sum::<u64>() / times.len() as u64;
    let weights: Vec<u64> = known.iter().map(|ms| ms.unwrap_or(mean)).collect();
    let packed = select::pack(&weights, total);
    let mut expected = vec![(0, 0u64); total];
    for (&shard, &weight) in packed.iter().zip(&weights) {
        expected[shard].0 += 1;
        expected[shard].1 += weight;
    }
    let mine = selected
        .into_iter()
        .zip(packed)
        .filter(|(_, shard)| *shard == index - 1)
        .map(|(s, _)| s)
        .collect();
    (mine, Some(Shards { index, expected }))
}

/// A scenario's artifacts directory: `<artifacts>/<file>`, the file
/// relative to the folder its `tests` glob starts in (`checkout.cdp` for
/// `e2e/scenarios/checkout.cdp` and the default glob), else to the
/// project root.
pub(crate) fn file_dir(config: &Config, scenario: &Scenario) -> PathBuf {
    config.artifacts.join(file_rel(config, scenario))
}

/// A scenario's path relative to the folder its `tests` glob starts in,
/// else to the project root.
pub(crate) fn file_rel(config: &Config, scenario: &Scenario) -> PathBuf {
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
pub(crate) fn project(cwd: &Path) -> Result<Config, ErrorInfo> {
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
    load: bool,
) -> Vec<Prepared> {
    let mut prepared = Vec::new();
    for step in steps {
        let mut resolver = Resolver::checking(&step.vars, run, saved);
        if load {
            resolver = resolver.in_load(LoadVars {
                vu: "vu",
                iteration: "iteration",
                rows: Rows::Any,
            });
        }
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

#[cfg(test)]
mod tests {
    use e2e::scenario::Settings;
    use e2e::scenario::select::Selected;

    use super::{Known, Shards, order, shard_by_time};

    fn known(ms: Option<u64>) -> Known {
        Known {
            ms,
            ..Known::default()
        }
    }

    #[test]
    fn the_longest_tests_start_first() {
        // Unknown ones first (they may be long), then by time; ties and
        // tests without times keep the plan's order.
        let tests = [
            known(Some(100)),
            known(None),
            known(Some(900)),
            known(Some(100)),
            known(None),
            known(Some(500)),
        ];
        assert_eq!(order(&tests), [1, 4, 2, 5, 0, 3]);
        // Without durations, the plan's order.
        assert_eq!(order(&[known(None); 4]), [0, 1, 2, 3]);
        // --failed-first: last time's failures before all; skipped last.
        let mut tests = tests;
        tests[3].failed = true;
        tests[2].skipped = true;
        assert_eq!(order(&tests), [3, 1, 4, 5, 0, 2]);
    }

    fn selected(n: usize) -> Vec<Selected> {
        (0..n)
            .map(|test| Selected {
                file: 0,
                test,
                settings: Settings::default(),
                title: format!("t{test}"),
            })
            .collect()
    }

    fn tests(part: &[Selected]) -> Vec<usize> {
        part.iter().map(|s| s.test).collect()
    }

    #[test]
    fn shards_split_by_time_once_times_are_known() {
        let times = [Some(900), Some(100), Some(500), None, Some(400), Some(100)];
        let ms = |s: &Selected| times[s.test];
        // The unknown test counts as the mean of the others (400).
        let (first, shards) = shard_by_time(selected(6), 1, 2, ms);
        let (second, _) = shard_by_time(selected(6), 2, 2, ms);
        assert_eq!(tests(&first), [0, 4]);
        assert_eq!(tests(&second), [1, 2, 3, 5]);
        assert_eq!(
            shards,
            Some(Shards {
                index: 1,
                expected: vec![(2, 1300), (4, 1100)],
            })
        );
        // A skipped test counts as nothing, nor in the mean (now 275).
        let mut skipping = selected(6);
        skipping[0].settings.skip = Some(String::new());
        let (_, shards) = shard_by_time(skipping, 1, 2, ms);
        let expected = shards.unwrap().expected;
        assert_eq!(expected.iter().map(|e| e.1).sum::<u64>(), 1375);
        // With no time known, by count as before.
        let (first, shards) = shard_by_time(selected(6), 1, 2, |_| None);
        assert_eq!((tests(&first), shards), (vec![0, 1, 2], None));
    }
}
