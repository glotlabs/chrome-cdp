//! A load run's virtual users (plan 23.1): one tokio task each. A user
//! keeps one Chrome of the pool; each iteration it takes the next of the
//! journeys in turn, in a fresh browser context with one tab: the file's
//! `@before` steps, the test's own and its `@after` steps (after a
//! failure too), then the context closes. Users loop until the run's
//! duration is up, their iterations are done, or the run stops; all
//! waiting is on the runtime's timers.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use engine::{ContextCloseRequest, ContextNewRequest, Engine};
use protocol::schema::SessionState;
use protocol::{ErrorInfo, ErrorKind, Hint, Output};
use serde_json::Value;
use tokio::time::Instant;

use super::plan::{self, How, Journey, LoadPlan, Step};
use super::pool::Pool;
use super::record::{self, Failed, Records, StepLine};
use super::step::{self, Call};
use super::think::Think;
use crate::test::vars::Resolver;

/// What every user of the run shares.
#[derive(Debug)]
pub(super) struct Shared {
    pub(super) plan: Arc<LoadPlan>,
    pub(super) pool: Pool,
    pub(super) records: Records,
    /// A bare `think`'s time.
    pub(super) think: Think,
    /// Set to stop starting iterations (Ctrl-C, a web server that exits).
    pub(super) stop: Arc<AtomicBool>,
    /// When users stop starting iterations.
    pub(super) ends: Instant,
    /// Each user's most iterations.
    pub(super) iterations: Option<u32>,
    /// What each file's `@before-all` saved (`http --save`), by file;
    /// set once they ran, before the users start.
    pub(super) file_saved: std::sync::OnceLock<Vec<BTreeMap<String, Value>>>,
    /// The seed of the users' think times.
    pub(super) seed: u64,
}

impl Shared {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

/// User `vu` (from 1): starts at `start`, in Chrome `browser`, and runs
/// iterations until the run is over for it.
pub(super) async fn run(shared: Arc<Shared>, vu: u32, browser: usize, start: Instant) {
    tokio::time::sleep_until(start).await;
    let mut random = Random::new(shared.seed ^ u64::from(vu).wrapping_mul(0x9e37_79b9));
    let journeys = shared.plan.journeys.len();
    let mut n: u64 = 0;
    while !shared.stopped()
        && Instant::now() < shared.ends
        && shared.iterations.is_none_or(|most| n < u64::from(most))
    {
        n += 1;
        let first = usize::try_from(vu - 1).unwrap_or(0);
        let journey = (first + usize::try_from(n - 1).unwrap_or(0)) % journeys;
        let user = User {
            shared: &shared,
            vu,
            iteration: n,
            journey,
        };
        Box::pin(user.iteration(browser, &mut random)).await;
    }
}

/// One iteration of a user.
struct User<'a> {
    shared: &'a Shared,
    vu: u32,
    iteration: u64,
    /// Index into the plan's journeys.
    journey: usize,
}

/// A step's error, and which step it was.
struct StepFailure {
    at: String,
    text: String,
    error: ErrorInfo,
}

