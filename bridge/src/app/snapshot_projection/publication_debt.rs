//! Test-only revisioned required owner publication, independent of frame ACK.
use super::*;
use crate::timing::write_wait::WriteContext;
use std::cell::RefCell;
mod ledger;
mod scope;
pub(super) use ledger::PublicationDebts;
use scope::intent_context;
pub(in crate::app) use scope::{PublicationStage, PublicationWork};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::app) struct PublicationReceipt {
    pub owner: String,
    pub revision: u64,
}

impl AppState {
    /// Capture the semantic author before joining resource-only publication debt.
    pub(in crate::app) fn mutation_intent_context(&self) -> Option<WriteContext> {
        intent_context()
    }
    fn publication_blockers(&self) -> BTreeSet<String> {
        self.committed_entities.borrowed_owner_ids(self)
    }
    pub(in crate::app) fn record_publication_borrow(&self, owner: &str) {
        self.snapshot_observations.publication_debts.record_borrow(
            owner,
            intent_context(),
            &self.publication_blockers(),
        );
    }
    fn refresh_publication_barrier(&self) {
        self.snapshot_observations
            .publication_debts
            .refresh_blockers(&self.publication_blockers());
    }
    pub(in crate::app) fn begin_publication_stage(&self, owner: &str) -> PublicationStage {
        self.refresh_publication_barrier();
        self.snapshot_observations.publication_debts.stage(owner)
    }
    pub(in crate::app) fn commit_publication_debt(
        &self,
        owner: &str,
    ) -> Option<PublicationReceipt> {
        self.snapshot_projection_initialized()
            .then(|| self.snapshot_observations.publication_debts.commit(owner))
    }
    pub(super) fn capture_publication_coverage(
        &self,
        delta: SnapshotDelta,
        covered: Option<&BTreeSet<String>>,
    ) -> SnapshotDelta {
        delta.set_publication_coverage(
            covered
                .map(|owners| {
                    self.snapshot_observations
                        .publication_debts
                        .coverage(owners)
                })
                .unwrap_or_default(),
        )
    }
    pub(super) fn accept_publication_coverage(&self, receipts: &[PublicationReceipt]) {
        self.snapshot_observations
            .publication_debts
            .accept(receipts);
    }
    pub(in crate::app) fn finish_publication_metadata(
        &self,
        receipt: Option<&PublicationReceipt>,
        owner: &str,
    ) {
        METADATA_TAIL.with(|observer| {
            if let Some(callback) = observer.borrow_mut().as_mut() {
                callback(self, owner);
            }
        });
        if let Some(receipt) = receipt {
            self.snapshot_observations
                .publication_debts
                .metadata(receipt);
        }
    }
    pub(in crate::app) fn origin_publication_complete(
        &self,
        context: &WriteContext,
        owner: &str,
    ) -> bool {
        self.snapshot_observations
            .publication_debts
            .proof(context, owner)
    }
    pub(in crate::app) fn postpone_publication_debt(&self, receipt: Option<&PublicationReceipt>) {
        if let Some(receipt) = receipt {
            self.snapshot_observations
                .publication_debts
                .postpone(std::slice::from_ref(receipt), &self.publication_blockers());
        }
    }
    pub(in crate::app) fn postpone_publication_coverage(&self, receipts: &[PublicationReceipt]) {
        self.snapshot_observations
            .publication_debts
            .postpone(receipts, &self.publication_blockers());
    }
    pub(in crate::app) fn begin_publication_coverage_work(
        &self,
        receipts: &[PublicationReceipt],
    ) -> PublicationWork {
        self.refresh_publication_barrier();
        self.snapshot_observations
            .publication_debts
            .work_coverage(receipts)
    }
    pub(in crate::app) fn begin_publication_owner_work(
        &self,
        owners: &BTreeSet<String>,
    ) -> PublicationWork {
        self.refresh_publication_barrier();
        self.snapshot_observations
            .publication_debts
            .work_owners(owners)
    }
    pub(in crate::app) fn begin_publication_metadata_work(
        &self,
        receipt: Option<&PublicationReceipt>,
    ) -> PublicationWork {
        self.refresh_publication_barrier();
        self.snapshot_observations
            .publication_debts
            .work_receipt(receipt)
    }
    pub(super) fn begin_pending_publication_work(&self) -> PublicationWork {
        let owners = crate::timing::resource_lock!(self.snapshot_observations.pending_owners)
            .unwrap()
            .clone();
        self.begin_publication_owner_work(&owners)
    }
}

type MetadataCallback = Box<dyn FnMut(&AppState, &str)>;
thread_local! { static METADATA_TAIL: RefCell<Option<MetadataCallback>> = const { RefCell::new(None) }; }
struct MetadataObserver;
impl Drop for MetadataObserver {
    fn drop(&mut self) {
        METADATA_TAIL.with(|observer| {
            observer.borrow_mut().take();
        });
    }
}
pub(in crate::app) fn observe_metadata_tail(
    callback: impl FnMut(&AppState, &str) + 'static,
) -> impl Drop {
    METADATA_TAIL.with(|observer| {
        assert!(
            observer.borrow().is_none(),
            "one metadata-tail observer per thread"
        );
        *observer.borrow_mut() = Some(Box::new(callback));
    });
    MetadataObserver
}

type InstallCallback = Box<dyn FnMut(&AppState)>;
type PreparedCallback = Box<dyn FnMut(&str, &[PublicationReceipt])>;
thread_local! {
    static INSTALL_RECEIPTS: RefCell<Option<InstallCallback>> = const { RefCell::new(None) };
    static PREPARED: RefCell<Option<PreparedCallback>> = const { RefCell::new(None) };
}
struct InstallObserver;
impl Drop for InstallObserver {
    fn drop(&mut self) {
        INSTALL_RECEIPTS.with(|observer| {
            observer.borrow_mut().take();
        });
    }
}
pub(in crate::app) fn observe_install_receipts(
    callback: impl FnMut(&AppState) + 'static,
) -> impl Drop {
    INSTALL_RECEIPTS.with(|observer| {
        assert!(observer.borrow().is_none());
        *observer.borrow_mut() = Some(Box::new(callback));
    });
    InstallObserver
}
pub(super) fn before_install_receipts(app: &AppState) {
    INSTALL_RECEIPTS.with(|observer| {
        if let Some(callback) = observer.borrow_mut().as_mut() {
            callback(app);
        }
    });
}
struct PreparedObserver;
impl Drop for PreparedObserver {
    fn drop(&mut self) {
        PREPARED.with(|observer| {
            observer.borrow_mut().take();
        });
    }
}
pub(in crate::app) fn observe_prepared(
    callback: impl FnMut(&str, &[PublicationReceipt]) + 'static,
) -> impl Drop {
    PREPARED.with(|observer| {
        assert!(observer.borrow().is_none());
        *observer.borrow_mut() = Some(Box::new(callback));
    });
    PreparedObserver
}
impl AppState {
    pub(in crate::app) fn record_prepared_publication(
        &self,
        owner: &str,
        projection: Option<&SnapshotDelta>,
    ) {
        PREPARED.with(|observer| {
            if let Some(callback) = observer.borrow_mut().as_mut() {
                callback(
                    owner,
                    projection
                        .map(SnapshotDelta::publication_coverage)
                        .unwrap_or(&[]),
                );
            }
        });
    }
}
