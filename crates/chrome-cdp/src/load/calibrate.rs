//! `load <profile> --calibrate` (plan 23.5): how many users this machine
//! holds on a profile's journeys. Users go up from one, each looping the
//! profile's journeys by weight (a closed model, whatever the profile's),
//! [`RAMP_RATE`] a second up to twice the profile's most users (or
//! `--users`; `--duration` sets the ramp's length), until the load
//! generator saturates or the site starts failing ([`Watch`]), and the run
//! says which came first and at how many users.
//!
//! The result is kept in `e2e/.results/load/calibration.json`, by profile
//! and backend ([`Calibrations`]); a later run of the profile whose users
//! go above [`WARN_AT`] of what the machine held warns before it starts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::metrics::Second;
use super::profile::{Model, UserStage, Users};

/// How many users a calibration adds a second.
pub(crate) const RAMP_RATE: f64 = 2.0;

/// A run's users above this share of what a calibration found warn.
pub(crate) const WARN_AT: f64 = 0.8;

/// The site is failing when, for this many seconds in a row, more than
/// [`FAILING_REQUESTS`] of the second's requests (at least
/// [`FEWEST_REQUESTS`] of them) or more than [`FAILING_ITERATIONS`] of its
/// iterations failed.
pub(crate) const FAILING_FOR: u64 = 5;
pub(crate) const FAILING_REQUESTS: f64 = 0.05;
pub(crate) const FEWEST_REQUESTS: u64 = 10;
pub(crate) const FAILING_ITERATIONS: f64 = 0.1;

/// `calibration.json`'s name, in the load runs' folder.
pub(crate) const FILE: &str = "calibration.json";

/// The fewest users a calibration ramps to.
const FEWEST: u32 = 10;

/// The users a calibration of a profile whose most users at once are
/// `most` goes up to, and over how long.
pub(crate) fn ramp(most: u32, users: Option<u32>, duration: Option<Duration>) -> (u32, Duration) {
    let to = users.unwrap_or_else(|| most.saturating_mul(2).max(FEWEST));
    let over = duration.unwrap_or_else(|| Duration::from_secs_f64(f64::from(to) / RAMP_RATE));
    (to, over.max(Duration::from_secs(1)))
}

/// The model of a calibration up to `to` users over `over`: from one
/// user, evenly.
pub(crate) fn model(to: u32, over: Duration) -> Model {
    Model::Closed(Users {
        stages: vec![UserStage {
            from: 1,
            to,
            duration: over,
        }],
        iterations: None,
    })
}

/// What a calibration found first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum First {
    /// The load generator saturated.
    Generator,
    /// The site's requests or iterations started failing.
    Server,
    /// The ramp ended.
    Neither,
}

impl First {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Generator => "generator",
            Self::Server => "server",
            Self::Neither => "neither",
        }
    }
}

/// What a calibration found, when it did.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Found {
    pub(crate) first: First,
    /// The second it began: the first of the limit's seconds in a row.
    pub(crate) from: u64,
    /// The users at once then.
    pub(crate) users: u32,
    pub(crate) message: String,
}

/// Watches a calibration's seconds for the site failing: [`FAILING_FOR`]
/// failing seconds, not counting those with too little to judge (fewer
/// than [`FEWEST_REQUESTS`] requests and no iteration) in between.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    /// The first of the failing seconds in a row so far.
    since: Mutex<Option<u64>>,
}

impl Watch {
    /// Takes `second`; what was found, once the site failed for
    /// [`FAILING_FOR`] in a row.
    pub(crate) fn second(&self, second: &Second) -> Option<Found> {
        let mut since = self.since.lock().unwrap_or_else(PoisonError::into_inner);
        // A second with too little to judge (a starved machine has some)
        // neither ends a run of failing seconds nor adds to it.
        if second.requests < FEWEST_REQUESTS && second.iterations == 0 {
            return None;
        }
        let Some(why) = failing(second) else {
            *since = None;
            return None;
        };
        let from = *since.get_or_insert(second.t);
        (second.t + 1 - from >= FAILING_FOR).then(|| Found {
            first: First::Server,
            from,
            users: second.users,
            message: format!(
                "the site started failing: {why}, for {FAILING_FOR}s in a row, at {} users",
                second.users
            ),
        })
    }
}

/// Why `second` counts as the site failing: `12% of requests failed`.
fn failing(second: &Second) -> Option<String> {
    #[allow(clippy::cast_precision_loss)]
    let share = |n: u64, of: u64| n as f64 / of as f64;
    if second.requests >= FEWEST_REQUESTS
        && share(second.errors, second.requests) > FAILING_REQUESTS
    {
        return Some(format!(
            "{} of requests failed",
            super::status::percent(share(second.errors, second.requests) * 100.0)
        ));
    }
    if second.iterations > 0 && share(second.failed, second.iterations) > FAILING_ITERATIONS {
        return Some(format!(
            "{} of iterations failed",
            super::status::percent(share(second.failed, second.iterations) * 100.0)
        ));
    }
    None
}

/// The most users at once in `timeline` before second `from` (all of it
/// for `None`).
pub(crate) fn held_before(timeline: &[Second], from: Option<u64>) -> u32 {
    timeline
        .iter()
        .filter(|s| from.is_none_or(|from| s.t < from))
        .map(|s| s.users)
        .max()
        .unwrap_or(0)
}

