//! `expect --perf` (plan 17, milestone 17.2): performance budgets on
//! `perf`'s numbers (see `crate::observe::perf`).
//!
//! A budget is `<metric><op><value>`: `lcp<2.5s`, `cls<=0.1`,
//! `transfer<1mb`. Several, space-separated, must all hold; `good` stands
//! for the Web Vitals "good" thresholds. The grammar is checked before
//! any browser starts (`check_expect`, and `lint` for scenarios).
//!
//! It waits for `load` (within `--timeout`), then reads once: numbers do
//! not converge like a DOM does, so it does not retry. A metric not
//! measured yet (INP before an interaction, LCP before a contentful
//! paint) fails with what to do, unless `--perf-missing ok`.

use std::fmt::Write as _;
use std::time::Duration;

use protocol::schema::{ExpectPerf, PerfBudgetCheck, PerfData, PerfThrottling};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, format, suggest};
use serde_json::{Map, Value, json};
use tokio::time::Instant;

use super::{Failed, Passed, expected};
use cdp::domains::{dom, runtime};

use crate::context::Scoped as _;
use crate::frames::World;
use crate::locator::LocateOptions;
use crate::request::ExpectRequest;
use crate::snapshot::{NodeKey, Target};
use crate::tabs::Page;
use crate::wait::Condition;
use crate::{Engine, Running};

/// `--perf good`: the Web Vitals "good" thresholds.
pub(crate) const GOOD: &str = "lcp<=2500 cls<=0.1 inp<=200";

/// What a metric counts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Ms,
    Bytes,
    Count,
    /// CLS: a score with no unit.
    Score,
}

impl Unit {
    fn name(self) -> &'static str {
        match self {
            Self::Ms => "ms",
            Self::Bytes => "bytes",
            Self::Count => "count",
            Self::Score => "",
        }
    }

    /// The units a limit may have (lowercase), and what each multiplies
    /// by. Sizes are binary, as chrome-cdp shows them (`12KB`).
    fn suffixes(self) -> &'static [(&'static str, f64)] {
        match self {
            Self::Ms => &[("ms", 1.0), ("s", 1000.0)],
            Self::Bytes => &[
                ("b", 1.0),
                ("kb", 1024.0),
                ("mb", 1024.0 * 1024.0),
                ("gb", 1024.0 * 1024.0 * 1024.0),
            ],
            Self::Count | Self::Score => &[],
        }
    }

    /// What units a limit of this takes, in words.
    fn takes(self) -> &'static str {
        match self {
            Self::Ms => "ms or s (none means ms)",
            Self::Bytes => "b, kb, mb or gb (none means bytes)",
            Self::Count => "no unit: it is a count",
            Self::Score => "no unit: it is a score",
        }
    }

    /// `3120ms`, `1.2MB`, `1,204`, `0.012`.
    // Values here are times, sizes and counts well within f64 and u64.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn show(self, value: f64) -> String {
        match self {
            Self::Ms => format!("{}ms", value.round() as u64),
            Self::Bytes => format::size(value.max(0.0).round() as u64),
            Self::Count => format::thousands(value.max(0.0).round() as usize),
            Self::Score => {
                let text = format!("{value:.3}");
                let text = text.trim_end_matches('0').trim_end_matches('.');
                if text.is_empty() { "0" } else { text }.to_owned()
            }
        }
    }
}

/// The metrics a budget names: `PerfData`'s fields.
const METRICS: &[(&str, Unit)] = &[
    ("ttfb", Unit::Ms),
    ("fcp", Unit::Ms),
    ("dcl", Unit::Ms),
    ("load", Unit::Ms),
    ("lcp", Unit::Ms),
    ("cls", Unit::Score),
    ("inp", Unit::Ms),
    ("tbt", Unit::Ms),
    ("requests", Unit::Count),
    ("transfer", Unit::Bytes),
    ("elements", Unit::Count),
    ("heap", Unit::Bytes),
];

/// The names of the metrics `expect --perf` takes.
pub fn perf_metrics() -> impl Iterator<Item = &'static str> {
    METRICS.iter().map(|m| m.0)
}

/// One budget: `lcp<2.5s` is `lcp`, `<`, 2500 (ms).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Budget {
    /// As typed.
    text: String,
    metric: &'static str,
    unit: Unit,
    op: &'static str,
    /// In the metric's unit.
    limit: f64,
}

