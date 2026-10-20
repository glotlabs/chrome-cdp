//! Knowing its own limits (plan 23.5). A browser load generator usually
//! runs out of CPU before the site does, and then every number goes up; a
//! report that blames the server would be wrong.
//!
//! Each second the run reads the machine ([`probe`]: its CPU use, load
//! average and free memory, its Chromes' and its own CPU and memory) and
//! how late its own timers fired that second (the **scheduling lag**: its
//! users' waits, an open model's planned arrivals, its own second), and
//! the pages report how late their main thread ran a timer (`vitals.js`).
//! A [`Detector`] calls the generator saturated when, for
//! [`SATURATED_AFTER`] in a row, the CPU use is above `[load.capacity]
//! cpu`, the free memory below `memory`, or the lag's p99 above `lag`.
//! Those seconds are marked in `metrics.jsonl` and the charts, the
//! thresholds leave them out, and with `on_saturation = "abort"` the run
//! stops. Its summary ([`Capacity::summary`]) says what the machine held,
//! and what to do.

pub(crate) mod probe;

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use e2e::config::{Capacity as Limits, SATURATED_AFTER};
use protocol::schema::{LoadCapacity, LoadSaturation};
use serde::Serialize;

use super::metrics::Mark;
use super::metrics::hist::{Hist, Unit};
use probe::System;

/// A limit of `[load.capacity]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Limit {
    Cpu,
    Memory,
    Lag,
}

impl Limit {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Memory => "memory",
            Self::Lag => "lag",
        }
    }
}

/// A limit crossed, and the value that crossed it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Over {
    pub(crate) limit: Limit,
    pub(crate) value: f64,
}

impl Over {
    /// `cpu 97%`, `free memory 4%`, `lag 120 ms`.
    pub(crate) fn text(self) -> String {
        match self.limit {
            Limit::Cpu => format!("cpu {:.0}%", self.value),
            Limit::Memory => format!("free memory {:.0}%", self.value),
            Limit::Lag => format!("lag {}", super::status::ms_text(self.value)),
        }
    }

    /// `cpu 97% (limit 90%)`.
    pub(crate) fn against(self, limits: &Limits) -> String {
        let limit = match self.limit {
            Limit::Cpu => format!("{}%", limits.cpu),
            Limit::Memory => format!("{}%", limits.memory),
            Limit::Lag => super::status::ms_text(millis(limits.lag)),
        };
        format!("{} (limit {limit})", self.text())
    }
}

/// One second of the generator, as `metrics.jsonl`'s capacity line has it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub(crate) struct Sample {
    /// Its end, in seconds since the users started.
    pub(crate) t: u64,
    pub(crate) users: u32,
    /// The Chromes started.
    pub(crate) browsers: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cpu: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) load: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) memory_free: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) chrome_cpu: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) chrome_rss: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) own_cpu: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) own_rss: Option<u64>,
    /// How late the run's timers fired this second, in ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) lag_p99: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) lag_max: Option<f64>,
    /// The p99 of the main-thread lag of the page views reported this
    /// second, in ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) page_lag: Option<f64>,
}

/// What the detector made of a second.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Seen {
    pub(crate) mark: Mark,
    /// The limit crossed this second, if one was.
    pub(crate) over: Option<Over>,
}

/// Saturated or not, from one second's sample to the next.
#[derive(Debug, Clone)]
pub(crate) struct Detector {
    limits: Limits,
    /// The first of the seconds in a row past a limit, so far.
    since: Option<u64>,
    saturated: bool,
    spans: Vec<LoadSaturation>,
}

impl Detector {
    pub(crate) fn new(limits: Limits) -> Self {
        Self {
            limits,
            since: None,
            saturated: false,
            spans: Vec::new(),
        }
    }

    /// The limit `sample` crosses, if one: the CPU first, then memory,
    /// then the lag.
    pub(crate) fn over(&self, sample: &Sample) -> Option<Over> {
        let limits = &self.limits;
        if let Some(cpu) = sample.cpu.filter(|c| *c > f64::from(limits.cpu)) {
            return Some(Over {
                limit: Limit::Cpu,
                value: cpu,
            });
        }
        if let Some(free) = sample.memory_free.filter(|m| *m < f64::from(limits.memory)) {
            return Some(Over {
                limit: Limit::Memory,
                value: free,
            });
        }
        sample
            .lag_p99
            .filter(|l| *l > millis(limits.lag))
            .map(|lag| Over {
                limit: Limit::Lag,
                value: lag,
            })
    }