impl User<'_> {
    fn journey(&self) -> &Journey {
        &self.shared.plan.journeys[self.journey]
    }

    async fn iteration(&self, browser: usize, random: &mut Random) {
        let shared = self.shared;
        let journey = self.journey();
        let started = Instant::now();
        let started_ms = epoch_ms();
        let setup = |error: ErrorInfo| {
            self.record_runner_step(started, &error);
            shared.records.iteration(self.journey, true);
        };
        let engine = match shared.pool.engine(browser).await {
            Ok(engine) => engine,
            Err(error) => return setup(error),
        };
        let context = match new_context(&engine, &shared.plan).await {
            Ok(context) => context,
            Err(error) => return setup(crashed(&engine, error)),
        };
        if let Some(emulate) = &journey.emulate {
            let made = engine::in_context(
                Some(context.clone()),
                Box::pin(engine.emulate(emulate.clone())),
            )
            .await;
            if let Err(error) = made {
                return setup(crashed(&engine, error));
            }
        }
        let hooks = &shared.plan.hooks[journey.file];
        let mut deadline = started + journey.timeout;
        let mut saved = shared
            .file_saved
            .get()
            .and_then(|files| files.get(journey.file))
            .cloned()
            .unwrap_or_default();
        let test_id = format!(
            "{}-u{}-{}",
            shared.plan.plan.run.run_id, self.vu, self.iteration
        );
        let mut failure = None;
        let parts: [(&[Step], Option<&str>); 2] =
            [(&hooks.before, Some("before")), (&journey.steps, None)];
        'parts: for (steps, hook) in parts {
            for step in steps {
                let ran = self
                    .step(
                        &engine,
                        &context,
                        step,
                        hook,
                        &test_id,
                        &mut saved,
                        random,
                        &mut deadline,
                    )
                    .await;
                if let Err(error) = ran {
                    failure = Some(StepFailure {
                        at: step.at.clone(),
                        text: step.text.clone(),
                        error,
                    });
                    break 'parts;
                }
            }
        }
        for step in &hooks.after {
            if engine.state() != SessionState::Running {
                break;
            }
            let ran = self
                .step(
                    &engine,
                    &context,
                    step,
                    Some("after"),
                    &test_id,
                    &mut saved,
                    random,
                    &mut deadline,
                )
                .await;
            if let Err(error) = ran
                && failure.is_none()
            {
                failure = Some(StepFailure {
                    at: step.at.clone(),
                    text: step.text.clone(),
                    error,
                });
            }
        }
        if let Some(failed) = &failure
            && engine.state() == SessionState::Running
            && let Some(n) = shared.records.failure()
        {
            let dir = record::failure_dir(&shared.records.failures_dir, n, self.vu, self.iteration);
            let about = Failed {
                journey: &journey.name,
                place: &journey.place,
                vu: self.vu,
                iteration: self.iteration,
                at: &failed.at,
                step: &failed.text,
                error: &failed.error,
            };
            Box::pin(record::capture(&engine, &context, &dir, &about, started_ms)).await;
        } else if failure.is_some() {
            // A crashed Chrome's failure has no page to keep; counted.
            let _ = shared.records.failure();
        }
        if engine.state() == SessionState::Running {
            let _ = engine
                .context_close(ContextCloseRequest {
                    ids: vec![context],
                    all: false,
                    timeout_ms: CLOSE_TIMEOUT_MS,
                    background: false,
                })
                .await;
        }
        shared.records.iteration(self.journey, failure.is_some());
    }

    /// Runs `step` of the iteration and records it; an `http --save`
    /// keeps its answer in `saved`. A `think` moves the deadline on by
    /// the time it waited.
    #[allow(clippy::too_many_arguments)]
    async fn step(
        &self,
        engine: &Engine,
        context: &str,
        step: &Step,
        hook: Option<&str>,
        test_id: &str,
        saved: &mut BTreeMap<String, Value>,
        random: &mut Random,
        deadline: &mut Instant,
    ) -> Result<(), ErrorInfo> {
        let start = Instant::now();
        let result = match self.call(step, test_id, saved) {
            Err(error) => Err(error),
            Ok(_) if start >= *deadline => Err(out_of_time()),
            Ok(Call::Think(time)) => {
                let waited = time.unwrap_or(self.shared.think).pick(random.unit());
                tokio::time::sleep(waited).await;
                *deadline += waited;
                Ok(())
            }
            Ok(call) => {
                let saves = call.saves().map(str::to_owned);
                match Box::pin(step::run(engine, context, call)).await {
                    Ok(out) => {
                        if let Some(name) = saves {
                            saved.insert(name, body(out));
                        }
                        if Instant::now() > *deadline {
                            Err(out_of_time())
                        } else {
                            Ok(())
                        }
                    }
                    Err(error) => Err(crashed(engine, error)),
                }
            }
        };
        let end = Instant::now();
        let error = result.as_ref().err();
        let (outcome, kind) = step::result(error);
        self.shared.records.step(&StepLine {
            vu: self.vu,
            iteration: self.iteration,
            journey: &self.journey().place,
            hook,
            at: &step.at,
            command: &step.text,
            start_ms: self.shared.records.since(start),
            end_ms: self.shared.records.since(end),
            result: outcome,
            kind,
            error: error.map(|e| e.message.as_str()),
        });
        result
    }

    /// `step`'s call: made once, or now from its words with this
    /// iteration's variables.
    fn call(
        &self,
        step: &Step,
        test_id: &str,
        saved: &BTreeMap<String, Value>,
    ) -> Result<Call, ErrorInfo> {
        match &step.how {
            How::Fixed(call) => Ok(call.clone()),
            How::Each(prepared) => {
                let resolver =
                    Resolver::running(&prepared.vars, &self.shared.plan.plan.run, test_id, saved);
                let words = plan::expand(prepared, &resolver)?;
                let cli = crate::script::command(&words, &self.shared.plan.global)?;
                step::call(&cli)
            }
        }
    }

    /// Records what the runner did around the steps (a new browser
    /// context) when it failed.
    fn record_runner_step(&self, start: Instant, error: &ErrorInfo) {
        let (outcome, kind) = step::result(Some(error));
        let journey = self.journey();
        self.shared.records.step(&StepLine {
            vu: self.vu,
            iteration: self.iteration,
            journey: &journey.place,
            hook: Some("setup"),
            at: &journey.place,
            command: "context new",
            start_ms: self.shared.records.since(start),
            end_ms: self.shared.records.since(Instant::now()),
            result: outcome,
            kind,
            error: Some(error.message.as_str()),
        });
    }
}

