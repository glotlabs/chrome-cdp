//! Load profiles (plan 23.3): `e2e/load/<name>.toml`, a load run written
//! down, so the LLM that writes the scenarios can write it too and it can
//! be reviewed. It names the backend, the journeys (tests of the project's
//! scenarios, by weight, each with the data its users take), the model
//! (`[arrivals]`, open, or `[users]`, closed), the think time and what a
//! failure does.
//!
//! Every problem of a profile comes back at once, each at its
//! `file:line:col`; the journeys' tests and data files are checked later,
//! with the scenarios ([`super::plan`]).

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use protocol::script::Span;
use protocol::{ErrorInfo, ErrorKind, Hint};
use serde::Deserialize;
use toml::Spanned;

use super::think::Think;
use crate::script::Problem;

/// Where profiles live, relative to the project root.
pub(crate) const DIR: &str = "e2e/load";

/// The think time of a profile that sets none.
pub(crate) const THINK: &str = "2s..8s";

/// The most users at once of an `[arrivals]` profile without `max_users`.
pub(crate) const MAX_USERS: u32 = 100;

/// `max_failures` without it, with `on_failure = "stop-run"`.
pub(crate) const MAX_FAILURES: u64 = 100;

/// How long a `[users]` profile with `iterations` and no `duration` may
/// run at most.
pub(crate) const UNTIL_DONE: Duration = Duration::from_secs(24 * 60 * 60);

/// A load profile.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Profile {
    /// As named in messages: relative to the project root when inside it.
    pub(crate) name: String,
    /// `backend`: a real backend of the config; `None`, the config's
    /// default (or `--backend`).
    pub(crate) backend: Option<String>,
    pub(crate) journeys: Vec<JourneySpec>,
    pub(crate) model: Model,
    /// The time of a bare `think`.
    pub(crate) think: Think,
    /// `think_between_steps`: a `think` after every navigation and action
    /// that the scenario did not follow with one.
    pub(crate) think_between_steps: bool,
    pub(crate) on_failure: OnFailure,
    /// With `on_failure = "stop-run"`, the failed iterations that end the
    /// run.
    pub(crate) max_failures: u64,
}

/// A `[[journeys]]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JourneySpec {
    /// `test` as written: `e2e/scenarios/shop.cdp:search`.
    pub(crate) test: String,
    /// The scenario file, absolute.
    pub(crate) file: PathBuf,
    /// Which of its tests.
    pub(crate) pick: TestPick,
    pub(crate) weight: u32,
    pub(crate) data: Option<DataSpec>,
    /// Where `test` is, for its problems.
    pub(crate) at: Span,
}

/// A journey's test in its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TestPick {
    /// `file:test name`.
    Name(String),
    /// `file:12`: the test at line 12, as `test` takes it.
    Line(u32),
}

/// A journey's `data`: one row per iteration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DataSpec {
    /// As written.
    pub(crate) written: String,
    /// Absolute.
    pub(crate) path: PathBuf,
    pub(crate) order: DataOrder,
    pub(crate) exhausted: DataExhausted,
    pub(crate) at: Span,
}

/// `data_order`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum DataOrder {
    /// Row 1, 2, 3, … across all users.
    #[default]
    Sequential,
    /// A row picked at random each time.
    Random,
}

/// `data_exhausted`: what happens after the last row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum DataExhausted {
    /// Start again at the first.
    #[default]
    Wrap,
    /// The run ends: each row is used once (single-use accounts).
    Stop,
}

/// The load model: exactly one of `[arrivals]` and `[users]`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Model {
    Open(Arrivals),
    Closed(Users),
}

impl Model {
    /// `open` or `closed`.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Open(_) => "open",
            Self::Closed(_) => "closed",
        }
    }

    /// How long users keep arriving, or starting iterations.
    pub(crate) fn duration(&self) -> Duration {
        match self {
            Self::Open(a) => a.stages.iter().map(|s| s.duration).sum(),
            Self::Closed(u) => u.stages.iter().map(|s| s.duration).sum(),
        }
    }
}

/// `[arrivals]`: users arrive at a rate, whether or not earlier ones
/// finished; each does one iteration.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Arrivals {
    pub(crate) stages: Vec<RateStage>,
    /// The most users at once; an arrival above it is dropped.
    pub(crate) max_users: u32,
}

