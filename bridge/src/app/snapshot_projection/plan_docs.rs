//! Canonical document availability shares the maintenance observation lifecycle.
use super::*;
use crate::app::read_view::ReadViewSlot;
use crate::store::Store;

pub(super) struct PlanDocObservation {
    version: u64,
    available: bool,
}

impl PlanDocObservation {
    /// Allocate before the physical probe, so later successful ingestion fences
    /// an older negative observation even if that batch installs last.
    pub(super) fn load(view: &ReadViewSlot, store: Option<&Store>, owner: &str) -> Self {
        let version = view.next_version();
        let available = store.is_some_and(|store| store.has_plan_docs(owner));
        Self { version, available }
    }
}

impl AppState {
    pub(in crate::app) fn snapshot_plan_docs_available(&self, owner: &str) -> bool {
        if !self.snapshot_projection_initialized() {
            // Startup callers own AppState before shared admission is enabled.
            return self
                .store
                .as_ref()
                .is_some_and(|store| store.has_plan_docs(owner));
        }
        self.snapshot_observations
            .plan_docs
            .get(owner)
            .is_some_and(|observed| observed.available)
    }

    pub(super) fn capture_plan_doc_owners(&mut self) -> Vec<String> {
        // Missing from the live map can mean borrowed by an in-flight producer.
        // Keep those accepted observations until that producer settles.
        if self.committed_entities.has_borrowed(self) {
            return Vec::new();
        }
        let retired = self
            .snapshot_observations
            .plan_docs
            .keys()
            .filter(|owner| !self.plans.contains_key(*owner))
            .cloned()
            .collect::<Vec<_>>();
        for owner in retired {
            self.snapshot_observations.plan_docs.remove(&owner);
            self.mark_snapshot_owner_pending(&owner);
        }
        self.plans.keys().cloned().collect()
    }

    pub(super) fn install_plan_doc_observation(
        &mut self,
        owner: String,
        observed: PlanDocObservation,
    ) {
        if !self.plans.contains_key(&owner) {
            return;
        }
        self.commit_plan_doc_observation(owner, observed);
    }

    fn commit_plan_doc_observation(&mut self, owner: String, observed: PlanDocObservation) {
        let previous = self.snapshot_observations.plan_docs.get(&owner);
        if previous.is_some_and(|previous| previous.version >= observed.version) {
            return;
        }
        if previous.is_none_or(|previous| previous.available != observed.available) {
            self.mark_snapshot_owner_pending(&owner);
        }
        self.snapshot_observations.plan_docs.insert(owner, observed);
    }

    /// A successful ingestion already proves at least one canonical file was
    /// persisted. Carry that fact into publication without probing the tree.
    pub(in crate::app) fn record_ingested_plan_docs(&mut self, owner: &str) {
        let observed = PlanDocObservation {
            version: self.read_view.next_version(),
            available: true,
        };
        self.commit_plan_doc_observation(owner.to_owned(), observed);
    }

    pub(super) fn refresh_owner_plan_docs(&mut self, owner: &str) {
        if self.snapshot_observations.plan_docs.contains_key(owner)
            && !self.plans.contains_key(owner)
            && !self.committed_entities.has_borrowed(self)
        {
            self.snapshot_observations.plan_docs.remove(owner);
        }
    }
}
