//! Test-only required owner-publication receipts. Accounting is scaffolded
//! until the real borrowed/stale/metadata-tail controls have failed.
use super::*;
use crate::timing::write_wait::WriteContext;
use std::cell::RefCell;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct PublicationReceipt {
    pub owner: String,
    pub revision: u64,
}

#[derive(Default)]
pub(super) struct PublicationDebts;
pub(in crate::app) struct PublicationStage;
impl Drop for PublicationStage {
    fn drop(&mut self) {}
}

impl AppState {
    pub(in crate::app) fn begin_publication_stage(&self, _owner: &str) -> PublicationStage {
        let _debts = &self.snapshot_observations.publication_debts;
        PublicationStage
    }
    pub(in crate::app) fn commit_publication_debt(
        &self,
        _owner: &str,
    ) -> Option<PublicationReceipt> {
        None
    }
    pub(super) fn capture_publication_coverage(
        &self,
        delta: SnapshotDelta,
        _covered: Option<&BTreeSet<String>>,
    ) -> SnapshotDelta {
        delta.set_publication_coverage(vec![])
    }
    pub(super) fn accept_publication_coverage(&self, _receipts: &[PublicationReceipt]) {}
    pub(in crate::app) fn finish_publication_metadata(
        &self,
        _receipt: Option<&PublicationReceipt>,
        owner: &str,
    ) {
        METADATA_TAIL.with(|observer| {
            if let Some(callback) = observer.borrow_mut().as_mut() {
                callback(self, owner);
            }
        });
    }
    pub(in crate::app) fn origin_publication_complete(
        &self,
        _context: &WriteContext,
        _owner: &str,
    ) -> bool {
        false
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
