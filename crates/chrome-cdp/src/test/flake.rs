//! Flake diagnosis (plan 18, milestone 18.3): why a test's runs disagreed.
//!
//! A test is flaky when a retry passed or `--repeat` had both outcomes.
//! Each run (or try) keeps its steps (`steps.jsonl`) and its requests with
//! when they were sent and answered (`requests.txt`), both from the run's
//! start. [`diagnose`] finds the step most failing runs failed at and
//! compares the failing runs with the passing ones up to it, rule by rule:
//!
//! | Cause | Rule |
//! |---|---|
//! | `unsettled` | a request open when a step (an action, or the failing step) acted in failing runs was answered before it acted in every passing run |
//! | `race` | two API requests were answered in one order in failing runs and in the other in every passing run |
//! | `moving-target` | the failing step's element was moving, covered or replaced |
//! | `different-requests` | failing runs sent a request no passing run sent (or not one every passing run sent), or one no mock answered |
//! | `time-or-randomness` | determinism is off, and the failing assertion saw another value in each failing run |
//! | `shared-state` | a real backend, and the runs passed until one failed, then all failed |
//! | `unknown` | none of the above: the first step whose result differs |
//!
//! Causes are ranked by how many failing runs they explain, then in that
//! order. Everything here is pure, so the rules have unit tests on
//! hand-made runs.

use std::cmp::Ordering;
use std::fmt::Write as _;
use std::path::PathBuf;

use protocol::ErrorKind;
use protocol::schema::{FlakeCause, TestFailure, TestFlake};

use super::artifacts::{StepRecord, Timed, cut};

/// One run (or try) of a test.
#[derive(Debug, Clone)]
pub(super) struct RunRecord {
    /// `run 3`, `try 2`.
    pub(super) label: String,
    /// Its number among the test's runs (or tries), from 1.
    pub(super) number: usize,
    /// Its artifacts.
    pub(super) dir: PathBuf,
    /// Why it failed; `None` when it passed.
    pub(super) failure: Option<TestFailure>,
    pub(super) steps: Vec<StepRecord>,
    pub(super) requests: Vec<Timed>,
}

/// What the rules know of the test run as a whole.
#[derive(Debug, Clone, Default)]
pub(super) struct Setting {
    /// Against a real backend (whose state outlives a test).
    pub(super) real: bool,
    /// The `[e2e.determinism]` settings that are off, of `freeze_time`
    /// and `random_seed`.
    pub(super) unfixed: Vec<&'static str>,
    /// `disable_animations = false`.
    pub(super) animations: bool,
}

/// The causes, in the order ties are ranked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Cause {
    Unsettled,
    Race,
    MovingTarget,
    DifferentRequests,
    Time,
    SharedState,
    Unknown,
}

impl Cause {
    fn name(self) -> &'static str {
        match self {
            Self::Unsettled => "unsettled",
            Self::Race => "race",
            Self::MovingTarget => "moving-target",
            Self::DifferentRequests => "different-requests",
            Self::Time => "time-or-randomness",
            Self::SharedState => "shared-state",
            Self::Unknown => "unknown",
        }
    }

    fn summary(self) -> &'static str {
        match self {
            Self::Unsettled => "acted before the page settled",
            Self::Race => "requests raced",
            Self::MovingTarget => "moving target",
            Self::DifferentRequests => "different requests",
            Self::Time => "time or randomness",
            Self::SharedState => "shared state",
            Self::Unknown => "unknown",
        }
    }

    /// How sure a cause that explains every failing run is: a timing rule
    /// reads times that are close to each other, and shared state is a
    /// guess from the order of the runs.
    fn weight(self) -> f64 {
        match self {
            Self::MovingTarget => 0.95,
            Self::Unsettled => 0.9,
            Self::Race | Self::DifferentRequests => 0.85,
            Self::Time => 0.8,
            Self::SharedState => 0.5,
            Self::Unknown => 0.0,
        }
    }
}

/// A cause that fits, and how many failing runs it explains.
#[derive(Debug, Clone)]
struct Found {
    cause: Cause,
    explains: usize,
    evidence: Vec<String>,
    hint: Option<String>,
}

/// A failing run, and where its failing step is among its steps.
struct Failed<'a> {
    run: &'a RunRecord,
    failure: &'a TestFailure,
    at: Option<usize>,
}

