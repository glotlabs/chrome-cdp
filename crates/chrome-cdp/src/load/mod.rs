//! `chrome-cdp load` (plan 23): many users at once, each a real browser
//! running the app's own JavaScript, through the project's scenario tests
//! against a real backend.
//!
//! A run is described by a [`profile`] (`e2e/load/<name>.toml`: the
//! journeys by weight with their [`data`], an open or closed [`model`],
//! think times and what a failure does), or by flags for a run without
//! one. It works out its plan as `test` does ([`plan`]: the config, the
//! backend, the scenarios and their variables), checks every step as a
//! load step before any browser starts, and refuses the mock backend;
//! `--plan` stops there and says what would run. Then it drives its
//! Chromes in-process, without the daemon: a [`pool`] of Chromes, each an
//! [`engine::Engine`] of its own over one CDP connection, and one tokio
//! task per virtual user ([`user`]) on a multi-threaded runtime sized to
//! the CPUs. Each user's iterations run in fresh browser contexts of its
//! Chrome, their steps call the engine directly ([`step`]), leanly, and
//! every step's timing and result goes to the [`record`]s. Ctrl-C stops
//! the run, giving the running iterations `[load] grace` ([`halt`]).
//!
//! The [`metrics`] take every request, step, iteration and page view, by
//! group, a second at a time; while the run goes a [`status`] line says how
//! it goes, and at its end it writes `summary.json` (what `--json` prints)
//! and `metrics.jsonl` (each second of each group), the [`report`]s
//! (`report.md`, and `report.html` with charts), each beside an earlier
//! run's numbers with `--compare` ([`compare`]). `--metrics-listen` serves
//! the live numbers to Prometheus ([`listen`]).
//!
//! It also watches itself ([`capacity`]): the machine's CPU and memory, its
//! Chromes' and its own, and how late its timers fire. When the machine is
//! saturated the status line says so, the thresholds leave those seconds
//! out, and with `on_saturation = "abort"` the run stops, as its numbers
//! would measure the client, not the site. `--calibrate` ramps the users
//! until either the machine or the site gives, to find how many it holds
//! ([`calibrate`]).

mod calibrate;
mod capacity;
mod compare;
mod data;
mod halt;
pub(crate) mod init;
mod lengths;
mod listen;
mod metrics;
mod model;
mod plan;
mod pool;
pub(crate) mod profile;
mod record;
mod report;
mod status;
mod step;
pub(crate) mod think;
mod threshold;
mod user;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use engine::OpenRequest;
use protocol::schema::{LoadData, LoadJourney, LoadMetric, LoadStage, LoadStopped, LoadTimes};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Mode, Output};
use tokio::task::JoinSet;

use crate::cli::{DEFAULT_TIMEOUT, GlobalArgs, LoadArgs, LoadBrowser};
use crate::output;
use halt::Halt;
use lengths::Lengths;
use metrics::Metrics;
use plan::LoadPlan;
use pool::Pool;
use profile::{Arrivals, Model};
use record::Records;
use status::Status;
use user::Shared;

/// How many users share a Chrome by default (`--users-per-browser`):
/// `bench load` found steps slower with 25 (plan 23.1's decisions log).
pub(crate) const USERS_PER_BROWSER: u32 = 10;

/// Above this many users in one Chrome, its one browser process and CDP
/// connection slow every step of theirs down; the run warns.
const CROWDED_BROWSER: u32 = 25;

/// The error kind of a step whose Chrome crashed.
pub(crate) const BROWSER_CRASH: &str = "browser_crash";

/// The error kind of a user whose page outgrew `[load] renderer_memory`.
pub(crate) const MEMORY: &str = "memory";

/// How many console entries and requests each user's tab keeps, for a
/// kept failure's `console.txt` and `requests.txt`: more than an
/// iteration makes, far fewer than `open`'s, as a returning user's tab
/// lives for the whole run.
const LOGGED: usize = 300;

/// The stack of the runtime's threads.
const STACK_SIZE: usize = 8 * 1024 * 1024;

/// Where load runs keep their files, in the artifacts directory.
const RUNS_DIR: &str = "load";

/// An open model starts the Chromes for this much more than the users it
/// expects at once, the rest when needed.
const EXPECTED_MARGIN: f64 = 1.25;

/// `summary.json`'s name in the run's folder.
const SUMMARY: &str = "summary.json";

/// How often the metrics take a second.
const SECOND: Duration = Duration::from_secs(1);

pub(crate) fn run(args: &LoadArgs, global: &GlobalArgs) -> u8 {
    let mode = global.mode();
    let result = load(args, global, mode).and_then(|data| {
        let text = text(&data);
        if data.planned {
            return Ok(Output::typed(&data, text));
        }
        if let Some(stopped) = data.stopped.as_ref().filter(|s| s.reason == "saturated") {
            // The run ran: its numbers come first, then why they do not count.
            if mode == Mode::Text {
                output::stdout(&format!("{text}\n"));
            }
            let mut error = ErrorInfo::new(
                ErrorKind::Saturated,
                format!("the load generator saturated: {}", stopped.message),
            );
            if let Some(c) = &data.capacity {
                error = error.note(
                    "held",
                    format!(
                        "{} users ({} per Chrome) on these journeys without saturating, of {} \
                         at most",
                        c.held_users, c.held_per_browser, c.peak_users
                    ),
                );
                if let Some(hint) = &c.hint {
                    error = error.hint(Hint::text(hint.clone()));
                }
            }
            return Err(error.details(serde_json::to_value(&data).unwrap_or_default()));
        }
        if let Some(stopped) = data.stopped.as_ref().filter(|s| s.reason == "max_failures") {
            return Err(ErrorInfo::new(
                ErrorKind::ExpectFailed,
                format!("the run stopped: {}", stopped.message),
            )
            .note("errors", errors_line(&data.errors))
            .hint(Hint::text(match &data.failures_dir {
                Some(dir) => format!("the site looks down; see what the users saw in {dir}"),
                None => "the site looks down; run a journey as a test: chrome-cdp test <path>"
                    .to_owned(),
            }))
            .details(serde_json::to_value(&data).unwrap_or_default()));
        }
        if data.iterations > 0 && data.failed == data.iterations {
            return Err(ErrorInfo::new(
                ErrorKind::ExpectFailed,
                format!(
                    "every iteration failed ({} of {})",
                    data.failed, data.iterations
                ),
            )
            .note("errors", errors_line(&data.errors))
            .hint(Hint::text(match &data.failures_dir {
                Some(dir) => format!("see what the users saw in {dir}"),
                None => "run the journey as a test first: chrome-cdp test <path>".to_owned(),
            }))
            .details(serde_json::to_value(&data).unwrap_or_default()));
        }
        let failed: Vec<&protocol::schema::LoadThreshold> =
            data.thresholds.iter().filter(|t| !t.passed).collect();
        if !failed.is_empty() {
            // The run ran: its numbers come first, then what failed.
            if mode == Mode::Text {
                output::stdout(&format!("{text}\n"));
            }
            let mut error = ErrorInfo::new(
                ErrorKind::ThresholdFailed,
                format!(
                    "{} of {} {} failed",
                    failed.len(),
                    data.thresholds.len(),
                    if data.thresholds.len() == 1 {
                        "threshold"
                    } else {
                        "thresholds"
                    }
                ),
            );
            for t in failed {
                error = error.note("failed", threshold_line(t));
            }
            let report = data
                .files
                .as_ref()
                .map_or_else(|| data.dir.clone(), |f| f.report.clone());
            return Err(error
                .hint(Hint::text(format!("the run's numbers are in {report}")))
                .details(serde_json::to_value(&data).unwrap_or_default()));
        }
        Ok(Output::typed(&data, text).limit(args.max_chars, "--max-chars 0 for all of it"))
    });
    output::print(mode, result)
}

