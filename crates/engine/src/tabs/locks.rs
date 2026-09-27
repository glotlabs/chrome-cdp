//! One async mutex per tab, so two commands never interleave their steps
//! on the same page (plan 02, milestone 2.4).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

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