/// Why `runs` (a test's runs or tries, in order) disagreed; `None` unless
/// some passed and some failed.
pub(super) fn diagnose(runs: &[RunRecord], setting: &Setting) -> Option<TestFlake> {
    let failing: Vec<&RunRecord> = runs.iter().filter(|r| r.failure.is_some()).collect();
    let passing: Vec<&RunRecord> = runs.iter().filter(|r| r.failure.is_none()).collect();
    if failing.is_empty() || passing.is_empty() {
        return None;
    }
    // The step most failing runs failed at; the rules compare those runs.
    let mut places: Vec<((&str, &str), usize)> = Vec::new();
    for run in &failing {
        let failure = run.failure.as_ref()?;
        let place = (failure.at.as_str(), failure.step.as_str());
        match places.iter_mut().find(|(p, _)| *p == place) {
            Some((_, n)) => *n += 1,
            None => places.push((place, 1)),
        }
    }
    let mut most = places.first()?;
    for place in &places {
        if place.1 > most.1 {
            most = place;
        }
    }
    let ((at, step), _) = *most;
    let here: Vec<Failed<'_>> = failing
        .iter()
        .filter_map(|run| {
            let failure = run.failure.as_ref()?;
            (failure.at == at && failure.step == step).then(|| Failed {
                run,
                failure,
                at: failing_step(run, failure),
            })
        })
        .collect();

    let mut found: Vec<Found> = [
        unsettled(&here, &passing),
        race(&here, &passing),
        moving_target(&here, setting),
        different_requests(&here, &passing),
        time_or_randomness(&here, setting),
        shared_state(runs, setting),
    ]
    .into_iter()
    .flatten()
    .filter(|f| f.explains > 0)
    .collect();
    found.sort_by(|a, b| b.explains.cmp(&a.explains).then(a.cause.cmp(&b.cause)));
    if found.is_empty() {
        found.push(unknown(&here, &passing));
    }
    let total = failing.len();
    let mut causes = found.into_iter().map(|f| FlakeCause {
        cause: f.cause.name().to_owned(),
        summary: f.cause.summary().to_owned(),
        confidence: confidence(f.cause, f.explains, total),
        evidence: f.evidence,
        hint: f.hint,
    });
    let first = causes.next()?;
    Some(TestFlake {
        cause: first.cause,
        summary: first.summary,
        confidence: first.confidence,
        evidence: first.evidence,
        hint: first.hint,
        runs: count(runs.len()),
        failed_runs: count(total),
        at: Some(at.to_owned()),
        step: Some(step.to_owned()),
        others: causes.collect(),
    })
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The cause's weight, times the share of failing runs it explains, to
/// two decimals.
fn confidence(cause: Cause, explains: usize, total: usize) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let share = explains.min(total) as f64 / total.max(1) as f64;
    (cause.weight() * share * 100.0).round() / 100.0
}

/// Where `failure`'s step is among `run`'s steps.
fn failing_step(run: &RunRecord, failure: &TestFailure) -> Option<usize> {
    run.steps
        .iter()
        .position(|s| !s.ok && s.at == failure.at && s.command == failure.step)
        .or_else(|| run.steps.iter().rposition(|s| !s.ok))
}

/// A step's identity across runs: its place, its text, and how many
/// steps with both ran before it (an `@include` used twice).
fn step_key(steps: &[StepRecord], i: usize) -> (&str, &str, usize) {
    let s = &steps[i];
    let nth = steps[..i]
        .iter()
        .filter(|o| o.at == s.at && o.command == s.command)
        .count();
    (s.at.as_str(), s.command.as_str(), nth)
}

/// Where the step `key` names is among `steps`.
fn find_step(steps: &[StepRecord], key: (&str, &str, usize)) -> Option<usize> {
    steps
        .iter()
        .enumerate()
        .filter(|(_, s)| s.at == key.0 && s.command == key.1)
        .nth(key.2)
        .map(|(i, _)| i)
}

fn start(step: &StepRecord) -> i64 {
    i64::try_from(step.start_ms).unwrap_or(i64::MAX)
}

fn end(step: &StepRecord) -> i64 {
    start(step).saturating_add(i64::try_from(step.duration_ms).unwrap_or(i64::MAX))
}

