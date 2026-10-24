//! Daemon-only retention service. No database or filesystem work under app guards.
use crate::app::{off_the_workers, AppState};
use crate::retention::RetentionConfig;
use crate::store::{Store, StoreError};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime};

const DISCOVERY_PAGE: usize = 100;
const ATTACHMENT_GRACE: Duration = Duration::from_secs(3600);

/// Owned by daemon startup; shutdown prevents further batches immediately.
pub struct RetentionHandle(tokio::task::JoinHandle<()>);
impl Drop for RetentionHandle {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl AppState {
    /// Start once, then repeat while the daemon remains alive. Constructing an
    /// AppState in a test or utility does not start maintenance work.
    pub async fn spawn_retention(
        state: Arc<Mutex<Self>>,
        interval: Duration,
    ) -> Option<RetentionHandle> {
        let weak = Arc::downgrade(&state);
        let (store, config) = off_the_workers(move || {
            let app = state.lock().unwrap();
            (app.store.clone(), app.retention)
        })
        .await;
        let store = store?;
        assert!(!interval.is_zero(), "retention interval must be positive");
        Some(RetentionHandle(tokio::spawn(async move {
            loop {
                if weak.strong_count() == 0 {
                    break;
                }
                if let Err(error) = history_cycle(&weak, &store, config).await {
                    eprintln!("conversation retention: {error}");
                }
                // Orphan cleanup is independent of whether history pruning is enabled.
                if let Err(error) = attachment_cycle(&weak, &store).await {
                    eprintln!("attachment retention: {error}");
                }
                tokio::time::sleep(interval).await;
            }
        })))
    }