impl Budget {
    fn holds(&self, value: f64) -> bool {
        match self.op {
            "<" => value < self.limit,
            "<=" => value <= self.limit,
            ">" => value > self.limit,
            _ => value >= self.limit,
        }
    }

    /// `lcp < 2500ms`.
    fn words(&self) -> String {
        format!("{} {} {}", self.metric, self.op, self.unit.show(self.limit))
    }
}

/// Checks `expect --perf`'s budgets without a browser: what `lint` and
/// the command line run.
pub fn check_perf_budgets(text: &str) -> Result<(), ErrorInfo> {
    parse(text).map(drop)
}

/// The budgets of `text`, `good` expanded.
pub(crate) fn parse(text: &str) -> Result<Vec<Budget>, ErrorInfo> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.is_empty() {
        return Err(bad(text, "--perf needs a budget, e.g. lcp<2.5s"));
    }
    let mut budgets = Vec::new();
    for token in tokens {
        if token.eq_ignore_ascii_case("good") {
            for good in GOOD.split(' ') {
                budgets.push(budget(good, text)?);
            }
        } else {
            budgets.push(budget(token, text)?);
        }
    }
    Ok(budgets)
}

/// A `BadArgs` about `--perf <all>`, with an example to follow.
fn bad(all: &str, message: impl Into<String>) -> ErrorInfo {
    ErrorInfo::new(ErrorKind::BadArgs, message).hint(example(all))
}

/// `chrome-cdp expect --perf 'lcp<2.5s cls<0.1 tbt<200'`.
fn example(all: &str) -> Hint {
    let example = if all.split_whitespace().next().is_some() {
        "lcp<2.5s cls<0.1 tbt<200"
    } else {
        "good"
    };
    Hint::run(Cmd::new("expect").flag("perf", example)).why("<metric><op><value>, all must hold")
}

fn budget(token: &str, all: &str) -> Result<Budget, ErrorInfo> {
    let lower = token.to_ascii_lowercase();
    let Some(at) = lower.find(['<', '>', '=', '!']) else {
        return Err(bad(
            all,
            format!("--perf: `{token}` is not a budget: <metric><op><value>, as in lcp<2.5s"),
        ));
    };
    let (name, rest) = lower.split_at(at);
    let (op, value) = [("<=", "<="), (">=", ">="), ("<", "<"), (">", ">")]
        .into_iter()
        .find_map(|(prefix, op)| rest.strip_prefix(prefix).map(|v| (op, v)))
        .ok_or_else(|| {
            bad(
                all,
                format!("--perf: `{token}` compares with {}; use <, <=, > or >=", {
                    let end = rest
                        .find(|c: char| !matches!(c, '<' | '>' | '=' | '!'))
                        .unwrap_or(rest.len());
                    &rest[..end]
                }),
            )
        })?;
    let Some(&(metric, unit)) = METRICS.iter().find(|m| m.0 == name) else {
        return Err(unknown_metric(token, name, all));
    };
    let limit = limit(value, unit).map_err(|why| {
        bad(
            all,
            format!("--perf: `{token}`: {why}; {metric} takes {}", unit.takes()),
        )
    })?;
    Ok(Budget {
        text: token.to_owned(),
        metric,
        unit,
        op,
        limit,
    })
}

/// `lpc<2500`: not a metric; the closest one, and the list.
fn unknown_metric(token: &str, name: &str, all: &str) -> ErrorInfo {
    let names: Vec<&str> = perf_metrics().collect();
    let message = if name.is_empty() {
        format!("--perf: `{token}` names no metric")
    } else {
        format!("--perf: `{name}` is not a metric")
    };
    let info = ErrorInfo::new(ErrorKind::BadArgs, message).note("metrics", names.join(", "));
    match suggest::similar(name, names.iter().copied(), 1).first() {
        Some(close) => {
            // The whole value again, with the name fixed.
            let fixed: Vec<String> = all
                .split_whitespace()
                .map(|t| {
                    if t == token {
                        format!("{close}{}", &t[name.len()..])
                    } else {
                        t.to_owned()
                    }
                })
                .collect();
            info.hint(
                Hint::run(Cmd::new("expect").flag("perf", fixed.join(" ")))
                    .why("did you mean this?"),
            )
        }
        None => info.hint(example(all)),
    }
}

