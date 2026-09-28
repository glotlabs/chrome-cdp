//! `test --backend <real> --verify-mocks` (plan 11, milestone 11.11):
//! keeps the mocks honest.
//!
//! After each test, the runner takes the real exchanges of its browser
//! context (the pages' requests from the HAR export, and the test's
//! `http` steps), in the order they happened, and feeds them to a mock
//! engine of its own with the sets the test would have in mock mode. So
//! sequences and collections see the same history they would have seen,
//! and each real answer meets the answer those mocks give. Statuses and
//! the JSON *shape* are compared ([`mock::drift::shape`]), not values; a
//! request in a set's scope that no mock answers is listed too.
//!
//! `--update` rewrites a drifted mock's JSON data file from the first
//! real answer it differed on ([`mock::drift::update`]), keeping the
//! values whose shape still fits. Inline bodies, templates, collections
//! and a different status are left for a person.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use mock::record::Exchange;
use mock::{MockSet, Mocks, Outcome, Response, Source};
use protocol::schema::{MockDrift, UnmockedRequest, VerifyData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};
use serde_json::Value;

/// A real exchange, and when it started (ms since 1970).
pub(super) struct Seen {
    pub(super) at_ms: i64,
    pub(super) exchange: Exchange,
}

/// What the run's tests found so far.
pub(super) struct Verifier {
    dir: PathBuf,
    cwd: PathBuf,
    seed: u64,
    found: Mutex<Found>,
}

#[derive(Default)]
struct Found {
    compared: usize,
    mocks: BTreeSet<String>,
    drifted: Vec<Drifted>,
    unmocked: Vec<UnmockedRequest>,
}

struct Drifted {
    drift: MockDrift,
    /// The first real JSON body it differed on (for `--update`), unless
    /// the status differed.
    real: Option<Value>,
}

impl Verifier {
    pub(super) fn new(config: &e2e::Config) -> Self {
        let seed = match config.determinism.random_seed {
            Some(e2e::config::RandomSeed::Seed(seed)) => seed,
            _ => 0,
        };
        Self {
            dir: config.mocks.clone(),
            cwd: std::env::current_dir().unwrap_or_else(|_| config.root.clone()),
            seed,
            found: Mutex::new(Found::default()),
        }
    }

    fn load(&self, sets: &[String]) -> Result<Vec<MockSet>, String> {
        load(&self.dir, &self.cwd, sets)
    }

    /// The scope of `sets` (for the HAR export): every set's `scope`.
    pub(super) fn scope(&self, sets: &[String]) -> Vec<String> {
        scope_of(&self.load(sets).unwrap_or_default())
    }

    /// Compares one test's real exchanges with what `sets` answer.
    pub(super) fn compare(&self, sets: &[String], mut seen: Vec<Seen>) -> Result<(), String> {
        if sets.is_empty() {
            return Ok(());
        }
        let mut mocks = Mocks::new(self.seed);
        mocks.apply(self.load(sets)?, false);
        seen.sort_by_key(|s| s.at_ms);
        let mut found = self.found.lock().unwrap_or_else(PoisonError::into_inner);
        for Seen { at_ms, exchange: e } in &seen {
            if e.status == 0 {
                continue;
            }
            let request = mock::Request {
                method: &e.method,
                url: &e.url,
                headers: &e.request_headers,
                body: e.request_body.as_deref().map(str::as_bytes),
            };
            match mocks.decide(&request, *at_ms) {
                Outcome::Answer(answer) => {
                    let Response::Fulfill { status, body, .. } = &answer.response else {
                        continue;
                    };
                    found.compared += 1;
                    found.mocks.insert(answer.mock.at.clone());
                    let (problems, real) = differences(e, *status, body);
                    if problems.is_empty() {
                        continue;
                    }
                    let at = answer.mock.at.clone();
                    match found.drifted.iter_mut().find(|d| d.drift.mock.at == at) {
                        Some(known) => {
                            for p in problems {
                                if !known.drift.problems.contains(&p) {
                                    known.drift.problems.push(p);
                                }
                            }
                        }
                        None => found.drifted.push(Drifted {
                            drift: MockDrift {
                                mock: answer.mock,
                                data_file: answer.data_file.map(|f| f.display().to_string()),
                                request: format!("{} {}", e.method, e.url),
                                problems,
                                updated: false,
                            },
                            real,
                        }),
                    }
                }
                Outcome::Unmatched { set, .. } => {
                    found.compared += 1;
                    let path = e.url.split('?').next().unwrap_or(&e.url);
                    match found
                        .unmocked
                        .iter_mut()
                        .find(|u| u.method == e.method && u.url.split('?').next() == Some(path))
                    {
                        Some(known) => known.count += 1,
                        None => found.unmocked.push(UnmockedRequest {
                            method: e.method.clone(),
                            url: e.url.clone(),
                            status: e.status,
                            set,
                            count: 1,
                        }),
                    }
                }
                Outcome::Pass | Outcome::Preflight { .. } => {}
            }
        }
        Ok(())
    }

    /// What was found, for backend `backend`; with `update`, the drifted
    /// data files are rewritten first. The findings start over.
    pub(super) fn finish(&self, backend: &str, update: bool) -> VerifyData {
        let found = std::mem::take(&mut *self.found.lock().unwrap_or_else(PoisonError::into_inner));
        let mut updated = Vec::new();
        let mut drifted = Vec::new();
        for Drifted { mut drift, real } in found.drifted {
            if update && let (Some(file), Some(real)) = (&drift.data_file, &real) {
                match rewrite(Path::new(file), real) {
                    Ok(true) => {
                        drift.updated = true;
                        updated.push(file.clone());
                    }
                    Ok(false) => {}
                    Err(e) => tracing::warn!("cannot update {file}: {e}"),
                }
            }
            drifted.push(drift);
        }
        VerifyData {
            backend: backend.to_owned(),
            compared: found.compared,
            mocks: found.mocks.len(),
            drifted,
            unmocked: found.unmocked,
            updated,
        }
    }
}

