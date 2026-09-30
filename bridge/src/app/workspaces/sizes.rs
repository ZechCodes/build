//! The size walker (#273): each workspace's size on disk, measured when the
//! Workspaces tab asks rather than on every sweep.
//!
//! `workspace.measure_sizes` only queues. The walker is a thread of its own,
//! niced below the daemon, that takes one workspace at a time: it reads the
//! workspace's boundary under the app mutex, walks it with the mutex released
//! under the reclaim service's budget, and writes the size and when it was
//! measured onto the workspace's lifecycle record. The workspace list is then
//! noted changed, so every subscribed client is sent the size. A walk that
//! ran out of budget writes nothing, and the last size stands.

use crate::app::AppState;
use crate::changes::BoardLists;
use crate::reclaim::containment::WorkspaceBoundary;
use crate::reclaim::{size_is_fresh, size_within, Budget};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long the walker sleeps with nothing queued before it looks at the
/// stop flag again. A request or the daemon stopping wakes it sooner.
const IDLE_WAIT: Duration = Duration::from_secs(30);

fn now_ms() -> i64 {
    i64::try_from(crate::agent::now_ms()).unwrap_or(i64::MAX)
}

impl AppState {
    /// Queue a size walk for each workspace named, or every one the reclaim
    /// service measures (of `project_id`, when named), whose size is not
    /// recent. Answers the ones queued now: one already queued, being walked
    /// or recently measured is left as it is.
    pub fn workspace_measure_sizes(
        &self,
        workspace_ids: Option<&[String]>,
        project_id: Option<&str>,
    ) -> Vec<String> {
        let now = now_ms();
        let wanted: Vec<String> = self
            .measured_workspaces(project_id)
            .filter(|workspace| workspace_ids.is_none_or(|ids| ids.contains(&workspace.id)))
            .filter(|workspace| !size_is_fresh(self.workspace_lifecycle.get(&workspace.id), now))
            .map(|workspace| workspace.id.clone())
            .collect();
        self.size_requests.request(wanted)
    }

    /// Run the walker for the life of the daemon, on a thread of its own so
    /// no runtime worker waits on the app mutex or the disk.
    pub fn spawn_workspace_sizes(state: Arc<Mutex<AppState>>) {
        let spawned = std::thread::Builder::new()
            .name("bridge-sizes".into())
            .spawn(move || {
                crate::priority::lower_this_thread(crate::priority::CHILD_NICE);
                let (requests, stop) = {
                    let app = state.lock().unwrap();
                    (
                        Arc::clone(&app.size_requests),
                        Arc::clone(&app.reclaim_stop),
                    )
                };
                while !stop.load(Ordering::Relaxed) {
                    if !Self::measure_next_workspace_size(&state, now_ms()) {
                        requests.wait(IDLE_WAIT);
                    }
                }
            });
        if let Err(error) = spawned {
            eprintln!("workspace sizes: the walker did not start: {error}");
        }
    }

    /// Walk the next queued workspace, if there is one, and write what it
    /// measured. `false` when nothing was queued.
    pub(in crate::app) fn measure_next_workspace_size(
        state: &Arc<Mutex<AppState>>,
        now_ms: i64,
    ) -> bool {
        let (requests, workspace_id, walk) = {
            let app = state.lock().unwrap();
            let requests = Arc::clone(&app.size_requests);
            let Some(workspace_id) = requests.next() else {
                return false;
            };
            let walk = app.size_walk(&workspace_id);
            (requests, workspace_id, walk)
        };
        let size = walk.and_then(|(boundary, budget)| size_within(&boundary, &budget()));
        if let Some(size) = size {
            state
                .lock()
                .unwrap()
                .settle_workspace_size(&workspace_id, size, now_ms);
        }
        requests.done(&workspace_id);
        true
    }

    /// What walking one workspace needs from the app state: its boundary, and
    /// a budget made on the walker's own thread. `None` when it is gone.
    fn size_walk(
        &self,
        workspace_id: &str,
    ) -> Option<(WorkspaceBoundary, impl FnOnce() -> Budget)> {
        let workspace = self.workspaces.get(workspace_id)?;
        let boundary = self.workspace_boundary(workspace)?;
        let policy = self.reclaim_policy_now();
        let stop = Arc::clone(&self.reclaim_stop);
        Some((boundary, move || policy.budget(stop)))
    }

    /// Write a measured size onto the workspace's lifecycle record, and send
    /// it to every client. A workspace removed during the walk gets nothing.
    fn settle_workspace_size(&mut self, workspace_id: &str, size: u64, now_ms: i64) {
        if self.workspaces.get(workspace_id).is_none() {
            return;
        }
        let record = self
            .workspace_lifecycle
            .entry(workspace_id.to_string())
            .or_default();
        record.size_bytes = Some(size);
        record.size_measured_at_ms = Some(now_ms);
        self.persist_workspace_lifecycle();
        self.note_board_lists_changed(BoardLists::WORKSPACES);
    }
}
