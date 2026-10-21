use super::*;
use crate::timing::write_wait::{IntervalCategory, Obligation, WaitGuard, WaitKind, WriteId};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub(in crate::app::snapshot_projection) struct PublicationDebts {
    next: AtomicU64,
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    committed: BTreeMap<String, u64>,
    stages: BTreeMap<String, Vec<Staged>>,
    pending: BTreeMap<PublicationReceipt, Debt>,
    proofs: BTreeMap<String, BTreeMap<WriteId, Vec<Proof>>>,
}
struct Staged {
    revision: u64,
    origin: Option<WriteContext>,
}
struct Debt {
    context: WriteContext,
    token: Obligation,
    queued: Option<WaitGuard>,
    accepted: bool,
    metadata: bool,
    deferred: bool,
    processing: usize,
}
struct Proof {
    context: WriteContext,
    receipt: PublicationReceipt,
    complete: bool,
}

impl PublicationDebts {
    fn next_revision(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed) + 1
    }
    pub(super) fn stage(self: &Arc<Self>, owner: &str) -> PublicationStage {
        let revision = self.next_revision();
        let origin = intent_context();
        self.state
            .lock()
            .unwrap()
            .stages
            .entry(owner.into())
            .or_default()
            .push(Staged { revision, origin });
        let work = self.work_owners(&[owner.to_owned()].into_iter().collect());
        PublicationStage {
            ledger: Arc::clone(self),
            owner: owner.into(),
            revision,
            work: Some(work),
        }
    }
    pub(super) fn end_stage(&self, owner: &str, revision: u64) {
        let mut state = self.state.lock().unwrap();
        let stages = state.stages.get_mut(owner).unwrap();
        assert_eq!(stages.pop().unwrap().revision, revision);
        if stages.is_empty() {
            state.stages.remove(owner);
        }
        // The sequence allocator is never rolled back or recycled.
    }
    pub(super) fn commit(&self, owner: &str) -> PublicationReceipt {
        let mut state = self.state.lock().unwrap();
        let (revision, origin) = state
            .stages
            .get(owner)
            .and_then(|stages| stages.last())
            .map(|stage| (stage.revision, stage.origin.clone()))
            .unwrap_or_else(|| (self.next_revision(), intent_context()));
        let receipt = PublicationReceipt {
            owner: owner.into(),
            revision,
        };
        state.committed.insert(owner.into(), revision);
        if let Some(origin) = origin {
            let token = origin.child();
            let context = token.context();
            for id in context.origins() {
                let proofs = state
                    .proofs
                    .entry(owner.into())
                    .or_default()
                    .entry(*id)
                    .or_default();
                proofs.retain(|proof| !proof.context.same_registry(&context));
                proofs.push(Proof {
                    context: context.clone(),
                    receipt: receipt.clone(),
                    complete: false,
                });
            }
            state.pending.insert(
                receipt.clone(),
                Debt {
                    context,
                    token,
                    queued: None,
                    accepted: false,
                    metadata: false,
                    deferred: false,
                    processing: 0,
                },
            );
        }
        receipt
    }
    pub(super) fn coverage(&self, owners: &BTreeSet<String>) -> Vec<PublicationReceipt> {
        let state = self.state.lock().unwrap();
        owners
            .iter()
            .filter_map(|owner| {
                let revision = state
                    .stages
                    .get(owner)
                    .and_then(|stages| stages.last())
                    .map(|stage| stage.revision)
                    .or_else(|| state.committed.get(owner).copied())?;
                Some(PublicationReceipt {
                    owner: owner.clone(),
                    revision,
                })
            })
            .collect()
    }
    fn covered(receipt: &PublicationReceipt, coverage: &[PublicationReceipt]) -> bool {
        coverage
            .iter()
            .any(|covered| covered.owner == receipt.owner && covered.revision >= receipt.revision)
    }
    pub(super) fn accept(&self, coverage: &[PublicationReceipt]) {
        self.change(|receipt, debt| {
            if Self::covered(receipt, coverage) {
                debt.accepted = true;
            }
        });
    }
    pub(super) fn metadata(&self, receipt: &PublicationReceipt) {
        self.change(|key, debt| {
            if key == receipt {
                debt.metadata = true;
            }
        });
    }
    fn change(&self, mut change: impl FnMut(&PublicationReceipt, &mut Debt)) {
        let completed = {
            let mut state = self.state.lock().unwrap();
            for (receipt, debt) in &mut state.pending {
                change(receipt, debt);
            }
            let done: Vec<_> = state
                .pending
                .iter()
                .filter(|(_, debt)| debt.accepted && debt.metadata)
                .map(|(receipt, _)| receipt.clone())
                .collect();
            let mut completed = Vec::new();
            for receipt in done {
                let debt = state.pending.remove(&receipt).unwrap();
                state.finish_proof(&receipt, &debt.context);
                completed.push(debt);
            }
            completed
        };
        for debt in completed {
            drop(debt.queued);
            debt.token.complete();
        }
    }
    pub(super) fn proof(&self, context: &WriteContext, owner: &str) -> bool {
        let state = self.state.lock().unwrap();
        let Some(proofs) = state.proofs.get(owner) else {
            return false;
        };
        context.origins().iter().all(|id| {
            proofs.get(id).is_some_and(|proofs| {
                proofs
                    .iter()
                    .find(|proof| proof.context.same_registry(context))
                    .is_some_and(|proof| proof.complete)
            })
        })
    }
    pub(super) fn postpone(&self, coverage: &[PublicationReceipt]) {
        let mut state = self.state.lock().unwrap();
        for (receipt, debt) in &mut state.pending {
            if Self::covered(receipt, coverage) {
                debt.deferred = true;
                Self::queue(debt);
            }
        }
    }
    fn queue(debt: &mut Debt) {
        if debt.deferred && debt.processing == 0 && debt.queued.is_none() {
            debt.queued = Some(
                debt.context
                    .interval(IntervalCategory::KnownWait(WaitKind::Publication)),
            );
        }
    }
    pub(super) fn work_owners(self: &Arc<Self>, owners: &BTreeSet<String>) -> PublicationWork {
        self.work(|receipt| owners.contains(&receipt.owner))
    }
    pub(super) fn work_coverage(
        self: &Arc<Self>,
        coverage: &[PublicationReceipt],
    ) -> PublicationWork {
        self.work(|receipt| Self::covered(receipt, coverage))
    }
    pub(super) fn work_receipt(
        self: &Arc<Self>,
        receipt: Option<&PublicationReceipt>,
    ) -> PublicationWork {
        self.work(|key| receipt.is_some_and(|receipt| key == receipt))
    }
    fn work(self: &Arc<Self>, selected: impl Fn(&PublicationReceipt) -> bool) -> PublicationWork {
        let (receipts, contexts, waits) = {
            let mut state = self.state.lock().unwrap();
            let mut receipts = Vec::new();
            let mut contexts = Vec::new();
            let mut waits = Vec::new();
            for (receipt, debt) in &mut state.pending {
                if selected(receipt) {
                    debt.processing += 1;
                    receipts.push(receipt.clone());
                    contexts.push(debt.context.clone());
                    waits.extend(debt.queued.take());
                }
            }
            (receipts, contexts, waits)
        };
        drop(waits); // Close the actual queue before publisher CPU work starts.
        PublicationWork::bind(Arc::clone(self), receipts, contexts)
    }
    pub(super) fn end_work(&self, receipts: &[PublicationReceipt]) {
        let mut state = self.state.lock().unwrap();
        for receipt in receipts {
            if let Some(debt) = state.pending.get_mut(receipt) {
                debt.processing -= 1;
                Self::queue(debt);
            }
        }
    }
}

impl State {
    fn finish_proof(&mut self, receipt: &PublicationReceipt, context: &WriteContext) {
        let Some(by_origin) = self.proofs.get_mut(&receipt.owner) else {
            return;
        };
        for id in context.origins() {
            for proof in by_origin.get_mut(id).into_iter().flatten() {
                if proof.receipt == *receipt && proof.context.same_registry(context) {
                    proof.complete = true;
                }
            }
        }
    }
}
