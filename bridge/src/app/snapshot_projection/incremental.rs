//! Accepted-root fragments. Candidate preparation never mutates the old cache.
use super::*;
use crate::app::read_view::SharedJson;

mod cache;
mod layout;
mod refresh;
mod rows;

pub(in crate::app) use cache::GlobalProjection;

impl AppState {
    pub(in crate::app) fn prepare_incremental_owner_snapshot(
        &self,
        delta: SnapshotDelta,
        previous: &ReadView,
        requested: &BTreeSet<String>,
    ) -> Option<SnapshotDelta> {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.global");
        let (cache, owners) = previous.global.as_ref()?.refresh(self, requested)?;
        let mut delta = cache.publish(self, delta, previous);
        // Refresh requested and incidental clock/alias/root dependencies in
        // one pass. The owner publisher only adds named agent rows and history.
        delta = self.prepare_workspace_snapshot_details_for_owners(delta, previous, &owners);
        let facts = self.push_facts_incremental_snapshot(cache.lists(), &owners);
        let delta = self.prepare_observation_inputs(delta, previous);
        let delta = delta.set_facts(Arc::new(facts)).set_global(Arc::new(cache));
        Some(self.prepare_attention_notice_receipts(delta, Some(&owners)))
    }

    pub(in crate::app) fn prepare_incremental_task_snapshot(
        &self,
        delta: SnapshotDelta,
        previous: &ReadView,
    ) -> Option<SnapshotDelta> {
        self.prepare_incremental_owner_snapshot(delta, previous, &BTreeSet::new())
    }

    pub(in crate::app) fn prepare_full_global_snapshot(
        &self,
        delta: SnapshotDelta,
        previous: &ReadView,
    ) -> SnapshotDelta {
        let board = self.board_snapshot();
        let projects = self.snapshot_project_list();
        let (workspaces, reusable) = match self.workspace_list_snapshot(&json!({})) {
            Ok(value) => (value, true),
            Err(error) => {
                eprintln!("workspace snapshot: {error}");
                (
                    previous
                        .get_owned("workspace.list", "")
                        .expect("workspace observations warmed before publication"),
                    false,
                )
            }
        };
        let cache = GlobalProjection::seed(self, board, projects, workspaces, reusable);
        let delta = cache.publish(self, delta, previous);
        let facts = self.push_facts_shared_snapshot(cache.lists());
        let delta = self.prepare_observation_inputs(delta, previous);
        let delta = delta.set_facts(Arc::new(facts)).set_global(Arc::new(cache));
        self.prepare_attention_notice_receipts(delta, None)
    }
}