/// Whether a step only waits (a `wait` or an `expect`): it cannot act too
/// early, and one that passed means the page was ready for it.
fn waits(command: &str) -> bool {
    matches!(command.split_whitespace().next(), Some("wait" | "expect"))
}

/// A step as the evidence names it: `click "Pay" (checkout.cdp:14:3)`.
fn step_name(step: &StepRecord) -> String {
    format!("`{}` ({})", cut(&step.command, 60), step.at)
}

/// A request's identity across runs: its method and its URL with ids
/// (numbers, UUIDs, long hex runs, in the path or a query value) made
/// `:id`, without a fragment.
fn request_key(request: &Timed) -> String {
    format!("{} {}", request.method, template(&request.url))
}

fn template(url: &str) -> String {
    let url = url.split_once('#').map_or(url, |(u, _)| u);
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let (origin, path) = match base.find("://") {
        Some(i) => match base[i + 3..].find('/') {
            Some(j) => base.split_at(i + 3 + j),
            None => (base, ""),
        },
        None => ("", base),
    };
    let mut out = format!("{origin}{}", engine::fingerprint_score::page_template(path));
    if !query.is_empty() {
        let pairs: Vec<String> = query
            .split('&')
            .map(|pair| match pair.split_once('=') {
                Some((k, v)) if engine::fingerprint_score::page_template(v) == ":id" => {
                    format!("{k}=:id")
                }
                _ => pair.to_owned(),
            })
            .collect();
        let _ = write!(out, "?{}", pairs.join("&"));
    }
    out
}

/// A request key as the evidence shows it: without the origin.
fn shown(key: &str) -> String {
    let Some((method, url)) = key.split_once(' ') else {
        return key.to_owned();
    };
    let path = url
        .find("://")
        .and_then(|i| url[i + 3..].find('/').map(|j| &url[i + 3 + j..]))
        .unwrap_or(url);
    format!("`{method} {path}`")
}

/// The URL pattern a `wait --response` for request `key` takes.
fn response_pattern(key: &str) -> String {
    let url = key.split_once(' ').map_or(key, |(_, u)| u);
    let path = url
        .find("://")
        .and_then(|i| url[i + 3..].find('/').map(|j| &url[i + 3 + j..]))
        .unwrap_or(url);
    let path = path.split_once('?').map_or(path, |(p, _)| p);
    format!("**{}", path.replace(":id", "*"))
}

/// A request's identity in a run: its key, and how many requests with
/// the key the run sent before it.
type RequestId = (String, usize);

/// `run`'s requests the page sent: not the browser's own favicon fetch,
/// which comes in some runs only.
fn sent(run: &RunRecord) -> impl Iterator<Item = &Timed> {
    run.requests.iter().filter(|r| {
        let path = r.url.split(['?', '#']).next().unwrap_or_default();
        !path.ends_with("/favicon.ico")
    })
}

/// `run`'s requests with their identity.
fn requests(run: &RunRecord) -> Vec<(RequestId, &Timed)> {
    let mut seen: Vec<String> = Vec::new();
    sent(run)
        .map(|r| {
            let key = request_key(r);
            let nth = seen.iter().filter(|k| **k == key).count();
            seen.push(key.clone());
            ((key, nth), r)
        })
        .collect()
}

fn duration(ms: i64) -> String {
    protocol::format::duration(std::time::Duration::from_millis(ms.unsigned_abs()))
}

/// `in 2 of 3 failing runs`, `in all 3 …`, `in the one …`; `what` is
/// plural.
fn of(n: usize, total: usize, what: &str) -> String {
    if total == 1 && n == 1 {
        format!("in the one {}", what.strip_suffix('s').unwrap_or(what))
    } else if n == total {
        format!("in all {total} {what}")
    } else {
        format!("in {n} of {total} {what}")
    }
}