    /// Takes the next second's sample.
    pub(crate) fn see(&mut self, sample: &Sample) -> Seen {
        let over = self.over(sample);
        let Some(crossed) = over else {
            self.since = None;
            self.saturated = false;
            return Seen {
                mark: Mark::Clean,
                over,
            };
        };
        let since = *self.since.get_or_insert(sample.t);
        let mark = if self.saturated {
            if let Some(span) = self.spans.last_mut() {
                span.to_s = sample.t;
                if span.limit == crossed.limit.as_str() {
                    span.value = worse(crossed.limit, span.value, crossed.value);
                }
            }
            Mark::Saturated
        } else if sample.t + 1 - since >= SATURATED_AFTER.as_secs() {
            self.saturated = true;
            self.spans.push(LoadSaturation {
                from_s: since,
                to_s: sample.t,
                limit: crossed.limit.as_str().to_owned(),
                value: crossed.value,
            });
            Mark::Began(since)
        } else {
            Mark::Over
        };
        Seen { mark, over }
    }

    pub(crate) fn saturated(&self) -> bool {
        self.saturated
    }

    pub(crate) fn spans(&self) -> &[LoadSaturation] {
        &self.spans
    }
}

/// The worse of two values of `limit`: free memory is worse lower.
fn worse(limit: Limit, a: f64, b: f64) -> f64 {
    match limit {
        Limit::Memory => a.min(b),
        Limit::Cpu | Limit::Lag => a.max(b),
    }
}

/// The run's capacity watch, shared by its users and its ticker.
#[derive(Debug)]
pub(crate) struct Capacity {
    limits: Limits,
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    /// This second's and the run's timer lag, and page lag.
    lag: Hist,
    lag_total: Hist,
    page_lag: Hist,
    page_lag_total: Hist,
    detector: Detector,
    /// Every second's sample.
    samples: Vec<Sample>,
}

