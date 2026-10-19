//! What a load run keeps (plan 23.1): the counts of iterations and steps
//! by result, every step's start, end and result in `steps.jsonl` with
//! `--raw` (23.4 aggregates them), and the page, console and requests of
//! the first `--keep-failures` failed iterations, as `test` keeps a
//! failed test's.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use engine::Engine;
use protocol::ErrorInfo;
use protocol::schema::{LoadSteps, NetworkData};
use serde::Serialize;
use serde_json::json;
use tokio::time::Instant;

use crate::test::artifacts;

/// `steps.jsonl`'s name in the run's folder.
pub(super) const STEPS: &str = "steps.jsonl";
/// The kept failures' folder in the run's folder.
pub(super) const FAILURES: &str = "failures";

/// How long reading the page for a kept failure may take, per call.
const READ_TIMEOUT_MS: u64 = 5_000;
/// How much of the page `snapshot.txt` keeps.
const SNAPSHOT_CHARS: usize = 200_000;

/// One line of `steps.jsonl`.
#[derive(Debug, Serialize)]
pub(super) struct StepLine<'a> {
    /// The user, from 1.
    pub(super) vu: u32,
    /// The user's iteration, from 1.
    pub(super) iteration: u64,
    /// The journey's `file:line`.
    pub(super) journey: &'a str,
    /// `before-all`, `before`, `after` or `after-all`; absent for the
    /// journey's own steps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) hook: Option<&'a str>,
    /// The step's `file:line:col`.
    pub(super) at: &'a str,
    /// The step as written.
    pub(super) command: &'a str,
    /// When it started and ended, in ms since the run started, on the
    /// runtime's clock.
    pub(super) start_ms: f64,
    pub(super) end_ms: f64,
    /// `ok`, `failed`, `timeout` or `error`.
    pub(super) result: &'static str,
    /// The error's kind, for any result but `ok`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) kind: Option<String>,
    /// The error's message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<&'a str>,
}

/// The run's counts.
#[derive(Debug, Default)]
pub(super) struct Tally {
    pub(super) iterations: u64,
    pub(super) failed: u64,
    pub(super) steps: LoadSteps,
    pub(super) errors: BTreeMap<String, u64>,
    /// Per journey: iterations and failed ones.
    pub(super) journeys: Vec<(u64, u64)>,
    /// Browser contexts made for the users, and how long making them took
    /// in all.
    pub(super) contexts: u64,
    pub(super) context_setup: std::time::Duration,
}

/// What the run keeps, shared by its users.
#[derive(Debug)]
pub(super) struct Records {
    /// When the users started: the runtime's clock's zero.
    pub(super) started: Instant,
    raw: Option<Mutex<BufWriter<File>>>,
    pub(super) raw_path: Option<PathBuf>,
    tally: Mutex<Tally>,
    /// Failed iterations so far.
    failures: AtomicU64,
    keep: u64,
    pub(super) failures_dir: PathBuf,
}

impl Records {
    /// Records for a run in `dir` with `journeys` journeys; with `raw`,
    /// `steps.jsonl` is written as steps end.
    pub(super) fn new(dir: &Path, journeys: usize, raw: bool, keep: u32) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let raw_path = raw.then(|| dir.join(STEPS));
        let raw = raw_path
            .as_deref()
            .map(File::create)
            .transpose()?
            .map(|f| Mutex::new(BufWriter::new(f)));
        Ok(Self {
            started: Instant::now(),
            raw,
            raw_path,
            tally: Mutex::new(Tally {
                journeys: vec![(0, 0); journeys],
                ..Tally::default()
            }),
            failures: AtomicU64::new(0),
            keep: u64::from(keep),
            failures_dir: dir.join(FAILURES),
        })
    }

    /// Starts the clock: the users start now.
    pub(super) fn start(&mut self) {
        self.started = Instant::now();
    }

    fn tally(&self) -> std::sync::MutexGuard<'_, Tally> {
        self.tally.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// ms since the run started.
    pub(super) fn since(&self, at: Instant) -> f64 {
        let ms = at.saturating_duration_since(self.started).as_secs_f64() * 1000.0;
        (ms * 1000.0).round() / 1000.0
    }

    /// Counts a step, and writes its line with `--raw`.
    pub(super) fn step(&self, line: &StepLine<'_>) {
        {
            let mut tally = self.tally();
            match line.result {
                "ok" => tally.steps.ok += 1,
                "failed" => tally.steps.failed += 1,
                "timeout" => tally.steps.timeout += 1,
                _ => tally.steps.error += 1,
            }
            if let Some(kind) = &line.kind {
                *tally.errors.entry(kind.clone()).or_default() += 1;
            }
        }
        if let Some(raw) = &self.raw
            && let Ok(text) = serde_json::to_string(line)
        {
            let mut raw = raw.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = writeln!(raw, "{text}");
        }
    }

    /// Counts an iteration of journey `journey`.
    pub(super) fn iteration(&self, journey: usize, failed: bool) {
        let mut tally = self.tally();
        tally.iterations += 1;
        tally.failed += u64::from(failed);
        if let Some(counts) = tally.journeys.get_mut(journey) {
            counts.0 += 1;
            counts.1 += u64::from(failed);
        }
    }

    /// Counts a browser context made for a user, in `took`.
    pub(super) fn context_made(&self, took: std::time::Duration) {
        let mut tally = self.tally();
        tally.contexts += 1;
        tally.context_setup += took;
    }

    /// A failed iteration: its number from 1 while the run keeps them
    /// (`--keep-failures`), else `None` (it is only counted).
    pub(super) fn failure(&self) -> Option<u64> {
        let n = self.failures.fetch_add(1, Ordering::Relaxed) + 1;
        (n <= self.keep).then_some(n)
    }

    /// How many failed iterations were kept.
    pub(super) fn kept(&self) -> u64 {
        self.failures.load(Ordering::Relaxed).min(self.keep)
    }

    /// The counts so far.
    pub(super) fn counts(&self) -> Tally {
        let tally = self.tally();
        Tally {
            iterations: tally.iterations,
            failed: tally.failed,
            steps: tally.steps,
            errors: tally.errors.clone(),
            journeys: tally.journeys.clone(),
            contexts: tally.contexts,
            context_setup: tally.context_setup,
        }
    }

    /// Writes what `steps.jsonl` holds so far.
    pub(super) fn flush(&self) {
        if let Some(raw) = &self.raw {
            let _ = raw.lock().unwrap_or_else(PoisonError::into_inner).flush();
        }
    }
}

