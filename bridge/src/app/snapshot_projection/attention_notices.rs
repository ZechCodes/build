//! A deferred attention note remains owed until its exact candidate is accepted.
use super::*;

impl AppState {
    /// Retain the stamp's dependency closure, including independently subscribed
    /// aliases. A later rebind may add a new closure but cannot erase this one.
    pub(in crate::app) fn mark_deferred_attention_notice(&self, owner: &str) {
        if !self.snapshot_projection_initialized() {
            return;
        }
        self.mark_observation_owner_changed(owner);
        for affected in self.snapshot_affected_owner_ids(owner) {
            self.mark_snapshot_owner_pending(&affected);
            self.snapshot_observations
                .deferred_attention_notices
                .mark(affected);
        }
    }

    /// Fast candidates cover only the rows actually refreshed. A complete
    /// candidate also covers owed owners that have permanently disappeared.
    pub(super) fn prepare_attention_notice_receipts(
        &self,
        delta: SnapshotDelta,
        covered: Option<&BTreeSet<String>>,
    ) -> SnapshotDelta {
        let receipts = self
            .snapshot_observations
            .deferred_attention_notices
            .snapshot()
            .into_iter()
            .filter(|(owner, _)| covered.is_none_or(|owners| owners.contains(owner)))
            .collect();
        delta.set_attention_notice_receipts(receipts)
    }

    /// Early notes retain their existing order. This second note guarantees a
    /// fresh payload when an off-lock flusher consumed them before publication.
    pub(super) fn publish_attention_notice_receipts(&self, receipts: &[(String, u64)]) {
        if receipts.is_empty() {
            return;
        }
        self.snapshot_observations
            .deferred_attention_notices
            .acknowledge(receipts);
        for (owner, _) in receipts {
            self.changes.note_entity(owner);
        }
    }
}