/// A limit in `unit`: a number, then maybe one of its units.
fn limit(value: &str, unit: Unit) -> Result<f64, String> {
    let end = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(value.len());
    let (number, suffix) = value.split_at(end);
    let Ok(number) = number.parse::<f64>() else {
        return Err(if value.is_empty() {
            "the limit is missing".to_owned()
        } else {
            format!("`{value}` is not a number")
        });
    };
    if suffix.is_empty() {
        return Ok(number);
    }
    unit.suffixes()
        .iter()
        .find(|(name, _)| *name == suffix)
        .map(|(_, factor)| number * factor)
        .ok_or_else(|| format!("`{suffix}` is not a unit of it"))
}

/// What the page measured for a metric.
struct Measured {
    value: Option<f64>,
    /// Shown instead of the value (`under 16ms`).
    shown: Option<String>,
    /// Why there is no value, and what to do.
    missing: Option<&'static str>,
    element: Option<String>,
    /// The element's ref (LCP's, when its budget failed).
    reference: Option<String>,
}

impl Measured {
    fn value(value: f64) -> Self {
        Self {
            value: Some(value),
            shown: None,
            missing: None,
            element: None,
            reference: None,
        }
    }

    fn missing(why: &'static str) -> Self {
        Self {
            value: None,
            shown: None,
            missing: Some(why),
            element: None,
            reference: None,
        }
    }

    fn or(value: Option<u64>, why: &'static str) -> Self {
        #[allow(clippy::cast_precision_loss)]
        value.map_or_else(|| Self::missing(why), |v| Self::value(v as f64))
    }
}

const NO_NAVIGATION: &str = "the page has no navigation to time; goto a page";
const NOT_LOADED: &str = "not reached yet; wait --load";

// Counts and byte sizes fit well within f64.
#[allow(clippy::cast_precision_loss)]
fn measure(metric: &str, data: &PerfData) -> Measured {
    let nav = data.navigation.as_ref();
    let from_nav = |field: fn(&protocol::schema::PerfNavigation) -> Option<u64>| match nav {
        None => Measured::missing(NO_NAVIGATION),
        Some(nav) => Measured::or(field(nav), NOT_LOADED),
    };
    match metric {
        "ttfb" => from_nav(|n| n.ttfb_ms),
        "dcl" => from_nav(|n| n.dom_content_loaded_ms),
        "load" => from_nav(|n| n.load_ms),
        "fcp" => Measured::or(data.fcp_ms, "nothing painted yet; wait --load"),
        "lcp" => match &data.lcp {
            Some(lcp) => Measured {
                element: lcp.element.clone(),
                ..Measured::value(lcp.ms as f64)
            },
            None => Measured::missing("no contentful paint yet; wait --load"),
        },
        "cls" => Measured::value(data.cls.value),
        "inp" => match &data.inp {
            Some(inp) => Measured::value(inp.ms as f64),
            // The browser reports no interaction faster than 16ms: the
            // bound stands for them.
            None if data.interactions > 0 => Measured {
                shown: Some("under 16ms".to_owned()),
                ..Measured::value(16.0)
            },
            None => Measured::missing("no interaction yet; interact first"),
        },
        "tbt" => Measured::value(data.tbt_ms as f64),
        "requests" => Measured::value(data.requests as f64),
        "transfer" => Measured::value(data.transfer_bytes as f64),
        "elements" => Measured::value(data.elements as f64),
        _ => data.metrics.get("JSHeapUsedSize").map_or_else(
            || Measured::missing("Chrome did not report the heap; try again"),
            |&heap| Measured::value(heap),
        ),
    }
}

/// Every budget against `data`.
fn evaluate(
    budgets: &[Budget],
    data: &PerfData,
    missing_ok: bool,
) -> Vec<(Budget, Measured, bool)> {
    budgets
        .iter()
        .map(|budget| {
            let measured = measure(budget.metric, data);
            let held = measured
                .value
                .map_or(missing_ok, |value| budget.holds(value));
            (budget.clone(), measured, held)
        })
        .collect()
}

