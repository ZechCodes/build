//! Daemon registration and post-persistence invalidation only.
use crate::app::{off_the_workers, AppState};
use crate::changes::{BoardLists, Kind};
use crate::reviews::sync::{self, ReviewSyncHandle};
use std::sync::{Arc, Mutex};

impl AppState {
    /// Explicit daemon startup; constructing an AppState never starts Git work.
    pub async fn spawn_review_sync(state: Arc<Mutex<AppState>>) -> Option<ReviewSyncHandle> {
        let weak = Arc::downgrade(&state);
        let store = off_the_workers(move || state.lock().unwrap().store.clone()).await?;
        let alive = weak.clone();
        Some(sync::spawn(
            store,
            Arc::new(move || alive.strong_count() > 0),
            Arc::new(move |result| {
                let Some(state) = weak.upgrade() else {
                    return;
                };
                let app = state.lock().unwrap();
                let Some(project) = app
                    .projects
                    .iter()
                    .find(|project| project.repo_path.to_string_lossy() == result.project_path)
                else {
                    return;
                };
                app.note_tasks_changed(&project.id, &result.task_id);
                app.changes.note_kind(&result.workspace_id, Kind::State);
                app.changes.note_board_lists(BoardLists::WORKSPACES);
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn startup_waits_for_app_mutex_off_the_runtime_worker() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new_unrooted(dir.path(), "main", true, "unused").shared();
        let (held, held_rx) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let holding = state.clone();
        let holder = std::thread::spawn(move || {
            let _guard = holding.lock().unwrap();
            held.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(3)).is_ok()
        });
        held_rx.await.unwrap();
        let registration = tokio::spawn(AppState::spawn_review_sync(state));
        // On one worker, a startup locking directly would prevent this timer.
        tokio::time::sleep(Duration::from_millis(30)).await;
        release.send(()).unwrap();
        assert!(holder.join().unwrap(), "startup blocked the sole worker");
        assert!(registration.await.unwrap().is_none());
    }
}