/// A stage of `[arrivals]`: the rate goes from `from` to `to` arrivals a
/// second over `duration`, evenly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RateStage {
    pub(crate) from: f64,
    pub(crate) to: f64,
    pub(crate) duration: Duration,
}

/// `[users]`: a number of users, each looping the journeys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Users {
    pub(crate) stages: Vec<UserStage>,
    /// Each user's most iterations.
    pub(crate) iterations: Option<u32>,
}

impl Users {
    /// The most users at once.
    pub(crate) fn peak(&self) -> u32 {
        self.stages
            .iter()
            .map(|s| s.from.max(s.to))
            .max()
            .unwrap_or(0)
    }
}

/// A stage of `[users]`: the count goes from `from` to `to` over
/// `duration`, evenly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UserStage {
    pub(crate) from: u32,
    pub(crate) to: u32,
    pub(crate) duration: Duration,
}

/// `on_failure`: what a failed iteration does to the run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum OnFailure {
    /// The user goes on (closed) or ends (open); the run goes on.
    #[default]
    Continue,
    /// After `max_failures` failed iterations, the run stops: the site is
    /// down.
    StopRun,
}

impl OnFailure {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::StopRun => "stop-run",
        }
    }
}

/// The profile `arg` names: a path to a `.toml` file, or a name in
/// [`DIR`] of the project at `root`. `None` when it names neither (then
/// it is a scenario path).
pub(crate) fn find(arg: &str, root: &Path, cwd: &Path) -> Option<PathBuf> {
    if Path::new(arg)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
    {
        return Some(cwd.join(arg));
    }
    if arg.contains(['/', '\\', ':']) || arg.ends_with(".cdp") {
        return None;
    }
    let path = root.join(DIR).join(format!("{arg}.toml"));
    path.is_file().then_some(path)
}

/// The profiles in the project at `root`, by name.
pub(crate) fn names(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root.join(DIR))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            (path.extension()? == "toml").then(|| path.file_stem()?.to_str().map(str::to_owned))?
        })
        .collect();
    names.sort();
    names
}

/// Reads the profile at `path`; its paths are relative to the project
/// `root`.
pub(crate) fn read(path: &Path, root: &Path) -> Result<Profile, ErrorInfo> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::NotFound,
            format!("cannot read the load profile {}: {e}", path.display()),
        )
        .hint(Hint::text(format!(
            "profiles are {DIR}/<name>.toml; chrome-cdp init load writes an example"
        )))
    })?;
    let name = path
        .strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string();
    parse(&text, &name, root)
}