impl Capacity {
    pub(crate) fn new(limits: Limits) -> Self {
        Self {
            limits,
            inner: Mutex::new(Inner {
                lag: Hist::new(Unit::Ms),
                lag_total: Hist::new(Unit::Ms),
                page_lag: Hist::new(Unit::Ms),
                page_lag_total: Hist::new(Unit::Ms),
                detector: Detector::new(limits),
                samples: Vec::new(),
            }),
        }
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn limits(&self) -> &Limits {
        &self.limits
    }

    /// A timer of the run fired `by` after it was due.
    pub(crate) fn late(&self, by: Duration) {
        let ms = by.as_secs_f64() * 1000.0;
        let mut inner = self.inner();
        inner.lag.record(ms);
        inner.lag_total.record(ms);
    }

    /// A page view's main thread ran its timers `p99` ms late at p99.
    pub(crate) fn page(&self, p99: Option<f64>) {
        if let Some(ms) = p99 {
            let mut inner = self.inner();
            inner.page_lag.record(ms);
            inner.page_lag_total.record(ms);
        }
    }

    /// Ends second `t`, with `users` running in `browsers` Chromes and
    /// the machine as `system` read it: the second's sample, and what the
    /// detector made of it.
    pub(crate) fn second(
        &self,
        t: u64,
        users: u32,
        browsers: u32,
        system: System,
    ) -> (Sample, Seen) {
        let mut inner = self.inner();
        let inner = &mut *inner;
        let sample = Sample {
            t,
            users,
            browsers,
            cpu: system.cpu,
            load: system.load,
            memory_free: system.memory_free,
            chrome_cpu: system.chrome_cpu,
            chrome_rss: system.chrome_rss,
            own_cpu: system.own_cpu,
            own_rss: system.own_rss,
            lag_p99: (inner.lag.count() > 0).then(|| inner.lag.quantile(0.99)),
            lag_max: (inner.lag.count() > 0).then(|| inner.lag.max()),
            page_lag: (inner.page_lag.count() > 0).then(|| inner.page_lag.quantile(0.99)),
        };
        inner.lag.reset();
        inner.page_lag.reset();
        let seen = inner.detector.see(&sample);
        inner.samples.push(sample);
        (sample, seen)
    }

    /// Whether the generator is saturated now.
    pub(crate) fn saturated(&self) -> bool {
        self.inner().detector.saturated()
    }

    /// The summary of the run's `seconds`, at most `peak` users at once.
    pub(crate) fn summary(&self, seconds: u64, peak: u32) -> LoadCapacity {
        let inner = self.inner();
        let samples = &inner.samples;
        let spans = inner.detector.spans().to_vec();
        let first = spans.first().map(|s| s.from_s);
        let before: Vec<&Sample> = samples
            .iter()
            .filter(|s| first.is_none_or(|from| s.t < from))
            .collect();
        let held = before.iter().max_by_key(|s| s.users);
        let held_users = match first {
            None => peak.max(held.map_or(0, |s| s.users)),
            Some(_) => held.map_or(0, |s| s.users),
        };
        let held_per_browser = held.map_or(held_users, |s| held_users.div_ceil(s.browsers.max(1)));
        let cpu: Vec<f64> = samples.iter().filter_map(|s| s.cpu).collect();
        let most = |values: &mut dyn Iterator<Item = f64>| values.reduce(f64::max);
        LoadCapacity {
            cpu_limit: self.limits.cpu,
            memory_limit: self.limits.memory,
            lag_limit_ms: millis(self.limits.lag),
            on_saturation: self.limits.on_saturation.as_str().to_owned(),
            machine: probe::Probe::machine(),
            saturated_s: spans.iter().map(|s| s.to_s + 1 - s.from_s).sum(),
            seconds,
            saturations: spans,
            peak_users: peak,
            held_users,
            held_per_browser,
            cpu_p50: quantile(&cpu, 0.5),
            cpu_p95: quantile(&cpu, 0.95),
            cpu_max: most(&mut cpu.iter().copied()),
            load_max: most(&mut samples.iter().filter_map(|s| s.load)),
            memory_free_min: samples
                .iter()
                .filter_map(|s| s.memory_free)
                .reduce(f64::min),
            lag_p99_ms: inner.lag_total.quantile(0.99),
            lag_max_ms: inner.lag_total.max(),
            page_lag_p99_ms: (inner.page_lag_total.count() > 0)
                .then(|| inner.page_lag_total.quantile(0.99)),
            chrome_cpu_max: most(&mut samples.iter().filter_map(|s| s.chrome_cpu)),
            chrome_rss_max: samples.iter().filter_map(|s| s.chrome_rss).max(),
            own_cpu_max: most(&mut samples.iter().filter_map(|s| s.own_cpu)),
            own_rss_max: samples.iter().filter_map(|s| s.own_rss).max(),
            calibrated_users: None,
            hint: None,
        }
    }

    /// The generator's numbers of the last second, in Prometheus' text
    /// format (plan 23.4's `--metrics-listen`).
    pub(crate) fn prometheus(&self) -> String {
        use std::fmt::Write as _;
        let inner = self.inner();
        let last = inner.samples.last().copied().unwrap_or_default();
        let saturated_s: u64 = inner
            .detector
            .spans()
            .iter()
            .map(|s| s.to_s + 1 - s.from_s)
            .sum();
        let mut out = String::new();
        let mut gauge = |name: &str, kind: &str, help: &str, value: Option<f64>| {
            if let Some(value) = value {
                let _ = writeln!(
                    out,
                    "# HELP {name} {help}\n# TYPE {name} {kind}\n{name} {value}"
                );
            }
        };
        gauge(
            "chrome_cdp_load_client_saturated",
            "gauge",
            "1 while the load generator is saturated (plan 23.5), else 0.",
            Some(if inner.detector.saturated() { 1.0 } else { 0.0 }),
        );
        #[allow(clippy::cast_precision_loss)]
        gauge(
            "chrome_cdp_load_client_saturated_seconds_total",
            "counter",
            "Seconds the load generator was saturated, which the thresholds leave out.",
            Some(saturated_s as f64),
        );
        gauge(
            "chrome_cdp_load_client_cpu_ratio",
            "gauge",
            "The machine's CPU use, of all its CPUs, the last second.",
            last.cpu.map(|c| c / 100.0),
        );
        gauge(
            "chrome_cdp_load_client_memory_free_ratio",
            "gauge",
            "The machine's free memory, of all of it.",
            last.memory_free.map(|m| m / 100.0),
        );
        gauge(
            "chrome_cdp_load_client_lag_seconds",
            "gauge",
            "How late the run's timers fired the last second, at p99.",
            last.lag_p99.map(|l| l / 1000.0),
        );
        out
    }

    /// The samples so far.
    #[cfg(test)]
    pub(crate) fn samples(&self) -> Vec<Sample> {
        self.inner().samples.clone()
    }
}

/// The value at quantile `q` of `values`, rounded to 0.1; `None` for none.
fn quantile(values: &[f64], q: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let i = ((sorted.len() as f64 * q).ceil() as usize).clamp(1, sorted.len()) - 1;
    Some((sorted[i] * 10.0).round() / 10.0)
}

/// `d` in ms.
fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// What a saturated run's summary says to do: lower the load to what the
/// machine held, make each user cheaper, or use a bigger machine.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Advice<'a> {
    /// The users the machine held, and the most the run had.
    pub(crate) held: u32,
    pub(crate) peak: u32,
    /// An open model's peak rate, a second.
    pub(crate) rate: Option<f64>,
    /// Whether `[load] block` has `third-party` already.
    pub(crate) third_party_blocked: bool,
    /// `chrome` or `chrome-headless-shell`.
    pub(crate) browser: &'a str,
}

