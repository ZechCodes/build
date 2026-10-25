//! Receiver cleanup registration copies handles only; disk work stays in the
//! dedicated niced service, outside RPC and the app mutex.
use crate::app::AppState;
use crate::reviews::maintenance::{self, MaintenanceHandle, Registration};
use std::sync::{Arc, Mutex};

impl AppState {
    pub fn spawn_receiver_maintenance(state: &Arc<Mutex<Self>>) -> Option<MaintenanceHandle> {
        let weak = Arc::downgrade(state);
        let alive = weak.clone();
        let capture = Arc::new(move || {
            let Some(state) = weak.upgrade() else {
                return Registration::Unavailable;
            };
            let Ok(app) = state.try_lock() else {
                return Registration::Busy;
            };
            match &app.store {
                Some(store) => {
                    Registration::Ready(store.clone(), app.state_root.join("review-receivers"))
                }
                None => Registration::Unavailable,
            }
        });
        match maintenance::spawn(capture, Arc::new(move || alive.strong_count() > 0)) {
            Ok(handle) => Some(handle),
            Err(error) => {
                eprintln!("review receivers: cannot start maintenance: {error}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_does_not_wait_for_busy_app_mutex() {
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::new_unrooted(directory.path(), "main", true, "unused").shared();
        let guard = state.lock().unwrap();
        let started = std::time::Instant::now();
        let service = AppState::spawn_receiver_maintenance(&state).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
        drop(guard);
        drop(service);
    }
}
