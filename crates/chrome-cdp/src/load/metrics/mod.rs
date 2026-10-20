//! A load run's numbers (plan 23.4): what the server did (each endpoint's
//! requests: count, errors, latency, cache and phases) and what users saw
//! (each step's and journey's times, each page view's Web Vitals, uncaught
//! exceptions).
//!
//! Every duration goes into an HDR histogram ([`hist`]) per group, for the
//! whole run and for the current second; each second [`Metrics::tick`]
//! writes the second's line per group to `metrics.jsonl` and starts the
//! next. Memory stays bounded by the groups (at most [`group::MOST_GROUPS`]
//! endpoints) and the run's seconds (one small [`Second`] each, for the
//! report's charts), not by the requests.

pub(crate) mod group;
pub(crate) mod hist;
mod prometheus;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use engine::{LoadRequest, PageVitals};
use protocol::schema::{LoadException, LoadMetric, LoadPhases};
use serde::Serialize;
use serde_json::json;

use group::{Grouper, MOST_GROUPS, OTHER};
use hist::{Hist, Unit};

use super::threshold::{Stat, Target};

/// `metrics.jsonl`'s name in the run's folder.
pub(crate) const METRICS: &str = "metrics.jsonl";

/// How many seconds the status line's recent numbers cover.
const RECENT: usize = 10;

/// How many different exception messages are kept.
const EXCEPTIONS: usize = 20;

/// The Web Vitals and navigation times of a page view, in the order the
/// summary lists them.
pub(crate) const VITALS: [&str; 6] = ["ttfb", "fcp", "lcp", "load", "cls", "inp"];

/// What a group measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Kind {
    /// An endpoint's requests.
    Request,
    /// A step of a scenario, by its line.
    Step,
    /// A journey's iterations, from when they were due.
    Journey,
    /// A Web Vital or navigation time of the page views.
    Vital,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Step => "step",
            Self::Journey => "journey",
            Self::Vital => "vital",
        }
    }
}

/// A group's numbers over a span.
#[derive(Debug, Clone)]
struct Stats {
    hist: Hist,
    count: u64,
    errors: u64,
    /// Requests a cache answered (not in `hist`: the server never saw them).
    cached: u64,
}

impl Stats {
    fn new(unit: Unit) -> Self {
        Self {
            hist: Hist::new(unit),
            count: 0,
            errors: 0,
            cached: 0,
        }
    }

    fn record(&mut self, value: Option<f64>, error: bool, cached: bool) {
        self.count += 1;
        self.errors += u64::from(error);
        self.cached += u64::from(cached);
        if let Some(value) = value.filter(|_| !cached) {
            self.hist.record(value);
        }
    }

    fn reset(&mut self) {
        self.hist.reset();
        self.count = 0;
        self.errors = 0;
        self.cached = 0;
    }
}

/// One group.
#[derive(Debug)]
struct Series {
    kind: Kind,
    name: String,
    /// A step's command, a journey's test name.
    label: Option<String>,
    total: Stats,
    window: Stats,
    /// Its errors by kind: a request's status (`500`) or Chrome's error, a
    /// step's error kind.
    errors_by: BTreeMap<String, u64>,
    /// A request's phases summed (DNS, connect, TLS, time to first byte,
    /// download), and how many requests had them.
    phases: [f64; 5],
    phased: u64,
}

impl Series {
    fn new(kind: Kind, name: String, label: Option<String>) -> Self {
        let unit = if kind == Kind::Vital && name == "cls" {
            Unit::Score
        } else {
            Unit::Ms
        };
        Self {
            kind,
            name,
            label,
            total: Stats::new(unit),
            window: Stats::new(unit),
            errors_by: BTreeMap::new(),
            phases: [0.0; 5],
            phased: 0,
        }
    }

    fn record(&mut self, value: Option<f64>, error: Option<&str>, cached: bool) {
        self.total.record(value, error.is_some(), cached);
        self.window.record(value, error.is_some(), cached);
        if let Some(kind) = error {
            *self.errors_by.entry(kind.to_owned()).or_default() += 1;
        }
    }

    fn unit(&self) -> Unit {
        if self.kind == Kind::Vital && self.name == "cls" {
            Unit::Score
        } else {
            Unit::Ms
        }
    }

