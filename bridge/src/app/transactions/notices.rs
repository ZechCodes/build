use crate::app::board::attention::AgentNews;
use crate::app::read_view::SnapshotDelta;
use crate::app::AppState;

/// A successful inner save can precede its outer owner's settlement. Retain
/// only the event descriptors until all related owners can publish together.
pub(super) struct MutationNotice {
    pub owner: String,
    pub news: Vec<AgentNews>,
    pub attention_dirty: bool,
    #[cfg(test)]
    pub publication_receipt:
        Option<crate::app::snapshot_projection::publication_debt::PublicationReceipt>,
}

impl AppState {
    pub(super) fn settle_committed_mutation(
        &mut self,
        owner: String,
        news: Vec<AgentNews>,
        attention_dirty: bool,
        projection: Option<SnapshotDelta>,
    ) {
        #[cfg(test)]
        let publication_receipt = self.commit_publication_debt(&owner);
        self.committed_entities.queue_notice(MutationNotice {
            owner,
            news,
            attention_dirty,
            #[cfg(test)]
            publication_receipt: publication_receipt.clone(),
        });
        if self.committed_entities.has_borrowed(self) {
            #[cfg(test)]
            self.postpone_publication_debt(publication_receipt.as_ref());
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
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("finisher.notices");
        let notices = self.committed_entities.take_notices();
        #[cfg(test)]
        let publication_work = self.begin_publication_coverage_work(
            &notices
                .iter()
                .filter_map(|notice| notice.publication_receipt.clone())
                .collect::<Vec<_>>(),
        );
        // Attention persistence prunes absent owners. All borrowed owners must
        // be back before writing, while their in-memory anchors were committed
        // before preparing the combined snapshot.
        if notices.iter().any(|notice| notice.attention_dirty) {
            self.persist_attention();
        }
        for notice in notices {
            #[cfg(test)]
            let receipt = notice.publication_receipt.clone();
            #[cfg(test)]
            let metadata_work = self.begin_publication_metadata_work(receipt.as_ref());
            let MutationNotice { owner, news, .. } = notice;
            self.publish_read_owner(&owner, true);
            self.push_agent_news(&owner, news);
            self.changes.note_entity(&owner);
            self.publish_live_roster_for(&owner);
            #[cfg(test)]
            self.finish_publication_metadata(receipt.as_ref(), &owner);
            #[cfg(test)]
            metadata_work.complete();
        }
        #[cfg(test)]
        publication_work.complete();
    }
}

#[cfg(test)]
impl AppState {
    pub(in crate::app) fn flush_publication_metadata_for_test(&mut self) {
        self.flush_committed_mutation_notices();
    }
}
