//! A load run's Chromes (plan 23.1): N processes, each driven by an
//! [`Engine`] of the run's own over one CDP connection, whose browser
//! contexts share it through flattened target sessions. A user keeps its
//! Chrome for the whole run; when that Chrome crashes, the first user to
//! notice starts it again, and the others wait for it.

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
    /// How many times a Chrome crashed and was started again.
    restarts: AtomicU64,
}

#[derive(Debug)]
struct Browser {
    engine: Arc<Engine>,
    /// Held while the Chrome starts again.
    restart: tokio::sync::Mutex<()>,
}

impl Pool {
    /// Starts `count` Chromes at once with `open`; the engines' sessions
    /// are `<name>-1`, `<name>-2`, ….
    pub(super) async fn start(
        name: &str,
        count: u32,
        open: OpenRequest,
    ) -> Result<Self, ErrorInfo> {
        let browsers: Vec<Browser> = (1..=count)
            .map(|i| Browser {
                engine: Engine::new(format!("{name}-{i}"), None, None),
                restart: tokio::sync::Mutex::new(()),
            })
            .collect();
        let started =
            futures_util::future::join_all(browsers.iter().map(|b| b.engine.open(open.clone())))
                .await;
        let pool = Self {
            browsers,
            open,
            restarts: AtomicU64::new(0),
        };
        if let Some(Err(error)) = started.into_iter().find(Result::is_err) {
            pool.close().await;
            return Err(error);
        }
        Ok(pool)
    }

    /// How many Chromes there are.
    pub(super) fn len(&self) -> usize {
        self.browsers.len()
    }

    /// Chrome `index`'s engine, started again first if it crashed.
    pub(super) async fn engine(&self, index: usize) -> Result<Arc<Engine>, ErrorInfo> {
        let browser = &self.browsers[index % self.browsers.len()];
        if browser.engine.state() != SessionState::Running {
            let _restart = browser.restart.lock().await;
            if browser.engine.state() != SessionState::Running {
                tracing::warn!(
                    session = browser.engine.session(),
                    "a load run's Chrome went away; starting it again"
                );
                browser.engine.open(self.open.clone()).await?;
                self.restarts.fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok(browser.engine.clone())
    }

    /// How many times a Chrome was started again.
    pub(super) fn restarts(&self) -> u64 {
        self.restarts.load(Ordering::Relaxed)
    }

    /// Shuts every Chrome down.
    pub(super) async fn close(&self) {
        futures_util::future::join_all(self.browsers.iter().map(|b| b.engine.close())).await;
    }
}
