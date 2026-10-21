use super::*;

impl PublicationDebts {
    pub(in crate::app::snapshot_projection::publication_debt) fn record_borrow(
        &self,
        owner: &str,
        origin: Option<WriteContext>,
        blockers: &BTreeSet<String>,
    ) {
        let mut state = self.state.lock().unwrap();
        state.borrowed.insert(owner.to_owned(), origin);
        state.refresh_blockers(blockers);
    }
    pub(in crate::app::snapshot_projection::publication_debt) fn refresh_blockers(
        &self,
        blockers: &BTreeSet<String>,
    ) {
        self.state.lock().unwrap().refresh_blockers(blockers);
    }
    pub(in crate::app::snapshot_projection::publication_debt) fn postpone(
        &self,
        coverage: &[PublicationReceipt],
        blockers: &BTreeSet<String>,
    ) {
        let mut state = self.state.lock().unwrap();
        state.refresh_blockers(blockers);
        for (receipt, debt) in &mut state.pending {
            if Self::covered(receipt, coverage) {
                debt.deferred = true;
                debt.queue();
            }
        }
    }
}
impl State {
    fn refresh_blockers(&mut self, blockers: &BTreeSet<String>) {
        // A restored owner cannot lend its earlier cause to a later raw absence.
        self.borrowed.retain(|owner, _| blockers.contains(owner));
        for debt in self.pending.values_mut() {
            let own: Vec<_> = debt
                .context
                .origins()
                .iter()
                .copied()
                .filter(|id| {
                    !blockers.is_empty()
                        && blockers.iter().all(|owner| {
                            self.borrowed
                                .get(owner)
                                .and_then(Option::as_ref)
                                .is_some_and(|cause| {
                                    debt.context.same_registry(cause)
                                        && cause.origins().contains(id)
                                })
                        })
                })
                .collect();
            if own != debt.own_queue_origins {
                // Close under the old cause before opening the new partition.
                debt.queued.clear();
                debt.own_queue_origins = own;
                debt.queue();
            }
        }
    }
}
impl Debt {
    pub(super) fn queue(&mut self) {
        if !self.deferred || self.processing != 0 || !self.queued.is_empty() {
            return;
        }
        let (own, other): (Vec<_>, Vec<_>) = self
            .context
            .origins()
            .iter()
            .copied()
            .partition(|id| self.own_queue_origins.contains(id));
        self.queue_for(&own, IntervalCategory::OwnExternal);
        self.queue_for(&other, IntervalCategory::KnownWait(WaitKind::Publication));
    }
    fn queue_for(&mut self, origins: &[WriteId], category: IntervalCategory) {
        if !origins.is_empty() {
            self.queued
                .push(self.context.interval_for(origins, category));
        }
    }
}

impl PublicationDebts {
    pub(in crate::app::snapshot_projection::publication_debt) fn queue_own_origins(
        &self,
        context: &WriteContext,
        blockers: &BTreeSet<String>,
    ) -> Vec<WriteId> {
        let state = self.state.lock().unwrap();
        context
            .origins()
            .iter()
            .copied()
            .filter(|id| {
                !blockers.is_empty()
                    && blockers.iter().all(|owner| {
                        state
                            .borrowed
                            .get(owner)
                            .and_then(Option::as_ref)
                            .is_some_and(|cause| {
                                context.same_registry(cause) && cause.origins().contains(id)
                            })
                    })
            })
            .collect()
    }
}