/// How a run uses its Chromes.
#[derive(Debug, Clone, Copy)]
struct Shape {
    /// The most users at once: a closed model's peak, an open model's
    /// `max_users`.
    capacity: u32,
    /// The Chromes, and the most users in each.
    browsers: u32,
    per_browser: u32,
    /// How many Chromes start before the users; the others start when
    /// first needed.
    eager: u32,
    /// The users an open model is expected to reach at once, from the
    /// journeys' lengths in an earlier run; a closed model's peak.
    expected: Option<u32>,
}

impl Shape {
    fn of(plan: &LoadPlan, args: &LoadArgs, lengths: &Lengths) -> Result<Self, ErrorInfo> {
        let (capacity, expected) = match &plan.model {
            Model::Closed(users) => (users.peak(), Some(users.peak())),
            Model::Open(arrivals) => (
                arrivals.max_users,
                mean_length(plan, lengths).map(|mean| model::expected_users(arrivals, mean)),
            ),
        };
        let browsers = browsers(capacity, args)?;
        let per_browser = capacity.div_ceil(browsers);
        let eager = match (&plan.model, expected) {
            (Model::Open(_), Some(users)) => {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let users = (f64::from(users) * EXPECTED_MARGIN).ceil() as u32;
                users.div_ceil(per_browser).clamp(1, browsers)
            }
            _ => browsers,
        };
        Ok(Self {
            capacity,
            browsers,
            per_browser,
            eager,
            expected,
        })
    }
}

/// The mean length of an iteration, by the journeys' weights, from an
/// earlier run on the same backend; `None` when a journey has none.
fn mean_length(plan: &LoadPlan, lengths: &Lengths) -> Option<Duration> {
    let mut total = 0.0;
    let mut weights = 0.0;
    for journey in &plan.journeys {
        let length = lengths.get(&plan.plan.backend.name, &journey_key(plan, journey))?;
        total += length.mean_ms * f64::from(journey.weight);
        weights += f64::from(journey.weight);
    }
    (weights > 0.0).then(|| Duration::from_secs_f64(total / weights / 1000.0))
}

/// A journey's key in the lengths file.
fn journey_key(plan: &LoadPlan, journey: &plan::Journey) -> String {
    let file = crate::test::durations::key(
        &plan.plan.config.root,
        &plan.plan.scenarios[journey.file].path,
    );
    lengths::key(&file, &journey.name)
}

/// The run, from its plan to its counts.
fn load(args: &LoadArgs, global: &GlobalArgs, mode: Mode) -> Result<LoadData, ErrorInfo> {
    let plan = Arc::new(LoadPlan::make(args, global)?);
    let linted = crate::lint::for_run(
        &plan.plan.config,
        &plan.plan.scenarios,
        &plan.plan.backend.name,
        global,
    )?;
    if linted.errors > 0 {
        return Err(crate::lint::failed(&linted, "lint"));
    }
    let runs = plan.plan.config.artifacts.join(RUNS_DIR);
    let mut lengths = Lengths::read(&runs);
    let shape = Shape::of(&plan, args, &lengths)?;
    if shape.per_browser > CROWDED_BROWSER && mode == Mode::Text {
        output::stderr(&format!(
            "warning: {} users in one Chrome slow each other down past {CROWDED_BROWSER}; \
             give it more Chromes (--users-per-browser {USERS_PER_BROWSER})\n",
            shape.per_browser
        ));
    }
    let mut calibrations = calibrate::Calibrations::read(&runs);
    let calibrated = plan
        .profile
        .as_deref()
        .and_then(|p| calibrations.get(p, &plan.plan.backend.name))
        .cloned();
    if let Some(found) = calibrated.as_ref().filter(|_| !args.calibrate)
        && let Some(warning) = calibrate::warning(shape.expected.unwrap_or(shape.capacity), found)
        && mode == Mode::Text
    {
        output::stderr(&format!("warning: {warning}\n"));
    }
    let earlier = match &args.compare {
        Some(arg) => {
            let cwd = std::env::current_dir().unwrap_or_default();
            Some(compare::read(arg, &runs, &cwd)?)
        }
        None => None,
    };
    let timeout = global.timeout_or(DEFAULT_TIMEOUT).max(DEFAULT_TIMEOUT);
    // `--plan` starts nothing, so it needs no Chrome: only which one.
    let chrome = if args.plan {
        None
    } else {
        Some(find_browser(args, timeout)?)
    };
    if mode == Mode::Text {
        for warning in chrome.iter().flat_map(|c| &c.warnings) {
            output::stderr(&format!("warning: {warning}\n"));
        }
    }
    let kind = match &chrome {
        Some(chrome) => browser_kind(chrome),
        None => planned_kind(args)?,
    };
    let run_id = plan.plan.run.run_id.clone();
    let dir = runs.join(&run_id);
    let mut data = LoadData {
        run: run_id.clone(),
        dir: dir.display().to_string(),
        backend: plan.plan.backend.name.clone(),
        planned: args.plan,
        profile: plan.profile.clone(),
        model: plan.model.name().to_owned(),
        stages: stages(&plan.model),
        journeys: plan
            .journeys
            .iter()
            .map(|j| LoadJourney {
                place: j.place.clone(),
                name: j.name.clone(),
                weight: j.weight,
                data: j.data.as_ref().map(|d| d.file.clone()),
                iterations: 0,
                failed: 0,
                iteration_ms: LoadTimes::default(),
                previous_ms: args
                    .plan
                    .then(|| lengths.get(&plan.plan.backend.name, &journey_key(&plan, j)))
                    .flatten()
                    .map(|l| (l.mean_ms * 10.0).round() / 10.0),
            })
            .collect(),
        users: shape.expected.unwrap_or(shape.capacity),
        max_users: match &plan.model {
            Model::Open(a) => Some(a.max_users),
            Model::Closed(_) => None,
        },
        arrivals: 0,
        dropped: 0,
        iteration_ms: LoadTimes::default(),
        stopped: None,
        browsers: shape.browsers,
        browsers_started: shape.eager,
        browser: kind.to_owned(),
        users_per_browser: shape.per_browser,
        duration_ms: 0,
        iterations: 0,
        failed: 0,
        steps: protocol::schema::LoadSteps::default(),
        errors: BTreeMap::new(),
        browser_crashes: 0,
        cache: plan.plan.config.load.cache.as_str().to_owned(),
        contexts: 0,
        context_setup_ms: 0.0,
        failures_kept: 0,
        failures_dir: None,
        raw: None,
        requests: None,
        endpoints: Vec::new(),
        step_times: Vec::new(),
        vitals: Vec::new(),
        request_errors: BTreeMap::new(),
        exceptions: Vec::new(),
        exceptions_total: 0,
        files: None,
        thresholds: plan
            .thresholds
            .iter()
            .map(|t| threshold::result(t, None, false, Some("not checked: --plan".to_owned())))
            .collect(),
        compare: None,
        metrics_listen: args.metrics_listen.map(|a| a.to_string()),
        capacity: None,
        calibration: None,
    };
    if args.plan {
        if let Model::Open(arrivals) = &plan.model {
            data.arrivals = model::total_arrivals(arrivals);
        }
        return Ok(data);
    }
    let open = OpenRequest {
        chrome,
        headed: Some(args.headed),
        load: true,
        console_buffer: Some(LOGGED),
        network_buffer: Some(LOGGED),
        timeout_ms: millis(timeout),
        ..OpenRequest::default()
    };
    let cannot_write = |e: std::io::Error| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot write the run's files in {}: {e}", dir.display()),
        )
    };
    std::fs::create_dir_all(&dir).map_err(cannot_write)?;
    let load_config = &plan.plan.config.load;
    let grouper = metrics::group::Grouper::new(
        &load_config.endpoints,
        load_config.group_assets.unwrap_or(true),
        plan.plan.backend.base_url.as_deref(),
    );
    let metrics = Arc::new(Metrics::new(grouper, Some(&dir)).map_err(cannot_write)?);
    let records = Records::new(
        &dir,
        plan.journeys.len(),
        args.raw,
        args.keep_failures,
        metrics,
    )
    .map_err(cannot_write)?;
    if mode == Mode::Text {
        output::stdout(&starting(&plan, args, shape, kind));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let servers = std::sync::OnceLock::new();
    let servers_started = AtomicBool::new(false);
    let test_plan = &plan.plan;
    // `watch`: Ctrl-C is the run's to take; the web servers stay up until
    // its report is written.
    let ran = crate::test::with_servers(
        test_plan,
        &servers,
        &stop,
        mode,
        &servers_started,
        true,
        || {
            crate::test::with_backend(test_plan, mode, || {
                drive(Drive {
                    plan: plan.clone(),
                    args,
                    shape,
                    listen: args.metrics_listen,
                    open: open.clone(),
                    records,
                    stop: stop.clone(),
                    mode,
                })
            })
        },
    );
    let ran = ran??;
    let records = &ran.records;
    records.flush();
    let counts = records.counts();
    let summary = records.metrics.summary(ran.took);
    for ((journey, tally), out) in plan
        .journeys
        .iter()
        .zip(&counts.journeys)
        .zip(&mut data.journeys)
    {
        out.iterations = tally.iterations;
        out.failed = tally.failed;
        if let Some(metric) = summary.journeys.iter().find(|m| m.name == journey.place) {
            out.iteration_ms = times(metric);
        }
        if tally.iterations > 0 {
            #[allow(clippy::cast_precision_loss)]
            let mean_ms = tally.ran.as_secs_f64() * 1000.0 / tally.iterations as f64;
            lengths.set(
                &plan.plan.backend.name,
                &journey_key(&plan, journey),
                lengths::Length {
                    mean_ms: (mean_ms * 10.0).round() / 10.0,
                    iterations: tally.iterations,
                    run: run_id.clone(),
                },
            );
        }
    }
    lengths.write();
    data.users = match &plan.model {
        Model::Closed(users) => users.peak(),
        Model::Open(_) => ran.peak,
    };
    data.arrivals = ran.arrivals;
    data.dropped = ran.dropped;
    data.iteration_ms = times(&summary.iterations);
    data.stopped.clone_from(&ran.stopped);
    if ran.listened.is_some() {
        data.metrics_listen.clone_from(&ran.listened);
    }
    data.duration_ms = millis(ran.took);
    data.iterations = counts.iterations;
    data.failed = counts.failed;
    data.steps = counts.steps;
    data.errors = counts.errors;
    data.browser_crashes = ran.restarts;
    data.browsers_started = ran.started;
    data.contexts = counts.contexts;
    data.context_setup_ms = mean_ms(counts.context_setup, counts.contexts);
    data.failures_kept = records.kept();
    data.failures_dir = (records.kept() > 0).then(|| records.failures_dir.display().to_string());
    data.raw = records.raw_path.as_ref().map(|p| p.display().to_string());
    data.requests = summary.requests;
    data.endpoints = summary.endpoints;
    data.step_times = summary.steps;
    data.vitals = summary.vitals;
    data.request_errors = summary.request_errors;
    data.exceptions = summary.exceptions;
    data.exceptions_total = summary.exceptions_total;
    data.thresholds = verdicts(&plan, &ran, records);
    let mut capacity = ran.capacity.clone();
    if !capacity.saturations.is_empty() {
        capacity.hint = Some(
            capacity::Advice {
                held: capacity.held_users,
                peak: capacity.peak_users,
                rate: match &plan.model {
                    Model::Open(arrivals) => Some(model::peak_rate(arrivals)),
                    Model::Closed(_) => None,
                },
                third_party_blocked: load_config.block.contains(&e2e::config::Block::ThirdParty),
                browser: kind,
            }
            .text(),
        );
    }
    if !args.calibrate
        && let Some(found) = calibrated.as_ref().filter(|c| c.first == "generator")
    {
        capacity.calibrated_users = Some(found.held_users);
    }
    if args.calibrate {
        let calibration = calibration(&ran, &capacity, &records.metrics.timeline(), &calibrations);
        if let Some(profile) = &plan.profile {
            calibrations.set(
                profile,
                &plan.plan.backend.name,
                calibrate::Calibration {
                    first: calibration.first.clone(),
                    users: calibration.users,
                    held_users: calibration.held_users,
                    held_per_browser: calibration.held_per_browser,
                    browser: kind.to_owned(),
                    users_per_browser: shape.per_browser,
                    cpus: std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
                    run: run_id.clone(),
                },
            );
            calibrations.write();
        }
        data.calibration = Some(calibration);
    }
    data.capacity = Some(capacity);
    data.compare = earlier.as_ref().map(|e| compare::compare(e, &data));
    let path = |name: &str| dir.join(name).display().to_string();
    data.files = Some(protocol::schema::LoadFiles {
        summary: path(SUMMARY),
        metrics: path(metrics::METRICS),
        report: path(report::MARKDOWN),
        html: path(report::HTML),
    });
    if let Some((pattern, n)) = &summary.overflow
        && mode == Mode::Text
    {
        output::stderr(&format!(
            "warning: the requests made more than {} endpoints; {n} more went into `other`, \
             most like {pattern}: add it to [load] endpoints\n",
            metrics::group::MOST_GROUPS
        ));
    }
    write_summary(&dir, &data);
    crate::test::artifacts::write(&dir.join(report::MARKDOWN), &report::markdown(&data));
    crate::test::artifacts::write(
        &dir.join(report::HTML),
        &report::html(&data, &records.metrics.timeline()),
    );
    Ok(data)
}

