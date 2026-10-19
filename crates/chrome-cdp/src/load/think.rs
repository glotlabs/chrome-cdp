//! `think` (plan 23): a load run's user pausing between steps, as a
//! person would. A no-op everywhere else, so a scenario runs as a test at
//! full speed.

use std::fmt;
use std::time::Duration;

use protocol::Output;
use protocol::schema::ThinkData;

use crate::cli::{ThinkArgs, parse_duration};

/// A think time: `3s`, or `2s..8s` (a time picked evenly between).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Think {
    pub(crate) min: Duration,
    pub(crate) max: Duration,
}

impl Think {
    /// `3s`, `2s..8s`, `0s`.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let (min, max) = match text.split_once("..") {
            Some((min, max)) => (min.trim(), max.trim()),
            None => (text.trim(), text.trim()),
        };
        let min = duration(min)?;
        let max = duration(max)?;
        if max < min {
            return Err(format!(
                "`{text}`: the longest time comes after the shortest, e.g. 2s..8s"
            ));
        }
        Ok(Self { min, max })
    }

    /// A time between `min` and `max`, from `unit` (0 to 1).
    pub(crate) fn pick(&self, unit: f64) -> Duration {
        let span = self.max.saturating_sub(self.min).as_secs_f64();
        self.min + Duration::from_secs_f64((span * unit.clamp(0.0, 1.0)).max(0.0))
    }
}

impl fmt::Display for Think {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let show = |d: Duration| protocol::format::duration(d);
        if self.min == self.max {
            write!(f, "{}", show(self.min))
        } else {
            write!(f, "{}..{}", show(self.min), show(self.max))
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
    let data = ThinkData {
        min_ms: args.time.map(|t| millis(t.min)),
        max_ms: args.time.map(|t| millis(t.max)),
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
        let fixed = Think::parse("3s").unwrap();
        assert_eq!(fixed.min, Duration::from_secs(3));
        assert_eq!(fixed.max, Duration::from_secs(3));
        assert_eq!(fixed.to_string(), "3s");
        let range = Think::parse("2s..8s").unwrap();
        assert_eq!(range.min, Duration::from_secs(2));
        assert_eq!(range.max, Duration::from_secs(8));
        assert_eq!(range.pick(0.5), Duration::from_secs(5));
        assert_eq!(range.pick(0.0), Duration::from_secs(2));
        assert_eq!(range.pick(1.0), Duration::from_secs(8));
        assert_eq!(Think::parse("0").unwrap().max, Duration::ZERO);
        assert_eq!(Think::parse("0s..1s").unwrap().min, Duration::ZERO);
        assert!(Think::parse("8s..2s").is_err());
        assert!(Think::parse("3").is_err());
        assert!(Think::parse("soon").is_err());
    }
}