/// Parses a profile's `text`; `name` is its file as messages name it.
pub(crate) fn parse(text: &str, name: &str, root: &Path) -> Result<Profile, ErrorInfo> {
    let file: Arc<str> = Arc::from(name);
    let at = |range: Range<usize>| span(text, &file, range);
    let raw: RawProfile = toml::from_str(text).map_err(|e| {
        let message = e.message().trim().to_owned();
        let place = e
            .span()
            .map_or_else(|| name.to_owned(), |r| at(r).to_string());
        ErrorInfo::new(ErrorKind::BadArgs, format!("{place}: {message}")).hint(Hint::text(HELP))
    })?;
    let mut problems = Problems {
        list: Vec::new(),
        at: &at,
    };
    let backend = raw.backend.map(Spanned::into_inner);
    // `think` written after `[arrivals]` or `[users]` lands in that table.
    let model_think = match (&raw.arrivals, &raw.users) {
        (Some(a), _) => a.get_ref().think.clone(),
        (None, Some(u)) => u.get_ref().think.clone(),
        (None, None) => None,
    };
    let think_raw = match (raw.think, model_think) {
        (Some(top), Some(_)) => {
            problems.push(
                top.span(),
                "think is set twice: at the top and in the load model's table",
                "keep one; a key after [arrivals] or [users] belongs to that table",
            );
            Some(top)
        }
        (top, inner) => top.or(inner),
    };
    let journeys = journeys(raw.journeys, root, &mut problems);
    let model = match (raw.arrivals, raw.users) {
        (Some(arrivals), None) => arrivals_model(arrivals, &mut problems).map(Model::Open),
        (None, Some(users)) => users_model(users, &mut problems).map(Model::Closed),
        (Some(_), Some(users)) => {
            problems.push(
                users.span(),
                "a profile has [arrivals] (open: users arrive at a rate) or [users] (closed: \
                 a number of users loop), not both",
                "keep [arrivals], the default model: under [users] a slow server slows the \
                 arrivals down too and hides how slow it is",
            );
            None
        }
        (None, None) => {
            problems.push(
                0..0,
                "the profile has no load model: add [arrivals] or [users]",
                "e.g. [arrivals] with stages = [{ rate = \"0..10/s\", duration = \"1m\" }]",
            );
            None
        }
    };
    let think = think_raw.map_or_else(
        || Think::parse(THINK).ok(),
        |t| match Think::parse(t.get_ref()) {
            Ok(think) => Some(think),
            Err(e) => {
                problems.push(
                    t.span(),
                    format!("think: {e}"),
                    "e.g. \"3s\", \"2s..8s\" or \"exp(4s)\"",
                );
                None
            }
        },
    );
    let on_failure = match raw
        .on_failure
        .as_ref()
        .map(|s| (s.span(), s.get_ref().as_str()))
    {
        None | Some((_, "continue")) => OnFailure::Continue,
        Some((_, "stop-run")) => OnFailure::StopRun,
        Some((span, other)) => {
            problems.push(
                span,
                format!("on_failure: `{other}` is not \"continue\" or \"stop-run\""),
                "\"stop-run\" with max_failures = 100 ends a run against a site that is down",
            );
            OnFailure::Continue
        }
    };
    let max_failures = match raw.max_failures {
        None => MAX_FAILURES,
        Some(n) if on_failure != OnFailure::StopRun => {
            problems.push(
                n.span(),
                "max_failures counts towards on_failure = \"stop-run\", which the profile does not set",
                "add on_failure = \"stop-run\"",
            );
            MAX_FAILURES
        }
        Some(n) => positive(&n, "max_failures", &mut problems).map_or(MAX_FAILURES, u64::from),
    };
    if !problems.list.is_empty() {
        return Err(crate::script::invalid(problems.list).hint(Hint::text(HELP)));
    }
    match (model, think) {
        (Some(model), Some(think)) => Ok(Profile {
            name: name.to_owned(),
            backend,
            journeys,
            model,
            think,
            think_between_steps: raw.think_between_steps.unwrap_or(false),
            on_failure,
            max_failures,
        }),
        _ => Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{name}: not a load profile"),
        )),
    }
}

/// The hint of every profile problem.
const HELP: &str =
    "docs/e2e/load.md#profiles has every key; chrome-cdp init load writes an example";

/// A profile's problems so far.
struct Problems<'a> {
    list: Vec<Problem>,
    at: &'a dyn Fn(Range<usize>) -> Span,
}

impl Problems<'_> {
    fn push(&mut self, range: Range<usize>, message: impl Into<String>, hint: impl Into<String>) {
        self.list.push(Problem {
            span: (self.at)(range),
            info: ErrorInfo::new(ErrorKind::BadArgs, message).hint(Hint::text(hint.into())),
        });
    }
}

fn journeys(
    raw: Option<Spanned<Vec<Spanned<RawJourney>>>>,
    root: &Path,
    problems: &mut Problems<'_>,
) -> Vec<JourneySpec> {
    let Some(raw) = raw.filter(|j| !j.get_ref().is_empty()) else {
        problems.push(
            0..0,
            "the profile has no journeys",
            "add [[journeys]] with test = \"e2e/scenarios/shop.cdp:search\" (a scenario file \
             and a test's name or line)",
        );
        return Vec::new();
    };
    let mut journeys = Vec::new();
    for entry in raw.into_inner() {
        let entry = entry.into_inner();
        let test_span = entry.test.span();
        let test = entry.test.into_inner();
        let Some((file, pick)) = split_test(&test) else {
            problems.push(
                test_span,
                format!(
                    "test = \"{test}\": name a test of a scenario file, FILE:NAME or FILE:LINE"
                ),
                "e.g. test = \"e2e/scenarios/shop.cdp:search\"",
            );
            continue;
        };
        let weight = match entry.weight {
            None => 1,
            Some(w) => match positive(&w, "weight", problems) {
                Some(w) => w,
                None => continue,
            },
        };
        let order = match entry
            .data_order
            .as_ref()
            .map(|s| (s.span(), s.get_ref().as_str()))
        {
            None | Some((_, "sequential")) => DataOrder::Sequential,
            Some((_, "random")) => DataOrder::Random,
            Some((span, other)) => {
                problems.push(
                    span,
                    format!("data_order: `{other}` is not \"sequential\" or \"random\""),
                    "\"sequential\" takes the rows in order, across all users",
                );
                DataOrder::Sequential
            }
        };
        let exhausted = match entry
            .data_exhausted
            .as_ref()
            .map(|s| (s.span(), s.get_ref().as_str()))
        {
            None | Some((_, "wrap")) => DataExhausted::Wrap,
            Some((_, "stop")) => DataExhausted::Stop,
            Some((span, other)) => {
                problems.push(
                    span,
                    format!("data_exhausted: `{other}` is not \"wrap\" or \"stop\""),
                    "\"stop\" ends the run after the last row, for single-use accounts",
                );
                DataExhausted::Wrap
            }
        };
        let data = match entry.data {
            Some(data) => Some(DataSpec {
                path: root.join(data.get_ref()),
                at: (problems.at)(data.span()),
                written: data.into_inner(),
                order,
                exhausted,
            }),
            None => {
                if let Some(s) = entry
                    .data_order
                    .as_ref()
                    .map(Spanned::span)
                    .or_else(|| entry.data_exhausted.as_ref().map(Spanned::span))
                {
                    problems.push(
                        s,
                        "data_order and data_exhausted are about a journey's data, which it has not",
                        "add data = \"e2e/load/users.csv\" to the journey",
                    );
                }
                None
            }
        };
        journeys.push(JourneySpec {
            file: root.join(file),
            pick,
            weight,
            data,
            at: (problems.at)(test_span),
            test,
        });
    }
    journeys
}