/// What a `--calibrate` run found, from what it saw first.
fn calibration(
    ran: &Ran,
    capacity: &protocol::schema::LoadCapacity,
    timeline: &[metrics::Second],
    file: &calibrate::Calibrations,
) -> protocol::schema::LoadCalibration {
    let (ramp_to, over) = match ran.ramp {
        Some(ramp) => ramp,
        None => (ran.peak, Duration::ZERO),
    };
    let found = ran.found.clone().unwrap_or(calibrate::Found {
        first: calibrate::First::Neither,
        from: 0,
        users: ran.peak,
        message: format!(
            "neither gave: the ramp ended at {} users, within the machine's limits and with the \
             site answering",
            ran.peak
        ),
    });
    let (held_users, held_per_browser) = match found.first {
        calibrate::First::Generator => (capacity.held_users, capacity.held_per_browser),
        calibrate::First::Server => {
            let held = calibrate::held_before(timeline, Some(found.from));
            (held, held.div_ceil(ran.started.max(1)))
        }
        calibrate::First::Neither => (ran.peak, ran.peak.div_ceil(ran.started.max(1))),
    };
    protocol::schema::LoadCalibration {
        first: found.first.as_str().to_owned(),
        message: found.message,
        users: found.users,
        held_users,
        held_per_browser,
        ramp_to,
        ramp_rate: if over.is_zero() {
            0.0
        } else {
            (f64::from(ramp_to.saturating_sub(1)) / over.as_secs_f64() * 100.0).round() / 100.0
        },
        file: file.path().display().to_string(),
    }
}

/// Each threshold over the whole run.
fn verdicts(plan: &LoadPlan, ran: &Ran, records: &Records) -> Vec<protocol::schema::LoadThreshold> {
    let gauges = metrics::Gauges {
        users: 0,
        arrivals: ran.arrivals,
        dropped: ran.dropped,
    };
    let seconds = ran.took.as_secs_f64();
    let metrics = &records.metrics;
    plan.thresholds
        .iter()
        .enumerate()
        .map(|(i, th)| {
            let measured = metrics.value(&th.target, th.written.stat, seconds, gauges);
            let note = match (&measured, &th.target) {
                (None, threshold::Target::Endpoint(name)) => {
                    let names = metrics.endpoint_names();
                    let close =
                        protocol::suggest::similar(name, names.iter().map(String::as_str), 3);
                    Some(if close.is_empty() {
                        format!("no request had the endpoint {name}")
                    } else {
                        format!(
                            "no request had the endpoint {name}; the closest that did: {}",
                            close.join(", ")
                        )
                    })
                }
                (Some(None), _) if ran.capacity.saturated_s > 0 => Some(format!(
                    "{} had no value outside the {} saturated seconds",
                    th.written.group, ran.capacity.saturated_s
                )),
                (Some(None), _) => Some(format!("{} had no value to measure", th.written.group)),
                _ => None,
            };
            threshold::result(th, measured.flatten(), ran.aborted.contains(&i), note)
        })
        .collect()
}