/// Acted before the page settled: a request sent before a step started
/// was still open when the step acted in failing runs, and answered
/// before it acted in every passing run.
/// A step acts when it ends, after waiting for its element; the steps
/// looked at are the actions up to the failing step, and it.
fn unsettled(here: &[Failed<'_>], passing: &[&RunRecord]) -> Option<Found> {
    /// The request that fits best so far.
    struct Best {
        /// The failing runs it was open in.
        runs: usize,
        /// The step it was open at, in the first failing run.
        step: usize,
        id: RequestId,
        /// How long after the step's start failing runs answered it, at
        /// the most; `None` when one never did.
        lag: Option<i64>,
        /// How long before the step's start passing runs answered it, at
        /// the least.
        margin: i64,
    }
    let first = here.iter().find(|f| f.at.is_some())?;
    let failed_at = first.at?;
    let steps = &first.run.steps;
    let mut best: Option<Best> = None;
    for (index, step) in steps.iter().enumerate().take(failed_at + 1) {
        if matches!(step.hook.as_deref(), Some("setup" | "check"))
            || (index != failed_at && waits(&step.command))
        {
            continue;
        }
        let key = step_key(steps, index);
        // Every passing run ran the step: when it acted there.
        let Some(passed_acts) = passing
            .iter()
            .map(|p| find_step(&p.steps, key).map(|at| (*p, end(&p.steps[at]))))
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        // The requests open when it acted, in each failing run, with how
        // long after that they were answered (`None`: never).
        let mut open: Vec<(RequestId, usize, Option<i64>)> = Vec::new();
        for failed in here {
            let Some(at) = find_step(&failed.run.steps, key) else {
                continue;
            };
            // Sent before the step started (not by the step), and not
            // answered when it acted.
            let (began, acted) = (start(&failed.run.steps[at]), end(&failed.run.steps[at]));
            for (id, request) in requests(failed.run) {
                let is_open =
                    request.sent_ms < began && request.answered_ms.is_none_or(|a| a > acted);
                if !is_open {
                    continue;
                }
                let lag = request.answered_ms.map(|a| a - acted);
                match open.iter_mut().find(|(o, _, _)| *o == id) {
                    Some((_, runs, worst)) => {
                        *runs += 1;
                        *worst = match (*worst, lag) {
                            (Some(w), Some(l)) => Some(w.max(l)),
                            _ => None,
                        };
                    }
                    None => open.push((id, 1, lag)),
                }
            }
        }
        for (id, runs, lag) in open {
            // Answered before the step in every passing run: by how much,
            // at the closest.
            let margins: Option<Vec<i64>> = passed_acts
                .iter()
                .map(|(p, acted)| {
                    let request = requests(p).into_iter().find(|(o, _)| *o == id)?.1;
                    request
                        .answered_ms
                        .filter(|a| a <= acted)
                        .map(|a| acted - a)
                })
                .collect();
            let Some(margin) = margins.and_then(|m| m.into_iter().min()) else {
                continue;
            };
            if best.as_ref().is_none_or(|b| runs > b.runs) {
                best = Some(Best {
                    runs,
                    step: index,
                    id,
                    lag,
                    margin,
                });
            }
        }
    }
    let best = best?;
    let key = &best.id.0;
    let step = &steps[best.step];
    let answered = match best.lag {
        Some(lag) => format!("it was answered up to {} later", duration(lag)),
        None => "in some it was never answered".to_owned(),
    };
    let mut evidence = vec![
        format!(
            "{} was still open when {} acted, {}; {answered}",
            shown(key),
            step_name(step),
            of(best.runs, here.len(), "failing runs"),
        ),
        format!(
            "{} it was answered before the step acted ({} before, at the closest)",
            of(passing.len(), passing.len(), "passing runs"),
            duration(best.margin)
        ),
    ];
    // The step that sent it, in the first failing run.
    let sent = requests(first.run)
        .into_iter()
        .find(|(id, _)| *id == best.id)
        .map(|(_, r)| r.sent_ms);
    let sender = sent.and_then(|at| steps[..=best.step].iter().rev().find(|s| start(s) <= at));
    let wait = format!("wait --response '{}'", response_pattern(key));
    let hint = match sender {
        Some(sender) if sender.at != step.at => {
            evidence.push(format!("{} sent it", step_name(sender)));
            format!(
                "add `{wait}` after {} (which sends it), or an `expect` of what its answer shows before {}",
                sender.at, step.at
            )
        }
        _ => format!(
            "wait for it before {}: `{wait}` after the step that sends it, or an `expect` of what its answer shows",
            step.at
        ),
    };
    Some(Found {
        cause: Cause::Unsettled,
        explains: best.runs,
        evidence,
        hint: Some(hint),
    })
}

/// When `run` answered request `id`, if it did before `limit`.
fn answered_by(run: &RunRecord, id: &RequestId, limit: i64) -> Option<i64> {
    requests(run)
        .into_iter()
        .find(|(o, _)| o == id)?
        .1
        .answered_ms
        .filter(|a| *a <= limit)
}

/// When the step a failing run failed at ended, or the same step in a
/// passing run; the run's end when there is none.
fn limit_of(run: &RunRecord, key: Option<(&str, &str, usize)>) -> i64 {
    key.and_then(|key| find_step(&run.steps, key))
        .map_or(i64::MAX, |j| end(&run.steps[j]))
}

/// Requests raced: two API requests were answered in one order in failing
/// runs, and in the other in every passing run, both before the failing
/// step ended.
fn race(here: &[Failed<'_>], passing: &[&RunRecord]) -> Option<Found> {
    let first = here.iter().find(|f| f.at.is_some())?;
    let key = step_key(&first.run.steps, first.at?);
    let api: Vec<RequestId> = requests(first.run)
        .into_iter()
        .filter(|(_, r)| matches!(r.kind.as_str(), "fetch" | "xhr"))
        .map(|(id, _)| id)
        .collect();
    let order = |run: &RunRecord, a: &RequestId, b: &RequestId| {
        let limit = limit_of(run, Some(key));
        let (ta, tb) = (answered_by(run, a, limit)?, answered_by(run, b, limit)?);
        Some(ta.cmp(&tb))
    };
    // (explains, first answered in failing runs, second)
    let mut best: Option<(usize, &RequestId, &RequestId)> = None;
    for (x, a) in api.iter().enumerate() {
        for b in &api[x + 1..] {
            for (early, late) in [(a, b), (b, a)] {
                let n = here
                    .iter()
                    .filter(|f| order(f.run, early, late) == Some(Ordering::Less))
                    .count();
                let flipped = passing
                    .iter()
                    .all(|p| order(p, early, late) == Some(Ordering::Greater));
                if n > 0 && flipped && best.is_none_or(|b| n > b.0) {
                    best = Some((n, early, late));
                }
            }
        }
    }
    let (n, early, late) = best?;
    Some(Found {
        cause: Cause::Race,
        explains: n,
        evidence: vec![
            format!(
                "{} was answered before {}, {}",
                shown(&early.0),
                shown(&late.0),
                of(n, here.len(), "failing runs")
            ),
            format!(
                "{} it was answered after it",
                of(passing.len(), passing.len(), "passing runs")
            ),
        ],
        hint: Some(format!(
            "the page depends on which answer comes last: make it not depend on the order, or wait for both (`wait --response '{}'` and `wait --response '{}'`) before {}; a `delay` on one mock replays the failing order",
            response_pattern(&early.0),
            response_pattern(&late.0),
            first.failure.at
        )),
    })
}

/// A moving target: the failing step's element did not stop moving, was
/// covered, or kept being replaced.
fn moving_target(here: &[Failed<'_>], setting: &Setting) -> Option<Found> {
    let moved = |f: &&Failed<'_>| {
        let error = &f.failure.error;
        let check = error
            .details
            .as_ref()
            .and_then(|d| d["check"].as_str())
            .unwrap_or_default();
        matches!(error.kind, ErrorKind::Obscured | ErrorKind::RefDetached)
            || matches!(check, "stable" | "attached" | "hit")
    };
    let moving: Vec<&Failed<'_>> = here.iter().filter(moved).collect();
    let first = moving.first()?;
    let error = &first.failure.error;
    let details = error.details.as_ref();
    let check = details
        .and_then(|d| d["check"].as_str())
        .unwrap_or_default();
    let mut evidence = vec![format!(
        "{} the step could not act: {}",
        of(moving.len(), here.len(), "failing runs"),
        error.message
    )];
    let covering = details.and_then(|d| {
        let c = &d["covering"];
        let role = c["role"]
            .as_str()
            .filter(|r| !r.is_empty() && *r != "generic");
        let name = c["name"].as_str().filter(|n| !n.is_empty());
        match (role, name) {
            (Some(role), Some(name)) => Some(format!("role={role}[name=\"{name}\"]")),
            (Some(role), None) => Some(format!("role={role}")),
            _ => c["tag"].as_str().map(str::to_owned),
        }
    });
    if let Some(covering) = &covering {
        evidence.push(format!("covered by {covering}"));
    }
    let hint = match (check, &covering) {
        ("stable", _) if setting.animations => {
            "the element animates: turn animations off (disable_animations = true in [e2e.determinism], the default), or wait for the animation to end before the step".to_owned()
        }
        ("stable", _) => "the element moves: wait until it stops before the step (an `expect` of the state it ends in)".to_owned(),
        ("attached", _) => "the page replaces the element: wait for what replaces it before the step (an `expect` of the new content)".to_owned(),
        (_, Some(covering)) => format!("wait until the covering element is gone before the step: `expect '{covering}' hidden`"),
        _ => "wait until the element can be acted on before the step (an `expect` of what is in front of it going away)".to_owned(),
    };
    Some(Found {
        cause: Cause::MovingTarget,
        explains: moving.len(),
        evidence,
        hint: Some(hint),
    })
}

/// Different requests: failing runs sent a request no passing run sent
/// (or not one every passing run sent), or one that no mock answered, up
/// to the end of the failing step.
fn different_requests(here: &[Failed<'_>], passing: &[&RunRecord]) -> Option<Found> {
    let first = here.iter().find(|f| f.at.is_some())?;
    let key = step_key(&first.run.steps, first.at?);
    let sent = |run: &RunRecord| -> Vec<(String, bool)> {
        let limit = limit_of(run, Some(key));
        let mut keys: Vec<(String, bool)> = Vec::new();
        for r in sent(run).filter(|r| r.sent_ms <= limit) {
            let k = request_key(r);
            match keys.iter_mut().find(|(o, _)| *o == k) {
                Some((_, unmatched)) => *unmatched |= r.unmatched,
                None => keys.push((k, r.unmatched)),
            }
        }
        keys
    };
    let failed: Vec<Vec<(String, bool)>> = here.iter().map(|f| sent(f.run)).collect();
    let passed: Vec<Vec<(String, bool)>> = passing.iter().map(|p| sent(p)).collect();
    let has = |set: &[(String, bool)], k: &str| set.iter().any(|(o, _)| o == k);
    let unmatched_in = |set: &[(String, bool)], k: &str| set.iter().any(|(o, u)| o == k && *u);
    let mut best: Option<(usize, String, &str)> = None;
    let mut consider = |n: usize, k: &str, how: &'static str| {
        if n > 0 && best.as_ref().is_none_or(|b| n > b.0) {
            best = Some((n, k.to_owned(), how));
        }
    };
    let mut keys: Vec<&String> = Vec::new();
    for (k, _) in failed.iter().flatten() {
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    for k in &keys {
        if !passed.iter().any(|p| has(p, k)) {
            consider(failed.iter().filter(|f| has(f, k)).count(), k, "extra");
        }
        if !passed.iter().any(|p| unmatched_in(p, k)) {
            consider(
                failed.iter().filter(|f| unmatched_in(f, k)).count(),
                k,
                "unmatched",
            );
        }
    }
    if let Some(p) = passed.first() {
        for (k, _) in p {
            if passed.iter().all(|o| has(o, k)) {
                consider(failed.iter().filter(|f| !has(f, k)).count(), k, "missing");
            }
        }
    }
    let (n, k, how) = best?;
    let runs = of(n, here.len(), "failing runs");
    let (evidence, hint) = match how {
        "extra" => (
            format!(
                "{} was sent {runs}, and in none of the {} passing runs",
                shown(&k),
                passing.len()
            ),
            format!(
                "find why the page sends {} only sometimes; if it should, give every run the same answer for it (a mock in the test's @mocks)",
                shown(&k)
            ),
        ),
        "unmatched" => (
            format!("no mock answered {} {runs}", shown(&k)),
            format!(
                "add a mock for {} (`mock explain` says why none matched)",
                shown(&k)
            ),
        ),
        _ => (
            format!(
                "every passing run sent {}, and it was not sent {runs}",
                shown(&k)
            ),
            format!(
                "find what makes the page skip {} in some runs, or wait for it before the step",
                shown(&k)
            ),
        ),
    };
    Some(Found {
        cause: Cause::DifferentRequests,
        explains: n,
        evidence: vec![evidence],
        hint: Some(hint),
    })
}

/// Time or randomness: determinism is off, and the failing assertion saw
/// another value in each failing run (a date, an id).
fn time_or_randomness(here: &[Failed<'_>], setting: &Setting) -> Option<Found> {
    if setting.unfixed.is_empty() {
        return None;
    }
    let actual: Vec<String> = here
        .iter()
        .filter_map(|f| {
            let value = &f.failure.error.details.as_ref()?["actual"];
            match value {
                serde_json::Value::Null => None,
                serde_json::Value::String(s) => Some(s.clone()),
                other => Some(other.to_string()),
            }
        })
        .collect();
    let mut distinct: Vec<&String> = Vec::new();
    for value in &actual {
        if !distinct.contains(&value) {
            distinct.push(value);
        }
    }
    if distinct.len() < 2 {
        return None;
    }
    let values: Vec<String> = distinct
        .iter()
        .take(3)
        .map(|v| format!("\"{}\"", cut(v, 40)))
        .collect();
    let more = if distinct.len() > 3 { ", …" } else { "" };
    let seen = if distinct.len() == actual.len() {
        "another value in each failing run".to_owned()
    } else {
        format!(
            "{} different values in {} failing runs",
            distinct.len(),
            actual.len()
        )
    };
    let off = setting.unfixed.join(" and ");
    let verb = if setting.unfixed.len() == 1 {
        "is"
    } else {
        "are"
    };
    Some(Found {
        cause: Cause::Time,
        explains: distinct.len(),
        evidence: vec![
            format!("the failing step saw {seen}: {}{more}", values.join(", ")),
            format!("{off} {verb} off in [e2e.determinism]"),
        ],
        hint: Some(format!(
            "turn {off} back on in [e2e.determinism], or expect a pattern that fits every value (`text /…/`)"
        )),
    })
}

/// Shared state (real backends only: mock state is per test): the runs
/// passed until one failed, then every later run failed too.
fn shared_state(runs: &[RunRecord], setting: &Setting) -> Option<Found> {
    if !setting.real {
        return None;
    }
    let mut ordered: Vec<&RunRecord> = runs.iter().collect();
    ordered.sort_by_key(|r| r.number);
    let first_failed = ordered.iter().position(|r| r.failure.is_some())?;
    if first_failed == 0 || ordered[first_failed..].iter().any(|r| r.failure.is_none()) {
        return None;
    }
    let failed = ordered.len() - first_failed;
    let label = &ordered[first_failed].label;
    Some(Found {
        cause: Cause::SharedState,
        explains: failed,
        evidence: vec![format!(
            "every run before {label} passed, and every run from it on failed: an earlier run left data behind in the backend"
        )],
        hint: Some(
            "clean up in @after, or give each run data of its own (`${test.id}` in names); `--jobs 1` shows whether tests running at once share it".to_owned(),
        ),
    })
}

/// Unknown: the first step whose result differs between the first
/// failing run and the first passing run.
fn unknown(here: &[Failed<'_>], passing: &[&RunRecord]) -> Found {
    let mut found = Found {
        cause: Cause::Unknown,
        explains: 0,
        evidence: Vec::new(),
        hint: None,
    };
    let (Some(failed), Some(passed)) = (here.first(), passing.first()) else {
        return found;
    };
    let (f, p) = (failed.run, *passed);
    found.hint = Some(format!(
        "compare {} with {}",
        p.dir.join(super::artifacts::STEPS).display(),
        f.dir.join(super::artifacts::STEPS).display()
    ));
    let result = |s: &StepRecord| {
        let text = if s.ok {
            s.output.clone()
        } else {
            s.error
                .as_ref()
                .map(|e| e.message.clone())
                .unwrap_or_default()
        };
        normalized(&text)
    };
    let end = failed.at.map_or(f.steps.len(), |k| k + 1);
    for i in 0..end {
        let step = &f.steps[i];
        if step.hook.as_deref() == Some("setup") {
            continue;
        }
        let Some(j) = find_step(&p.steps, step_key(&f.steps, i)) else {
            continue;
        };
        let other = &p.steps[j];
        let (mine, theirs) = (result(step), result(other));
        if step.ok != other.ok || mine != theirs {
            found.evidence.push(format!(
                "the first step whose result differs is {}: {} (passed) gave \"{}\", {} (failed) gave \"{}\"",
                step_name(step),
                p.label,
                cut(&theirs, 80),
                f.label,
                cut(&mine, 80)
            ));
            break;
        }
    }
    found
}

/// A step's result without what timing decides: durations become `…`.
fn normalized(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find(|c: char| c.is_ascii_digit()) {
        let (before, from) = rest.split_at(i);
        out.push_str(before);
        let digits = from
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(from.len());
        let unit = if from[digits..].starts_with("ms") {
            2
        } else {
            usize::from(
                from[digits..].starts_with('s')
                    && !from[digits + 1..].starts_with(|c: char| c.is_alphanumeric()),
            )
        };
        let boundary = !before.ends_with(|c: char| c.is_alphanumeric());
        if unit > 0 && boundary {
            out.push('…');
        } else {
            out.push_str(&from[..digits + unit]);
        }
        rest = &from[digits + unit..];
    }
    out.push_str(rest);
    out
}

/// The lines under a flaky test's line in the text reporter.
pub(super) fn text(flake: &TestFlake) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "    flaky: {}", headline(flake));
    let _ = writeln!(text, "    {}", cause_line(flake));
    for line in &flake.evidence {
        let _ = writeln!(text, "      {line}");
    }
    if let Some(hint) = &flake.hint {
        let _ = writeln!(text, "      fix: {hint}");
    }
    text
}

/// `likely cause: requests raced (confidence 0.85)`, or `cause unknown`.
fn cause_line(flake: &TestFlake) -> String {
    if flake.cause == Cause::Unknown.name() {
        "cause unknown".to_owned()
    } else {
        format!(
            "likely cause: {} (confidence {:.2})",
            flake.summary, flake.confidence
        )
    }
}

/// `3 of 20 runs failed at checkout.cdp:14:3 (click "Pay")`.
fn headline(flake: &TestFlake) -> String {
    let runs = if flake.runs == 1 { "run" } else { "runs" };
    let mut line = format!("{} of {} {runs} failed", flake.failed_runs, flake.runs);
    if let (Some(at), Some(step)) = (&flake.at, &flake.step) {
        let _ = write!(line, " at {at} (`{}`)", cut(step, 60));
    }
    line
}

/// The "Flaky" section of `report.md`: what the runs show, and which
/// runs to compare.
pub(super) fn section(flake: &TestFlake, runs: &[RunRecord]) -> String {
    let mut md = String::from("\n## Flaky\n\n");
    let _ = writeln!(md, "{}; the others passed.\n", headline(flake));
    if flake.cause == Cause::Unknown.name() {
        md.push_str("Cause unknown: no rule fits.\n\n");
    } else {
        let _ = writeln!(
            md,
            "Likely cause: **{}** (confidence {:.2})\n",
            flake.summary, flake.confidence
        );
    }
    for line in &flake.evidence {
        let _ = writeln!(md, "- {line}");
    }
    if let Some(hint) = &flake.hint {
        let _ = writeln!(md, "\nFix: {hint}");
    }
    if !flake.others.is_empty() {
        md.push_str("\nAlso possible:\n\n");
        for other in &flake.others {
            let _ = writeln!(
                md,
                "- {} (confidence {:.2}): {}",
                other.summary,
                other.confidence,
                other.evidence.first().map_or("", String::as_str)
            );
        }
    }
    let list = |failed: bool| {
        let labels: Vec<String> = runs
            .iter()
            .filter(|r| r.failure.is_some() == failed)
            .map(|r| r.label.clone())
            .collect();
        let shown = labels
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        if labels.len() > 10 {
            format!("{shown}, … ({} in all)", labels.len())
        } else {
            shown
        }
    };
    let _ = writeln!(md, "\nFailed: {}. Passed: {}.", list(true), list(false));
    if let (Some(f), Some(p)) = (
        runs.iter().find(|r| r.failure.is_some()),
        runs.iter().find(|r| r.failure.is_none()),
    ) {
        let _ = writeln!(
            md,
            "Each run's steps.jsonl and requests.txt (with when each request was sent and answered) are in its folder: compare {} with {}.",
            f.dir.display(),
            p.dir.display()
        );
    }
    md
}

#[cfg(test)]
mod tests;
