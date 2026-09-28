//! `test --watch` (plan 11, milestone 11.7): after a run, wait for a
//! scenario, step, mock or data file (or a file of `[e2e] watch`, such as
//! the web server's build output) to change, and run the tests it affects
//! again, in the same browser session and with the web servers still up,
//! until Ctrl-C.
//!
//! Files are polled (every 250 ms, by modification time and size), so no
//! file-system events are needed and every platform behaves the same.
//! What a change affects:
//! - a scenario file, or a step file one includes: that file's tests;
//! - a mock file: the tests that use its set; a mock data file: every test
//!   with mock sets;
//! - anything else (the config, a `watch` glob's files): every test.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use e2e::scenario::select;
use protocol::{ErrorInfo, Mode};

use super::plan::{Plan, Planned};
use crate::output;

const POLL: Duration = Duration::from_millis(250);
/// How long files must stay unchanged before the run starts (an editor
/// that writes several files, a build that takes a moment).
const SETTLE: Duration = Duration::from_millis(300);

/// A file's modification time and size.
type Stamp = Option<(SystemTime, u64)>;

/// Watches and runs again until `stop` is set; returns the last run's
/// exit code (`code` is the first run's). `make` reads the plan again
/// (every test the command line selects); `rerun` runs a plan's tests and
/// prints their result.
pub(super) fn run(
    first: &Plan,
    mut code: u8,
    stop: &AtomicBool,
    mode: Mode,
    mut rerun: impl FnMut(&Plan) -> u8,
    make: impl Fn() -> Result<Plan, ErrorInfo>,
    explicit_paths: bool,
) -> u8 {
    let mut current: Option<Plan> = None;
    // Files that failed to load are watched too, so fixing them reruns.
    let mut extra: BTreeSet<PathBuf> = BTreeSet::new();
    loop {
        let plan = current.as_ref().unwrap_or(first);
        let mut files = watched(plan, explicit_paths);
        files.extend(extra.iter().cloned());
        // Taken before saying so, so a change right after counts.
        let before = stamps(&files, plan, explicit_paths, &extra);
        if mode == Mode::Text {
            output::stdout(&format!(
                "watching {} files for changes (Ctrl-C stops)\n",
                files.len()
            ));
        }
        let Some(changed) = wait(&before, &files, plan, explicit_paths, &extra, stop) else {
            return code;
        };
        let full = match make() {
            Ok(full) => full,
            Err(info) => {
                // Watch the files the error names, and wait for a fix.
                extra = named_files(&info);
                code = output::print(mode, Err(info));
                continue;
            }
        };
        extra.clear();
        let affected = affected(&full, &changed);
        let Ok(mut run) = make() else {
            continue;
        };
        let mut index = 0;
        run.tests.retain(|_| {
            index += 1;
            affected[index - 1]
        });
        if mode == Mode::Text {
            let names: Vec<String> = changed.iter().map(|p| display(p)).take(3).collect();
            output::stdout(&format!(
                "\n{}{} changed\n",
                names.join(", "),
                if changed.len() > 3 {
                    format!(" and {} more", changed.len() - 3)
                } else {
                    String::new()
                }
            ));
        }
        if run.tests.is_empty() {
            if mode == Mode::Text {
                output::stdout("no test uses it\n");
            }
        } else {
            code = rerun(&run);
        }
        current = Some(full);
        if stop.load(Ordering::SeqCst) {
            return code;
        }
    }
}

fn display(path: &Path) -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    path.strip_prefix(&cwd)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Every file a plan's tests depend on, and those that would add tests.
fn watched(plan: &Plan, explicit_paths: bool) -> BTreeSet<PathBuf> {
    let config = &plan.config;
    let mut files = BTreeSet::new();
    files.insert(config.file.clone());
    for scenario in &plan.scenarios {
        files.insert(absolute(&scenario.path));
    }
    for deps in dependencies(plan).into_values() {
        files.extend(deps);
    }
    if !explicit_paths {
        files.extend(select::files_matching(&config.root, &config.tests));
    }
    files.extend(select::files_under_globs(&config.mocks, &["**".to_owned()]));
    files.extend(select::files_under_globs(&config.root, &config.watch));
    files
}

/// The files `files` and the glob matches stand for now, with their
/// stamps (a file gone has none).
fn stamps(
    files: &BTreeSet<PathBuf>,
    plan: &Plan,
    explicit_paths: bool,
    extra: &BTreeSet<PathBuf>,
) -> BTreeMap<PathBuf, Stamp> {
    let mut all = files.clone();
    // New files that match a glob count as changes too.
    all.extend(watched(plan, explicit_paths));
    all.extend(extra.iter().cloned());
    all.into_iter()
        .map(|path| {
            let stamp = std::fs::metadata(&path)
                .ok()
                .and_then(|m| Some((m.modified().ok()?, m.len())));
            (path, stamp)
        })
        .collect()
}