/// Writes `summary.json`: the run's data, as `--json` prints it.
fn write_summary(dir: &std::path::Path, data: &LoadData) {
    if let Ok(text) = serde_json::to_string_pretty(data) {
        crate::test::artifacts::write(&dir.join(SUMMARY), &text);
    }
}

/// The model's stages, for the data.
fn stages(model: &Model) -> Vec<LoadStage> {
    match model {
        Model::Open(a) => a
            .stages
            .iter()
            .map(|s| LoadStage {
                kind: "arrivals".into(),
                from: s.from,
                to: s.to,
                duration_ms: millis(s.duration),
            })
            .collect(),
        Model::Closed(u) => u
            .stages
            .iter()
            .map(|s| LoadStage {
                kind: "users".into(),
                from: f64::from(s.from),
                to: f64::from(s.to),
                duration_ms: millis(s.duration),
            })
            .collect(),
    }
}

/// A group's times, as `iteration_ms` gives them.
fn times(metric: &LoadMetric) -> LoadTimes {
    LoadTimes {
        p50: metric.p50,
        p90: metric.p90,
        p95: metric.p95,
        p99: metric.p99,
        max: metric.max,
    }
}

/// The Chrome the users run (`--browser`): chrome-headless-shell unless
/// `--browser chrome` or `--headed`, else full Chrome, with a warning
/// when the shell was wanted and is not installed. `$CHROME_CDP_CHROME`
/// beats both.
fn find_browser(args: &LoadArgs, timeout: Duration) -> Result<browser::Chrome, ErrorInfo> {
    let host = browser::Host::current();
    let explicit = host.var(browser::CHROME_ENV).is_some();
    let shell = if explicit {
        None
    } else {
        browser::locate_headless_shell(&host)
    };
    match pick(args.browser, args.headed, explicit, shell.is_some())? {
        Pick::Shell => shell.ok_or_else(|| {
            ErrorInfo::new(ErrorKind::ChromeNotFound, "chrome-headless-shell went away")
        }),
        Pick::Chrome => {
            let mut chrome = crate::install::find_chrome(None, false, timeout)?;
            let shell_asked = args.browser == Some(LoadBrowser::HeadlessShell);
            if explicit && shell_asked && !chrome.headless_shell {
                chrome.warnings.insert(
                    0,
                    format!(
                        "--browser headless-shell, but {} names {}, which is not \
                         chrome-headless-shell: the users run in it",
                        browser::CHROME_ENV,
                        chrome.path.display()
                    ),
                );
            }
            Ok(chrome)
        }
        Pick::ChromeInstead => {
            let mut chrome = crate::install::find_chrome(None, false, timeout)?;
            chrome.warnings.insert(
                0,
                format!(
                    "chrome-headless-shell is not installed, so the users run in full Chrome, \
                     which takes more memory per user; install it: {}",
                    Cmd::new("install").switch("headless-shell")
                ),
            );
            Ok(chrome)
        }
    }
}

/// Which browser a load run starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    /// The installed chrome-headless-shell.
    Shell,
    /// The Chrome `open` would start ($CHROME_CDP_CHROME's, if set).
    Chrome,
    /// Full Chrome, since the shell is not installed: with a warning.
    ChromeInstead,
}

/// What to start for `--browser asked` (and `--headed`), with or without
/// `$CHROME_CDP_CHROME` (`explicit`) and an installed shell.
fn pick(
    asked: Option<LoadBrowser>,
    headed: bool,
    explicit: bool,
    shell_installed: bool,
) -> Result<Pick, ErrorInfo> {
    if headed && asked == Some(LoadBrowser::HeadlessShell) {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            "chrome-headless-shell cannot show windows",
        )
        .hint("drop --headed, or use --browser chrome"));
    }
    let wanted = asked.unwrap_or(if headed {
        LoadBrowser::Chrome
    } else {
        LoadBrowser::HeadlessShell
    });
    Ok(match wanted {
        LoadBrowser::Chrome => Pick::Chrome,
        _ if explicit => Pick::Chrome,
        LoadBrowser::HeadlessShell if shell_installed => Pick::Shell,
        LoadBrowser::HeadlessShell if asked.is_none() => Pick::ChromeInstead,
        LoadBrowser::HeadlessShell => {
            return Err(ErrorInfo::new(
                ErrorKind::ChromeNotFound,
                "chrome-headless-shell is not installed",
            )
            .hint(
                Hint::run(Cmd::new("install").switch("headless-shell"))
                    .or("--browser chrome runs the users in full Chrome"),
            ));
        }
    })
}

/// How many Chromes the run uses for `users` at once: `--browsers`, else
/// enough for `--users-per-browser` each, never more than there are
/// users.
fn browsers(users: u32, args: &LoadArgs) -> Result<u32, ErrorInfo> {
    let users = users.max(1);
    let needed = users.div_ceil(args.users_per_browser);
    match args.browsers {
        None => Ok(needed),
        Some(n) if n.saturating_mul(args.users_per_browser) < users => Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!(
                "--browsers {n} with --users-per-browser {} hold {} users, not {users}",
                args.users_per_browser,
                n * args.users_per_browser,
            ),
        )
        .hint(Hint::text(format!(
            "use --browsers {needed}, or a larger --users-per-browser"
        )))),
        Some(n) => Ok(n.min(users)),
    }
}

/// The browser kind a run would start, without looking for full Chrome.
fn planned_kind(args: &LoadArgs) -> Result<&'static str, ErrorInfo> {
    let host = browser::Host::current();
    let explicit = host.var(browser::CHROME_ENV).is_some();
    let shell = !explicit && browser::locate_headless_shell(&host).is_some();
    Ok(match pick(args.browser, args.headed, explicit, shell)? {
        Pick::Shell => "chrome-headless-shell",
        Pick::Chrome | Pick::ChromeInstead => "chrome",
    })
}

/// `chrome-headless-shell` or `chrome`.
fn browser_kind(chrome: &browser::Chrome) -> &'static str {
    if chrome.headless_shell {
        "chrome-headless-shell"
    } else {
        "chrome"
    }
}

/// `1 Chrome`, `2 users`.
fn count(n: impl Into<u64>, one: &str, many: &str) -> String {
    let n = n.into();
    format!("{n} {}", if n == 1 { one } else { many })
}

/// What the run is about to do: `load: 50 users in 2 Chromes
/// (chrome-headless-shell), 1 journey, 1m against backend local
/// (http://localhost:4174)`, or with a profile `load: e2e/load/shop.toml,
/// open, 3 stages over 13m, at most 400 users in up to 40 Chromes …`.
fn starting(plan: &LoadPlan, args: &LoadArgs, shape: Shape, kind: &str) -> String {
    let backend = &plan.plan.backend;
    let journeys = count(
        u64::try_from(plan.journeys.len()).unwrap_or(0),
        "journey",
        "journeys",
    );
    let mut text = match &plan.profile {
        None => {
            let mut text = format!(
                "load: {} in {} ({kind}), {journeys}, {}",
                count(shape.capacity, "user", "users"),
                count(shape.browsers, "Chrome", "Chromes"),
                protocol::format::duration(args.duration.unwrap_or(plan::DURATION))
            );
            if let Some(n) = args.iterations {
                let _ = write!(text, " (at most {n} iterations each)");
            }
            text
        }
        Some(profile) => {
            let stages = match &plan.model {
                Model::Open(a) => a.stages.len(),
                Model::Closed(u) => u.stages.len(),
            };
            let users = match &plan.model {
                Model::Open(_) => format!(
                    "at most {} in up to {}",
                    count(shape.capacity, "user", "users"),
                    count(shape.browsers, "Chrome", "Chromes")
                ),
                Model::Closed(_) => format!(
                    "up to {} in {}",
                    count(shape.capacity, "user", "users"),
                    count(shape.browsers, "Chrome", "Chromes")
                ),
            };
            format!(
                "load: {profile}, {} model, {} over {}, {users} ({kind}), {journeys}",
                plan.model.name(),
                count(u64::try_from(stages).unwrap_or(0), "stage", "stages"),
                protocol::format::duration(plan.model.duration()),
            )
        }
    };
    let _ = write!(text, " against backend {}", backend.name);
    if let Some(url) = &backend.base_url {
        let _ = write!(text, " ({url})");
    }
    text.push('\n');
    text
}

