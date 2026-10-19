//! The durations file (plan 22.5): for each test, by backend, file and
//! name, a moving average of its passing durations and how its last run
//! ended. The runner reads it to start the longest tests first, to split
//! `--shard` by time, and for `--failed-first`; every run updates it.
//!
//! It is `.durations.json` beside the artifacts directory (`[e2e]
//! durations` names another), meant to be cached in CI rather than
//! committed: it changes on every run. A file that does not parse is a
//! warning, and the run writes a new one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::exec::{Outcome, Status};
use super::plan::Plan;

/// The file's format version.
const VERSION: u32 = 1;

/// How many passing runs the average spans: each new one weighs 1/5, so
/// a test that got slower shows it within a few runs, and one slow run
/// does not move it much.
const WINDOW: u32 = 5;

/// How a test's last run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Last {
    Passed,
    Failed,
    Flaky,
}

/// What the file keeps of a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Entry {
    /// The moving average of its passing durations; `None` until it
    /// passes once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) ms: Option<u64>,
    /// How many passing runs the average has, at most [`WINDOW`].
    #[serde(default)]
    pub(super) runs: u32,
    pub(super) last: Last,
}

/// Tests by file (relative to the project root, with `/`), then name.
type Tests = BTreeMap<String, BTreeMap<String, Entry>>;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct File {
    version: u32,
    /// By backend: a real backend's tests take other times than the
    /// mocks'.
    backends: BTreeMap<String, Tests>,
}

/// The durations file as the run read it.
#[derive(Debug, Default)]
pub(super) struct Durations {
    path: PathBuf,
    backend: String,
    file: File,
    /// Why the file could not be read, when it could not.
    pub(super) warning: Option<String>,
}

impl Durations {
    /// Reads `path` for `backend`'s tests; a missing file is an empty one.
    pub(super) fn load(path: &Path, backend: &str) -> Self {
        let mut durations = Self {
            path: path.to_path_buf(),
            backend: backend.to_owned(),
            ..Self::default()
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return durations,
            Err(e) => {
                durations.warning = Some(format!("cannot read {}: {e}", path.display()));
                return durations;
            }
        };
        match serde_json::from_str::<File>(&text) {
            Ok(file) if file.version == VERSION => durations.file = file,
            Ok(file) => {
                durations.warning = Some(format!(
                    "{} has version {}, not {VERSION}; the run writes a new one",
                    path.display(),
                    file.version
                ));
            }
            Err(e) => {
                durations.warning = Some(format!(
                    "{} is not a durations file ({e}); the run writes a new one",
                    path.display()
                ));
            }
        }
        durations
    }

    fn tests(&self) -> Option<&Tests> {
        self.file.backends.get(&self.backend)
    }

    /// Whether the file has any test of the backend.
    pub(super) fn known(&self) -> bool {
        self.tests().is_some_and(|tests| !tests.is_empty())
    }

    /// The file's path.
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// What the file has of test `name` of `file` (a [`key`]).
    pub(super) fn get(&self, file: &str, name: &str) -> Option<&Entry> {
        self.tests()?.get(file)?.get(name)
    }

    /// The average passing duration of test `name` of `file`.
    pub(super) fn ms(&self, file: &str, name: &str) -> Option<u64> {
        self.get(file, name)?.ms
    }

    /// Whether test `name` of `file` failed its last run.
    pub(super) fn failed(&self, file: &str, name: &str) -> bool {
        self.get(file, name).is_some_and(|e| e.last == Last::Failed)
    }

    /// Records how `plan`'s tests ended, drops the tests that no longer
    /// exist, and writes the file; an error says why it could not.
    pub(super) fn record(&self, plan: &Plan, outcomes: &[Outcome]) -> Result<(), String> {
        let root = &plan.config.root;
        let mut file = self.file.clone();
        let tests = file.backends.entry(self.backend.clone()).or_default();
        for (test, outcome) in plan.tests.iter().zip(outcomes) {
            let file = key(root, &plan.scenarios[test.file].path);
            let entry = tests.entry(file).or_default().entry(test.name.clone());
            update(entry, outcome.status, outcome.duration);
        }
        // Gone: a file that is not there, and a test that a file the run
        // read no longer has.
        let read: BTreeMap<String, Vec<&str>> = plan
            .scenarios
            .iter()
            .map(|s| {
                let names = s.tests.iter().map(|t| t.name.as_str()).collect();
                (key(root, &s.path), names)
            })
            .collect();
        prune(&mut file.backends, root, &read);
        file.version = VERSION;
        self.write(&file)
    }