    /// Its numbers over a run of `seconds`.
    fn metric(&self, seconds: f64) -> LoadMetric {
        let stats = &self.total;
        let hist = &stats.hist;
        #[allow(clippy::cast_precision_loss)]
        let ratio = |n: u64, of: u64| {
            if of == 0 {
                0.0
            } else {
                (n as f64 / of as f64 * 10_000.0).round() / 10_000.0
            }
        };
        #[allow(clippy::cast_precision_loss)]
        let rate = if seconds > 0.0 {
            (stats.count as f64 / seconds * 100.0).round() / 100.0
        } else {
            0.0
        };
        #[allow(clippy::cast_precision_loss)]
        let phase = |i: usize| (self.phases[i] / self.phased as f64 * 10.0).round() / 10.0;
        LoadMetric {
            name: self.name.clone(),
            label: self.label.clone(),
            count: stats.count,
            rate,
            errors: stats.errors,
            error_rate: ratio(stats.errors, stats.count),
            unit: self.unit().as_str().to_owned(),
            p50: hist.quantile(0.5),
            p75: hist.quantile(0.75),
            p90: hist.quantile(0.9),
            p95: hist.quantile(0.95),
            p99: hist.quantile(0.99),
            max: hist.max(),
            mean: hist.mean(),
            cached: stats.cached,
            error_kinds: self.errors_by.clone(),
            phases: (self.phased > 0).then(|| LoadPhases {
                dns_ms: phase(0),
                connect_ms: phase(1),
                tls_ms: phase(2),
                ttfb_ms: phase(3),
                download_ms: phase(4),
            }),
        }
    }
}

/// What the run's gauges were at a tick.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Gauges {
    /// Users running now.
    pub(crate) users: u32,
    /// Arrivals and dropped arrivals so far (an open model's).
    pub(crate) arrivals: u64,
    pub(crate) dropped: u64,
}

/// One second of the run, for `metrics.jsonl`'s run line, the status line
/// and the report's charts.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub(crate) struct Second {
    /// Its end, in seconds since the users started.
    pub(crate) t: u64,
    pub(crate) users: u32,
    pub(crate) arrivals: u64,
    pub(crate) dropped: u64,
    pub(crate) iterations: u64,
    /// Iterations that failed.
    pub(crate) failed: u64,
    pub(crate) requests: u64,
    /// Requests that failed or got a status of 400 or more.
    pub(crate) errors: u64,
    /// Request p95 in ms; absent with no request that second.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) p95: Option<f64>,
}

/// The last [`RECENT`] seconds, for the status line.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Recent {
    pub(crate) seconds: u64,
    pub(crate) arrivals: u64,
    pub(crate) iterations: u64,
    pub(crate) requests: u64,
    pub(crate) errors: u64,
    /// Their requests' p95, in ms.
    pub(crate) p95: Option<f64>,
}

/// The run's numbers, shared by its users and its Chromes' sinks.
#[derive(Debug)]
pub(crate) struct Metrics {
    grouper: Grouper,
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    series: Vec<Series>,
    index: HashMap<(Kind, String), usize>,
    /// Endpoint groups so far ([`OTHER`] not counted).
    endpoints: usize,
    /// For the requests that went into [`OTHER`]: the pattern each most
    /// likely misses, and how many.
    overflow: HashMap<String, u64>,
    /// Every request, step and iteration.
    requests: Series,
    steps: Series,
    iterations: Series,
    /// The last seconds' requests, and their counts.
    recent: VecDeque<(Hist, Second)>,
    /// Request errors by class: `http_4xx`, `http_5xx`, `request_failed`.
    request_errors: BTreeMap<String, u64>,
    exceptions: BTreeMap<String, u64>,
    exceptions_total: u64,
    timeline: Vec<Second>,
    /// The arrivals and dropped ones at the last tick.
    last: Gauges,
    out: Option<BufWriter<File>>,
}