/// What [`drive`] needs.
struct Drive<'a> {
    plan: Arc<LoadPlan>,
    args: &'a LoadArgs,
    shape: Shape,
    /// `--metrics-listen`.
    listen: Option<std::net::SocketAddr>,
    open: OpenRequest,
    records: Records,
    stop: Arc<AtomicBool>,
    mode: Mode,
}

/// What the users did.
struct Ran {
    records: Records,
    /// Chromes started again.
    restarts: u64,
    /// Chromes started at all.
    started: u32,
    /// From the first user's start to the last iteration's end.
    took: Duration,
    peak: u32,
    arrivals: u64,
    dropped: u64,
    stopped: Option<LoadStopped>,
    /// The thresholds that stopped the run, by index.
    aborted: Vec<usize>,
    /// Where `--metrics-listen` listened (its port, when 0 was asked).
    listened: Option<String>,
    /// The load generator's own use and limits.
    capacity: protocol::schema::LoadCapacity,
    /// `--calibrate`: what it found, and the users and length of its ramp.
    found: Option<calibrate::Found>,
    ramp: Option<(u32, Duration)>,
}

/// Runs the users on a runtime of the run's own: starts the Chromes,
/// runs the `@before-all` hooks, the users, then the `@after-all` hooks.
fn drive(drive: Drive<'_>) -> Result<Ran, ErrorInfo> {
    let threads = std::thread::available_parallelism().map_or(2, std::num::NonZero::get);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        // Page commands nest deep futures; the daemon runs them on a main
        // thread's stack.
        .thread_stack_size(STACK_SIZE)
        .thread_name("load")
        .enable_all()
        .build()
        .map_err(|e| {
            ErrorInfo::new(
                ErrorKind::LaunchFailed,
                format!("cannot start the runtime: {e}"),
            )
        })?;
    runtime.block_on(users(drive))
}

async fn users(drive: Drive<'_>) -> Result<Ran, ErrorInfo> {
    let Drive {
        plan,
        args,
        shape,
        listen,
        open,
        records,
        stop,
        mode,
    } = drive;
    let halt = Arc::new(Halt::new(stop));
    let grace = plan.plan.config.load.grace();
    let listener = tokio::spawn(halt.clone().listen(move |n| {
        if mode != Mode::Text {
            return;
        }
        Status::say(&if n == 1 {
            format!(
                "stopping: no new users; the running iterations have {} to finish \
                 (Ctrl-C again stops them at once)\n",
                protocol::format::duration(grace)
            )
        } else {
            "stopping now\n".to_owned()
        });
    }));
    let scraped = match listen {
        Some(addr) => match listen::bind(addr).await {
            Ok(l) => {
                if let Ok(bound) = l.local_addr() {
                    output::stderr(&format!(
                        "serving the live numbers on http://{bound}/metrics\n"
                    ));
                }
                Some(l)
            }
            Err(error) => {
                listener.abort();
                return Err(error);
            }
        },
        None => None,
    };
    let name = format!("load-{}", plan.plan.run.run_id);
    let metrics = records.metrics.clone();
    let watched = Arc::new(capacity::Capacity::new(plan.plan.config.load.capacity));
    let sink: engine::LoadSink = Arc::new({
        let watched = watched.clone();
        move |event| match event {
            engine::LoadEvent::Request(request) => metrics.request(&request),
            engine::LoadEvent::Vitals(vitals) => {
                watched.page(vitals.lag);
                metrics.vitals(&vitals);
            }
            engine::LoadEvent::Exception(message) => metrics.exception(&message),
        }
    });
    let started = Pool::start(
        &name,
        shape.browsers,
        shape.eager,
        open,
        &plan.blocked,
        &sink,
    )
    .await;
    let pool = match started {
        Ok(pool) => pool,
        Err(error) => {
            listener.abort();
            return Err(error);
        }
    };
    let mut records = records;
    records.start();
    let browsers = pool.len();
    let shared = Arc::new(Shared {
        plan: plan.clone(),
        pool,
        started: records.started,
        records,
        halt: halt.clone(),
        file_saved: std::sync::OnceLock::new(),
        seed: seed(&plan.plan.run.run_id),
        cache: plan.plan.config.load.cache,
        renderer_memory: plan.plan.config.load.renderer_memory,
        weights: plan.journeys.iter().map(|j| j.weight).collect(),
        per_browser: shape.per_browser,
        places: (0..browsers).map(|_| AtomicU32::new(0)).collect(),
        parked: (0..browsers).map(|_| Mutex::new(Vec::new())).collect(),
        active: AtomicU32::new(0),
        peak: AtomicU32::new(0),
        arrivals: AtomicU64::new(0),
        dropped: AtomicU64::new(0),
        watch: threshold::Watch::default(),
        capacity: watched,
        calibrating: args.calibrate.then(user::Calibrating::default),
    });
    let status = (mode == Mode::Text).then(Status::new);
    let ticker = tokio::spawn(tick(shared.clone(), status));
    let listened = scraped
        .as_ref()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.to_string());
    let serving = scraped.map(|l| tokio::spawn(listen::serve(l, shared.clone())));
    let result = run_users(shared.clone(), grace).await;
    ticker.abort();
    let _ = ticker.await;
    if let Some(serving) = serving {
        serving.abort();
        let _ = serving.await;
    }
    shared
        .records
        .metrics
        .finish(shared.gauges(), shared.capacity.saturated());
    if let Some(status) = status {
        status.end();
    }
    listener.abort();
    shared.close_parked().await;
    let Ok(shared) = Arc::try_unwrap(shared) else {
        return Err(ErrorInfo::new(
            ErrorKind::ProtocolError,
            "a load run's user outlived the run",
        ));
    };
    shared.pool.close().await;
    let (took, cut_short) = result?;
    let peak = shared.peak.load(Ordering::Relaxed);
    let capacity = shared.capacity.summary(took.as_secs(), peak);
    let stopped = halt.reason().map(|(reason, message)| LoadStopped {
        reason: reason.as_str().to_owned(),
        message,
        cut_short,
    });
    Ok(Ran {
        restarts: shared.pool.restarts(),
        started: shared.pool.started(),
        took,
        peak,
        capacity,
        found: shared.calibrating.as_ref().and_then(|c| {
            c.found
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }),
        ramp: shared
            .calibrating
            .as_ref()
            .and_then(|_| match &shared.plan.model {
                Model::Closed(users) => users.stages.first().map(|s| (s.to, s.duration)),
                Model::Open(_) => None,
            }),
        arrivals: shared.arrivals.load(Ordering::Relaxed),
        dropped: shared.dropped.load(Ordering::Relaxed),
        stopped,
        listened,
        aborted: (0..shared.plan.thresholds.len())
            .filter(|&i| shared.watch.aborted(i))
            .collect(),
        records: shared.records,
    })
}

