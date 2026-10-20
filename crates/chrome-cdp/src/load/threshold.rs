//! Thresholds (plan 23.4): what a load run must hold to pass, so it can
//! gate a deploy. A profile's `[thresholds]` names a statistic of a group
//! and a limit:
//!
//! ```toml
//! [thresholds]
//! "p95(GET /api/products/:id)" = "< 300ms"
//! "p99(step checkout.cdp:14)"  = "< 2s"
//! "error_rate"                 = "< 1%"
//! "p75(lcp)"                   = "< 2.5s"
//! "rate(dropped)"              = { limit = "== 0", abort = true }
//! ```
//!
//! The statistics are `p50` … `p99.9` (any percentile), `min`, `max`,
//! `mean`, `count`, `rate` (a second) and `error_rate`; a bare
//! `error_rate` is every request's. A group is an endpoint (`GET
//! /api/cart`, `static js`, `other`), `requests`, `step FILE:LINE`,
//! `steps`, `journey NAME` (or its `file:line`), `iterations`, a Web Vital
//! (`ttfb`, `fcp`, `lcp`, `load`, `cls`, `inp`), `dropped`, `arrivals` or
//! `exceptions`. Every group but an endpoint is known before the run, and
//! an unknown one is an error then, with the closest names; an endpoint no
//! request had fails at the end, with the closest that some had.
//!
//! Each second the run checks them over the run so far: one that starts
//! failing is said, and one with `abort = true` that fails for 30 seconds
//! in a row stops the run. At the end each is checked over the whole run;
//! one that fails makes the run exit with `ThresholdFailed`.

use std::time::Duration;

use protocol::schema::LoadThreshold;

use super::metrics::VITALS;
use super::record::step_group;

/// How long a threshold with `abort = true` may fail before the run stops.
pub(crate) const ABORT_AFTER: Duration = Duration::from_secs(30);

/// A statistic of a group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Stat {
    /// A percentile, from 0 to 1.
    Quantile(f64),
    Min,
    Max,
    Mean,
    Count,
    /// How many a second, over the run.
    Rate,
    /// Errors over count, from 0 to 1.
    ErrorRate,
}

/// `<`, `<=`, `>`, `>=`, `==`, `!=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

impl Op {
    fn holds(self, value: f64, limit: f64) -> bool {
        let same = (value - limit).abs() <= limit.abs() * 1e-9 + 1e-12;
        match self {
            Self::Lt => value < limit && !same,
            Self::Le => value < limit || same,
            Self::Gt => value > limit && !same,
            Self::Ge => value > limit || same,
            Self::Eq => same,
            Self::Ne => !same,
        }
    }
}

/// What a limit is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unit {
    /// ms (written `300ms`, `2s` or `1m`).
    Ms,
    /// A share, written `1%` (kept from 0 to 1).
    Percent,
    /// A second (written `5/s` or `60/m`).
    PerSecond,
    /// A plain number: a count, a score, a share from 0 to 1.
    Number,
}

impl Unit {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ms => "ms",
            Self::Percent => "%",
            Self::PerSecond => "/s",
            Self::Number => "",
        }
    }

    /// `value`, as the limit was written: a share in %.
    fn shown(self, value: f64) -> f64 {
        let value = if self == Self::Percent {
            value * 100.0
        } else {
            value
        };
        (value * 10_000.0).round() / 10_000.0
    }
}

/// A threshold as the profile writes it, checked for its syntax.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Written {
    /// The key: `p95(GET /api/products/:id)`.
    pub(crate) name: String,
    /// The limit as written: `< 300ms`.
    pub(crate) condition: String,
    pub(crate) stat: Stat,
    /// The group as written: `GET /api/products/:id`, `step checkout.cdp:14`.
    pub(crate) group: String,
    pub(crate) op: Op,
    /// In ms, from 0 to 1 (a share), a second, or plain.
    pub(crate) limit: f64,
    pub(crate) unit: Unit,
    pub(crate) abort: bool,
}

/// A group a threshold's statistic is of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    Requests,
    /// An endpoint's group, e.g. `GET /api/cart`.
    Endpoint(String),
    Steps,
    /// A step's group: `step e2e/shop.cdp:14`.
    Step(String),
    Iterations,
    /// A journey, by its place.
    Journey(String),
    Vital(String),
    Dropped,
    Arrivals,
    Exceptions,
}

/// A threshold whose group is known.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Threshold {
    pub(crate) written: Written,
    pub(crate) target: Target,
}