/// `e2e/a.cdp:search` → (`e2e/a.cdp`, the test named `search`);
/// `e2e/a.cdp:12` → the test at line 12.
fn split_test(test: &str) -> Option<(&str, TestPick)> {
    let end = test.find(".cdp:")? + ".cdp".len();
    let (file, rest) = (&test[..end], test[end + 1..].trim());
    if rest.is_empty() {
        return None;
    }
    Some(match rest.parse::<u32>() {
        Ok(line) => (file, TestPick::Line(line)),
        Err(_) => (file, TestPick::Name(rest.to_owned())),
    })
}

fn arrivals_model(raw: Spanned<RawArrivals>, problems: &mut Problems<'_>) -> Option<Arrivals> {
    let span = raw.span();
    let raw = raw.into_inner();
    let max_users = match &raw.max_users {
        None => MAX_USERS,
        Some(n) => positive(n, "max_users", problems)?,
    };
    let stages: Vec<RateStage> = match (raw.stages, raw.rate, raw.duration) {
        (Some(stages), None, None) => {
            let stages_span = stages.span();
            let stages: Vec<_> = stages
                .into_inner()
                .into_iter()
                .filter_map(|s| {
                    let s = s.into_inner();
                    let (from, to) = rate(&s.rate, problems)?;
                    let duration = stage_duration(&s.duration, problems)?;
                    Some(RateStage { from, to, duration })
                })
                .collect();
            if stages.is_empty() {
                problems.push(
                    stages_span,
                    "[arrivals] stages is empty",
                    "e.g. stages = [{ rate = \"0..10/s\", duration = \"1m\" }]",
                );
            }
            stages
        }
        (None, Some(r), Some(d)) => {
            let (from, to) = rate(&r, problems)?;
            let duration = stage_duration(&d, problems)?;
            vec![RateStage { from, to, duration }]
        }
        (None, Some(r), None) => {
            problems.push(
                r.span(),
                "[arrivals] rate needs a duration: how long users arrive at it",
                "e.g. rate = \"10/s\" with duration = \"5m\"",
            );
            return None;
        }
        (None, None, _) => {
            problems.push(
                span,
                "[arrivals] needs stages (or a rate and a duration)",
                "e.g. stages = [{ rate = \"0..10/s\", duration = \"1m\" }, { rate = \"10/s\", \
                 duration = \"5m\" }]",
            );
            return None;
        }
        (Some(stages), _, _) => {
            problems.push(
                stages.span(),
                "[arrivals] has stages, or one rate and duration, not both",
                "put the rate in a stage: { rate = \"10/s\", duration = \"5m\" }",
            );
            return None;
        }
    };
    if !stages.is_empty() && stages.iter().all(|s| s.from == 0.0 && s.to == 0.0) {
        problems.push(
            span,
            "[arrivals]: no user ever arrives at a rate of 0",
            "e.g. rate = \"10/s\"",
        );
        return None;
    }
    (!stages.is_empty()).then_some(Arrivals { stages, max_users })
}