    /// Copy only registered paths while holding the guard; never probe the disk.
    fn retention_attachment_homes(&self) -> Vec<PathBuf> {
        let roots = self
            .runs
            .values()
            .map(|run| run.worktree.path.clone())
            .chain(
                self.plans
                    .values()
                    .filter_map(|plan| plan.workspace.as_ref().map(|w| w.checkout.clone())),
            )
            .chain(self.project_snapshot_scratch_paths())
            .chain(self.workspaces.iter().map(|w| w.root.clone()))
            .chain(
                self.workspaces
                    .iter()
                    .flat_map(|w| w.directories.iter().map(|d| d.path.clone())),
            );
        let mut homes: Vec<_> = roots
            .map(|root| root.join(".build/attachments"))
            .chain([self.local_attachments_dir()])
            .collect();
        homes.sort();
        homes.dedup();
        homes
    }
}

async fn history_cycle(
    state: &Weak<Mutex<AppState>>,
    store: &Store,
    config: RetentionConfig,
) -> Result<(), StoreError> {
    let now = crate::store::now_rfc3339();
    let mut after = None;
    let mut conversations = 0u64;
    let mut candidates = 0u64;
    let mut protected = 0u64;
    loop {
        if state.strong_count() == 0 {
            break;
        }
        let listing = store.clone();
        let cursor = after.clone();
        let ids = off_the_workers(move || {
            listing.retention_conversations_after(cursor.as_deref(), DISCOVERY_PAGE)
        })
        .await?;
        if ids.is_empty() {
            break;
        }
        after = ids.last().cloned();
        for id in ids {
            conversations += 1;
            if config.enabled {
                prune(state, store, &id, config, &now).await?;
            } else {
                let reporting = store.clone();
                let timestamp = now.clone();
                let report = off_the_workers(move || {
                    reporting.retention_report_conversation(&id, config.policy(), &timestamp)
                })
                .await?;
                candidates += report.items_to_prune;
                protected += u64::from(report.protected);
            }
            tokio::task::yield_now().await;
        }
    }
    if !config.enabled {
        eprintln!("conversation retention dry run: {conversations} conversations, {candidates} older items, {protected} protected conversations; pruning disabled");
    }
    Ok(())
}

async fn prune(
    state: &Weak<Mutex<AppState>>,
    store: &Store,
    id: &str,
    config: RetentionConfig,
    now: &str,
) -> Result<(), StoreError> {
    loop {
        if state.strong_count() == 0 {
            return Ok(());
        }
        let pruning = store.clone();
        let conversation_id = id.to_owned();
        let timestamp = now.to_owned();
        let batch = off_the_workers(move || {
            pruning.prune_conversation_batch(&conversation_id, config.policy(), &timestamp)
        })
        .await?;
        if batch.items_deleted > 0 {
            if let (Some(boundary), Some(state)) = (batch.boundary, state.upgrade()) {
                let conversation_id = id.to_owned();
                off_the_workers(move || {
                    state.lock().unwrap().publish_retention_boundary(
                        &conversation_id,
                        boundary,
                        batch.items_deleted,
                    )
                })
                .await;
            }
        }
        if !batch.has_more {
            return Ok(());
        }
        tokio::task::yield_now().await;
    }
}

async fn attachment_cycle(state: &Weak<Mutex<AppState>>, store: &Store) -> Result<(), StoreError> {
    let Some(state) = state.upgrade() else {
        return Ok(());
    };
    let homes = off_the_workers(move || state.lock().unwrap().retention_attachment_homes()).await;
    let sweeping = store.clone();
    let cutoff = SystemTime::now()
        .checked_sub(ATTACHMENT_GRACE)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let removed =
        off_the_workers(move || sweeping.sweep_unreferenced_attachments(&homes, cutoff)).await?;
    if removed > 0 {
        eprintln!("attachment retention: removed {removed} unreferenced files");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::app::AppState;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn config_is_preserved_by_settings_persistence_and_invalid_config_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let policy = json!({"enabled":true,"min_messages":10,"max_messages":25,"window_days":3});
        std::fs::write(&path, json!({"retention":policy}).to_string()).unwrap();
        let app = AppState::new_unrooted(dir.path(), "main", true, "unused")
            .with_config(&path)
            .unwrap();
        assert!(app.retention.enabled);
        let persisted = app.config_value(&app.projects_dir, app.default_harness, app.isolation);
        assert_eq!(persisted["retention"], policy);
        std::fs::write(
            &path,
            json!({"retention":{"enabled":true,"max_messages":0}}).to_string(),
        )
        .unwrap();
        assert!(AppState::new_unrooted(dir.path(), "main", true, "unused")
            .with_config(path)
            .is_err());
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
        let registration =
            tokio::spawn(AppState::spawn_retention(state, Duration::from_secs(3600)));
        tokio::time::sleep(Duration::from_millis(30)).await;
        release.send(()).unwrap();
        assert!(holder.join().unwrap(), "startup blocked the sole worker");
        assert!(registration.await.unwrap().is_none());
    }
    #[tokio::test]
    async fn disabled_history_still_sweeps_on_startup_and_periodically() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::Store::new(dir.path()).unwrap();
        let home = store.attachments_dir();
        std::fs::create_dir_all(&home).unwrap();
        let old_file = |name: &str| {
            let path = home.join(name);
            let file = std::fs::File::create(&path).unwrap();
            file.set_times(
                std::fs::FileTimes::new().set_modified(std::time::SystemTime::UNIX_EPOCH),
            )
            .unwrap();
            path
        };
        let mut app = AppState::new_unrooted(dir.path(), "main", true, "unused");
        app.store = Some(store);
        app.retention.enabled = false;
        let state = app.shared();
        let startup = old_file("startup.png");
        let service = AppState::spawn_retention(state.clone(), Duration::from_millis(30))
            .await
            .unwrap();
        async fn wait_for_sweep(path: &std::path::Path) {
            tokio::time::timeout(Duration::from_secs(3), async {
                while path.exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
        }
        wait_for_sweep(&startup).await;
        let periodic = old_file("periodic.png");
        wait_for_sweep(&periodic).await;
        drop(service);
    }
}