impl Metrics {
    /// The run's metrics, grouping requests with `grouper`; with `dir`,
    /// writing `metrics.jsonl` there.
    pub(crate) fn new(grouper: Grouper, dir: Option<&Path>) -> std::io::Result<Self> {
        let out = dir
            .map(|d| File::create(d.join(METRICS)).map(BufWriter::new))
            .transpose()?;
        Ok(Self {
            grouper,
            inner: Mutex::new(Inner {
                series: Vec::new(),
                index: HashMap::new(),
                endpoints: 0,
                overflow: HashMap::new(),
                requests: Series::new(Kind::Request, "requests".into(), None),
                steps: Series::new(Kind::Step, "steps".into(), None),
                iterations: Series::new(Kind::Journey, "iterations".into(), None),
                recent: VecDeque::new(),
                request_errors: BTreeMap::new(),
                exceptions: BTreeMap::new(),
                exceptions_total: 0,
                timeline: Vec::new(),
                last: Gauges::default(),
                out,
            }),
        })
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A request that ended.
    pub(crate) fn request(&self, request: &LoadRequest) {
        let Some(name) = self
            .grouper
            .group(&request.method, &request.url, &request.kind)
        else {
            return;
        };
        let (class, kind) = match (request.status, &request.error) {
            (Some(status), _) if status >= 500 => (Some("http_5xx"), Some(status.to_string())),
            (Some(status), _) if status >= 400 => (Some("http_4xx"), Some(status.to_string())),
            (Some(_), _) => (None, None),
            (None, error) => (
                Some("request_failed"),
                Some(error.clone().unwrap_or_else(|| "failed".to_owned())),
            ),
        };
        let cached = request.cache.is_some();
        let mut inner = self.inner();
        let inner = &mut *inner;
        if let Some(class) = class {
            *inner.request_errors.entry(class.to_owned()).or_default() += 1;
        }
        inner
            .requests
            .record(request.duration_ms, kind.as_deref(), cached);
        let index = match inner.index.get(&(Kind::Request, name.clone())) {
            Some(&i) => i,
            None if inner.endpoints >= MOST_GROUPS => {
                *inner
                    .overflow
                    .entry(Grouper::likely_pattern(&name))
                    .or_default() += 1;
                inner.at(Kind::Request, OTHER, None)
            }
            None => {
                inner.endpoints += 1;
                inner.at(Kind::Request, &name, None)
            }
        };
        let series = &mut inner.series[index];
        series.record(request.duration_ms, kind.as_deref(), cached);
        if let Some(t) = request.timing.as_ref().filter(|_| !cached) {
            #[allow(clippy::cast_precision_loss)]
            let phases = [
                t.dns_ms as f64,
                t.connect_ms.saturating_sub(t.tls_ms) as f64,
                t.tls_ms as f64,
                t.wait_ms as f64,
                t.receive_ms.unwrap_or(0) as f64,
            ];
            for (sum, ms) in series.phases.iter_mut().zip(phases) {
                *sum += ms;
            }
            series.phased += 1;
        }
    }

    /// A step that ended: its group (`step e2e/shop.cdp:14`), command and
    /// time, and its error's kind if it failed.
    pub(crate) fn step(&self, group: &str, command: &str, ms: f64, error: Option<&str>) {
        let mut inner = self.inner();
        inner.steps.record(Some(ms), error, false);
        let i = inner.at(Kind::Step, group, Some(command));
        inner.series[i].record(Some(ms), error, false);
    }

    /// An iteration of journey `place` (its test `name`) that ended,
    /// `ms` after it was due.
    pub(crate) fn iteration(&self, place: &str, name: &str, ms: f64, failed: bool) {
        let error = failed.then_some("failed");
        let mut inner = self.inner();
        inner.iterations.record(Some(ms), error, false);
        let i = inner.at(Kind::Journey, place, Some(name));
        inner.series[i].record(Some(ms), error, false);
    }

    /// A page view's Web Vitals.
    pub(crate) fn vitals(&self, vitals: &PageVitals) {
        let mut inner = self.inner();
        let values = [
            vitals.ttfb,
            vitals.fcp,
            vitals.lcp,
            vitals.load,
            vitals.cls,
            vitals.inp,
        ];
        for (name, value) in VITALS.iter().zip(values) {
            if let Some(value) = value {
                let i = inner.at(Kind::Vital, name, None);
                inner.series[i].record(Some(value), None, false);
            }
        }
    }

    /// An uncaught exception in a page.
    pub(crate) fn exception(&self, message: &str) {
        let mut inner = self.inner();
        inner.exceptions_total += 1;
        if inner.exceptions.len() < EXCEPTIONS || inner.exceptions.contains_key(message) {
            *inner.exceptions.entry(message.to_owned()).or_default() += 1;
        }
    }

    /// Ends second `t` of the run with the gauges then: writes its lines
    /// to `metrics.jsonl` and starts the next.
    pub(crate) fn tick(&self, t: u64, gauges: Gauges) -> Second {
        let mut inner = self.inner();
        let inner = &mut *inner;
        let window = &inner.requests.window;
        let second = Second {
            t,
            users: gauges.users,
            arrivals: gauges.arrivals.saturating_sub(inner.last.arrivals),
            dropped: gauges.dropped.saturating_sub(inner.last.dropped),
            iterations: inner.iterations.window.count,
            failed: inner.iterations.window.errors,
            requests: window.count,
            errors: window.errors,
            p95: (window.hist.count() > 0).then(|| window.hist.quantile(0.95)),
        };
        inner.last = gauges;
        if let Some(out) = &mut inner.out {
            let mut line = serde_json::to_value(second).unwrap_or_default();
            line["group"] = json!("run");
            let _ = writeln!(out, "{line}");
            for series in inner.series.iter().filter(|s| s.window.count > 0) {
                let w = &series.window;
                let line = json!({
                    "t": t,
                    "kind": series.kind.as_str(),
                    "group": series.name,
                    "count": w.count,
                    "errors": w.errors,
                    "cached": w.cached,
                    "p50": w.hist.quantile(0.5),
                    "p90": w.hist.quantile(0.9),
                    "p95": w.hist.quantile(0.95),
                    "p99": w.hist.quantile(0.99),
                    "max": w.hist.max(),
                });
                let _ = writeln!(out, "{line}");
            }
            let _ = out.flush();
        }
        if inner.recent.len() >= RECENT
            && let Some((mut hist, _)) = inner.recent.pop_front()
        {
            hist.reset();
            hist.add(&inner.requests.window.hist);
            inner.recent.push_back((hist, second));
        } else {
            inner
                .recent
                .push_back((inner.requests.window.hist.clone(), second));
        }
        for series in &mut inner.series {
            series.window.reset();
        }
        inner.requests.window.reset();
        inner.steps.window.reset();
        inner.iterations.window.reset();
        inner.timeline.push(second);
        second
    }

    #[allow(clippy::option_option)]
    /// What statistic `stat` of `target` is after `seconds` of the run,
    /// with the gauges then: `None` for an endpoint no request had;
    /// `Some(None)` for a time of a group with no times.
    pub(crate) fn value(
        &self,
        target: &Target,
        stat: Stat,
        seconds: f64,
        gauges: Gauges,
    ) -> Option<Option<f64>> {
        let inner = self.inner();
        #[allow(clippy::cast_precision_loss)]
        let counted = |n: u64| match stat {
            Stat::Count => Some(n as f64),
            Stat::Rate if seconds > 0.0 => Some(n as f64 / seconds),
            Stat::Rate => Some(0.0),
            _ => None,
        };
        let series = match target {
            Target::Dropped => return Some(counted(gauges.dropped)),
            Target::Arrivals => return Some(counted(gauges.arrivals)),
            Target::Exceptions => return Some(counted(inner.exceptions_total)),
            Target::Requests => Some(&inner.requests),
            Target::Steps => Some(&inner.steps),
            Target::Iterations => Some(&inner.iterations),
            Target::Endpoint(name) => match inner.index.get(&(Kind::Request, name.clone())) {
                Some(&i) => Some(&inner.series[i]),
                None => return None,
            },
            Target::Step(name) => inner.find(Kind::Step, name),
            Target::Journey(place) => inner.find(Kind::Journey, place),
            Target::Vital(name) => inner.find(Kind::Vital, name),
        };
        let Some(series) = series else {
            return Some(match stat {
                Stat::Count | Stat::Rate | Stat::ErrorRate => Some(0.0),
                _ => None,
            });
        };
        let total = &series.total;
        let hist = &total.hist;
        #[allow(clippy::cast_precision_loss)]
        Some(match stat {
            Stat::Count | Stat::Rate => counted(total.count),
            Stat::ErrorRate if total.count == 0 => Some(0.0),
            Stat::ErrorRate => Some(total.errors as f64 / total.count as f64),
            _ if hist.count() == 0 => None,
            Stat::Quantile(q) => Some(hist.quantile(q)),
            Stat::Min => Some(hist.min()),
            Stat::Max => Some(hist.max()),
            Stat::Mean => Some(hist.mean()),
        })
    }

    /// The endpoints requests had, for a threshold's that none had.
    pub(crate) fn endpoint_names(&self) -> Vec<String> {
        self.inner()
            .series
            .iter()
            .filter(|s| s.kind == Kind::Request)
            .map(|s| s.name.clone())
            .collect()
    }

    /// Ends the run's last second, cut short, with the gauges then.
    pub(crate) fn finish(&self, gauges: Gauges) -> Second {
        let t = self.inner().timeline.last().map_or(1, |s| s.t + 1);
        self.tick(t, gauges)
    }

    /// The last seconds' numbers.
    pub(crate) fn recent(&self) -> Recent {
        let inner = self.inner();
        let mut recent = Recent::default();
        let mut hist: Option<Hist> = None;
        for (h, s) in &inner.recent {
            recent.seconds += 1;
            recent.arrivals += s.arrivals;
            recent.iterations += s.iterations;
            recent.requests += s.requests;
            recent.errors += s.errors;
            match &mut hist {
                Some(all) => all.add(h),
                None => hist = Some(h.clone()),
            }
        }
        recent.p95 = hist.filter(|h| h.count() > 0).map(|h| h.quantile(0.95));
        recent
    }

    /// Each second so far.
    pub(crate) fn timeline(&self) -> Vec<Second> {
        self.inner().timeline.clone()
    }

    /// Every group's numbers over a run of `took`.
    pub(crate) fn summary(&self, took: Duration) -> Summary {
        let inner = self.inner();
        let seconds = took.as_secs_f64();
        let of = |kind: Kind| -> Vec<LoadMetric> {
            let mut list: Vec<LoadMetric> = inner
                .series
                .iter()
                .filter(|s| s.kind == kind)
                .map(|s| s.metric(seconds))
                .collect();
            match kind {
                // The busiest endpoints first.
                Kind::Request => {
                    list.sort_by(|a, b| b.count.cmp(&a.count).then(a.name.cmp(&b.name)));
                }
                Kind::Vital => list.sort_by_key(|m| VITALS.iter().position(|v| *v == m.name)),
                Kind::Step | Kind::Journey => {}
            }
            list
        };
        let mut exceptions: Vec<LoadException> = inner
            .exceptions
            .iter()
            .map(|(message, &count)| LoadException {
                message: message.clone(),
                count,
            })
            .collect();
        exceptions.sort_by(|a, b| b.count.cmp(&a.count).then(a.message.cmp(&b.message)));
        Summary {
            requests: (inner.requests.total.count > 0).then(|| inner.requests.metric(seconds)),
            endpoints: of(Kind::Request),
            steps: of(Kind::Step),
            journeys: of(Kind::Journey),
            iterations: inner.iterations.metric(seconds),
            vitals: of(Kind::Vital),
            request_errors: inner.request_errors.clone(),
            exceptions,
            exceptions_total: inner.exceptions_total,
            overflow: inner
                .overflow
                .iter()
                .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
                .map(|(pattern, &n)| (pattern.clone(), n)),
        }
    }
}

impl Inner {
    fn find(&self, kind: Kind, name: &str) -> Option<&Series> {
        self.index
            .get(&(kind, name.to_owned()))
            .map(|&i| &self.series[i])
    }

