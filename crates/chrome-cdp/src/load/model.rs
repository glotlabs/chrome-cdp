//! When users arrive and start (plan 23.3), worked out from the profile's
//! stages alone, so it is the same however the site answers: the due time
//! of each arrival of an open model, when each user of a closed model
//! starts and stops, and which journey an iteration takes.

use std::time::Duration;

use super::profile::{Arrivals, RateStage, UserStage, Users};

/// The due times of an open model's arrivals, from the run's start: the
/// k-th (from 0) is due when the stages' rate, summed from the start, is
/// k + ½. A ramp's arrivals come closer and closer; a rate of 0 brings
/// none.
#[derive(Debug, Clone)]
pub(crate) struct Schedule<'a> {
    stages: &'a [RateStage],
    /// The stage the next arrival is in.
    stage: usize,
    /// When it starts, in s.
    start: f64,
    /// Arrivals due before it.
    before: f64,
    /// The next arrival, from 0.
    next: u64,
}

impl<'a> Schedule<'a> {
    pub(crate) fn new(arrivals: &'a Arrivals) -> Self {
        Self {
            stages: &arrivals.stages,
            stage: 0,
            start: 0.0,
            before: 0.0,
            next: 0,
        }
    }
}

impl Iterator for Schedule<'_> {
    type Item = Duration;

    fn next(&mut self) -> Option<Duration> {
        #[allow(clippy::cast_precision_loss)]
        let wanted = self.next as f64 + 0.5;
        while let Some(stage) = self.stages.get(self.stage) {
            let length = stage.duration.as_secs_f64();
            if let Some(t) = reaches(stage, wanted - self.before)
                && t < length
            {
                self.next += 1;
                return Some(Duration::from_secs_f64(self.start + t));
            }
            self.before += f64::midpoint(stage.from, stage.to) * length;
            self.start += length;
            self.stage += 1;
        }
        None
    }
}

/// When, from the start of `stage`, its arrivals reach `count`.
fn reaches(stage: &RateStage, count: f64) -> Option<f64> {
    if count <= 0.0 {
        return Some(0.0);
    }
    let (a, b) = (stage.from, stage.to);
    let length = stage.duration.as_secs_f64();
    if (b - a).abs() < f64::EPSILON {
        return (a > 0.0).then(|| count / a);
    }
    // a·t + (b − a)·t² / 2L = count
    let k = (b - a) / length;
    let disc = a * a + 2.0 * k * count;
    if disc < 0.0 {
        return None;
    }
    Some((disc.sqrt() - a) / k)
}

/// The arrival rate's peak, a second.
pub(crate) fn peak_rate(arrivals: &Arrivals) -> f64 {
    arrivals
        .stages
        .iter()
        .map(|s| s.from.max(s.to))
        .fold(0.0, f64::max)
}

/// How many arrivals the stages bring in all.
pub(crate) fn total_arrivals(arrivals: &Arrivals) -> u64 {
    let total: f64 = arrivals
        .stages
        .iter()
        .map(|s| f64::midpoint(s.from, s.to) * s.duration.as_secs_f64())
        .sum();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let n = total.round().max(0.0) as u64;
    n
}

/// The users at once an open model is expected to reach: the peak rate
/// times how long an iteration takes (Little's law), at most `max_users`.
pub(crate) fn expected_users(arrivals: &Arrivals, iteration: Duration) -> u32 {
    let users = (peak_rate(arrivals) * iteration.as_secs_f64()).ceil();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let users = users.clamp(0.0, f64::from(arrivals.max_users)) as u32;
    users
}

impl UserStage {
    /// The count `t` s into the stage.
    fn at(&self, t: f64) -> f64 {
        let length = self.duration.as_secs_f64();
        let (a, b) = (f64::from(self.from), f64::from(self.to));
        a + (b - a) * (t / length).clamp(0.0, 1.0)
    }
}

impl Users {
    /// When user `vu` (from 1) may start an iteration, `from` or later
    /// (from the run's start): while the stages' count is above `vu − 1`,
    /// or when it rises to it. `None`: never again.
    pub(crate) fn next_start(&self, vu: u32, from: Duration) -> Option<Duration> {
        let v = f64::from(vu.saturating_sub(1));
        let from = from.as_secs_f64();
        let mut start = 0.0;
        for stage in &self.stages {
            let end = start + stage.duration.as_secs_f64();
            if end <= from {
                start = end;
                continue;
            }
            let t0 = from.max(start);
            if stage.at(t0 - start) > v {
                return Some(Duration::from_secs_f64(t0));
            }
            let (a, b) = (f64::from(stage.from), f64::from(stage.to));
            if b > v && b > a {
                let t = start + stage.duration.as_secs_f64() * (v - a) / (b - a);
                if t < end {
                    return Some(Duration::from_secs_f64(t.max(t0)));
                }
            }
            start = end;
        }
        None
    }
}

