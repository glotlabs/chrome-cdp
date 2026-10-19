//! A load run's Chromes (plan 23.1): N processes, each driven by an
//! [`Engine`] of the run's own over one CDP connection, whose browser
//! contexts share it through flattened target sessions. A user keeps its
//! Chrome for the whole run; when that Chrome crashes, the first user to
//! notice starts it again, and the others wait for it. The Chromes past
//! the first few may start only when a user first needs them (plan 23.3:
//! an open model's users arrive over time).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use engine::{Engine, OpenRequest};
use protocol::ErrorInfo;
use protocol::schema::SessionState;

/// The run's Chromes.
#[derive(Debug)]
pub(super) struct Pool {
    browsers: Vec<Browser>,
    /// How each Chrome starts.
    open: OpenRequest,
    /// What every context blocks.
    blocked: Vec<engine::BlockedUrl>,
    /// How many times a Chrome crashed and was started again.
    restarts: AtomicU64,
}

#[derive(Debug)]
struct Browser {
    engine: Arc<Engine>,
    /// Held while the Chrome starts again.
    restart: tokio::sync::Mutex<()>,
    /// How many times it was started again.
    generation: AtomicU64,
    /// Whether it was started at all.
    started: std::sync::atomic::AtomicBool,
}

impl Pool {
    /// A pool of `count` Chromes, started with `open` and each blocking
    /// `blocked` (which a Chrome started again keeps); the first `now` of
    /// them start at once, the others when first needed. The engines'
    /// sessions are `<name>-1`, `<name>-2`, ….
    pub(super) async fn start(
        name: &str,
        count: u32,
        now: u32,
        open: OpenRequest,
        blocked: &[engine::BlockedUrl],
    ) -> Result<Self, ErrorInfo> {
        let browsers: Vec<Browser> = (1..=count)
            .map(|i| Browser {
                engine: Engine::new(format!("{name}-{i}"), None, None),
                restart: tokio::sync::Mutex::new(()),
                generation: AtomicU64::new(0),
                started: std::sync::atomic::AtomicBool::new(false),
            })
            .collect();
        let pool = Self {
            browsers,
            open,
            blocked: blocked.to_vec(),
            restarts: AtomicU64::new(0),
        };
        let now = usize::try_from(now).unwrap_or(usize::MAX).max(1);
        let started =
            futures_util::future::join_all(pool.browsers.iter().take(now).map(|b| pool.launch(b)))
                .await;
        if let Some(Err(error)) = started.into_iter().find(Result::is_err) {
            pool.close().await;
            return Err(error);
        }
        Ok(pool)
    }

    /// Starts `browser`'s Chrome.
    async fn launch(&self, browser: &Browser) -> Result<(), ErrorInfo> {
        browser.engine.open(self.open.clone()).await?;
        browser.started.store(true, Ordering::Relaxed);
        if self.blocked.is_empty() {
            return Ok(());
        }
        browser
            .engine
            .block_urls(self.blocked.clone(), self.open.timeout_ms)
            .await
    }

    /// How many Chromes there are.
    pub(super) fn len(&self) -> usize {
        self.browsers.len()
    }

    /// How many Chromes were started.
    pub(super) fn started(&self) -> u32 {
        let started = self
            .browsers
            .iter()
            .filter(|b| b.started.load(Ordering::Relaxed))
            .count();
        u32::try_from(started).unwrap_or(u32::MAX)
    }

    /// Chrome `index`'s engine, started first if it was not yet, or again
    /// if it crashed.
    pub(super) async fn engine(&self, index: usize) -> Result<Arc<Engine>, ErrorInfo> {
        let browser = &self.browsers[index % self.browsers.len()];
        if browser.engine.state() != SessionState::Running {
            let _restart = browser.restart.lock().await;
            if browser.engine.state() != SessionState::Running {
                if browser.started.load(Ordering::Relaxed) {
                    tracing::warn!(
                        session = browser.engine.session(),
                        "a load run's Chrome went away; starting it again"
                    );
                    browser.engine.open(self.open.clone()).await?;
                    browser.generation.fetch_add(1, Ordering::Relaxed);
                    self.restarts.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.launch(browser).await?;
                }
            }
        }
        Ok(browser.engine.clone())
    }

    /// How many times Chrome `index` was started again: a context of an
    /// earlier start is gone.
    pub(super) fn generation(&self, index: usize) -> u64 {
        self.browsers[index % self.browsers.len()]
            .generation
            .load(Ordering::Relaxed)
    }

    /// How many times a Chrome was started again.
    pub(super) fn restarts(&self) -> u64 {
        self.restarts.load(Ordering::Relaxed)
    }

    /// Shuts every Chrome down.
    pub(super) async fn close(&self) {
        futures_util::future::join_all(
            self.browsers
                .iter()
                .filter(|b| b.started.load(Ordering::Relaxed))
                .map(|b| b.engine.close()),
        )
        .await;
    }
}
