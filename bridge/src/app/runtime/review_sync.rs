//! Daemon registration and post-persistence invalidation only.
use crate::app::{off_the_workers, AppState};
use crate::changes::{BoardLists, Kind};
use crate::reviews::sync::{self, ReviewSyncHandle};
use std::sync::{Arc, Mutex, Weak};

impl AppState {
    /// Explicit daemon startup; constructing an AppState never starts Git work.
    pub async fn spawn_review_sync(state: Arc<Mutex<AppState>>) -> Option<ReviewSyncHandle> {
        let weak = Arc::downgrade(&state);
        let (store, reader) = off_the_workers(move || {
            let app = state.lock().unwrap();
            (app.store.clone(), app.workspace_review_reader())
        })
        .await;
        let store = store?;
        let alive = weak.clone();
        let admission_state = weak.clone();
        let authority = store.clone();
        let admission = Arc::new(move |task: &str| {
            // Immutable task ownership is resolved off the app lock; archive
            // and lease acquisition then serialize under that lock.
            let (key, _) = authority.task_project_authority(task).ok()??;
            ReviewSyncLease::enter(&admission_state, &key)
                .map(|lease| Box::new(lease) as Box<dyn Send>)
        });
        Some(sync::spawn_admitted(
            store,
            Arc::new(move || alive.strong_count() > 0),
            Arc::new(move |result| {
                let Some(state) = weak.upgrade() else {
                    return;
                };
                let observed = reader.load(&result.workspace_id);
                let mut app = state.lock().unwrap();
                app.apply_review_sync_result(&result, observed);
            }),
            admission,
        ))
    }
    fn apply_review_sync_result(
        &mut self,
        result: &sync::reconcile::SyncResult,
        observed: Option<crate::app::snapshot_projection::WorkspaceReviewObservation>,
    ) {
        let Some(project) = self
            .project_by_key(&result.project_path)
            .map(|project| project.id.clone())
        else {
            return;
        };
        self.publish_review_observation(&project, &result.task_id, &result.workspace_id, observed);
        self.changes.note_kind(&result.workspace_id, Kind::State);
        self.changes.note_board_lists(BoardLists::WORKSPACES);
    }
}

/// Held through Git reconciliation, persistence and publication. It closes
/// the archive race without holding the app mutex over Git or SQL work.
struct ReviewSyncLease(Weak<Mutex<AppState>>);
impl ReviewSyncLease {
    fn enter(state: &Weak<Mutex<AppState>>, key: &str) -> Option<Self> {
        let shared = state.upgrade()?;
        let mut app = shared.lock().ok()?;
        if app
            .archived_project_keys()
            .iter()
            .any(|archived| archived == key)
        {
            return None;
        }
        app.active_deferred_filesystem_jobs += 1;
        Some(Self(state.clone()))
    }
}
impl Drop for ReviewSyncLease {
    fn drop(&mut self) {
        if let Some(state) = self.0.upgrade() {
            let mut app = state.lock().unwrap_or_else(|poison| poison.into_inner());
            app.active_deferred_filesystem_jobs =
                app.active_deferred_filesystem_jobs.saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn autosync_lease_blocks_archive_until_persistence_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = AppState::new_unrooted(dir.path(), "main", true, "unused");
        let project = app.add_project(dir.path().to_path_buf(), "main".into());
        let key = app.project(&project).unwrap().project_key.clone();
        let shared = app.shared();
        let weak = Arc::downgrade(&shared);
        let lease = ReviewSyncLease::enter(&weak, &key).unwrap();
        let refusal = shared
            .lock()
            .unwrap()
            .project_archive(&project)
            .unwrap_err();
        assert_eq!(refusal.code(), "conflict");
        drop(lease);
        let mut app = shared.lock().unwrap();
        assert_eq!(app.active_deferred_filesystem_jobs, 0);
        app.projects.at_mut(0).archived_at = Some(1);
        drop(app);
        assert!(ReviewSyncLease::enter(&weak, &key).is_none());
    }

    #[test]
    fn environments_extra_identity_review_sync_routes_uuid_keyed_project() {
        let dir = tempfile::tempdir().unwrap();
        let context = crate::harness::HarnessContext::resolved(
            dir.path().join("mcp.sock"),
            dir.path().to_path_buf(),
        )
        .unwrap();
        let mut app =
            AppState::new_unrooted_configured(dir.path().join("worktrees"), "main", true, context);
        let migrated = app.add_project(dir.path().to_path_buf(), "main".into());
        let sibling = crate::app::projects::test_support::register_sibling(&mut app, &migrated);
        let result = sync::reconcile::SyncResult {
            project_path: app.project(&sibling).unwrap().project_key.clone(),
            task_id: "review-task".into(),
            workspace_id: "review-workspace".into(),
            persisted: true,
            retry: false,
        };
        let before = app.changes.board_revision();
        app.apply_review_sync_result(&result, None);
        assert!(
            app.changes.board_revision() > before,
            "UUID-keyed review sync must publish its task and workspace invalidation"
        );
    }

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
