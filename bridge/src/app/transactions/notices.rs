use crate::app::board::attention::AgentNews;
use crate::app::read_view::SnapshotDelta;
use crate::app::AppState;

/// A successful inner save can precede its outer owner's settlement. Retain
/// only the event descriptors until all related owners can publish together.
pub(super) struct MutationNotice {
    pub owner: String,
    pub news: Vec<AgentNews>,
    pub attention_dirty: bool,
}

impl AppState {
    pub(super) fn settle_committed_mutation(
        &mut self,
        owner: String,
        news: Vec<AgentNews>,
        attention_dirty: bool,
        projection: Option<SnapshotDelta>,
    ) {
        self.committed_entities.queue_notice(MutationNotice {
            owner,
            news,
            attention_dirty,
        });
        if self.committed_entities.has_borrowed(self) {
            return;
        }
        if let Some(projection) = projection {
            self.publish_prepared_snapshot_projection(projection);
        } else {
            self.flush_pending_snapshot_projection();
        }
        self.flush_committed_mutation_notices();
    }

    pub(in crate::app) fn settle_failed_mutation(&mut self, owner: &str) {
        self.publish_read_owner(owner, false);
        if self.committed_entities.has_borrowed(self) || !self.committed_entities.has_notices() {
            return;
        }
        // The failed owner has already been restored. This pure projection
        // promotes earlier successful saves without publishing its candidate.
        self.flush_pending_snapshot_projection();
        self.flush_committed_mutation_notices();
    }

    fn flush_committed_mutation_notices(&mut self) {
        let notices = self.committed_entities.take_notices();
        // Attention persistence prunes absent owners. All borrowed owners must
        // be back before writing, while their in-memory anchors were committed
        // before preparing the combined snapshot.
        if notices.iter().any(|notice| notice.attention_dirty) {
            self.persist_attention();
        }
        for MutationNotice { owner, news, .. } in notices {
            self.publish_read_owner(&owner, true);
            self.push_agent_news(&owner, news);
            if !self.reap_orphaned_terminals().is_empty() {
                // A waiting screen or an agent without a recorded lineage can
                // retire without a nested canonical mutation. Publish that
                // actual runtime observation before notifying readers.
                self.publish_all_snapshot_projection();
            }
            self.changes.note_entity(&owner);
            self.publish_live_roster_for(&owner);
        }
    }
}
