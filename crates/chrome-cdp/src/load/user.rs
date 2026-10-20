//! A load run's virtual users (plans 23.1 and 23.3): one tokio task each.
//!
//! Under `[users]` (closed), user `vu` keeps one Chrome of the pool and
//! loops while the stages' count is above `vu − 1`, until its iterations
//! are done or the run stops. Under `[arrivals]` (open), a user arrives
//! when its arrival is due, takes a free place in a Chrome, does one
//! iteration and leaves; an arrival with no free place (`max_users`) is
//! dropped and counted, never queued, and its iteration's time counts from
//! when it was due. All waiting is on the runtime's timers.
//!
//! An iteration takes a journey (by weight, or in turn for a run without
//! a profile) and the next row of its data, then runs in a fresh browser
//! context with one tab: the file's `@before` steps, the test's own and its
//! `@after` steps (after a failure too), then the context closes.
//!
//! With `[load] cache = "returning"` (plan 23.2) a closed user keeps its
//! context, cookies, storage and HTTP cache from one iteration to the
//! next, and an open model's users take over the contexts earlier users
//! of their Chrome left, as returning visitors, until the Chrome crashes
//! or a page outgrows `[load] renderer_memory`. That limit is checked
//! between steps, at most once a second per user.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use e2e::config::Cache;
use engine::{CdpRequest, ContextCloseRequest, ContextNewRequest, EmulateRequest, Engine};
use protocol::schema::SessionState;
use protocol::{ErrorInfo, ErrorKind, Hint, Output};
use serde_json::{Value, json};
use tokio::time::Instant;

use super::data::Row;
use super::halt::{Halt, Reason};
use super::plan::{self, How, Journey, LoadPlan, Step};
use super::pool::Pool;
use super::profile::{OnFailure, Users};
use super::record::{self, Ended, Failed, Records, StepLine};
use super::step::{self, Call};
use crate::test::vars::{LoadVars, Resolver, Rows};

/// What every user of the run shares.
#[derive(Debug)]
pub(super) struct Shared {
    pub(super) plan: Arc<LoadPlan>,
    pub(super) pool: Pool,
    pub(super) records: Records,
    /// The run's stop.
    pub(super) halt: Arc<Halt>,
    /// When the users started: the stages' zero.
    pub(super) started: Instant,
    /// What each file's `@before-all` saved (`http --save`), by file;
    /// set once they ran, before the users start.
    pub(super) file_saved: std::sync::OnceLock<Vec<BTreeMap<String, Value>>>,
    /// The seed of the users' random numbers.
    pub(super) seed: u64,
    /// `[load] cache`: whether a user keeps its context.
    pub(super) cache: Cache,
    /// `[load] renderer_memory`, in bytes.
    pub(super) renderer_memory: Option<u64>,
    /// The journeys' weights.
    pub(super) weights: Vec<u32>,
    /// The most users in one Chrome.
    pub(super) per_browser: u32,
    /// The users in each Chrome now (an open model's).
    pub(super) places: Vec<AtomicU32>,
    /// The contexts returning users left in each Chrome (an open model's).
    pub(super) parked: Vec<Mutex<Vec<Kept>>>,
    /// Users running now, and the most at once.
    pub(super) active: AtomicU32,
    pub(super) peak: AtomicU32,
    /// An open model's arrivals, and those dropped at `max_users`.
    pub(super) arrivals: AtomicU64,
    pub(super) dropped: AtomicU64,
    /// The thresholds as the run goes.
    pub(super) watch: super::threshold::Watch,
    /// The load generator's own limits, and how late its timers fire.
    pub(super) capacity: Arc<super::capacity::Capacity>,
}

impl Shared {
    fn stopped(&self) -> bool {
        self.halt.stopped()
    }

    /// A user starts running.
    fn arrived(&self) {
        let now = self.active.fetch_add(1, Ordering::Relaxed) + 1;
        self.peak.fetch_max(now, Ordering::Relaxed);
    }

    /// A free place in a Chrome for an arriving user, the lowest Chrome
    /// first (the ones started at once come first); `None` when every
    /// Chrome is full.
    pub(super) fn take_place(&self) -> Option<usize> {
        self.places.iter().position(|users| {
            users
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                    (n < self.per_browser).then_some(n + 1)
                })
                .is_ok()
        })
    }

    /// The users and arrivals now, for the metrics.
    pub(super) fn gauges(&self) -> super::metrics::Gauges {
        super::metrics::Gauges {
            users: self.active.load(Ordering::Relaxed),
            arrivals: self.arrivals.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
        }
    }

    /// Closes the contexts returning users left.
    pub(super) async fn close_parked(&self) {
        for parked in &self.parked {
            let contexts: Vec<Kept> =
                std::mem::take(&mut *parked.lock().unwrap_or_else(PoisonError::into_inner));
            for kept in contexts {
                close_context(&kept.engine, kept.context).await;
            }
        }
    }
}

