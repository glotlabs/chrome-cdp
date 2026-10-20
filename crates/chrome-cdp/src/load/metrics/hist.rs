//! A load run's distributions (plan 23.4): HDR histograms, whose memory
//! depends on the range of the values, not on how many there are, and
//! which add up (per second into the run's, and one day across machines).

use hdrhistogram::Histogram;

/// Significant figures kept: 1% precision, about 18 KB a histogram up to
/// 10 s in µs.
const SIGFIGS: u8 = 2;

/// What a histogram's values are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unit {
    /// Durations, kept in µs and read in ms.
    Ms,
    /// A score (CLS), kept in ten-thousandths.
    Score,
}

impl Unit {
    /// How many stored units one value is.
    fn scale(self) -> f64 {
        match self {
            Self::Ms => 1000.0,
            Self::Score => 10_000.0,
        }
    }

    /// `ms`, or empty for a score.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ms => "ms",
            Self::Score => "",
        }
    }
}

/// One distribution.
#[derive(Debug, Clone)]
pub(crate) struct Hist {
    hist: Histogram<u64>,
    unit: Unit,
}

impl Hist {
    pub(crate) fn new(unit: Unit) -> Self {
        let mut hist = Histogram::new(SIGFIGS).unwrap_or_else(|_| {
            Histogram::new_with_bounds(1, u64::MAX / 4, SIGFIGS).unwrap_or_else(|_| unreachable!())
        });
        hist.auto(true);
        Self { hist, unit }
    }

    /// Records `value` (ms, or a score).
    pub(crate) fn record(&mut self, value: f64) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let stored = (value.max(0.0) * self.unit.scale()).round() as u64;
        if self.hist.record(stored).is_err() {
            self.hist.saturating_record(stored);
        }
    }

    pub(crate) fn count(&self) -> u64 {
        self.hist.len()
    }

    /// The value at quantile `q` (0 to 1), rounded to 0.1 ms (or 0.0001);
    /// 0 when empty.
    pub(crate) fn quantile(&self, q: f64) -> f64 {
        if self.hist.is_empty() {
            return 0.0;
        }
        // The middle of the value's bucket, not its top.
        self.read(self.hist.median_equivalent(self.hist.value_at_quantile(q)))
    }

    pub(crate) fn max(&self) -> f64 {
        if self.hist.is_empty() {
            return 0.0;
        }
        self.read(self.hist.median_equivalent(self.hist.max()))
    }

    pub(crate) fn min(&self) -> f64 {
        if self.hist.is_empty() {
            return 0.0;
        }
        self.read(self.hist.min())
    }

    pub(crate) fn mean(&self) -> f64 {
        if self.hist.is_empty() {
            return 0.0;
        }
        round(self.hist.mean() / self.unit.scale(), self.unit)
    }

    /// Adds `other`'s values.
    pub(crate) fn add(&mut self, other: &Self) {
        let _ = self.hist.add(&other.hist);
    }

    /// Forgets every value, keeping the memory.
    pub(crate) fn reset(&mut self) {
        self.hist.reset();
    }

    #[allow(clippy::cast_precision_loss)]
    fn read(&self, stored: u64) -> f64 {
        round(stored as f64 / self.unit.scale(), self.unit)
    }
}

/// Rounded to 0.1 ms, or a score to 0.0001.
fn round(value: f64, unit: Unit) -> f64 {
    match unit {
        Unit::Ms => (value * 10.0).round() / 10.0,
        Unit::Score => (value * 10_000.0).round() / 10_000.0,
    }
}

#[cfg(test)]
mod tests {
    use super::{Hist, Unit};

    #[test]
    fn quantiles_are_within_one_percent() {
        let mut hist = Hist::new(Unit::Ms);
        for ms in 1..=1000 {
            hist.record(f64::from(ms));
        }
        assert_eq!(hist.count(), 1000);
        for (q, want) in [(0.5, 500.0), (0.95, 950.0), (0.99, 990.0)] {
            let got = hist.quantile(q);
            assert!((got - want).abs() <= want * 0.01, "p{q}: {got}");
        }
        assert!((hist.max() - 1000.0).abs() <= 10.0);
        assert!((hist.mean() - 500.5).abs() <= 5.0, "{}", hist.mean());
        assert!((hist.min() - 1.0).abs() < 0.1);
    }

    #[test]
    fn small_and_large_times_and_scores_fit() {
        let mut hist = Hist::new(Unit::Ms);
        hist.record(0.25);
        hist.record(3_600_000.0);
        assert!((hist.min() - 0.3).abs() < 0.1 + f64::EPSILON);
        assert!((hist.max() - 3_600_000.0).abs() <= 36_000.0);
        let mut cls = Hist::new(Unit::Score);
        cls.record(0.05);
        cls.record(0.25);
        assert!((cls.quantile(0.5) - 0.05).abs() < 0.001);
        assert!((cls.max() - 0.25).abs() < 0.003);
        assert!(Hist::new(Unit::Ms).quantile(0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn histograms_add_up() {
        let mut a = Hist::new(Unit::Ms);
        let mut b = Hist::new(Unit::Ms);
        a.record(10.0);
        b.record(20.0);
        b.record(30.0);
        a.add(&b);
        assert_eq!(a.count(), 3);
        b.reset();
        assert_eq!(b.count(), 0);
    }
}