    /// Writes the file whole, through a temporary file beside it, so a
    /// run that reads it meanwhile sees the old one or the new one.
    fn write(&self, file: &File) -> Result<(), String> {
        let fail = |e: std::io::Error| format!("cannot write {}: {e}", self.path.display());
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        let mut text = serde_json::to_string_pretty(file).unwrap_or_default();
        text.push('\n');
        let mut temp = self.path.clone().into_os_string();
        temp.push(format!(".{}.tmp", std::process::id()));
        let temp = PathBuf::from(temp);
        std::fs::write(&temp, text).map_err(fail)?;
        std::fs::rename(&temp, &self.path).map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            fail(e)
        })
    }
}

/// Drops the tests that no longer exist, in every backend: those of a
/// file that is not there, and those a file the run `read` (its tests'
/// names by key) no longer has.
fn prune(backends: &mut BTreeMap<String, Tests>, root: &Path, read: &BTreeMap<String, Vec<&str>>) {
    for tests in backends.values_mut() {
        tests.retain(|file, names| {
            match read.get(file) {
                Some(exists) => names.retain(|name, _| exists.contains(&name.as_str())),
                None if !root.join(file).is_file() => names.clear(),
                None => {}
            }
            !names.is_empty()
        });
    }
    backends.retain(|_, tests| !tests.is_empty());
}

/// A test's entry after a run that ended `status` in `duration`: a pass
/// (or a flaky pass) moves the average, a failure only the last status,
/// and a test that did not run is left as it was.
fn update(
    entry: std::collections::btree_map::Entry<'_, String, Entry>,
    status: Status,
    duration: Duration,
) {
    let last = match status {
        Status::Passed => Last::Passed,
        Status::Flaky => Last::Flaky,
        Status::Failed => Last::Failed,
        Status::Skipped | Status::NotRun => return,
    };
    let entry = entry.or_insert(Entry {
        ms: None,
        runs: 0,
        last,
    });
    entry.last = last;
    if last == Last::Failed {
        return;
    }
    let ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    let runs = (entry.runs + 1).min(WINDOW);
    entry.ms = Some(match entry.ms {
        None => ms,
        Some(avg) => {
            // avg + (ms - avg) / runs, in integers.
            let (avg, ms, runs) = (i128::from(avg), i128::from(ms), i128::from(runs));
            u64::try_from(avg + (ms - avg) / runs).unwrap_or(0)
        }
    });
    entry.runs = runs;
}