fn users_model(raw: Spanned<RawUsers>, problems: &mut Problems<'_>) -> Option<Users> {
    let span = raw.span();
    let raw = raw.into_inner();
    let iterations = match &raw.iterations {
        None => None,
        Some(n) => Some(positive(n, "iterations", problems)?),
    };
    let stages = match (raw.stages, raw.count) {
        (Some(stages), None) => {
            if let Some(d) = raw.duration.as_ref().or(raw.ramp_up.as_ref()) {
                problems.push(
                    d.span(),
                    "[users] with stages takes each stage's duration, not duration or ramp_up",
                    "e.g. stages = [{ users = \"0..50\", duration = \"1m\" }, { users = 50, \
                     duration = \"5m\" }]",
                );
                return None;
            }
            let stages_span = stages.span();
            let stages: Vec<UserStage> = stages
                .into_inner()
                .into_iter()
                .filter_map(|s| {
                    let s = s.into_inner();
                    let (from, to) = user_count(&s.users, problems)?;
                    let duration = stage_duration(&s.duration, problems)?;
                    Some(UserStage { from, to, duration })
                })
                .collect();
            if stages.is_empty() {
                problems.push(
                    stages_span,
                    "[users] stages is empty",
                    "e.g. stages = [{ users = \"0..50\", duration = \"1m\" }]",
                );
                return None;
            }
            stages
        }
        (None, Some(count)) => {
            let count = positive(&count, "count", problems)?;
            let ramp_up = match &raw.ramp_up {
                None => Duration::ZERO,
                Some(d) => match super::think::duration(d.get_ref()) {
                    Ok(d) => d,
                    Err(e) => {
                        problems.push(d.span(), format!("ramp_up: {e}"), "e.g. ramp_up = \"1m\"");
                        return None;
                    }
                },
            };
            let duration = match (&raw.duration, iterations) {
                (Some(d), _) => stage_duration(d, problems)?,
                (None, Some(_)) => UNTIL_DONE,
                (None, None) => {
                    problems.push(
                        span,
                        "[users] needs a duration (or iterations): how long the users loop",
                        "e.g. count = 50 with duration = \"5m\"",
                    );
                    return None;
                }
            };
            let mut stages = Vec::new();
            if !ramp_up.is_zero() {
                stages.push(UserStage {
                    from: 0,
                    to: count,
                    duration: ramp_up,
                });
            }
            stages.push(UserStage {
                from: count,
                to: count,
                duration,
            });
            stages
        }
        (None, None) => {
            problems.push(
                span,
                "[users] needs count (or stages)",
                "e.g. count = 50 with duration = \"5m\"",
            );
            return None;
        }
        (Some(stages), Some(_)) => {
            problems.push(
                stages.span(),
                "[users] has count or stages, not both",
                "put the count in a stage: { users = 50, duration = \"5m\" }",
            );
            return None;
        }
    };
    let users = Users { stages, iterations };
    if users.peak() == 0 {
        problems.push(
            span,
            "[users]: the count never goes above 0",
            "e.g. count = 10",
        );
        return None;
    }
    Some(users)
}

/// A rate, `20/s`, `0..20/s`, `120/m`: arrivals a second, from and to.
fn rate(raw: &Spanned<String>, problems: &mut Problems<'_>) -> Option<(f64, f64)> {
    match parse_rate(raw.get_ref()) {
        Ok(r) => Some(r),
        Err(e) => {
            problems.push(
                raw.span(),
                format!("rate: {e}"),
                "e.g. \"20/s\", \"0..20/s\" or \"90/m\"",
            );
            None
        }
    }
}

/// `20/s` → (20, 20); `0..20/s` → (0, 20); `60/m` → (1, 1).
pub(crate) fn parse_rate(text: &str) -> Result<(f64, f64), String> {
    let one = |part: &str, unit: Option<&str>| -> Result<(f64, Option<f64>), String> {
        let (number, per) = match part.split_once('/') {
            Some((n, u)) => (n.trim(), Some(u.trim())),
            None => (part.trim(), unit),
        };
        let n: f64 = number
            .parse()
            .ok()
            .filter(|n: &f64| n.is_finite() && *n >= 0.0)
            .ok_or_else(|| format!("`{text}`: `{number}` is not a number of users"))?;
        let scale = match per {
            None => None,
            Some("s") => Some(1.0),
            Some("m") => Some(60.0),
            Some("h") => Some(3600.0),
            Some(other) => return Err(format!("`{text}`: `/{other}` is not /s, /m or /h")),
        };
        Ok((n, scale))
    };
    let (from, to) = match text.split_once("..") {
        Some((from, to)) => (from, to),
        None => (text, text),
    };
    let (to, to_scale) = one(to, None)?;
    let to_scale = to_scale.ok_or_else(|| format!("`{text}`: say per what, e.g. {text}/s"))?;
    let (from, from_scale) = one(from, None)?;
    let from_scale = from_scale.unwrap_or(to_scale);
    Ok((from / from_scale, to / to_scale))
}

