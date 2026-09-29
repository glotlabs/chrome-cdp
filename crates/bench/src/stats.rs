//! Summaries of timing samples.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Which way a metric improves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Better {
    Lower,
    Higher,
}

/// One measured quantity: its samples' median, 95th percentile and mean.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metric {
    pub name: String,
    /// `ms`, `x` (a ratio) or `tokens`.
    pub unit: String,
    pub better: Better,
    pub samples: usize,
    pub p50: f64,
    pub p95: f64,
    pub mean: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Metric {
    /// A timing, from its samples.
    pub fn millis(name: &str, samples: &[Duration]) -> Self {
        let ms: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
        Self::from_values(name, "ms", Better::Lower, &ms)
    }

    /// A single value (a ratio, a count).
    pub fn value(name: &str, unit: &str, better: Better, value: f64) -> Self {
        Self::from_values(name, unit, better, &[value])
    }

    pub fn from_values(name: &str, unit: &str, better: Better, values: &[f64]) -> Self {
        Self {
            name: name.to_owned(),
            unit: unit.to_owned(),
            better,
            samples: values.len(),
            p50: percentile(values, 50.0),
            p95: percentile(values, 95.0),
            mean: mean(values),
            note: None,
        }
    }

    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// The nearest-rank percentile `p` (0-100) of `values`; 0 when empty.
pub fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

#[allow(clippy::cast_precision_loss)]
pub fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_percentiles() {
        let v: Vec<f64> = (1..=10).map(f64::from).collect();
        assert!((percentile(&v, 50.0) - 5.0).abs() < f64::EPSILON);
        assert!((percentile(&v, 95.0) - 10.0).abs() < f64::EPSILON);
        assert!((percentile(&[7.0], 95.0) - 7.0).abs() < f64::EPSILON);
        assert!(percentile(&[], 50.0).abs() < f64::EPSILON);
        assert!((percentile(&[3.0, 1.0, 2.0], 50.0) - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn metrics_from_durations() {
        let m = Metric::millis("x", &[Duration::from_millis(10), Duration::from_millis(30)]);
        assert_eq!(m.samples, 2);
        assert!((m.p50 - 10.0).abs() < 1e-9);
        assert!((m.mean - 20.0).abs() < 1e-9);
    }
}