impl Advice<'_> {
    pub(crate) fn text(self) -> String {
        let mut steps = Vec::new();
        #[allow(clippy::cast_precision_loss)]
        match self.rate {
            _ if self.held == 0 => {
                steps
                    .push("start with far fewer users: it saturated before it held any".to_owned());
            }
            Some(rate) if self.peak > 0 => {
                let lower = rate * f64::from(self.held) / f64::from(self.peak);
                steps.push(format!(
                    "lower the rate to about {}/s",
                    trim(((lower * 0.9) * 10.0).floor() / 10.0)
                ));
            }
            _ => {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let most = (f64::from(self.held) * 0.9).floor().max(1.0) as u32;
                steps.push(format!(
                    "run at most {most} {}",
                    if most == 1 { "user" } else { "users" }
                ));
            }
        }
        if self.browser == "chrome" {
            steps.push(
                "run the users in chrome-headless-shell (chrome-cdp install --headless-shell)"
                    .to_owned(),
            );
        }
        if !self.third_party_blocked {
            steps.push("make each user cheaper with [load] block = [\"third-party\"]".to_owned());
        }
        steps.push("run the profile on a bigger machine".to_owned());
        let last = steps.pop().unwrap_or_default();
        if steps.is_empty() {
            last
        } else {
            format!("{}, or {last}", steps.join(", "))
        }
    }
}

/// What the summary says about the load generator: what it used, and
/// when it was saturated (which a run `gated` by thresholds leaves out).
pub(crate) fn text(c: &LoadCapacity, gated: bool) -> String {
    use std::fmt::Write as _;
    let ms = super::status::ms_text;
    let mut parts = Vec::new();
    if let (Some(p95), Some(max)) = (c.cpu_p95, c.cpu_max) {
        parts.push(format!("cpu {p95:.0}% at p95, {max:.0}% at most"));
    }
    if let Some(free) = c.memory_free_min {
        parts.push(format!("free memory {free:.0}% at least"));
    }
    parts.push(format!("timers {} late at p99", ms(c.lag_p99_ms)));
    if let Some(page) = c.page_lag_p99_ms {
        parts.push(format!("pages' main threads {} late at p99", ms(page)));
    }
    let mut text = format!(
        "client: {}
",
        parts.join(", ")
    );
    let mut used = Vec::new();
    if let (Some(cpu), Some(rss)) = (c.chrome_cpu_max, c.chrome_rss_max) {
        used.push(format!(
            "Chromes {cpu:.1} cores and {} at most",
            protocol::format::size(rss)
        ));
    }
    if let (Some(cpu), Some(rss)) = (c.own_cpu_max, c.own_rss_max) {
        used.push(format!(
            "chrome-cdp {cpu:.2} cores and {}",
            protocol::format::size(rss)
        ));
    }
    if !used.is_empty() {
        let _ = writeln!(text, "  {}", used.join("; "));
    }
    if let Some(first) = c.saturations.first() {
        let clock = |s: u64| format!("{:02}:{:02}", s / 60, s % 60);
        let _ = writeln!(
            text,
            "CLIENT SATURATED for {}s of {}s, first at {} ({} {}){}; it held {} users ({} per \
             Chrome) before, of {} at most",
            c.saturated_s,
            c.seconds,
            clock(first.from_s.saturating_sub(1)),
            first.limit,
            limit_value(&first.limit, first.value),
            if gated {
                ", so the thresholds leave those seconds out"
            } else {
                ""
            },
            c.held_users,
            c.held_per_browser,
            c.peak_users
        );
    } else if c.peak_users > 0 {
        let _ = writeln!(
            text,
            "  within its limits throughout: it held {} users ({} per Chrome)",
            c.held_users, c.held_per_browser
        );
    }
    if let Some(hint) = &c.hint {
        let _ = writeln!(text, "hint: {hint}");
    }
    text
}

/// A limit's value as the summary says it: `97%`, `120 ms`.
fn limit_value(limit: &str, value: f64) -> String {
    if limit == Limit::Lag.as_str() {
        super::status::ms_text(value)
    } else {
        format!("{value:.0}%")
    }
}

/// `2.5`, `3` (no trailing zeros).
fn trim(n: f64) -> String {
    let text = format!("{n:.1}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests;