/// What a kept failure says about itself, for its `report.md`.
#[derive(Debug)]
pub(super) struct Failed<'a> {
    pub(super) journey: &'a str,
    pub(super) place: &'a str,
    pub(super) vu: u32,
    pub(super) iteration: u64,
    pub(super) at: &'a str,
    pub(super) step: &'a str,
    pub(super) error: &'a ErrorInfo,
}

/// Keeps a failed iteration's page (`failure.png`, `snapshot.txt`), its
/// console (`console.txt`), its requests (`requests.txt`) and what failed
/// (`report.md`) in `dir`, read from browser context `context` before it
/// closes. A read that fails leaves its file out. `started_ms` is when the
/// iteration started (ms since the Unix epoch), for the requests' times.
pub(super) async fn capture(
    engine: &Engine,
    context: &str,
    dir: &Path,
    failed: &Failed<'_>,
    started_ms: u64,
) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let context = Some(context.to_owned());
    let png = dir.join(artifacts::SCREENSHOT);
    let screenshot = serde_json::from_value(json!({
        "path": png,
        "max_size": 0,
        "timeout_ms": READ_TIMEOUT_MS,
    }));
    let shot = match screenshot {
        Ok(req) => engine::in_context(context.clone(), Box::pin(engine.screenshot(req)))
            .await
            .err(),
        Err(e) => Some(ErrorInfo::new(
            protocol::ErrorKind::ProtocolError,
            e.to_string(),
        )),
    };
    if let Ok(req) = serde_json::from_value(json!({
        "interactive": true,
        "text_limit": 200,
        "timeout_ms": READ_TIMEOUT_MS,
        "max_chars": SNAPSHOT_CHARS,
    })) && let Ok(out) =
        engine::in_context(context.clone(), Box::pin(engine.snapshot(req))).await
    {
        artifacts::write(&dir.join(artifacts::SNAPSHOT), &out.text);
    }
    if let Ok(req) = serde_json::from_value(json!({
        "level": "debug",
        "since": { "kind": "seq", "seq": 0 },
        "limit": 0,
        "timeout_ms": READ_TIMEOUT_MS,
        "max_chars": 0,
    })) && let Ok(out) = engine::in_context(context.clone(), Box::pin(engine.console(req))).await
    {
        artifacts::write(&dir.join(artifacts::CONSOLE), &out.text);
    }
    if let Ok(req) = serde_json::from_value(json!({
        "since": { "kind": "seq", "seq": 0 },
        "limit": 0,
        "timeout_ms": READ_TIMEOUT_MS,
        "max_chars": 0,
    })) && let Ok(out) = engine::in_context(context.clone(), Box::pin(engine.network(req))).await
        && let Ok(data) = serde_json::from_value::<NetworkData>(out.data)
    {
        let (text, _, _) = artifacts::journal(&[data], Vec::new(), started_ms);
        artifacts::write(&dir.join(artifacts::REQUESTS), &text);
    }
    artifacts::write(&dir.join(artifacts::REPORT), &report(failed, shot.as_ref()));
}

/// A kept failure's `report.md`.
fn report(failed: &Failed<'_>, shot: Option<&ErrorInfo>) -> String {
    let mut text = format!("# {}\n\n", failed.journey);
    let _ = writeln!(text, "- Journey: {} ({})", failed.journey, failed.place);
    let _ = writeln!(text, "- User {}, iteration {}", failed.vu, failed.iteration);
    let _ = writeln!(text, "- Step: `{}` ({})", failed.step, failed.at);
    let error = failed.error;
    let _ = writeln!(text, "\n## Error\n\n```text\n{}", error.message);
    for note in &error.notes {
        let _ = writeln!(text, "  {}: {}", note.label, note.text);
    }
    if let Some(hint) = &error.hint {
        let _ = writeln!(text, "hint: {hint}");
    }
    text.push_str("```\n");
    let _ = writeln!(
        text,
        "\n## Files\n\n- {}: the page when it failed{}",
        artifacts::SCREENSHOT,
        shot.map(|e| format!(" (none: {})", e.message))
            .unwrap_or_default()
    );
    let _ = writeln!(
        text,
        "- {}: its controls (snapshot -i)\n- {}: its console\n- {}: its requests",
        artifacts::SNAPSHOT,
        artifacts::CONSOLE,
        artifacts::REQUESTS
    );
    text
}

/// A kept failure's folder: `<n>-vu<V>-i<I>` in the failures folder.
pub(super) fn failure_dir(failures: &Path, n: u64, vu: u32, iteration: u64) -> PathBuf {
    failures.join(format!("{n:03}-vu{vu}-i{iteration}"))
}
