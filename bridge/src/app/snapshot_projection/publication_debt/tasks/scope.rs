use super::*;
use crate::timing::write_wait::{IntervalCategory, WaitKind};

/// An actual publisher scope retains live work independently of settlement.
pub(in crate::app) struct TaskPublicationWork {
    ledger: Arc<TaskPublications>,
    tickets: Vec<u64>,
    binding: Option<PublicationWork>,
}
impl TaskPublications {
    pub(super) fn work(
        self: &Arc<Self>,
        owners: Arc<PublicationDebts>,
        ticket: Option<&TaskPublicationTicket>,
        project: Option<&str>,
        coverage: Option<u64>,
    ) -> TaskPublicationWork {
        let (tickets, leases, waits) = {
            let mut state = self.state.lock().unwrap();
            let mut tickets = Vec::new();
            let mut leases = Vec::new();
            let mut waits = Vec::new();
            for (id, pending) in &mut state.pending {
                let selected = ticket.is_some_and(|ticket| ticket.0 == *id)
                    || project.is_some_and(|project| project == pending.project)
                    || pending
                        .target
                        .zip(coverage)
                        .is_some_and(|(target, covered)| target <= covered);
                if selected {
                    pending.processing += 1;
                    tickets.push(*id);
                    for required in &mut pending.required {
                        // Ownership survives release of the ledger lock and any
                        // concurrent accepted-tail completion of the principal.
                        leases.push(required.token.child());
                        waits.append(&mut required.queued);
                    }
                }
            }
            (tickets, leases, waits)
        };
        drop(waits);
        let contexts = leases.iter().map(Obligation::context).collect();
        let binding = PublicationWork::bind(owners, Vec::new(), contexts);
        for lease in leases {
            lease.complete();
        }
        TaskPublicationWork {
            ledger: Arc::clone(self),
            tickets,
            binding: Some(binding),
        }
    }
    pub(in crate::app::snapshot_projection::publication_debt) fn refresh_blockers(
        &self,
        owners: &PublicationDebts,
        blockers: &BTreeSet<String>,
    ) {
        let mut state = self.state.lock().unwrap();
        for pending in state.pending.values_mut() {
            for required in &mut pending.required {
                let own = owners.queue_own_origins(&required.token.context(), blockers);
                if own != required.own_queue_origins {
                    required.queued.clear();
                    required.own_queue_origins = own;
                    if pending.metadata && pending.processing == 0 {
                        required.queue();
                    }
                }
            }
        }
    }
    fn end_work(&self, tickets: &[u64]) {
        let mut state = self.state.lock().unwrap();
        for ticket in tickets {
            if let Some(pending) = state.pending.get_mut(ticket) {
                pending.processing -= 1;
                if pending.processing == 0 && pending.metadata {
                    for required in &mut pending.required {
                        required.queue();
                    }
                }
            }
        }
    }
}
impl Required {
    fn queue(&mut self) {
        if !self.queued.is_empty() {
            return;
        }
        let context = self.token.context();
        let (own, other): (Vec<_>, Vec<_>) = context
            .origins()
            .iter()
            .copied()
            .partition(|id| self.own_queue_origins.contains(id));
        for (origins, category) in [
            (own, IntervalCategory::OwnExternal),
            (other, IntervalCategory::KnownWait(WaitKind::Publication)),
        ] {
            if !origins.is_empty() {
                self.queued.push(context.interval_for(&origins, category));
            }
        }
    }
}
impl Drop for TaskPublicationWork {
    fn drop(&mut self) {
        let binding = self.binding.take().unwrap();
        if std::thread::panicking() {
            drop(binding);
        } else {
            // An ordinary early return from candidate preparation is not an
            // abandoned publisher. Pending principal debt remains independently.
            binding.complete();
        }
        self.ledger.end_work(&self.tickets);
    }
}