    /// The series of `kind` named `name`, made if new.
    fn at(&mut self, kind: Kind, name: &str, label: Option<&str>) -> usize {
        if let Some(&i) = self.index.get(&(kind, name.to_owned())) {
            return i;
        }
        let i = self.series.len();
        self.series
            .push(Series::new(kind, name.to_owned(), label.map(str::to_owned)));
        self.index.insert((kind, name.to_owned()), i);
        i
    }
}

/// Every group's numbers at the end of a run.
#[derive(Debug, Clone)]
pub(crate) struct Summary {
    /// Every request; `None` when there was none.
    pub(crate) requests: Option<LoadMetric>,
    pub(crate) endpoints: Vec<LoadMetric>,
    pub(crate) steps: Vec<LoadMetric>,
    /// Each journey's iterations, by its place.
    pub(crate) journeys: Vec<LoadMetric>,
    /// Every iteration.
    pub(crate) iterations: LoadMetric,
    pub(crate) vitals: Vec<LoadMetric>,
    pub(crate) request_errors: BTreeMap<String, u64>,
    pub(crate) exceptions: Vec<LoadException>,
    pub(crate) exceptions_total: u64,
    /// The pattern the requests above the group limit most likely miss,
    /// and how many of them had it.
    pub(crate) overflow: Option<(String, u64)>,
}

#[cfg(test)]
mod tests;
