//! Own/join the existing watchdog guard. No deadline ledger or second watchdog.
use super::super::client_watch::{ClientWatchGuard, ClientWatchObservation, WatchReport};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
pub(super) struct WatchOwner {
    guard: Mutex<Option<ClientWatchGuard>>,
    observer: Arc<ClientWatchObservation>,
    started: AtomicBool,
    finished: AtomicBool,
    report: OnceLock<Arc<WatchReport>>,
    join_error: OnceLock<String>,
}
impl WatchOwner {
    pub fn new(guard: ClientWatchGuard) -> Arc<Self> {
        let observer = guard.observer();
        Arc::new(Self {
            guard: Mutex::new(Some(guard)),
            observer,
            started: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            report: OnceLock::new(),
            join_error: OnceLock::new(),
        })
    }
    pub fn observer(&self) -> Arc<ClientWatchObservation> {
        Arc::clone(&self.observer)
    }
    pub fn request_finish(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let owner = Arc::clone(self);
        tokio::spawn(async move {
            let guard = owner.guard.lock().unwrap().take();
            if let Some(guard) = guard {
                match tokio::task::spawn_blocking(move || guard.finish()).await {
                    Ok(report) => {
                        let _ = owner.report.set(Arc::new(report));
                    }
                    Err(error) => {
                        let _ = owner.join_error.set(error.to_string());
                    }
                }
            }
            owner.finished.store(true, Ordering::Release);
        });
    }
    pub fn report(&self) -> Option<Arc<WatchReport>> {
        self.report.get().cloned()
    }
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}