/// A budget's line: `3120ms > 2500ms ✗ (element: img#hero)`.
fn row(budget: &Budget, measured: &Measured, held: bool) -> String {
    let mark = if held { "✓" } else { "✗" };
    let Some(value) = measured.value else {
        let why = if held {
            "--perf-missing ok"
        } else {
            measured.missing.unwrap_or_default()
        };
        return format!("not measured {mark} ({why})");
    };
    let shown = measured
        .shown
        .clone()
        .unwrap_or_else(|| budget.unit.show(value));
    let relation = if value < budget.limit {
        "<"
    } else if value > budget.limit {
        ">"
    } else {
        "="
    };
    let mut text = format!(
        "{shown} {relation} {} {mark}",
        budget.unit.show(budget.limit)
    );
    if let Some(element) = &measured.element {
        let _ = write!(text, " (element: {element}");
        if let Some(reference) = &measured.reference {
            let _ = write!(text, " [{reference}]");
        }
        text.push(')');
    }
    text
}

/// `network slow-3g, cpu 4x`, or `none`.
fn throttling_text(throttling: &PerfThrottling) -> String {
    let mut parts = Vec::new();
    if let Some(network) = &throttling.network {
        parts.push(format!("network {network}"));
    }
    if let Some(cpu) = throttling.cpu {
        parts.push(format!("cpu {cpu}x"));
    }
    if parts.is_empty() {
        "none".to_owned()
    } else {
        parts.join(", ")
    }
}

/// The line after `passed:`: `lcp 812ms, tbt 0ms (throttling: none)`.
pub(super) fn passed_line(perf: &ExpectPerf) -> String {
    let mut seen = Vec::new();
    for check in &perf.budgets {
        if seen.iter().any(|(m, _)| *m == check.metric) {
            continue;
        }
        let unit = METRICS
            .iter()
            .find(|m| m.0 == check.metric)
            .map_or(Unit::Count, |m| m.1);
        let value = check
            .value
            .map_or_else(|| "not measured".to_owned(), |v| unit.show(v));
        seen.push((check.metric.as_str(), value));
    }
    let values: Vec<String> = seen.iter().map(|(m, v)| format!("{m} {v}")).collect();
    format!(
        "{} (throttling: {})",
        values.join(", "),
        throttling_text(&perf.throttling)
    )
}

/// The warning when an INP budget was checked on what chrome-cdp saw of a
/// page it attached to after it loaded (as `perf` warns).
pub(super) fn partial_inp(perf: &ExpectPerf) -> Option<&'static str> {
    (!perf.from_start && perf.budgets.iter().any(|b| b.metric == "inp")).then_some(
        "chrome-cdp attached after this page loaded, so INP counts only interactions of 104ms \
         or more; reload for all of them",
    )
}

/// The failure's `actual`: `1 budget over, 1 not measured (of 3)`.
fn actual(results: &[(Budget, Measured, bool)]) -> String {
    let over = results
        .iter()
        .filter(|(_, m, held)| !held && m.value.is_some())
        .count();
    let missing = results
        .iter()
        .filter(|(_, m, held)| !held && m.value.is_none())
        .count();
    let mut parts = Vec::new();
    if over > 0 {
        parts.push(format!(
            "{over} {} over",
            if over == 1 { "budget" } else { "budgets" }
        ));
    }
    if missing > 0 {
        parts.push(format!("{missing} not measured"));
    }
    format!("{} (of {})", parts.join(", "), results.len())
}

/// `every budget to hold: lcp < 2500ms, tbt < 200ms`.
pub(super) fn expected_budgets(text: &str) -> String {
    match parse(text) {
        Ok(budgets) => {
            let words: Vec<String> = budgets.iter().map(Budget::words).collect();
            format!("every budget to hold: {}", words.join(", "))
        }
        Err(_) => format!("budgets {text:?}"),
    }
}