/// A stage's `users`: `50` or `"0..50"`.
fn user_count(raw: &Spanned<toml::Value>, problems: &mut Problems<'_>) -> Option<(u32, u32)> {
    let parsed = match raw.get_ref() {
        toml::Value::Integer(n) => u32::try_from(*n).ok().map(|n| (n, n)),
        toml::Value::String(text) => match text.split_once("..") {
            Some((a, b)) => a.trim().parse().ok().zip(b.trim().parse().ok()),
            None => text.trim().parse().ok().map(|n| (n, n)),
        },
        _ => None,
    };
    if parsed.is_none() {
        problems.push(
            raw.span(),
            "users: a count of users, 50, or a ramp, \"0..50\"",
            "e.g. { users = \"0..50\", duration = \"1m\" }",
        );
    }
    parsed
}

/// A stage's duration, above 0.
fn stage_duration(raw: &Spanned<String>, problems: &mut Problems<'_>) -> Option<Duration> {
    match protocol::format::parse_duration(raw.get_ref()) {
        Ok(d) if !d.is_zero() => Some(d),
        Ok(_) => {
            problems.push(
                raw.span(),
                "duration: a stage lasts some time",
                "e.g. duration = \"1m\"",
            );
            None
        }
        Err(e) => {
            problems.push(
                raw.span(),
                format!("duration: {e}"),
                "e.g. duration = \"1m\"",
            );
            None
        }
    }
}

/// A whole number above 0.
fn positive(raw: &Spanned<i64>, key: &str, problems: &mut Problems<'_>) -> Option<u32> {
    match u32::try_from(*raw.get_ref()) {
        Ok(n) if n > 0 => Some(n),
        _ => {
            problems.push(
                raw.span(),
                format!("{key} = {}: a whole number above 0", raw.get_ref()),
                format!("e.g. {key} = 10"),
            );
            None
        }
    }
}

/// The [`Span`] of bytes `range` of `text`.
fn span(text: &str, file: &Arc<str>, range: Range<usize>) -> Span {
    let start = range.start.min(text.len());
    let before = &text[..start];
    let line = before.matches('\n').count() + 1;
    let column = before.rfind('\n').map_or(start, |i| start - i - 1) + 1;
    Span {
        file: file.clone(),
        line: u32::try_from(line).unwrap_or(u32::MAX),
        column: u32::try_from(column).unwrap_or(u32::MAX),
        start,
        end: range.end,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    backend: Option<Spanned<String>>,
    journeys: Option<Spanned<Vec<Spanned<RawJourney>>>>,
    arrivals: Option<Spanned<RawArrivals>>,
    users: Option<Spanned<RawUsers>>,
    think: Option<Spanned<String>>,
    think_between_steps: Option<bool>,
    on_failure: Option<Spanned<String>>,
    max_failures: Option<Spanned<i64>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJourney {
    test: Spanned<String>,
    weight: Option<Spanned<i64>>,
    data: Option<Spanned<String>>,
    data_order: Option<Spanned<String>>,
    data_exhausted: Option<Spanned<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawArrivals {
    stages: Option<Spanned<Vec<Spanned<RawRateStage>>>>,
    rate: Option<Spanned<String>>,
    duration: Option<Spanned<String>>,
    max_users: Option<Spanned<i64>>,
    think: Option<Spanned<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRateStage {
    rate: Spanned<String>,
    duration: Spanned<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUsers {
    count: Option<Spanned<i64>>,
    duration: Option<Spanned<String>>,
    ramp_up: Option<Spanned<String>>,
    stages: Option<Spanned<Vec<Spanned<RawUserStage>>>>,
    iterations: Option<Spanned<i64>>,
    think: Option<Spanned<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUserStage {
    users: Spanned<toml::Value>,
    duration: Spanned<String>,
}

#[cfg(test)]
mod tests;