/// A scenario file's key: its path relative to the project `root`, with
/// `/` between folders, so a file cached on one OS reads on another.
pub(crate) fn key(root: &Path, path: &Path) -> String {
    let Ok(relative) = path.strip_prefix(root) else {
        return path.display().to_string();
    };
    relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(tests: &mut BTreeMap<String, Entry>, status: Status, ms: u64) -> Entry {
        update(
            tests.entry("t".to_owned()),
            status,
            Duration::from_millis(ms),
        );
        tests["t"]
    }

    #[test]
    fn the_average_moves_with_passing_runs_only() {
        let mut tests = BTreeMap::new();
        // A first failure: a status, no time yet.
        let e = entry(&mut tests, Status::Failed, 900);
        assert_eq!((e.ms, e.runs, e.last), (None, 0, Last::Failed));
        let e = entry(&mut tests, Status::Passed, 1000);
        assert_eq!((e.ms, e.runs, e.last), (Some(1000), 1, Last::Passed));
        // The mean of the first runs…
        let e = entry(&mut tests, Status::Flaky, 2000);
        assert_eq!((e.ms, e.runs, e.last), (Some(1500), 2, Last::Flaky));
        // …a failure keeps the time…
        let e = entry(&mut tests, Status::Failed, 30_000);
        assert_eq!((e.ms, e.runs, e.last), (Some(1500), 2, Last::Failed));
        // …and a skipped or stopped test changes nothing.
        assert_eq!(entry(&mut tests, Status::Skipped, 0), e);
        assert_eq!(entry(&mut tests, Status::NotRun, 0), e);
        // After WINDOW runs, each new one weighs 1/WINDOW.
        for _ in 0..10 {
            entry(&mut tests, Status::Passed, 1500);
        }
        let e = entry(&mut tests, Status::Passed, 2500);
        assert_eq!((e.ms, e.runs), (Some(1700), WINDOW));
        let e = entry(&mut tests, Status::Passed, 0);
        assert_eq!(e.ms, Some(1360));
    }

    #[test]
    fn tests_that_no_longer_exist_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for file in ["read.cdp", "unread.cdp"] {
            std::fs::write(root.join(file), "").unwrap();
        }
        let e = Entry {
            ms: Some(1),
            runs: 1,
            last: Last::Passed,
        };
        let names = |names: &[&str]| -> BTreeMap<String, Entry> {
            names.iter().map(|n| ((*n).to_owned(), e)).collect()
        };
        let tests: Tests = [
            ("read.cdp".to_owned(), names(&["kept", "renamed"])),
            ("unread.cdp".to_owned(), names(&["kept"])),
            ("deleted.cdp".to_owned(), names(&["gone"])),
        ]
        .into();
        let mut backends = BTreeMap::from([
            ("mock".to_owned(), tests),
            (
                "staging".to_owned(),
                [("deleted.cdp".to_owned(), names(&["gone"]))].into(),
            ),
        ]);
        let read = BTreeMap::from([("read.cdp".to_owned(), vec!["kept", "new"])]);
        prune(&mut backends, root, &read);
        let left: Vec<(&str, Vec<&str>)> = backends["mock"]
            .iter()
            .map(|(f, t)| (f.as_str(), t.keys().map(String::as_str).collect()))
            .collect();
        // A file the run did not read keeps its tests while it exists.
        assert_eq!(
            left,
            [("read.cdp", vec!["kept"]), ("unread.cdp", vec!["kept"])]
        );
        // A backend with no tests left is gone too.
        assert!(!backends.contains_key("staging"));
    }

    #[test]
    fn a_file_key_is_relative_with_slashes() {
        let root = Path::new("/p");
        assert_eq!(
            key(root, &root.join("e2e").join("scenarios").join("a.cdp")),
            "e2e/scenarios/a.cdp"
        );
        assert_eq!(key(root, Path::new("/elsewhere/b.cdp")), "/elsewhere/b.cdp");
    }

    #[test]
    fn a_bad_file_is_a_warning_and_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".durations.json");
        let missing = Durations::load(&path, "mock");
        assert!(missing.warning.is_none());
        assert!(missing.get("a.cdp", "t").is_none());

        std::fs::write(&path, "{not json").unwrap();
        let bad = Durations::load(&path, "mock");
        assert!(
            bad.warning
                .as_deref()
                .is_some_and(|w| w.contains("is not a durations file")),
            "{:?}",
            bad.warning
        );
        assert!(bad.get("a.cdp", "t").is_none());

        std::fs::write(&path, r#"{"version": 99, "backends": {}}"#).unwrap();
        let newer = Durations::load(&path, "mock");
        assert!(
            newer
                .warning
                .as_deref()
                .is_some_and(|w| w.contains("has version 99")),
        );

        let good = r#"{"version": 1, "backends": {"mock": {"a.cdp": {
            "slow": {"ms": 2000, "runs": 3, "last": "passed"},
            "red": {"last": "failed"}
        }}}}"#;
        std::fs::write(&path, good).unwrap();
        let read = Durations::load(&path, "mock");
        assert!(read.warning.is_none());
        assert_eq!(read.ms("a.cdp", "slow"), Some(2000));
        assert_eq!(read.ms("a.cdp", "red"), None);
        assert!(read.failed("a.cdp", "red"));
        assert!(!read.failed("a.cdp", "slow"));
        // Another backend's tests are its own.
        assert!(
            Durations::load(&path, "staging")
                .get("a.cdp", "slow")
                .is_none()
        );
    }
}