impl Engine {
    /// `--perf`: waits for `load`, then reads the page's numbers once
    /// and checks every budget.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn perf_budgets(
        &self,
        running: &Running,
        page: &Page,
        req: &ExpectRequest,
        budgets: &[Budget],
        missing_ok: bool,
        options: &LocateOptions,
        timeout: Duration,
    ) -> Result<Passed, ErrorInfo> {
        let deadline = Instant::now() + timeout;
        let load = Condition::Load {
            dom_content_loaded: false,
        };
        if let Err(unmet) = self
            .probe_until(running, page, &load, options, deadline)
            .await?
        {
            let state = unmet
                .observed
                .get("readyState")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let failed = Failed {
                expected: format!("{}, once the page has loaded", expected(req, None)),
                actual: format!(
                    "the page did not load: document.readyState is {state:?} (after {})",
                    format::duration(timeout)
                ),
                ..Failed::default()
            };
            return Err(self.failure(page, req, failed, Some(timeout)));
        }
        // Reading takes a moment; it gets one even at the deadline.
        let left = deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_secs(1));
        let data = self.read_perf(running, page, true, left).await?;
        let mut results = evaluate(budgets, &data, missing_ok);
        if let Some((_, lcp, _)) = results
            .iter_mut()
            .find(|(b, m, held)| !held && b.metric == "lcp" && m.element.is_some())
        {
            lcp.reference = self.lcp_ref(running, page, left).await;
        }
        let perf = ExpectPerf {
            url: data.url.clone(),
            budgets: results
                .iter()
                .map(|(budget, measured, held)| PerfBudgetCheck {
                    budget: budget.text.clone(),
                    metric: budget.metric.to_owned(),
                    op: budget.op.to_owned(),
                    limit: budget.limit,
                    value: measured.value,
                    unit: budget.unit.name().to_owned(),
                    held: *held,
                    missing: measured.missing.map(str::to_owned),
                    element: measured.element.clone(),
                    reference: measured.reference.clone(),
                })
                .collect(),
            throttling: self.throttling()?,
            from_start: data.from_start,
        };
        if results.iter().all(|(_, _, held)| *held) {
            let mut values = Map::new();
            let mut quiet = Vec::new();
            for (budget, measured, _) in &results {
                values.insert(budget.metric.to_owned(), json!(measured.value));
                quiet.push(format!(
                    "{}={}",
                    budget.metric,
                    measured
                        .value
                        .map_or_else(|| "-".to_owned(), |v| format!("{v}"))
                ));
            }
            quiet.dedup();
            return Ok(Passed {
                actual: Value::Object(values),
                quiet: quiet.join(" "),
                count: None,
                retries: 0,
                perf: Some(perf),
            });
        }
        let mut failed = Failed {
            expected: expected(req, None),
            actual: actual(&results),
            ..Failed::default()
        };
        for (budget, measured, held) in &results {
            failed
                .notes
                .push((budget.metric.to_owned(), row(budget, measured, *held)));
        }
        failed
            .notes
            .push(("throttling".into(), throttling_text(&perf.throttling)));
        if let Some(warning) = partial_inp(&perf) {
            failed.notes.push(("note".into(), warning.to_owned()));
        }
        let no_interaction = results
            .iter()
            .any(|(b, m, held)| !held && b.metric == "inp" && m.value.is_none());
        failed.hint = Some(if no_interaction {
            Hint::text(
                "interact with the page first (click, fill, press), then expect again; or \
                 --perf-missing ok",
            )
        } else {
            Hint::run(Cmd::new("perf").scoped(&self.session)).why("shows all of the page's numbers")
        });
        failed.details.insert("perf".into(), json!(perf));
        Err(self.one_shot_failure(page, req, failed))
    }

    /// A ref for the page's LCP element (kept by the observer), if it is
    /// still in the document.
    async fn lcp_ref(&self, running: &Running, page: &Page, timeout: Duration) -> Option<String> {
        let generation = running
            .tabs
            .snapshot_memory(page.id, |m| m.refs.generation());
        let (frame, context) = self
            .frame_and_context(running, page, None, World::Isolated, timeout)
            .await
            .ok()?;
        let mut params = runtime::EvaluateParams::new(
            "(() => { const node = globalThis[Symbol.for('chrome-cdp.perf')]?.lcpNode?.deref(); \
             return node && node.isConnected ? node : null; })()"
                .to_owned(),
        );
        params.context_id = Some(context.context_id);
        let session = context.session.with_timeout(timeout);
        let object_id = session.call(params).await.ok()?.result.object_id?;
        let described = session
            .call(dom::DescribeNodeParams {
                object_id: Some(object_id.clone()),
                ..dom::DescribeNodeParams::default()
            })
            .await;
        let target = Target {
            key: NodeKey {
                frame,
                backend: described.ok()?.node.backend_node_id,
            },
            context,
            object_id,
            note: None,
        };
        let named = self.name_target(running, page, &target, generation).await;
        target.release();
        named.reference
    }

    /// The network and CPU throttling of the session (or context).
    fn throttling(&self) -> Result<PerfThrottling, ErrorInfo> {
        let state = self
            .settings
            .emulation
            .get_in(self.context_key()?.as_deref());
        let network = if state.offline {
            Some("offline".to_owned())
        } else {
            state.throttle.map(|t| {
                t.preset.unwrap_or_else(|| {
                    let mut text = format!("{}ms latency", t.latency_ms);
                    if let Some(down) = t.download_kbps {
                        let _ = write!(text, ", {down}kbit/s down");
                    }
                    if let Some(up) = t.upload_kbps {
                        let _ = write!(text, ", {up}kbit/s up");
                    }
                    text
                })
            })
        };
        Ok(PerfThrottling {
            network,
            cpu: state.cpu_throttle,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> Budget {
        let mut budgets = parse(text).unwrap_or_else(|e| panic!("{text}: {}", e.to_text()));
        assert_eq!(budgets.len(), 1, "{text}");
        budgets.remove(0)
    }

    #[test]
    fn units_convert_to_the_metrics_own() {
        let cases: &[(&str, &str, &str, f64)] = &[
            ("lcp<2.5s", "lcp", "<", 2500.0),
            ("lcp<2500", "lcp", "<", 2500.0),
            ("LCP<=2500ms", "lcp", "<=", 2500.0),
            ("ttfb<0.8s", "ttfb", "<", 800.0),
            ("fcp>=1s", "fcp", ">=", 1000.0),
            ("dcl<3s", "dcl", "<", 3000.0),
            ("load>10", "load", ">", 10.0),
            ("cls<0.1", "cls", "<", 0.1),
            ("inp<=200", "inp", "<=", 200.0),
            ("tbt<200ms", "tbt", "<", 200.0),
            ("requests<=40", "requests", "<=", 40.0),
            ("transfer<1mb", "transfer", "<", 1_048_576.0),
            ("transfer<500KB", "transfer", "<", 512_000.0),
            ("transfer<2048b", "transfer", "<", 2048.0),
            ("elements<1500", "elements", "<", 1500.0),
            ("heap<50mb", "heap", "<", 52_428_800.0),
            ("heap<1gb", "heap", "<", 1_073_741_824.0),
        ];
        for &(text, metric, op, limit) in cases {
            let budget = one(text);
            assert_eq!(
                (budget.metric, budget.op, budget.limit),
                (metric, op, limit),
                "{text}"
            );
            assert_eq!(budget.text, text);
        }
        assert_eq!(perf_metrics().count(), 12);
    }

    #[test]
    fn several_budgets_and_good() {
        let budgets = parse("  lcp<2.5s cls<0.1\ttbt<200 transfer<1mb ").unwrap();
        let metrics: Vec<&str> = budgets.iter().map(|b| b.metric).collect();
        assert_eq!(metrics, ["lcp", "cls", "tbt", "transfer"]);
        let good = parse("good").unwrap();
        let words: Vec<String> = good.iter().map(Budget::words).collect();
        assert_eq!(words, ["lcp <= 2500ms", "cls <= 0.1", "inp <= 200ms"]);
        assert_eq!(parse("good tbt<200").unwrap().len(), 4);
    }

    #[test]
    fn typos_fail_with_the_fix() {
        let err = parse("lpc<2500 cls<0.1").unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadArgs);
        assert_eq!(err.message, "--perf: `lpc` is not a metric");
        assert_eq!(
            err.hint.as_deref(),
            Some("chrome-cdp expect --perf 'lcp<2500 cls<0.1'   (did you mean this?)")
        );
        let cases: &[(&str, &str)] = &[
            ("", "--perf needs a budget, e.g. lcp<2.5s"),
            (
                "lcp2500",
                "--perf: `lcp2500` is not a budget: <metric><op><value>, as in lcp<2.5s",
            ),
            (
                "lcp=2500",
                "--perf: `lcp=2500` compares with =; use <, <=, > or >=",
            ),
            (
                "lcp=<2500",
                "--perf: `lcp=<2500` compares with =<; use <, <=, > or >=",
            ),
            ("<2500", "--perf: `<2500` names no metric"),
            (
                "lcp<",
                "--perf: `lcp<`: the limit is missing; lcp takes ms or s (none means ms)",
            ),
            (
                "lcp<fast",
                "--perf: `lcp<fast`: `fast` is not a number; lcp takes ms or s (none means ms)",
            ),
            (
                "lcp<2.5m",
                "--perf: `lcp<2.5m`: `m` is not a unit of it; lcp takes ms or s (none means ms)",
            ),
            (
                "cls<1s",
                "--perf: `cls<1s`: `s` is not a unit of it; cls takes no unit: it is a score",
            ),
            (
                "requests<1kb",
                "--perf: `requests<1kb`: `kb` is not a unit of it; requests takes no unit: it is a count",
            ),
            (
                "transfer<1s",
                "--perf: `transfer<1s`: `s` is not a unit of it; transfer takes b, kb, mb or gb (none means bytes)",
            ),
        ];
        for &(text, message) in cases {
            let err = parse(text).unwrap_err();
            assert_eq!(err.kind, ErrorKind::BadArgs, "{text}");
            assert_eq!(err.message, message, "{text}");
            assert!(err.hint.is_some(), "{text}");
        }
    }

    fn data() -> PerfData {
        serde_json::from_value(json!({
            "tab": "t1",
            "url": "http://127.0.0.1/",
            "navigation": {"type": "navigate", "ttfb_ms": 12, "dom_content_loaded_ms": 40, "load_ms": 90},
            "fcp_ms": 60,
            "lcp": {"ms": 3120, "rating": "poor", "element": "img#hero", "url": null},
            "cls": {"value": 0.0123, "rating": "good", "shifts": 1},
            "inp": null,
            "interactions": 0,
            "tbt_ms": 140,
            "long_tasks": 1,
            "requests": 3,
            "transfer_bytes": 2048,
            "elements": 20,
            "since_load_ms": 500,
            "from_start": true,
            "metrics": {"JSHeapUsedSize": 1_000_000.0},
        }))
        .unwrap()
    }

    #[test]
    fn rows_show_the_measured_value_and_the_limit() {
        let data = data();
        let rows: Vec<String> = evaluate(
            &parse("lcp<2.5s tbt<200 cls<0.1 transfer<1kb inp<200 requests>=3").unwrap(),
            &data,
            false,
        )
        .iter()
        .map(|(b, m, held)| format!("{} {}", b.metric, row(b, m, *held)))
        .collect();
        assert_eq!(
            rows,
            [
                "lcp 3120ms > 2500ms ✗ (element: img#hero)",
                "tbt 140ms < 200ms ✓",
                "cls 0.012 < 0.1 ✓",
                "transfer 2KB > 1KB ✗",
                "inp not measured ✗ (no interaction yet; interact first)",
                "requests 3 = 3 ✓",
            ]
        );
        let results = evaluate(&parse("inp<200 lcp<1s").unwrap(), &data, true);
        assert!(results[0].2);
        assert_eq!(
            row(&results[0].0, &results[0].1, results[0].2),
            "not measured ✓ (--perf-missing ok)"
        );
        assert_eq!(actual(&results), "1 budget over (of 2)");
        let results = evaluate(&parse("inp<200 lcp<1s").unwrap(), &data, false);
        assert_eq!(actual(&results), "1 budget over, 1 not measured (of 2)");
    }

    #[test]
    fn inp_under_16ms_counts_as_16() {
        let mut data = data();
        data.interactions = 2;
        let results = evaluate(&parse("inp<200 inp<10").unwrap(), &data, false);
        assert_eq!(
            row(&results[0].0, &results[0].1, results[0].2),
            "under 16ms < 200ms ✓"
        );
        assert!(!results[1].2);
    }

    #[test]
    fn throttling_in_words() {
        assert_eq!(
            throttling_text(&PerfThrottling {
                network: None,
                cpu: None
            }),
            "none"
        );
        assert_eq!(
            throttling_text(&PerfThrottling {
                network: Some("slow-3g".into()),
                cpu: Some(4.0)
            }),
            "network slow-3g, cpu 4x"
        );
    }
}
