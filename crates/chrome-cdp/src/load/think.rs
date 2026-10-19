//! `think` (plan 23): a load run's user pausing between steps, as a
//! person would. A no-op everywhere else, so a scenario runs as a test at
//! full speed.

use std::fmt;
use std::time::Duration;

use protocol::Output;
use protocol::schema::ThinkData;

use crate::cli::{ThinkArgs, parse_duration};

/// A think time: `3s`, `2s..8s` (a time picked evenly between), or
/// `exp(4s)` (exponential, 4 s on average: mostly short pauses, now and
/// then a long one, as people's are).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Think {
    /// From `min` to `max`, evenly; a fixed time when they are the same.
    Between { min: Duration, max: Duration },
    /// Exponential with this mean, at most [`EXP_CAP`] times it.
    Exp { mean: Duration },
}

/// An exponential think time is cut at this many times its mean, so one
/// unlucky pick does not stall a user for minutes.
pub(crate) const EXP_CAP: u32 = 10;

impl Think {
    /// `3s`, `2s..8s`, `exp(4s)`, `0s`.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let trimmed = text.trim();
        if let Some(mean) = trimmed
            .strip_prefix("exp(")
            .and_then(|rest| rest.strip_suffix(')'))
        {
            let mean = duration(mean)?;
            if mean.is_zero() {
                return Err(format!("`{text}`: the mean must be above 0, e.g. exp(4s)"));
            }
            return Ok(Self::Exp { mean });
        }
        let (min, max) = match trimmed.split_once("..") {
            Some((min, max)) => (min.trim(), max.trim()),
            None => (trimmed, trimmed),
        };
        let min = duration(min)?;
        let max = duration(max)?;
        if max < min {
            return Err(format!(
                "`{text}`: the longest time comes after the shortest, e.g. 2s..8s"
            ));
        }
        Ok(Self::Between { min, max })
    }

    /// A time from `unit` (0 to 1, evenly spread).
    pub(crate) fn pick(&self, unit: f64) -> Duration {
        let unit = unit.clamp(0.0, 1.0);
        match *self {
            Self::Between { min, max } => {
                let span = max.saturating_sub(min).as_secs_f64();
                min + Duration::from_secs_f64((span * unit).max(0.0))
            }
            Self::Exp { mean } => {
                let factor = (-(1.0 - unit).ln()).min(f64::from(EXP_CAP));
                Duration::from_secs_f64(mean.as_secs_f64() * factor.max(0.0))
            }
        }
    }

    /// The shortest and the longest time it picks.
    pub(crate) fn bounds(&self) -> (Duration, Duration) {
        match *self {
            Self::Between { min, max } => (min, max),
            Self::Exp { mean } => (Duration::ZERO, mean * EXP_CAP),
        }
    }

    /// The time it picks on average.
    pub(crate) fn mean(&self) -> Duration {
        match *self {
            Self::Between { min, max } => (min + max) / 2,
            // Cut at EXP_CAP means, which takes e^-10 off.
            Self::Exp { mean } => mean,
        }
    }
}

impl fmt::Display for Think {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let show = |d: Duration| protocol::format::duration(d);
        match *self {
            Self::Between { min, max } if min == max => write!(f, "{}", show(min)),
            Self::Between { min, max } => write!(f, "{}..{}", show(min), show(max)),
            Self::Exp { mean } => write!(f, "exp({})", show(mean)),
        }
    }
}

/// A duration that may be zero: `3s`, `500ms`, `0s`, `0`.
pub(crate) fn duration(text: &str) -> Result<Duration, String> {
    let number = text
        .trim()
        .trim_end_matches(|c: char| c.is_ascii_alphabetic());
    if number.parse::<f64>().is_ok_and(|n| n == 0.0) {
        return Ok(Duration::ZERO);
    }
    parse_duration(text)
}

/// `think` outside a load run: nothing happens.
pub(crate) fn run(args: &ThinkArgs) -> Output {
    let bounds = args.time.map(|t| t.bounds());
    let data = ThinkData {
        min_ms: bounds.map(|(min, _)| millis(min)),
        max_ms: bounds.map(|(_, max)| millis(max)),
        mean_ms: args
            .time
            .filter(|t| matches!(t, Think::Exp { .. }))
            .map(|t| millis(t.mean())),
        waited: false,
    };
    Output::typed(&data, String::new())
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::time::Duration;

    use super::Think;

    #[test]
    fn think_times_parse() {
        let secs = Duration::from_secs;
        let fixed = Think::parse("3s").unwrap();
        assert_eq!(fixed.bounds(), (secs(3), secs(3)));
        assert_eq!(fixed.to_string(), "3s");
        let range = Think::parse("2s..8s").unwrap();
        assert_eq!(range.bounds(), (secs(2), secs(8)));
        assert_eq!(range.to_string(), "2s..8s");
        assert_eq!(range.pick(0.5), secs(5));
        assert_eq!(range.pick(0.0), secs(2));
        assert_eq!(range.pick(1.0), secs(8));
        assert_eq!(range.mean(), secs(5));
        assert_eq!(Think::parse("0").unwrap().bounds().1, Duration::ZERO);
        assert_eq!(Think::parse("0s..1s").unwrap().bounds().0, Duration::ZERO);
        let exp = Think::parse("exp(4s)").unwrap();
        assert_eq!(exp, Think::Exp { mean: secs(4) });
        assert_eq!(exp.to_string(), "exp(4s)");
        assert_eq!(exp.pick(0.0), Duration::ZERO);
        assert_eq!(exp.pick(1.0), secs(40));
        assert!(Think::parse("exp(0s)").is_err());
        assert!(Think::parse("exp(4)").is_err());
        assert!(Think::parse("8s..2s").is_err());
        assert!(Think::parse("3").is_err());
        assert!(Think::parse("soon").is_err());
    }

    #[test]
    fn think_times_spread_as_asked() {
        let mut random = super::super::user::Random::new(7);
        let mut mean = |think: &str| {
            let think = Think::parse(think).unwrap();
            let (min, max) = think.bounds();
            let picks: Vec<f64> = (0..20_000)
                .map(|_| think.pick(random.unit()))
                .inspect(|d| assert!((min..=max).contains(d), "{d:?}"))
                .map(|d| d.as_secs_f64())
                .collect();
            picks.iter().sum::<f64>() / 20_000.0
        };
        assert!((mean("2s..8s") - 5.0).abs() < 0.1);
        assert!((mean("3s") - 3.0).abs() < 1e-9);
        // Exponential: the mean is the mean, and most picks are below it.
        assert!((mean("exp(4s)") - 4.0).abs() < 0.15);
    }
}