/// Picks among `weights` by weight, with `unit` from 0 to 1.
pub(crate) fn pick(weights: &[u32], unit: f64) -> usize {
    let total: u64 = weights.iter().map(|&w| u64::from(w)).sum();
    #[allow(clippy::cast_precision_loss)]
    let target = unit.clamp(0.0, 1.0) * total as f64;
    let mut sum = 0u64;
    for (i, &w) in weights.iter().enumerate() {
        sum += u64::from(w);
        #[allow(clippy::cast_precision_loss)]
        if target < sum as f64 {
            return i;
        }
    }
    weights.len().saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::profile::{Arrivals, RateStage, UserStage, Users};
    use super::super::user::Random;
    use super::{Schedule, expected_users, pick, total_arrivals};

    fn secs(s: f64) -> Duration {
        Duration::from_secs_f64(s)
    }

    fn arrivals(stages: &[(f64, f64, u64)]) -> Arrivals {
        Arrivals {
            stages: stages
                .iter()
                .map(|&(from, to, s)| RateStage {
                    from,
                    to,
                    duration: Duration::from_secs(s),
                })
                .collect(),
            max_users: 100,
        }
    }

    fn due(arrivals: &Arrivals) -> Vec<f64> {
        Schedule::new(arrivals)
            .map(|d| (d.as_secs_f64() * 1000.0).round() / 1000.0)
            .collect()
    }

    #[test]
    fn a_steady_rate_spaces_arrivals_evenly() {
        let a = arrivals(&[(2.0, 2.0, 3)]);
        assert_eq!(due(&a), [0.25, 0.75, 1.25, 1.75, 2.25, 2.75]);
        assert_eq!(total_arrivals(&a), 6);
    }

    #[test]
    fn a_ramp_brings_arrivals_closer() {
        // 0..4/s over 2 s: 4 arrivals, at √(k + ½) s.
        let a = arrivals(&[(0.0, 4.0, 2)]);
        let times = due(&a);
        assert_eq!(times, [0.707, 1.225, 1.581, 1.871]);
        // A ramp, a hold and a ramp down: the counts add up.
        let a = arrivals(&[(0.0, 10.0, 10), (10.0, 10.0, 10), (10.0, 0.0, 10)]);
        let times = due(&a);
        assert_eq!(times.len() as u64, total_arrivals(&a));
        assert_eq!(times.len(), 200);
        assert!(times.windows(2).all(|w| w[0] < w[1]));
        let held = times.iter().filter(|&&t| (10.0..20.0).contains(&t)).count();
        assert_eq!(held, 100);
        assert!(*times.last().unwrap() < 30.0);
        // No arrival at a rate of 0, then 1/s.
        let a = arrivals(&[(0.0, 0.0, 5), (1.0, 1.0, 2)]);
        assert_eq!(due(&a), [5.5, 6.5]);
    }

    #[test]
    fn expected_users_follow_littles_law() {
        let a = arrivals(&[(0.0, 20.0, 60), (20.0, 20.0, 60)]);
        assert_eq!(expected_users(&a, secs(5.0)), 100);
        assert_eq!(expected_users(&a, secs(2.5)), 50);
        assert_eq!(expected_users(&a, secs(60.0)), 100);
    }

    fn users(stages: &[(u32, u32, u64)]) -> Users {
        Users {
            stages: stages
                .iter()
                .map(|&(from, to, s)| UserStage {
                    from,
                    to,
                    duration: Duration::from_secs(s),
                })
                .collect(),
            iterations: None,
        }
    }

    #[test]
    fn users_start_as_the_count_rises_and_stop_as_it_falls() {
        // 23.1's --ramp-up: user v of 10 starts at (v − 1) s.
        let u = users(&[(0, 10, 10), (10, 10, 20)]);
        assert_eq!(u.next_start(1, secs(0.0)), Some(secs(0.0)));
        assert_eq!(u.next_start(6, secs(0.0)), Some(secs(5.0)));
        assert_eq!(u.next_start(6, secs(7.0)), Some(secs(7.0)));
        assert_eq!(u.next_start(10, secs(29.9)), Some(secs(29.9)));
        assert_eq!(u.next_start(10, secs(30.0)), None);
        assert_eq!(u.next_start(11, secs(0.0)), None);
        // Down to 2 and up again: user 5 waits for the rise.
        let u = users(&[(10, 10, 10), (10, 2, 8), (2, 2, 10), (2, 10, 8)]);
        assert_eq!(u.next_start(5, secs(10.0)), Some(secs(10.0)));
        // The count is 6 at 14 s, 4 at 16 s: user 5 stops past 15 s.
        assert_eq!(u.next_start(5, secs(14.0)), Some(secs(14.0)));
        assert_eq!(u.next_start(5, secs(16.0)), Some(secs(30.0)));
        assert_eq!(u.next_start(2, secs(20.0)), Some(secs(20.0)));
    }

    #[test]
    fn journeys_are_picked_by_weight() {
        let weights = [70, 25, 5];
        let mut random = Random::new(23);
        let mut counts = [0u32; 3];
        for _ in 0..100_000 {
            counts[pick(&weights, random.unit())] += 1;
        }
        for (count, weight) in counts.iter().zip(weights) {
            let share = f64::from(*count) / 100_000.0;
            let wanted = f64::from(weight) / 100.0;
            assert!((share - wanted).abs() < 0.01, "{counts:?}");
        }
        assert_eq!(pick(&[1], 0.99), 0);
        assert_eq!(pick(&[1, 1], 1.0), 1);
        assert_eq!(pick(&[1, 1], 0.0), 0);
    }
}