/// A profile's calibration, as `calibration.json` keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Calibration {
    /// `generator`, `server` or `neither`.
    pub(crate) first: String,
    /// The users at once when it happened, and the most before it.
    pub(crate) users: u32,
    pub(crate) held_users: u32,
    pub(crate) held_per_browser: u32,
    /// `chrome-headless-shell` or `chrome`, and how many users shared one.
    pub(crate) browser: String,
    pub(crate) users_per_browser: u32,
    /// The machine's CPUs.
    pub(crate) cpus: usize,
    /// The calibration run's id.
    pub(crate) run: String,
}

/// `calibration.json`: each profile's last calibration, by `profile on
/// backend`.
#[derive(Debug, Default)]
pub(crate) struct Calibrations {
    path: PathBuf,
    profiles: BTreeMap<String, Calibration>,
}

impl Calibrations {
    /// The file in `runs` (the load runs' folder); empty when it is not
    /// there or cannot be read.
    pub(crate) fn read(runs: &Path) -> Self {
        let path = runs.join(FILE);
        let profiles = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self { path, profiles }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn get(&self, profile: &str, backend: &str) -> Option<&Calibration> {
        self.profiles.get(&key(profile, backend))
    }

    pub(crate) fn set(&mut self, profile: &str, backend: &str, calibration: Calibration) {
        self.profiles.insert(key(profile, backend), calibration);
    }

    /// Writes the file; a failure is only logged, as the run's report is
    /// written anyway.
    pub(crate) fn write(&self) {
        match serde_json::to_string_pretty(&self.profiles) {
            Ok(text) => crate::test::artifacts::write(&self.path, &text),
            Err(e) => tracing::warn!("cannot write {}: {e}", self.path.display()),
        }
    }
}

/// `e2e/load/shop.toml on staging`.
fn key(profile: &str, backend: &str) -> String {
    format!("{profile} on {backend}")
}

/// The warning for a run of up to `users` at once, when a calibration of
/// its profile found this machine holds fewer than `users / WARN_AT`.
pub(crate) fn warning(users: u32, calibration: &Calibration) -> Option<String> {
    if calibration.first != First::Generator.as_str() {
        return None;
    }
    let held = f64::from(calibration.held_users);
    (f64::from(users) > held * WARN_AT).then(|| {
        format!(
            "this run goes up to {users} users, and --calibrate found this machine holds {} on \
             this profile (run {}): above {:.0}% of it, the generator may saturate",
            calibration.held_users,
            calibration.run,
            WARN_AT * 100.0
        )
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Calibration, Calibrations, First, Watch, held_before, ramp, warning};
    use crate::load::metrics::Second;

    fn second(t: u64, users: u32, requests: u64, errors: u64) -> Second {
        Second {
            t,
            users,
            requests,
            errors,
            ..Second::default()
        }
    }

    #[test]
    fn the_ramp_goes_to_twice_the_profile() {
        assert_eq!(ramp(100, None, None), (200, Duration::from_secs(100)));
        assert_eq!(
            ramp(100, Some(40), Some(Duration::from_secs(60))),
            (40, Duration::from_secs(60))
        );
        assert_eq!(ramp(2, None, None), (10, Duration::from_secs(5)));
        assert_eq!(ramp(2, Some(3), None).0, 3);
    }

    #[test]
    fn the_site_fails_for_five_seconds_in_a_row() {
        let watch = Watch::default();
        // 4% errors: not failing; too few requests: not judged.
        assert!(watch.second(&second(1, 10, 100, 4)).is_none());
        assert!(watch.second(&second(2, 10, 5, 5)).is_none());
        for t in 3..=5 {
            assert!(watch.second(&second(t, 20, 100, 20)).is_none(), "{t}");
        }
        // A second with nothing to judge keeps the run of failing seconds.
        assert!(watch.second(&second(6, 20, 0, 0)).is_none());
        let found = watch.second(&second(7, 30, 100, 20)).unwrap();
        assert_eq!(
            (found.first, found.from, found.users),
            (First::Server, 3, 30)
        );
        assert_eq!(
            found.message,
            "the site started failing: 20% of requests failed, for 5s in a row, at 30 users"
        );
        // Iterations count too.
        let watch = Watch::default();
        let failed = |t| Second {
            iterations: 10,
            failed: 5,
            ..second(t, 8, 0, 0)
        };
        assert!((1..=4).all(|t| watch.second(&failed(t)).is_none()));
        assert!(watch.second(&failed(5)).is_some());
        let timeline: Vec<Second> = (1..=6)
            .map(|t| second(t, u32::try_from(t).unwrap() * 3, 0, 0))
            .collect();
        assert_eq!(held_before(&timeline, Some(4)), 9);
        assert_eq!(held_before(&timeline, None), 18);
    }

    #[test]
    fn a_calibration_is_kept_and_warns_later_runs() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = Calibrations::read(dir.path());
        assert!(file.get("e2e/load/shop.toml", "staging").is_none());
        let found = Calibration {
            first: "generator".into(),
            users: 130,
            held_users: 120,
            held_per_browser: 12,
            browser: "chrome-headless-shell".into(),
            users_per_browser: 10,
            cpus: 16,
            run: "abc".into(),
        };
        file.set("e2e/load/shop.toml", "staging", found.clone());
        file.write();
        let again = Calibrations::read(dir.path());
        assert_eq!(again.get("e2e/load/shop.toml", "staging"), Some(&found));
        assert!(again.get("e2e/load/shop.toml", "local").is_none());
        assert!(warning(96, &found).is_none());
        assert_eq!(
            warning(100, &found).unwrap(),
            "this run goes up to 100 users, and --calibrate found this machine holds 120 on \
             this profile (run abc): above 80% of it, the generator may saturate"
        );
        // When the site failed first, the machine's limit is not known.
        let server = Calibration {
            first: "server".into(),
            ..found
        };
        assert!(warning(500, &server).is_none());
    }
}