/// Waits until files change and then stay the same for a moment; the
/// changed files, or `None` once `stop` is set.
fn wait(
    before: &BTreeMap<PathBuf, Stamp>,
    files: &BTreeSet<PathBuf>,
    plan: &Plan,
    explicit_paths: bool,
    extra: &BTreeSet<PathBuf>,
    stop: &AtomicBool,
) -> Option<Vec<PathBuf>> {
    loop {
        if stop.load(Ordering::SeqCst) {
            return None;
        }
        std::thread::sleep(POLL);
        let mut now = stamps(files, plan, explicit_paths, extra);
        if &now == before {
            continue;
        }
        // Until it settles.
        loop {
            std::thread::sleep(SETTLE);
            if stop.load(Ordering::SeqCst) {
                return None;
            }
            let later = stamps(files, plan, explicit_paths, extra);
            if later == now {
                break;
            }
            now = later;
        }
        let paths: BTreeSet<&PathBuf> = before.keys().chain(now.keys()).collect();
        let changed: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| before.get(*p).copied().flatten() != now.get(*p).copied().flatten())
            .cloned()
            .collect();
        if !changed.is_empty() {
            return Some(changed);
        }
    }
}

/// The files each scenario (by index) reads: itself and its `@include`d
/// step files.
fn dependencies(plan: &Plan) -> BTreeMap<usize, BTreeSet<PathBuf>> {
    let mut deps: BTreeMap<usize, BTreeSet<PathBuf>> = BTreeMap::new();
    for (index, scenario) in plan.scenarios.iter().enumerate() {
        deps.entry(index)
            .or_default()
            .insert(absolute(&scenario.path));
    }
    let mut add = |file: usize, steps: &[super::plan::Prepared]| {
        let set = deps.entry(file).or_default();
        for step in steps {
            set.insert(absolute(Path::new(step.at.file.as_ref())));
            for via in &step.via {
                set.insert(absolute(Path::new(via.file.as_ref())));
            }
        }
    };
    for test in &plan.tests {
        add(test.file, &test.steps);
    }
    for (file, hooks) in plan.hooks.iter().enumerate() {
        for steps in [
            &hooks.before_all,
            &hooks.after_all,
            &hooks.before,
            &hooks.after,
        ] {
            add(file, steps);
        }
    }
    deps
}

/// `path` made absolute, with `.` and `..` taken out (without reading
/// links, so it names the file as the scenario did).
fn absolute(path: &Path) -> PathBuf {
    use std::path::Component;
    let joined = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Which of `plan`'s tests the changed files affect.
pub(super) fn affected(plan: &Plan, changed: &[PathBuf]) -> Vec<bool> {
    let deps = dependencies(plan);
    let mut affected = vec![false; plan.tests.len()];
    let mut mark = |pick: &dyn Fn(&Planned) -> bool| {
        for (i, test) in plan.tests.iter().enumerate() {
            if pick(test) {
                affected[i] = true;
            }
        }
    };
    let config = &plan.config;
    let mocks = absolute(&config.mocks);
    for path in changed {
        let path = absolute(path);
        let files: Vec<usize> = deps
            .iter()
            .filter(|(_, d)| d.contains(&path))
            .map(|(f, _)| *f)
            .collect();
        if !files.is_empty() {
            mark(&|t| files.contains(&t.file));
        } else if let Ok(inside) = path.strip_prefix(&mocks) {
            if path.extension().is_some_and(|e| e == "toml") {
                let set = inside
                    .with_extension("")
                    .to_string_lossy()
                    .replace('\\', "/");
                mark(&|t| config.default_mocks.contains(&set) || t.settings.mocks.contains(&set));
            } else {
                mark(&|t| !config.default_mocks.is_empty() || !t.settings.mocks.is_empty());
            }
        } else if path.extension().is_some_and(|e| e == "cdp") && !path.exists() {
            // A scenario (or step file) that is gone: its tests are gone
            // too.
        } else {
            mark(&|_| true);
        }
    }
    affected
}

/// The files an error's `file:line:col` places name.
fn named_files(info: &ErrorInfo) -> BTreeSet<PathBuf> {
    let mut texts = vec![info.message.clone()];
    texts.extend(info.notes.iter().map(|n| n.text.clone()));
    texts
        .iter()
        .filter_map(|t| t.split(':').next())
        .filter(|f| {
            Path::new(f)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("cdp") || e.eq_ignore_ascii_case("toml"))
        })
        .map(|f| absolute(Path::new(f)))
        .collect()
}
