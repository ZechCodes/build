//! Always-on review synchronization, independent of subscriptions.
pub(crate) mod reconcile;
mod scheduler;
#[cfg(test)]
mod service_tests;
mod worker;

use crate::store::Store;
use reconcile::SyncResult;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Dropping the daemon's handle stops admission of further reconciliation work.
pub struct ReviewSyncHandle {
    stopped: Arc<AtomicBool>,
}
impl Drop for ReviewSyncHandle {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

pub(crate) fn spawn(
    store: Store,
    alive: Arc<dyn Fn() -> bool + Send + Sync>,
    invalidate: Arc<dyn Fn(SyncResult) + Send + Sync>,
) -> ReviewSyncHandle {
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    tokio::spawn(async move {
        let mut worker = worker::Worker::new(store, invalidate);
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if stop.load(Ordering::Acquire) || !alive() {
                break;
            }
            let stopping = stop.clone();
            let result = tokio::task::spawn_blocking(move || {
                worker.turn(&stopping);
                worker
            })
            .await;
            match result {
                Ok(returned) => worker = returned,
                Err(error) => {
                    eprintln!("review sync: worker failed: {error}");
                    break;
                }
            }
        }
    });
    ReviewSyncHandle { stopped }
}

#[cfg(test)]
mod multi_repository_tests;