/// Reads a `[thresholds]` key and its value: `"< 300ms"`, or `{ limit =
/// "< 300ms", abort = true }`.
pub(crate) fn parse(key: &str, value: &toml::Value) -> Result<Written, String> {
    let (condition, abort) = match value {
        toml::Value::String(text) => (text.as_str(), false),
        toml::Value::Table(table) => {
            if let Some(other) = table.keys().find(|k| *k != "limit" && *k != "abort") {
                return Err(format!(
                    "{key}: `{other}` is not a threshold's key; it has limit and abort"
                ));
            }
            let limit = table
                .get("limit")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| {
                    format!("{key}: say the limit, e.g. {{ limit = \"< 300ms\", abort = true }}")
                })?;
            let abort = match table.get("abort") {
                None => false,
                Some(toml::Value::Boolean(b)) => *b,
                Some(_) => return Err(format!("{key}: abort is true or false")),
            };
            (limit, abort)
        }
        _ => {
            return Err(format!(
                "{key}: a threshold's value is its limit, e.g. \"< 300ms\", or a table \
                 {{ limit = \"< 300ms\", abort = true }}"
            ));
        }
    };
    let (stat, group) = statistic(key)?;
    let (op, number, unit) =
        limit(condition).map_err(|e| format!("{key} = \"{condition}\": {e}"))?;
    let unit = fits(stat, &group, unit).map_err(|e| format!("{key} = \"{condition}\": {e}"))?;
    Ok(Written {
        name: key.trim().to_owned(),
        condition: condition.trim().to_owned(),
        stat,
        group,
        op,
        limit: number,
        unit,
        abort,
    })
}

/// `p95(GET /api/x)` → (p95, `GET /api/x`); `error_rate` → every
/// request's.
fn statistic(key: &str) -> Result<(Stat, String), String> {
    let key = key.trim();
    let (name, group) = match key.split_once('(') {
        Some((name, rest)) => {
            let Some(group) = rest.strip_suffix(')') else {
                return Err(format!("`{key}`: close the group with `)`"));
            };
            (name.trim(), group.trim())
        }
        None if key == "error_rate" => ("error_rate", "requests"),
        None => {
            return Err(format!(
                "`{key}` is not a statistic of a group; write e.g. p95(GET /api/cart), \
                 error_rate or rate(dropped)"
            ));
        }
    };
    if group.is_empty() {
        return Err(format!("`{key}`: name a group in the parentheses"));
    }
    let stat = match name {
        "min" => Stat::Min,
        "max" => Stat::Max,
        "mean" | "avg" => Stat::Mean,
        "count" => Stat::Count,
        "rate" => Stat::Rate,
        "error_rate" => Stat::ErrorRate,
        p if p.starts_with('p') => match p[1..].parse::<f64>() {
            Ok(n) if n > 0.0 && n < 100.0 => Stat::Quantile(n / 100.0),
            _ => {
                return Err(format!(
                    "`{name}` is not a percentile; write p50, p95, p99 or p99.9"
                ));
            }
        },
        _ => {
            return Err(format!(
                "`{name}` is not a statistic: p50 … p99.9, min, max, mean, count, rate or \
                 error_rate"
            ));
        }
    };
    Ok((stat, group.to_owned()))
}

/// `< 300ms` → (<, 300, ms); `< 1%` → (<, 0.01, %); `== 0`.
fn limit(text: &str) -> Result<(Op, f64, Unit), String> {
    let text = text.trim();
    let (op, rest) = [
        ("<=", Op::Le),
        (">=", Op::Ge),
        ("==", Op::Eq),
        ("!=", Op::Ne),
        ("<", Op::Lt),
        (">", Op::Gt),
    ]
    .into_iter()
    .find_map(|(s, op)| text.strip_prefix(s).map(|rest| (op, rest.trim())))
    .ok_or("start with <, <=, >, >=, == or !=")?;
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(rest.len());
    let (number, unit) = rest.split_at(end);
    let number: f64 = number
        .parse()
        .ok()
        .filter(|n: &f64| n.is_finite())
        .ok_or_else(|| format!("`{rest}` is not a number"))?;
    let (value, unit) = match unit.trim() {
        "" => (number, Unit::Number),
        "ms" => (number, Unit::Ms),
        "s" => (number * 1000.0, Unit::Ms),
        "m" => (number * 60_000.0, Unit::Ms),
        "%" => (number / 100.0, Unit::Percent),
        "/s" => (number, Unit::PerSecond),
        "/m" => (number / 60.0, Unit::PerSecond),
        other => {
            return Err(format!(
                "`{other}` is no unit; use ms, s, m, %, /s or /m (or none)"
            ));
        }
    };
    Ok((op, value, unit))
}