/// Closed-model user `vu` (from 1), in Chrome `browser`: loops while the
/// stages' count is above `vu − 1`, until its iterations are done or the
/// run stops.
pub(super) async fn closed(shared: Arc<Shared>, users: Users, vu: u32, browser: usize) {
    let mut random = Random::new(shared.seed ^ u64::from(vu).wrapping_mul(0x9e37_79b9));
    let mut n: u64 = 0;
    let mut kept = None;
    let mut running = false;
    loop {
        if shared.stopped() || users.iterations.is_some_and(|most| n >= u64::from(most)) {
            break;
        }
        let now = Instant::now();
        let Some(at) = users.next_start(vu, now.saturating_duration_since(shared.started)) else {
            break;
        };
        let due = shared.started + at;
        if due > now {
            if running {
                // Below the count for now: not a user at once.
                running = false;
                shared.active.fetch_sub(1, Ordering::Relaxed);
            }
            tokio::select! {
                () = tokio::time::sleep_until(due) => {
                    shared.capacity.late(Instant::now().saturating_duration_since(due));
                }
                () = shared.halt.stopping() => break,
            }
        }
        if !running {
            running = true;
            shared.arrived();
        }
        n += 1;
        let journey = if shared.plan.in_turn {
            let first = usize::try_from(vu - 1).unwrap_or(0);
            (first + usize::try_from(n - 1).unwrap_or(0)) % shared.plan.journeys.len()
        } else {
            super::model::pick(&shared.weights, random.unit())
        };
        let user = User {
            shared: &shared,
            vu,
            iteration: n,
            journey,
        };
        Box::pin(user.iteration(browser, &mut random, &mut kept, Instant::now())).await;
    }
    if running {
        shared.active.fetch_sub(1, Ordering::Relaxed);
    }
    if let Some(kept) = kept {
        close_context(&kept.engine, kept.context).await;
    }
}

/// Open-model user `vu` (from 1), arrived at `due` with a place in Chrome
/// `browser`: one iteration, then it leaves.
pub(super) async fn arrival(shared: Arc<Shared>, vu: u32, browser: usize, due: Instant) {
    // Leaves its place even when the run cuts it short.
    let _place = Place {
        shared: &shared,
        browser,
    };
    let mut random = Random::new(shared.seed ^ u64::from(vu).wrapping_mul(0x9e37_79b9));
    let journey = super::model::pick(&shared.weights, random.unit());
    let mut kept = shared.parked[browser]
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .pop();
    let user = User {
        shared: &shared,
        vu,
        iteration: 1,
        journey,
    };
    Box::pin(user.iteration(browser, &mut random, &mut kept, due)).await;
    if let Some(kept) = kept {
        shared.parked[browser]
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(kept);
    }
}

/// An open model's user's place in its Chrome, given back when it leaves.
struct Place<'a> {
    shared: &'a Shared,
    browser: usize,
}

impl Drop for Place<'_> {
    fn drop(&mut self) {
        self.shared.places[self.browser].fetch_sub(1, Ordering::AcqRel);
        self.shared.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A returning user's context, kept from one iteration to the next.
#[derive(Debug)]
pub(super) struct Kept {
    engine: Arc<Engine>,
    /// Its Chrome's start ([`Pool::generation`]): a Chrome started again
    /// has none of the contexts of the one before.
    generation: u64,
    context: String,
    /// What the context emulates.
    emulate: Option<EmulateRequest>,
}

/// One iteration of a user.
struct User<'a> {
    shared: &'a Shared,
    vu: u32,
    iteration: u64,
    /// Index into the plan's journeys.
    journey: usize,
}

/// An iteration's own variables.
struct IterationVars {
    /// `${vu.id}`: `<run>-u<vu>`.
    vu_id: String,
    /// `${test.id}` and `${iteration.id}`: `<run>-u<vu>-<iteration>`.
    test_id: String,
    /// The row of the journey's data it took.
    row: Option<Arc<Row>>,
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

