//! Store reads, transactions, notifications, and boot recovery stay in adapters.

use std::collections::HashMap;

use crate::operation::{OperationReceipt, OperationStatus};

pub(in crate::app) struct PendingOperationAcceptance {
    pub(in crate::app) conversation_owner_id: String,
    pub(in crate::app) receipt: OperationReceipt,
}

#[derive(Default)]
pub(in crate::app) struct OperationLedger {
    receipts: HashMap<String, OperationReceipt>,
    pending_acceptance: Option<PendingOperationAcceptance>,
}

impl OperationLedger {
    /// Preserve the current single slot: staging replaces its prior value.
    pub(in crate::app) fn stage_acceptance(
        &mut self,
        conversation_owner_id: String,
        receipt: OperationReceipt,
    ) {
        self.pending_acceptance = Some(PendingOperationAcceptance {
            conversation_owner_id,
            receipt,
        });
    }

    /// Store-backed success and failure both leave a matching value consumed.
    pub(in crate::app) fn consume_acceptance_for(
        &mut self,
        owner_id: &str,
    ) -> Option<PendingOperationAcceptance> {
        if self
            .pending_acceptance
            .as_ref()
            .is_some_and(|pending| pending.conversation_owner_id == owner_id)
        {
            self.pending_acceptance.take()
        } else {
            None
        }
    }

    /// The only path that hydrates this process-local mirror.
    pub(in crate::app) fn remember_in_memory_acceptance(&mut self, owner_id: &str) {
        if let Some(acceptance) = self.consume_acceptance_for(owner_id) {
            self.receipts
                .insert(acceptance.receipt.operation_id.clone(), acceptance.receipt);
        }
    }

    pub(in crate::app) fn cached(&self, operation_id: &str) -> Option<&OperationReceipt> {
        self.receipts.get(operation_id)
    }

    /// The no-Store twin of `Store::take_operation_requesters`: every answer
    /// this conversation owes an agent, oldest first, cleared as it is read so
    /// one terminal message settles it once.
    pub(in crate::app) fn take_requesters(
        &mut self,
        conversation_id: &str,
    ) -> Vec<crate::operation::OperationRequester> {
        let mut owed: Vec<(u64, crate::operation::OperationRequester)> = self
            .receipts
            .values_mut()
            .filter(|receipt| receipt.conversation_id == conversation_id)
            .filter_map(|receipt| {
                receipt
                    .requested_by
                    .take()
                    .map(|requester| (receipt.posted_sequence, requester))
            })
            .collect();
        owed.sort_by_key(|(sequence, _)| *sequence);
        owed.into_iter().map(|(_, requester)| requester).collect()
    }

    /// No-Store compare-and-set decision. Never use this as a second gate after
    /// an authoritative Store transition has succeeded.
    pub(in crate::app) fn cached_has_status(
        &self,
        operation_id: &str,
        expected: OperationStatus,
    ) -> bool {
        self.receipts
            .get(operation_id)
            .is_some_and(|receipt| receipt.status == expected)
    }

    /// Record an already-authorized transition without checking cached status.
    ///
    /// The Store-success adapter calls this unconditionally after Store returns
    /// true. The no-Store adapter first calls `cached_has_status`. Absence is
    /// silently ignored exactly as the current mirror update is.
    pub(in crate::app) fn record_transition_if_cached(
        &mut self,
        operation_id: &str,
        next: OperationStatus,
        execution_error: Option<&str>,
    ) -> Option<String> {
        let receipt = self.receipts.get_mut(operation_id)?;
        receipt.status = next;
        receipt.execution_error = execution_error.map(str::to_string);
        Some(receipt.entity_id.clone())
    }
}

#[cfg(test)]
mod tests;