/// How long closing an iteration's context may take.
const CLOSE_TIMEOUT_MS: u64 = 10_000;

/// A fresh browser context with one tab; its id.
pub(super) async fn new_context(engine: &Engine, plan: &LoadPlan) -> Result<String, ErrorInfo> {
    let made = Box::pin(engine.context_new(ContextNewRequest {
        url: None,
        base_url: plan.global.base_url.clone(),
        timeout_ms: CLOSE_TIMEOUT_MS,
        setup: Vec::new(),
        ahead: false,
    }))
    .await?;
    made.data["context"]["id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| ErrorInfo::new(ErrorKind::ProtocolError, "context new gave no id"))
}

/// Runs a file's `@before-all` or `@after-all` once, in a context of its
/// own in Chrome 0; what its `http --save` steps saved.
pub(super) async fn hook_once(
    shared: &Shared,
    file: usize,
    steps: &[Step],
    name: &'static str,
    mut saved: BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, ErrorInfo> {
    if steps.is_empty() {
        return Ok(saved);
    }
    let journey = shared
        .plan
        .journeys
        .iter()
        .position(|j| j.file == file)
        .unwrap_or(0);
    let user = User {
        shared,
        vu: 0,
        iteration: 0,
        journey,
    };
    let engine = shared.pool.engine(0).await?;
    let context = new_context(&engine, &shared.plan).await?;
    let test_id = format!("{}-f{}", shared.plan.plan.run.run_id, file + 1);
    let mut deadline = Instant::now() + user.journey().timeout;
    let mut random = Random::new(shared.seed);
    let mut result = Ok(());
    for step in steps {
        let ran = user
            .step(
                &engine,
                &context,
                step,
                Some(name),
                &test_id,
                &mut saved,
                &mut random,
                &mut deadline,
            )
            .await;
        if let Err(error) = ran {
            let at = step.at.clone();
            result = Err(ErrorInfo {
                message: format!("@{name} failed at {at}: {}", error.message),
                ..error
            });
            break;
        }
    }
    let _ = engine
        .context_close(ContextCloseRequest {
            ids: vec![context],
            all: false,
            timeout_ms: CLOSE_TIMEOUT_MS,
            background: false,
        })
        .await;
    result.map(|()| saved)
}

/// The error of a step that ran past its iteration's time.
fn out_of_time() -> ErrorInfo {
    ErrorInfo::new(
        ErrorKind::Timeout,
        "the iteration ran out of time before this step ended",
    )
    .hint(Hint::text(
        "give it longer: @timeout 60s in the test, or timeout = \"60s\" in [e2e] (think steps do not count)",
    ))
}

/// `error`, as a crash when the Chrome went away meanwhile (a page
/// command then fails in many ways).
fn crashed(engine: &Engine, error: ErrorInfo) -> ErrorInfo {
    if error.kind != ErrorKind::BrowserCrashed && engine.state() == SessionState::Crashed {
        return ErrorInfo {
            kind: ErrorKind::BrowserCrashed,
            ..error
        };
    }
    error
}

/// The JSON body of an `http` step's answer.
fn body(out: Output) -> Value {
    match out.data {
        Value::Object(mut map) => map.remove("body").unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A user's random numbers (SplitMix64): its think times.
#[derive(Debug)]
pub(super) struct Random(u64);

impl Random {
    pub(super) fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// A number from 0 to 1.
    pub(super) fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        #[allow(clippy::cast_precision_loss)]
        let unit = (z >> 11) as f64 / (1u64 << 53) as f64;
        unit
    }
}

/// How long the users take to start: user `vu` of `users` starts
/// `ramp_up × (vu - 1) / users` after the first.
pub(super) fn start_offset(ramp_up: Duration, vu: u32, users: u32) -> Duration {
    if users <= 1 {
        return Duration::ZERO;
    }
    ramp_up.mul_f64(f64::from(vu - 1) / f64::from(users))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Random, start_offset};

    #[test]
    fn users_start_evenly_over_the_ramp_up() {
        let ramp = Duration::from_secs(10);
        assert_eq!(start_offset(ramp, 1, 10), Duration::ZERO);
        assert_eq!(start_offset(ramp, 6, 10), Duration::from_secs(5));
        assert_eq!(start_offset(ramp, 1, 1), Duration::ZERO);
        assert_eq!(start_offset(Duration::ZERO, 7, 10), Duration::ZERO);
    }

    #[test]
    fn random_units_spread_from_0_to_1() {
        let mut random = Random::new(42);
        let picks: Vec<f64> = (0..10_000).map(|_| random.unit()).collect();
        assert!(picks.iter().all(|u| (0.0..1.0).contains(u)));
        let mean = picks.iter().sum::<f64>() / 10_000.0;
        assert!((mean - 0.5).abs() < 0.02, "{mean}");
    }
}