    /// Runs the iteration, due to start at `due` (its latency counts
    /// from then).
    async fn iteration(
        &self,
        browser: usize,
        random: &mut Random,
        kept: &mut Option<Kept>,
        due: Instant,
    ) {
        let shared = self.shared;
        let journey = self.journey();
        let row = match &journey.data {
            Some(data) => {
                let Some(row) = data.take(random) else {
                    shared.halt.stop(
                        Reason::DataExhausted,
                        format!(
                            "every row of {}'s data was used ({} rows, data_exhausted = \"stop\")",
                            journey.name,
                            data.len()
                        ),
                    );
                    return;
                };
                Some(row)
            }
            None => None,
        };
        let started = Instant::now();
        let started_ms = epoch_ms();
        let setup = |error: ErrorInfo| {
            self.record_runner_step(started, &error, "setup", "context new");
            self.ended(true, due, started);
        };
        let engine = match shared.pool.engine(browser).await {
            Ok(engine) => engine,
            Err(error) => return setup(error),
        };
        let generation = shared.pool.generation(browser);
        // A returning user's context, unless its Chrome is a new one or
        // the journey emulates nothing where the context does.
        let reused = match kept.take() {
            Some(k)
                if k.generation == generation
                    && Arc::ptr_eq(&k.engine, &engine)
                    && (journey.emulate.is_some() || k.emulate.is_none()) =>
            {
                Some(k)
            }
            Some(k) => {
                if k.generation == generation {
                    close_context(&k.engine, k.context).await;
                }
                None
            }
            None => None,
        };
        let (context, emulated) = if let Some(k) = reused {
            (k.context, k.emulate)
        } else {
            let making = Instant::now();
            match new_context(&engine, &shared.plan).await {
                Ok(context) => {
                    shared.records.context_made(making.elapsed());
                    (context, None)
                }
                Err(error) => return setup(crashed(&engine, error)),
            }
        };
        if let Some(emulate) = journey
            .emulate
            .as_ref()
            .filter(|e| emulated.as_ref() != Some(*e))
        {
            let made = engine::in_context(
                Some(context.clone()),
                Box::pin(engine.emulate(emulate.clone())),
            )
            .await;
            if let Err(error) = made {
                close_context(&engine, context).await;
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
        let vu_id = format!("{}-u{}", shared.plan.plan.run.run_id, self.vu);
        let vars = IterationVars {
            test_id: format!("{vu_id}-{}", self.iteration),
            vu_id,
            row,
        };
        let mut failure = None;
        let mut memory_checked = Instant::now();
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
                        &vars,
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
                if let Err(error) = self.memory(&engine, &context, &mut memory_checked).await {
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
                    &vars,
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
        if engine.state() == SessionState::Running {
            self.vitals(&engine, &context).await;
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
        let outgrown = failure
            .as_ref()
            .is_some_and(|f| step::kind(&f.error) == super::MEMORY);
        if engine.state() == SessionState::Running {
            if shared.cache == Cache::Returning && !outgrown {
                *kept = Some(Kept {
                    engine: engine.clone(),
                    generation,
                    context,
                    emulate: journey.emulate.clone().or(emulated),
                });
            } else {
                close_context(&engine, context).await;
            }
        }
        self.ended(failure.is_some(), due, started);
    }

    /// Counts the iteration, due at `due` and started at `start`, as it
    /// ends; with `on_failure = "stop-run"`, a failure past
    /// `max_failures` stops the run.
    fn ended(&self, failed: bool, due: Instant, start: Instant) {
        let shared = self.shared;
        let ended = Ended {
            journey: self.journey,
            failed,
            due,
            start,
            end: Instant::now(),
        };
        let journey = self.journey();
        let failures = shared.records.iteration(
            &ended,
            self.vu,
            self.iteration,
            &journey.place,
            &journey.name,
        );
        let plan = &shared.plan;
        if failed && plan.on_failure == OnFailure::StopRun && failures >= plan.max_failures {
            shared.halt.stop(
                Reason::MaxFailures,
                format!(
                    "{failures} iterations failed (max_failures = {}, on_failure = \"stop-run\")",
                    plan.max_failures
                ),
            );
        }
    }

    /// The Web Vitals of the page the iteration ends on, unless it
    /// reported them as it was left: an iteration's last page view.
    async fn vitals(&self, engine: &Engine, context: &str) {
        let read = engine::in_context(
            Some(context.to_owned()),
            Box::pin(engine.page_vitals(CLOSE_TIMEOUT_MS)),
        )
        .await;
        if let Ok(Some(vitals)) = read {
            self.shared.capacity.page(vitals.lag);
            self.shared.records.metrics.vitals(&vitals);
        }
    }

    /// With `[load] renderer_memory`, at most once a [`MEMORY_EVERY`]: the
    /// page's heap, and a `memory` error (recorded) when it is above the
    /// limit.
    async fn memory(
        &self,
        engine: &Engine,
        context: &str,
        checked: &mut Instant,
    ) -> Result<(), ErrorInfo> {
        let Some(limit) = self.shared.renderer_memory else {
            return Ok(());
        };
        if checked.elapsed() < MEMORY_EVERY {
            return Ok(());
        }
        let start = Instant::now();
        *checked = start;
        // A page between two documents has no heap to read: next time.
        let Ok(used) = heap(engine, context).await else {
            return Ok(());
        };
        if used <= limit {
            return Ok(());
        }
        let error = memory_error(used, limit);
        self.record_runner_step(start, &error, "memory", "renderer memory");
        Err(error)
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
        vars: &IterationVars,
        saved: &mut BTreeMap<String, Value>,
        random: &mut Random,
        deadline: &mut Instant,
    ) -> Result<(), ErrorInfo> {
        let start = Instant::now();
        let result = match self.call(step, vars, saved) {
            Err(error) => Err(error),
            Ok(_) if start >= *deadline => Err(out_of_time()),
            Ok(Call::Think(time)) => {
                let waited = time.unwrap_or(self.shared.plan.think).pick(random.unit());
                let until = Instant::now() + waited;
                tokio::time::sleep_until(until).await;
                self.shared
                    .capacity
                    .late(Instant::now().saturating_duration_since(until));
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
        vars: &IterationVars,
        saved: &BTreeMap<String, Value>,
    ) -> Result<Call, ErrorInfo> {
        match &step.how {
            How::Fixed(call) => Ok(call.clone()),
            How::Each(prepared) => {
                let resolver = Resolver::running(
                    &prepared.vars,
                    &self.shared.plan.plan.run,
                    &vars.test_id,
                    saved,
                )
                .in_load(LoadVars {
                    vu: &vars.vu_id,
                    iteration: &vars.test_id,
                    rows: vars.row.as_deref().map_or(Rows::None, Rows::Row),
                });
                let words = plan::expand(prepared, &resolver)?;
                let cli = crate::script::command(&words, &self.shared.plan.global)?;
                step::call(&cli)
            }
        }
    }

    /// Records what the runner did around the steps when it failed: the
    /// `setup` of a new browser context, a `memory` check.
    fn record_runner_step(&self, start: Instant, error: &ErrorInfo, hook: &str, command: &str) {
        let (outcome, kind) = step::result(Some(error));
        let journey = self.journey();
        self.shared.records.step(&StepLine {
            vu: self.vu,
            iteration: self.iteration,
            journey: &journey.place,
            hook: Some(hook),
            at: &journey.place,
            command,
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

/// How often a user's page heap is read, at most, with `[load]
/// renderer_memory`.
const MEMORY_EVERY: Duration = Duration::from_secs(1);

/// Closes `context`, if its Chrome still runs.
async fn close_context(engine: &Engine, context: String) {
    if engine.state() != SessionState::Running {
        return;
    }
    let _ = engine
        .context_close(ContextCloseRequest {
            ids: vec![context],
            all: false,
            timeout_ms: CLOSE_TIMEOUT_MS,
            background: false,
        })
        .await;
}

/// The heap of `context`'s page, in bytes: V8's, Blink's, and the array
/// buffers' and strings' storage outside it (`Runtime.getHeapUsage`).
async fn heap(engine: &Engine, context: &str) -> Result<u64, ErrorInfo> {
    let out = engine::in_context(
        Some(context.to_owned()),
        Box::pin(engine.cdp(CdpRequest {
            method: Some("Runtime.getHeapUsage".into()),
            params: serde_json::Map::new(),
            target: engine::Target::Page,
            tab: None,
            frame: None,
            listen: Vec::new(),
            count: 0,
            all_sessions: false,
            timeout_ms: CLOSE_TIMEOUT_MS,
            max_chars: 0,
        })),
    )
    .await?;
    let size = |name: &str| out.data[name].as_f64().unwrap_or(0.0).max(0.0);
    let bytes = size("usedSize") + size("embedderHeapUsedSize") + size("backingStorageSize");
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(bytes as u64)
}

/// The `memory` error of a page whose heap of `used` bytes is above
/// `limit`.
fn memory_error(used: u64, limit: u64) -> ErrorInfo {
    ErrorInfo::new(
        ErrorKind::ProtocolError,
        format!(
            "the page's heap is {}, above [load] renderer_memory ({})",
            protocol::format::size(used),
            protocol::format::size(limit)
        ),
    )
    .hint(Hint::text(
        "the app may leak memory as the journey loops; or raise renderer_memory in [load]",
    ))
    .details(json!({ "kind": super::MEMORY, "heap": used, "limit": limit }))
}

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
    let vars = IterationVars {
        vu_id: test_id.clone(),
        test_id,
        row: None,
    };
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
                &vars,
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
    close_context(&engine, context).await;
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

/// A user's random numbers (SplitMix64): its journeys, rows and think
/// times.
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

#[cfg(test)]
mod tests {
    use super::Random;

    #[test]
    fn random_units_spread_from_0_to_1() {
        let mut random = Random::new(42);
        let picks: Vec<f64> = (0..10_000).map(|_| random.unit()).collect();
        assert!(picks.iter().all(|u| (0.0..1.0).contains(u)));
        let mean = picks.iter().sum::<f64>() / 10_000.0;
        assert!((mean - 0.5).abs() < 0.02, "{mean}");
    }
}
