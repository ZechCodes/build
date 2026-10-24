use super::AppState;
use crate::store::RetentionBoundary;

impl AppState {
    /// Publish only after the off-lock pruning transaction has committed.
    pub(in crate::app) fn publish_retention_boundary(
        &mut self,
        conversation_id: &str,
        boundary: RetentionBoundary,
        deleted: usize,
    ) {
        let Some(owner) = self.entity_of_agent(conversation_id) else {
            return;
        };
        let roster = if let Some(plan) = self.plans.get_mut(&owner) {
            &mut plan.agents
        } else if let Some(run) = self.runs.get_mut(&owner) {
            &mut run.agents
        } else {
            return;
        };
        let Some(agent) = roster.by_id_mut(conversation_id) else {
            return;
        };
        // A reset may have committed between pruning and this publication.
        if agent.thread.id != boundary.thread_id {
            return;
        }
        agent.thread.apply_retention_boundary(
            boundary.history_start_sequence,
            boundary.high_watermark,
            deleted,
        );
        self.publish_owner_snapshot_projection(&owner);
        self.changes.note_entity(&owner);
    }
}