/// Takes a second of the run's metrics each second, with the load
/// generator's own (and says when it saturates, or stops the run then
/// with `on_saturation = "abort"`), and shows the status line, until
/// aborted.
async fn tick(shared: Arc<Shared>, status: Option<Status>) {
    let mut every = tokio::time::interval_at(shared.started + SECOND, SECOND);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let sampler = capacity::probe::Sampler::start(shared.pool.pids());
    let mut t = 0;
    loop {
        let due = every.tick().await;
        shared
            .capacity
            .late(tokio::time::Instant::now().saturating_duration_since(due));
        t += 1;
        let gauges = shared.gauges();
        let system = sampler.latest(shared.pool.pids());
        let (sample, seen) = shared
            .capacity
            .second(t, gauges.users, shared.pool.started(), system);
        let metrics = &shared.records.metrics;
        let second = metrics.tick(t, gauges, seen.mark, sample.cpu);
        metrics.line(&capacity_line(&sample, &seen));
        match &shared.calibrating {
            Some(calibrating) => calibrated(&shared, calibrating, &second, &seen, status.is_some()),
            None => saturation(&shared, &seen, status.is_some()),
        }
        watch(&shared, t, gauges, status.is_some());
        if let Some(status) = status {
            let client = status::Client {
                cpu: sample.cpu,
                saturated: shared
                    .capacity
                    .saturated()
                    .then_some(seen.over)
                    .flatten()
                    .map(capacity::Over::text),
            };
            let line = status::line(
                shared.started.elapsed(),
                &shared.plan.model,
                second.users,
                &metrics.recent(),
                &client,
            );
            status.show(t, &line);
        }
    }
}

/// A second's capacity line in `metrics.jsonl`: the sample, the limit it
/// crossed, and whether the generator is saturated (since when, at the
/// second it turns out to be).
fn capacity_line(sample: &capacity::Sample, seen: &capacity::Seen) -> serde_json::Value {
    let mut line = serde_json::to_value(sample).unwrap_or_default();
    line["group"] = serde_json::json!("capacity");
    if let Some(over) = seen.over {
        line["over"] = serde_json::json!(over.limit.as_str());
    }
    let saturated = matches!(
        seen.mark,
        metrics::Mark::Began(_) | metrics::Mark::Saturated
    );
    line["saturated"] = serde_json::json!(saturated);
    if let metrics::Mark::Began(from) = seen.mark {
        line["saturated_from"] = serde_json::json!(from);
    }
    line
}

/// A `--calibrate` run's second: stops the run when the generator
/// saturated or the site failed for long enough, whichever is first, and
/// says so (with `say`).
fn calibrated(
    shared: &Shared,
    calibrating: &user::Calibrating,
    second: &metrics::Second,
    seen: &capacity::Seen,
    say: bool,
) {
    let generator = match (seen.mark, seen.over) {
        (metrics::Mark::Began(from), Some(over)) => Some(calibrate::Found {
            first: calibrate::First::Generator,
            from,
            users: second.users,
            message: format!(
                "the load generator saturated: {} for {} in a row, at {} users",
                over.against(shared.capacity.limits()),
                protocol::format::duration(e2e::config::SATURATED_AFTER),
                second.users
            ),
        }),
        _ => None,
    };
    let server = calibrating.watch.second(second);
    let Some(found) = generator.or(server) else {
        return;
    };
    let mut kept = calibrating
        .found
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if kept.is_some() {
        return;
    }
    if say {
        Status::say(&format!("calibration: {}\n", found.message));
    }
    shared
        .halt
        .stop(halt::Reason::Calibrated, found.message.clone());
    *kept = Some(found);
}

/// Says when the load generator starts being saturated (with `say`), and
/// stops the run then with `on_saturation = "abort"`.
fn saturation(shared: &Shared, seen: &capacity::Seen, say: bool) {
    let metrics::Mark::Began(from) = seen.mark else {
        return;
    };
    let Some(over) = seen.over else {
        return;
    };
    let limits = shared.capacity.limits();
    let message = format!(
        "{} for {} in a row",
        over.against(limits),
        protocol::format::duration(e2e::config::SATURATED_AFTER)
    );
    let abort = limits.on_saturation == e2e::config::OnSaturation::Abort;
    if say {
        Status::say(&format!(
            "warning: CLIENT SATURATED: {message}, since {}: the numbers measure this machine, \
             not the site{}\n",
            status::clock(Duration::from_secs(from.saturating_sub(1))),
            if abort {
                "; stopping (on_saturation = \"abort\")"
            } else {
                "; the thresholds leave these seconds out"
            }
        ));
    }
    if abort {
        shared.halt.stop(halt::Reason::Saturated, message);
    }
}

/// Checks the thresholds over the run so far at second `t`: says one that
/// starts failing (with `say`), and stops the run for one with `abort =
/// true` that failed for 30 seconds.
fn watch(shared: &Shared, t: u64, gauges: metrics::Gauges, say: bool) {
    let thresholds = &shared.plan.thresholds;
    if thresholds.is_empty() {
        return;
    }
    let seconds = shared.started.elapsed().as_secs_f64();
    let values: Vec<Option<f64>> = thresholds
        .iter()
        .map(|th| {
            shared
                .records
                .metrics
                .value(&th.target, th.written.stat, seconds, gauges)
                .flatten()
        })
        .collect();
    for change in shared.watch.check(thresholds, t, &values) {
        match change {
            threshold::Change::Failing(i, value) => {
                if say {
                    Status::say(&format!(
                        "threshold failing: {}\n",
                        threshold::said(&thresholds[i].written, value)
                    ));
                }
            }
            threshold::Change::Abort(i) => {
                let w = &thresholds[i].written;
                shared.halt.stop(
                    halt::Reason::Threshold,
                    format!(
                        "{} {} failed for {} in a row (abort = true)",
                        w.name,
                        w.condition,
                        protocol::format::duration(threshold::ABORT_AFTER)
                    ),
                );
            }
        }
    }
}

/// The hooks and the users; how long the users ran, and how many
/// iterations were cut short.
async fn run_users(shared: Arc<Shared>, grace: Duration) -> Result<(Duration, u64), ErrorInfo> {
    let files: Vec<usize> = {
        let mut files: Vec<usize> = shared.plan.journeys.iter().map(|j| j.file).collect();
        files.sort_unstable();
        files.dedup();
        files
    };
    let mut saved = vec![BTreeMap::new(); shared.plan.plan.scenarios.len()];
    for &file in &files {
        let hooks = &shared.plan.hooks[file];
        saved[file] = user::hook_once(
            &shared,
            file,
            &hooks.before_all,
            "before-all",
            BTreeMap::new(),
        )
        .await?;
    }
    let _ = shared.file_saved.set(saved.clone());
    let mut users = JoinSet::new();
    match &shared.plan.model {
        Model::Closed(model) => {
            let browsers = shared.pool.len();
            for vu in 1..=model.peak() {
                let browser = usize::try_from(vu - 1).unwrap_or(0) % browsers;
                users.spawn(user::closed(shared.clone(), model.clone(), vu, browser));
            }
        }
        Model::Open(arrivals) => arrive(&shared, arrivals, &mut users).await?,
    }
    let cut_short = finish(&shared, &mut users, grace).await?;
    let took = shared.started.elapsed();
    let mut warnings = Vec::new();
    let after_all = files
        .iter()
        .any(|&f| !shared.plan.hooks[f].after_all.is_empty());
    if shared.halt.cut() {
        if after_all {
            warnings.push("@after-all did not run: the run stopped at once".to_owned());
        }
    } else {
        for &file in &files {
            let hooks = &shared.plan.hooks[file];
            if let Err(e) = user::hook_once(
                &shared,
                file,
                &hooks.after_all,
                "after-all",
                saved[file].clone(),
            )
            .await
            {
                warnings.push(e.message);
            }
        }
    }
    for warning in warnings {
        output::stderr(&format!("warning: {warning}\n"));
    }
    Ok((took, cut_short))
}

