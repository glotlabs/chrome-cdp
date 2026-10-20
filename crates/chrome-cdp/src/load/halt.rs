//! Stopping a load run (plan 23.3). A run stops early when Ctrl-C (or
//! SIGTERM) comes, when `on_failure = "stop-run"` counts `max_failures`
//! failed iterations, when a journey's data runs out under
//! `data_exhausted = "stop"`, when the backend's web server exits, when
//! a threshold with `abort = true` fails for 30 seconds in a row, or when
//! the load generator saturates with `on_saturation = "abort"` (plan 23.5).
//! Then no user arrives or starts an iteration any more, and the running
//! iterations get `[load] grace` to finish; a second Ctrl-C stops them at
//! once. Either way the run's report is written for what ran.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::watch;

/// Why a run stopped early.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reason {
    /// Ctrl-C or SIGTERM.
    Interrupted,
    /// `on_failure = "stop-run"`.
    MaxFailures,
    /// `data_exhausted = "stop"`.
    DataExhausted,
    /// The backend's web server exited.
    WebServer,
    /// A threshold with `abort = true` failed for 30 seconds in a row.
    Threshold,
    /// The load generator saturated, with `on_saturation = "abort"`.
    Saturated,
}

impl Reason {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Interrupted => "interrupted",
            Self::MaxFailures => "max_failures",
            Self::DataExhausted => "data_exhausted",
            Self::WebServer => "web_server",
            Self::Threshold => "threshold",
            Self::Saturated => "saturated",
        }
    }
}

/// How far the run is from its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    Running,
    /// No new iterations; the running ones finish within the grace.
    Stopping,
    /// The running iterations are cut short.
    Now,
}

/// A run's stop, shared by its users.
#[derive(Debug)]
pub(super) struct Halt {
    /// Set when the run stops; the web servers' watch sets it too.
    flag: Arc<AtomicBool>,
    level: watch::Sender<Level>,
    reason: Mutex<Option<(Reason, String)>>,
}

impl Halt {
    pub(super) fn new(flag: Arc<AtomicBool>) -> Self {
        Self {
            flag,
            level: watch::Sender::new(Level::Running),
            reason: Mutex::new(None),
        }
    }

    /// Stops the run for `reason` (the first reason given is the one
    /// kept): no new iterations.
    pub(super) fn stop(&self, reason: Reason, message: impl Into<String>) {
        {
            let mut kept = self.reason.lock().unwrap_or_else(PoisonError::into_inner);
            if kept.is_none() {
                *kept = Some((reason, message.into()));
            }
        }
        self.flag.store(true, Ordering::SeqCst);
        self.level.send_if_modified(|level| {
            let was = *level;
            *level = was.max(Level::Stopping);
            *level != was
        });
    }

    /// Cuts the running iterations short.
    pub(super) fn now(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.level.send_replace(Level::Now);
    }

    /// Whether the run is stopping: no new iterations.
    pub(super) fn stopped(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    /// Whether the running iterations were told to stop at once.
    pub(super) fn cut(&self) -> bool {
        *self.level.borrow() == Level::Now
    }

    /// Why the run stopped early, if it did.
    pub(super) fn reason(&self) -> Option<(Reason, String)> {
        self.reason
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Resolves when the run stops; also when something else set the
    /// flag (the web servers' watch), which it takes as their exit.
    pub(super) async fn stopping(&self) {
        let mut level = self.level.subscribe();
        loop {
            if *level.borrow_and_update() >= Level::Stopping {
                return;
            }
            if self.flag.load(Ordering::Relaxed) {
                self.stop(Reason::WebServer, "the backend's web server exited");
                return;
            }
            // The flag has no waker of its own: look again now and then.
            let _ = tokio::time::timeout(FLAG_EVERY, level.changed()).await;
        }
    }

    /// Resolves when the running iterations must stop: at once, or
    /// `grace` after the run started stopping.
    pub(super) async fn cut_after(&self, grace: Duration) {
        self.stopping().await;
        let mut level = self.level.subscribe();
        let now = level.wait_for(|l| *l == Level::Now);
        let _ = tokio::time::timeout(grace, now).await;
    }

    /// Takes Ctrl-C and SIGTERM until the run ends: the first stops the
    /// run, the second cuts the running iterations short. `told` is
    /// called with how many signals came, to say so.
    pub(super) async fn listen(self: Arc<Self>, told: impl Fn(u32) + Send) {
        let mut count = 0;
        loop {
            if !signal().await {
                return;
            }
            count += 1;
            told(count);
            if count == 1 {
                self.stop(Reason::Interrupted, "interrupted (Ctrl-C)");
            } else {
                self.now();
            }
        }
    }
}

/// How often [`Halt::stopping`] looks at the flag.
const FLAG_EVERY: Duration = Duration::from_millis(100);

/// The next Ctrl-C or SIGTERM; `false` when they cannot be listened to.
async fn signal() -> bool {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let Ok(mut term) = signal(SignalKind::terminate()) else {
            return tokio::signal::ctrl_c().await.is_ok();
        };
        tokio::select! {
            c = tokio::signal::ctrl_c() => c.is_ok(),
            t = term.recv() => t.is_some(),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::{Halt, Reason};

    #[tokio::test(start_paused = true)]
    async fn the_first_reason_is_kept_and_the_grace_runs_from_the_stop() {
        let halt = Arc::new(Halt::new(Arc::new(AtomicBool::new(false))));
        assert!(!halt.stopped());
        let cut = tokio::spawn({
            let halt = halt.clone();
            async move {
                halt.cut_after(Duration::from_secs(30)).await;
                tokio::time::Instant::now()
            }
        });
        tokio::time::sleep(Duration::from_secs(5)).await;
        let stopped_at = tokio::time::Instant::now();
        halt.stop(Reason::MaxFailures, "100 failed");
        halt.stop(Reason::Interrupted, "Ctrl-C");
        assert!(halt.stopped());
        assert!(!halt.cut());
        assert_eq!(halt.reason().unwrap().0, Reason::MaxFailures);
        let at = cut.await.unwrap();
        assert_eq!(at - stopped_at, Duration::from_secs(30));
    }

    #[tokio::test(start_paused = true)]
    async fn now_cuts_the_grace_short() {
        let halt = Arc::new(Halt::new(Arc::new(AtomicBool::new(false))));
        let cut = tokio::spawn({
            let halt = halt.clone();
            async move { halt.cut_after(Duration::from_secs(30)).await }
        });
        halt.stop(Reason::Interrupted, "Ctrl-C");
        tokio::time::sleep(Duration::from_secs(1)).await;
        halt.now();
        tokio::time::timeout(Duration::from_secs(1), cut)
            .await
            .unwrap()
            .unwrap();
        assert!(halt.cut());
    }

    #[tokio::test(start_paused = true)]
    async fn a_flag_set_elsewhere_is_the_web_servers_exit() {
        let flag = Arc::new(AtomicBool::new(false));
        let halt = Halt::new(flag.clone());
        flag.store(true, Ordering::SeqCst);
        halt.stopping().await;
        assert_eq!(halt.reason().unwrap().0, Reason::WebServer);
    }
}