/// The unit a statistic of `group` takes its limit in, from the one
/// written.
fn fits(stat: Stat, group: &str, unit: Unit) -> Result<Unit, String> {
    let counted = matches!(group, "dropped" | "arrivals" | "exceptions");
    match stat {
        Stat::Quantile(_) | Stat::Min | Stat::Max | Stat::Mean => {
            if counted {
                return Err(format!("{group} has a count and a rate, not times"));
            }
            match (group == "cls", unit) {
                (true, Unit::Number) | (false, Unit::Ms) => Ok(unit),
                (true, _) => Err("cls is a score, without a unit: e.g. < 0.1".to_owned()),
                (false, Unit::Number) => {
                    Err("say the unit of the time: e.g. < 300ms or < 2s".to_owned())
                }
                (false, _) => Err("a time's limit is in ms, s or m".to_owned()),
            }
        }
        Stat::Count => match unit {
            Unit::Number => Ok(unit),
            _ => Err("a count's limit is a plain number".to_owned()),
        },
        Stat::Rate => match unit {
            Unit::Number | Unit::PerSecond => Ok(Unit::PerSecond),
            _ => Err("a rate's limit is a number a second: e.g. < 5/s".to_owned()),
        },
        Stat::ErrorRate => {
            if counted || VITALS.contains(&group) {
                return Err(format!("{group} has no errors to count"));
            }
            match unit {
                Unit::Percent => Ok(unit),
                Unit::Number => Ok(Unit::Percent),
                _ => Err("an error rate's limit is a share: e.g. < 1%".to_owned()),
            }
        }
    }
}

/// What a threshold's group can name before the run.
#[derive(Debug, Default)]
pub(crate) struct Known {
    /// The steps' groups: `step e2e/shop.cdp:14`.
    pub(crate) steps: Vec<String>,
    /// The journeys' places and names.
    pub(crate) journeys: Vec<(String, String)>,
}

impl Known {
    /// The group `written` names; an error with the closest names when it
    /// names none.
    pub(crate) fn resolve(&self, written: &str) -> Result<Target, String> {
        let group = written.trim();
        match group {
            "requests" => return Ok(Target::Requests),
            "steps" => return Ok(Target::Steps),
            "iterations" => return Ok(Target::Iterations),
            "dropped" => return Ok(Target::Dropped),
            "arrivals" => return Ok(Target::Arrivals),
            "exceptions" => return Ok(Target::Exceptions),
            v if VITALS.contains(&v) => return Ok(Target::Vital(v.to_owned())),
            _ => {}
        }
        if let Some(step) = group.strip_prefix("step ") {
            let step = step.trim();
            let want = step_group(step);
            let found: Vec<&String> = self
                .steps
                .iter()
                .filter(|s| **s == want || s.ends_with(&format!("/{step}")))
                .collect();
            return match found.as_slice() {
                [one] => Ok(Target::Step((*one).clone())),
                [] => {
                    // As written: by the file's name, or more of its path.
                    let short: Vec<String> = self
                        .steps
                        .iter()
                        .map(|s| match s.rsplit_once('/') {
                            Some((_, file)) if !step.contains('/') => format!("step {file}"),
                            _ => s.clone(),
                        })
                        .collect();
                    Err(Self::unknown(group, short.iter().map(String::as_str)))
                }
                many => Err(format!(
                    "{group} is more than one step: {}; write more of its path",
                    many.iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            };
        }
        if let Some(journey) = group.strip_prefix("journey ") {
            let journey = journey.trim();
            let found: Vec<&(String, String)> = self
                .journeys
                .iter()
                .filter(|(place, name)| place == journey || name == journey)
                .collect();
            return match found.as_slice() {
                [one] => Ok(Target::Journey(one.0.clone())),
                [] => Err(Self::unknown(
                    group,
                    self.journeys
                        .iter()
                        .map(|(_, name)| format!("journey {name}"))
                        .collect::<Vec<_>>()
                        .iter()
                        .map(String::as_str),
                )),
                _ => Err(format!(
                    "{group} names more than one journey; name it by its file:line"
                )),
            };
        }
        let endpoint = group.split_once(' ').is_some_and(|(method, path)| {
            !method.is_empty()
                && method.bytes().all(|b| b.is_ascii_uppercase())
                && !path.trim().is_empty()
        });
        if endpoint || group == super::metrics::group::OTHER || group.starts_with("static ") {
            return Ok(Target::Endpoint(group.to_owned()));
        }
        let names: Vec<String> = [
            "requests",
            "steps",
            "iterations",
            "dropped",
            "arrivals",
            "exceptions",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .chain(VITALS.iter().map(|v| (*v).to_owned()))
        .chain(self.steps.iter().cloned())
        .chain(self.journeys.iter().map(|(_, n)| format!("journey {n}")))
        .collect();
        Err(Self::unknown(group, names.iter().map(String::as_str)))
    }

    fn unknown<'a>(group: &str, names: impl Iterator<Item = &'a str>) -> String {
        let names: Vec<&str> = names.collect();
        let close = protocol::suggest::similar(group, names.iter().copied(), 3);
        let mut text = format!("no group {group}");
        if close.is_empty() {
            text.push_str(
                "; a group is an endpoint (GET /api/cart), requests, step FILE:LINE, steps, \
                 journey NAME, iterations, a Web Vital (lcp, cls, inp, ttfb, fcp, load), \
                 dropped, arrivals or exceptions",
            );
        } else {
            text.push_str("; the closest: ");
            text.push_str(&close.join(", "));
        }
        text
    }
}

/// Watches the thresholds while the run goes: each second, over the run
/// so far.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    states: std::sync::Mutex<Vec<Watched>>,
}

/// A threshold as the run watches it.
#[derive(Debug, Clone, Copy, Default)]
struct Watched {
    /// The second it started failing, if it fails now.
    since: Option<u64>,
    /// It stopped the run.
    aborted: bool,
}

/// What a threshold did at a second.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Change {
    /// It started failing; the value now.
    Failing(usize, f64),
    /// It failed for [`ABORT_AFTER`] with `abort = true`: stop the run.
    Abort(usize),
}