/// An open model's arrivals, each when due, until the stages end or the
/// run stops: a user with a free place starts its iteration; with none
/// (`max_users` running), the arrival is dropped and counted.
async fn arrive(
    shared: &Arc<Shared>,
    arrivals: &Arrivals,
    users: &mut JoinSet<()>,
) -> Result<(), ErrorInfo> {
    let mut schedule = model::Schedule::new(arrivals);
    let mut next = schedule.next();
    let mut vu = 0u32;
    while let Some(at) = next {
        let due = shared.started + at;
        tokio::select! {
            biased;
            () = shared.halt.stopping() => break,
            Some(done) = users.join_next(), if !users.is_empty() => {
                ended(done)?;
                continue;
            }
            () = tokio::time::sleep_until(due) => {}
        }
        next = schedule.next();
        shared.arrivals.fetch_add(1, Ordering::Relaxed);
        if shared.active.load(Ordering::Relaxed) >= arrivals.max_users {
            shared.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let Some(browser) = shared.take_place() else {
            shared.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        let now = shared.active.fetch_add(1, Ordering::Relaxed) + 1;
        shared.peak.fetch_max(now, Ordering::Relaxed);
        vu += 1;
        users.spawn(user::arrival(shared.clone(), vu, browser, due));
    }
    Ok(())
}

/// Waits for the users to end; once the run stops, for `grace` at most
/// (or until a second Ctrl-C), then cuts the running iterations short.
/// How many were.
async fn finish(
    shared: &Shared,
    users: &mut JoinSet<()>,
    grace: Duration,
) -> Result<u64, ErrorInfo> {
    let cut = shared.halt.cut_after(grace);
    tokio::pin!(cut);
    let mut result = Ok(());
    loop {
        tokio::select! {
            done = users.join_next() => match done {
                None => return result.map(|()| 0),
                Some(done) => {
                    if result.is_ok() {
                        result = ended(done);
                    }
                }
            },
            () = &mut cut => {
                let running = shared.active.load(Ordering::Relaxed);
                users.abort_all();
                while users.join_next().await.is_some() {}
                return result.map(|()| u64::from(running));
            }
        }
    }
}

/// A user's task's end: an error if it panicked.
fn ended(done: Result<(), tokio::task::JoinError>) -> Result<(), ErrorInfo> {
    match done {
        Err(e) if e.is_panic() => Err(ErrorInfo::new(
            ErrorKind::ProtocolError,
            format!("a load run's user stopped unexpectedly: {e}"),
        )),
        _ => Ok(()),
    }
}

/// A seed for the users' random numbers from the run's id.
pub(super) fn seed(run_id: &str) -> u64 {
    run_id.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    }) ^ SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| {
        u64::try_from(d.as_nanos() & u128::from(u64::MAX)).unwrap_or(0)
    })
}