fn load(dir: &Path, cwd: &Path, sets: &[String]) -> Result<Vec<MockSet>, String> {
    sets.iter()
        .map(|name| {
            Source::find(dir, cwd, name)
                .and_then(|source| MockSet::load(&source))
                .map_err(|e| e.message)
        })
        .collect()
}

fn scope_of(sets: &[MockSet]) -> Vec<String> {
    let mut scope: Vec<String> = Vec::new();
    for set in sets {
        for glob in set.scope() {
            if !scope.contains(glob) {
                scope.push(glob.clone());
            }
        }
    }
    scope
}

/// The scope of the project's mock sets `sets`: every set's `scope`
/// (empty when one does not load).
pub(super) fn scope_of_sets(config: &e2e::Config, sets: &[String]) -> Vec<String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| config.root.clone());
    scope_of(&load(&config.mocks, &cwd, sets).unwrap_or_default())
}

/// How the mock's answer (`status`, `body`) differs from the real one
/// `e`; and the real JSON body, when only the shape differs.
fn differences(e: &Exchange, status: u16, body: &[u8]) -> (Vec<String>, Option<Value>) {
    if status != e.status {
        return (
            vec![format!(
                "status: the real answer is {}; the mock's is {status}",
                e.status
            )],
            None,
        );
    }
    let Some(real_body) = &e.response_body else {
        return (Vec::new(), None);
    };
    let real = serde_json::from_slice::<Value>(real_body).ok();
    let mocked = serde_json::from_slice::<Value>(body).ok();
    match (real, mocked) {
        (Some(real), Some(mocked)) => {
            let problems = mock::drift::shape(&real, &mocked);
            (problems, Some(real))
        }
        (Some(_), None) if !body.is_empty() => (
            vec!["the real answer is JSON; the mock's is not".to_owned()],
            None,
        ),
        (None, Some(_)) if !real_body.is_empty() => (
            vec!["the mock's answer is JSON; the real one is not".to_owned()],
            None,
        ),
        _ => (Vec::new(), None),
    }
}

/// Rewrites data file `file` from `real`; whether it changed.
fn rewrite(file: &Path, real: &Value) -> Result<bool, String> {
    let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    match mock::drift::update(&text, real)? {
        Some(new) => {
            std::fs::write(file, new).map_err(|e| e.to_string())?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// The findings in text, after the run's lines.
pub(super) fn text(data: &VerifyData) -> String {
    let mut text = format!(
        "mocks against backend {}: {} real answers compared with {} mocks",
        data.backend, data.compared, data.mocks
    );
    if data.drifted.is_empty() && data.unmocked.is_empty() {
        text.push_str("; no drift\n");
        return text;
    }
    text.push('\n');
    for d in &data.drifted {
        let file = d
            .data_file
            .as_deref()
            .and_then(|f| Path::new(f).file_name())
            .map(|f| format!(" ({})", f.to_string_lossy()))
            .unwrap_or_default();
        let _ = writeln!(
            text,
            "  drift: {} \"{}\"{file}, {}{}",
            d.mock.at,
            d.mock.name,
            d.request,
            if d.updated {
                " (data file updated)"
            } else {
                ""
            }
        );
        for p in &d.problems {
            let _ = writeln!(text, "    {p}");
        }
    }
    for u in &data.unmocked {
        let _ = writeln!(
            text,
            "  no mock: {} {} (real answer {}; in the scope of mock set {}){}",
            u.method,
            u.url,
            u.status,
            u.set,
            if u.count > 1 {
                format!(", {} times", u.count)
            } else {
                String::new()
            }
        );
    }
    text
}

/// The run's error when mocks drifted (and `--update` did not fix all of
/// them); `None` when they are honest.
pub(super) fn error(data: &VerifyData) -> Option<ErrorInfo> {
    let left: Vec<&MockDrift> = data.drifted.iter().filter(|d| !d.updated).collect();
    if left.is_empty() && data.unmocked.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if !left.is_empty() {
        parts.push(format!(
            "{} {} drifted",
            left.len(),
            if left.len() == 1 { "mock" } else { "mocks" }
        ));
    }
    if !data.unmocked.is_empty() {
        parts.push(format!(
            "{} real {} no mock",
            data.unmocked.len(),
            if data.unmocked.len() == 1 {
                "request has"
            } else {
                "requests have"
            }
        ));
    }
    let mut info = ErrorInfo::new(
        ErrorKind::ExpectFailed,
        format!(
            "the mocks do not match backend {}: {}",
            data.backend,
            parts.join(", ")
        ),
    );
    if let Some(first) = left.first() {
        info = info.note(
            "first",
            format!(
                "{} \"{}\": {}",
                first.mock.at,
                first.mock.name,
                first.problems.first().cloned().unwrap_or_default()
            ),
        );
    }
    let fixable = left
        .iter()
        .any(|d| d.data_file.is_some() && !d.problems.iter().any(|p| p.starts_with("status:")));
    let hint = if fixable {
        Hint::run(
            Cmd::new("test")
                .flag("backend", &data.backend)
                .switch("verify-mocks")
                .switch("update"),
        )
        .why("rewrites the drifted data files from the real answers; review the diff")
    } else {
        Hint::text("fix the mocks (or add the missing ones) to match the real API")
    };
    Some(info.hint(hint))
}