impl Watch {
    /// Takes the thresholds' `values` at second `t` (`None` for what has
    /// no value yet: not failing); what changed.
    pub(crate) fn check(
        &self,
        thresholds: &[Threshold],
        t: u64,
        values: &[Option<f64>],
    ) -> Vec<Change> {
        let mut states = self
            .states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        states.resize(thresholds.len(), Watched::default());
        let mut changes = Vec::new();
        for (i, (threshold, value)) in thresholds.iter().zip(values).enumerate() {
            let state = &mut states[i];
            match value {
                Some(v) if !holds(&threshold.written, *v) => {
                    let since = *state.since.get_or_insert_with(|| {
                        changes.push(Change::Failing(i, *v));
                        t
                    });
                    if threshold.written.abort
                        && !state.aborted
                        && t.saturating_sub(since) >= ABORT_AFTER.as_secs()
                    {
                        state.aborted = true;
                        changes.push(Change::Abort(i));
                    }
                }
                _ => state.since = None,
            }
        }
        changes
    }

    /// Whether threshold `i` stopped the run.
    pub(crate) fn aborted(&self, i: usize) -> bool {
        self.states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(i)
            .is_some_and(|s| s.aborted)
    }
}

/// `p95(GET /api/cart) < 300ms (412 ms now)`.
pub(crate) fn said(threshold: &Written, value: f64) -> String {
    let shown = threshold.unit.shown(value);
    format!(
        "{} {} ({shown}{} now)",
        threshold.name,
        threshold.condition,
        match threshold.unit {
            Unit::Ms => " ms",
            Unit::Percent => "%",
            Unit::PerSecond => "/s",
            Unit::Number => "",
        }
    )
}

/// Whether `value` holds `threshold`.
pub(crate) fn holds(threshold: &Written, value: f64) -> bool {
    threshold.op.holds(value, threshold.limit)
}

/// A threshold's result.
pub(crate) fn result(
    threshold: &Threshold,
    value: Option<f64>,
    aborted: bool,
    note: Option<String>,
) -> LoadThreshold {
    let w = &threshold.written;
    let passed = !aborted && value.is_some_and(|v| holds(w, v));
    LoadThreshold {
        name: w.name.clone(),
        condition: w.condition.clone(),
        passed,
        value: value.map(|v| w.unit.shown(v)),
        limit: w.unit.shown(w.limit),
        unit: w.unit.as_str().to_owned(),
        abort: w.abort,
        aborted,
        note,
    }
}

#[cfg(test)]
mod tests;
