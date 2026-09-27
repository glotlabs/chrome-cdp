//! Pages: the current page, and one async mutex per tab so two commands
//! never interleave their steps on the same page (plan 02, milestone 2.4).
//!
//! Plan 03 replaces the "current page" rule here with a real tab manager
//! (`tabs`, `tab <n>`, `--tab`); the per-tab locks stay.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use cdp::domains::target;
use cdp::{Connection, Session};
use tokio::sync::OwnedMutexGuard;

/// One async mutex per target id. Commands that act on a page hold its
/// guard for the whole command; reads (status, logs) take none.
#[derive(Debug, Default)]
pub(crate) struct TabLocks(Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>);

impl TabLocks {
    /// Waits for the page's turn.
    pub(crate) async fn lock(&self, target_id: &str) -> OwnedMutexGuard<()> {
        let mutex = {
            let mut map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            // Drop the locks of closed tabs nobody waits on.
            map.retain(|_, m| Arc::strong_count(m) > 1 || m.try_lock().is_err());
            map.entry(target_id.to_owned()).or_default().clone()
        };
        mutex.lock_owned().await
    }
}

/// An attached page.
#[derive(Debug, Clone)]
pub(crate) struct Page {
    pub(crate) target_id: String,
    pub(crate) session: Session,
}

/// The page commands act on: the one used last if it is still open, else
/// the first open page, else a new `about:blank` one.
pub(crate) async fn current_page(conn: &Connection, slot: &mut Option<Page>) -> cdp::Result<Page> {
    let targets = conn
        .browser()
        .call(target::GetTargetsParams::default())
        .await?
        .target_infos;
    let mut pages = targets.iter().filter(|t| t.r#type == "page");
    if let Some(page) = slot.as_ref()
        && targets
            .iter()
            .any(|t| t.r#type == "page" && t.target_id == page.target_id)
    {
        return Ok(page.clone());
    }
    let target_id = match pages.next() {
        Some(t) => t.target_id.clone(),
        None => {
            conn.browser()
                .call(target::CreateTargetParams::new("about:blank".into()))
                .await?
                .target_id
        }
    };
    let session = conn.attach(target_id.clone()).await?;
    let page = Page { target_id, session };
    *slot = Some(page.clone());
    Ok(page)
}

/// How many pages are open.
pub(crate) async fn count(conn: &Connection) -> cdp::Result<u32> {
    let targets = conn
        .browser()
        .call(target::GetTargetsParams::default())
        .await?
        .target_infos;
    let pages = targets.iter().filter(|t| t.r#type == "page").count();
    Ok(u32::try_from(pages).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::TabLocks;

    #[tokio::test]
    async fn one_command_per_tab_at_a_time() {
        let locks = Arc::new(TabLocks::default());
        let guard = locks.lock("a").await;
        // Another tab is free…
        drop(locks.lock("b").await);
        // …the same one waits.
        let waiting = tokio::spawn({
            let locks = locks.clone();
            async move { drop(locks.lock("a").await) }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiting.is_finished());
        drop(guard);
        tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .unwrap()
            .unwrap();
    }
}