/// `20/s`, `0.5/s`.
fn rate(r: f64) -> String {
    let text = format!("{r:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    format!("{text}/s")
}

/// A stage in words: `0..20/s for 2m`, `50 users for 5m`.
fn stage_text(stage: &LoadStage) -> String {
    let over = protocol::format::duration(Duration::from_millis(stage.duration_ms));
    if stage.kind == "arrivals" {
        if (stage.from - stage.to).abs() < f64::EPSILON {
            format!("{} for {over}", rate(stage.from))
        } else {
            let to = rate(stage.to);
            format!(
                "{}..{to} over {over}",
                rate(stage.from).trim_end_matches("/s")
            )
        }
    } else if (stage.from - stage.to).abs() < f64::EPSILON {
        format!("{} users for {over}", stage.from)
    } else {
        format!("{}..{} users over {over}", stage.from, stage.to)
    }
}

/// What `--plan` says.
fn plan_text(data: &LoadData) -> String {
    let mut text = format!("load plan: {} model", data.model);
    if let Some(profile) = &data.profile {
        let _ = write!(text, " of {profile}");
    }
    let _ = writeln!(text, " against backend {}", data.backend);
    let _ = writeln!(text, "stages:");
    for (i, stage) in data.stages.iter().enumerate() {
        let _ = writeln!(text, "  {}. {}", i + 1, stage_text(stage));
    }
    let total: u64 = data.stages.iter().map(|s| s.duration_ms).sum();
    let total = protocol::format::duration(Duration::from_millis(total));
    if let Some(max) = data.max_users {
        let _ = writeln!(
            text,
            "  about {} arrivals over {total}, each one iteration; at most {max} users at once",
            protocol::format::thousands(usize::try_from(data.arrivals).unwrap_or(usize::MAX))
        );
    } else {
        let _ = writeln!(text, "  {total} in all; each user loops its journeys");
    }
    let by_weight = data.profile.is_some();
    let _ = writeln!(
        text,
        "journeys{}:",
        if by_weight {
            " (by weight)"
        } else {
            " (in turn)"
        }
    );
    for journey in &data.journeys {
        let mut line = String::from("  ");
        if by_weight {
            let _ = write!(line, "{:>4}  ", journey.weight);
        }
        let _ = write!(line, "{} {}", journey.place, journey.name);
        if let Some(file) = &journey.data {
            let _ = write!(line, " (data {file})");
        }
        if let Some(ms) = journey.previous_ms {
            let _ = write!(
                line,
                ", {} an iteration last time",
                protocol::format::duration(Duration::from_secs_f64(ms / 1000.0))
            );
        }
        let _ = writeln!(text, "{line}");
    }
    let peak = match data.max_users {
        Some(max) if data.journeys.iter().all(|j| j.previous_ms.is_some()) => format!(
            "about {} at once (the peak rate × an iteration's length last time), at most {max}",
            count(data.users, "user", "users")
        ),
        Some(max) => format!(
            "unknown until a run of these journeys on backend {} has its iterations' length; \
             at most {max}",
            data.backend
        ),
        None => format!("{} at once", count(data.users, "user", "users")),
    };
    let _ = writeln!(text, "users: {peak}");
    if !data.thresholds.is_empty() {
        let _ = writeln!(text, "thresholds:");
        for t in &data.thresholds {
            let _ = writeln!(
                text,
                "  {} {}{}",
                t.name,
                t.condition,
                if t.abort {
                    " (stops the run after failing for 30s)"
                } else {
                    ""
                }
            );
        }
    }
    let _ = write!(
        text,
        "Chromes: {} ({}), up to {} users each",
        data.browsers, data.browser, data.users_per_browser
    );
    if data.browsers_started < data.browsers {
        let _ = write!(
            text,
            "; {} start before the users, the others when first needed",
            data.browsers_started
        );
    }
    text
}

/// The run in a few lines.
fn text(data: &LoadData) -> String {
    if data.planned {
        return plan_text(data);
    }
    let mut text = format!(
        "{} in {}: {} failed\n",
        count(data.iterations, "iteration", "iterations"),
        protocol::format::duration(Duration::from_millis(data.duration_ms)),
        data.failed
    );
    for journey in &data.journeys {
        let _ = writeln!(
            text,
            "  {} {}: {} iterations, {} failed",
            journey.place, journey.name, journey.iterations, journey.failed
        );
    }
    if data.max_users.is_some() {
        let _ = writeln!(
            text,
            "arrivals: {}, {} dropped at max_users ({}); at most {} at once",
            data.arrivals,
            data.dropped,
            data.max_users.unwrap_or(0),
            count(data.users, "user", "users")
        );
    }
    if data.iterations > 0 && data.profile.is_some() {
        let t = data.iteration_ms;
        let ms = |ms: f64| protocol::format::duration(Duration::from_secs_f64(ms / 1000.0));
        let _ = writeln!(
            text,
            "iterations took {} at p50, {} at p95, {} at most{}",
            ms(t.p50),
            ms(t.p95),
            ms(t.max),
            if data.max_users.is_some() {
                " (from when they were due)"
            } else {
                ""
            }
        );
    }
    let s = &data.steps;
    let _ = writeln!(
        text,
        "steps: {} ok, {} failed, {} timeout, {} error",
        s.ok, s.failed, s.timeout, s.error
    );
    if !data.errors.is_empty() {
        let _ = writeln!(text, "errors: {}", errors_line(&data.errors));
    }
    text.push_str(&requests_text(data));
    if data.contexts > 0 {
        let _ = writeln!(
            text,
            "browser contexts: {} made ({} cache), {} ms each",
            data.contexts, data.cache, data.context_setup_ms
        );
    }
    if data.browser_crashes > 0 {
        let _ = writeln!(
            text,
            "Chrome crashed {} {} and was started again",
            data.browser_crashes,
            if data.browser_crashes == 1 {
                "time"
            } else {
                "times"
            }
        );
    }
    if let Some(stopped) = &data.stopped {
        let _ = write!(text, "stopped early: {}", stopped.message);
        if stopped.cut_short > 0 {
            let _ = write!(
                text,
                "; {} cut short, not counted",
                count(stopped.cut_short, "iteration", "iterations")
            );
        }
        text.push('\n');
    }
    if let Some(dir) = &data.failures_dir {
        let _ = writeln!(text, "failures kept: {} in {dir}", data.failures_kept);
    }
    if let Some(raw) = &data.raw {
        let _ = writeln!(text, "step timings: {raw}");
    }
    if let Some(capacity) = &data.capacity {
        text.push_str(&capacity::text(capacity));
    }
    if let Some(c) = &data.calibration {
        let _ = writeln!(
            text,
            "calibration: {}; it held {} users ({} per Chrome) before (the ramp went up to {}, \
             {}/s)\n  saved in {}{}",
            c.message,
            c.held_users,
            c.held_per_browser,
            c.ramp_to,
            c.ramp_rate,
            c.file,
            if c.first == "generator" {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let at = (f64::from(c.held_users) * calibrate::WARN_AT).floor() as u32;
                format!("; later runs of this profile warn above {at} users")
            } else {
                String::new()
            }
        );
    }
    if let Some(compare) = &data.compare {
        let _ = writeln!(text, "{}", compare::line(compare));
    }
    if let Some(files) = &data.files {
        let _ = writeln!(text, "report: {}", files.html);
    }
    if !data.thresholds.is_empty() {
        let passed = data.thresholds.iter().filter(|t| t.passed).count();
        let left_out = data
            .capacity
            .as_ref()
            .map_or(0, |c| c.saturated_s)
            .min(data.duration_ms.div_ceil(1000));
        let _ = writeln!(
            text,
            "thresholds: {passed} of {} passed{}",
            data.thresholds.len(),
            if left_out > 0 {
                format!(" (without the {left_out} saturated seconds)")
            } else {
                String::new()
            }
        );
        for t in &data.thresholds {
            let mark = if t.passed { "ok  " } else { "FAIL" };
            let _ = writeln!(text, "  {mark} {}", threshold_line(t));
        }
    }
    text.trim_end().to_owned()
}

/// How many endpoints the text lists, the busiest first.
const SHOWN_ENDPOINTS: usize = 10;

/// What the server did and users saw: the requests, the busiest
/// endpoints, the page views' Web Vitals, the request errors and the
/// exceptions.
fn requests_text(data: &LoadData) -> String {
    let mut text = String::new();
    let Some(all) = &data.requests else {
        return text;
    };
    let ms = status::ms_text;
    let _ = writeln!(
        text,
        "requests: {} ({}/s), {} errors, p50 {}, p95 {}, p99 {}",
        protocol::format::thousands(usize::try_from(all.count).unwrap_or(usize::MAX)),
        all.rate,
        status::percent(all.error_rate * 100.0),
        ms(all.p50),
        ms(all.p95),
        ms(all.p99)
    );
    let shown = data.endpoints.len().min(SHOWN_ENDPOINTS);
    let width = data.endpoints[..shown]
        .iter()
        .map(|e| e.name.chars().count())
        .max()
        .unwrap_or(0);
    for e in &data.endpoints[..shown] {
        let _ = write!(
            text,
            "  {:width$}  {:>7}  {:>6}  p50 {:>7}  p95 {:>7}  p99 {:>7}",
            e.name,
            e.count,
            status::percent(e.error_rate * 100.0),
            ms(e.p50),
            ms(e.p95),
            ms(e.p99)
        );
        if e.cached > 0 {
            let _ = write!(text, "  ({} cached)", e.cached);
        }
        text.push('\n');
    }
    if data.endpoints.len() > shown {
        let _ = writeln!(
            text,
            "  … {} more endpoints in the report",
            data.endpoints.len() - shown
        );
    }
    if !data.request_errors.is_empty() {
        let _ = writeln!(
            text,
            "request errors: {}",
            errors_line(&data.request_errors)
        );
    }
    let views = data.vitals.iter().map(|v| v.count).max().unwrap_or(0);
    if views > 0 {
        let vitals: Vec<String> = data
            .vitals
            .iter()
            .map(|v| {
                let value = if v.unit.is_empty() {
                    format!("{:.3}", v.p75)
                } else {
                    ms(v.p75)
                };
                format!("{} {value}", v.name)
            })
            .collect();
        let _ = writeln!(text, "page views: {views}; at p75: {}", vitals.join(", "));
    }
    if data.exceptions_total > 0 {
        let first = data
            .exceptions
            .first()
            .map(|e| format!(", most often {} ({})", e.message, e.count))
            .unwrap_or_default();
        let _ = writeln!(
            text,
            "uncaught exceptions: {}{first}",
            data.exceptions_total
        );
    }
    text
}

/// `p95(GET /api/cart) < 300ms: 412 ms`, `p75(inp) < 200ms: no value
/// (inp had no value to measure)`.
fn threshold_line(t: &protocol::schema::LoadThreshold) -> String {
    let value = match t.value {
        Some(v) => format!("{v}{}", unit_suffix(&t.unit)),
        None => "no value".to_owned(),
    };
    let mut line = format!("{} {}: {value}", t.name, t.condition);
    if t.aborted {
        line.push_str(" (stopped the run)");
    }
    if let Some(note) = &t.note {
        let _ = write!(line, " ({note})");
    }
    line
}

/// ` ms`, `%`, `/s` or nothing, after a number.
fn unit_suffix(unit: &str) -> String {
    match unit {
        "ms" => " ms".to_owned(),
        other => other.to_owned(),
    }
}

/// `ExpectFailed 3, Timeout 1`.
fn errors_line(errors: &BTreeMap<String, u64>) -> String {
    errors
        .iter()
        .map(|(kind, n)| format!("{kind} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `total` over `n`, in ms with a decimal.
fn mean_ms(total: Duration, n: u64) -> f64 {
    if n == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let ms = total.as_secs_f64() * 1000.0 / n as f64;
    (ms * 10.0).round() / 10.0
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{Pick, pick};
    use crate::cli::LoadBrowser;

    #[test]
    fn the_shell_unless_told_otherwise() {
        assert_eq!(pick(None, false, false, true).unwrap(), Pick::Shell);
        assert_eq!(
            pick(None, false, false, false).unwrap(),
            Pick::ChromeInstead
        );
        let shell = Some(LoadBrowser::HeadlessShell);
        assert_eq!(pick(shell, false, false, true).unwrap(), Pick::Shell);
        let err = pick(shell, false, false, false).unwrap_err();
        assert_eq!(err.message, "chrome-headless-shell is not installed");
        assert_eq!(
            err.hint.as_deref(),
            Some(
                "chrome-cdp install --headless-shell, or --browser chrome runs the users in \
                 full Chrome"
            )
        );
        let chrome = Some(LoadBrowser::Chrome);
        assert_eq!(pick(chrome, false, false, true).unwrap(), Pick::Chrome);
        // $CHROME_CDP_CHROME is the browser.
        assert_eq!(pick(None, false, true, false).unwrap(), Pick::Chrome);
        assert_eq!(pick(shell, false, true, false).unwrap(), Pick::Chrome);
        // Windows are full Chrome's.
        assert_eq!(pick(None, true, false, true).unwrap(), Pick::Chrome);
        assert!(pick(shell, true, false, true).is_err());
    }
}
