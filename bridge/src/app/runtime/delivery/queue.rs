//! This type owns queue order and in-flight accounting only. AppState-dependent
//! preparation and the per-turn SettlingHandle guard remain in the runtime adapter.

use std::collections::HashMap;

use super::types::PendingAgentTurn;
use crate::app::TabKey;
use crate::operation::OperationReceipt;

#[derive(Default)]
pub(in crate::app) struct DeliveryQueue {
    queued: Vec<PendingAgentTurn>,
    owners_in_flight: HashMap<String, usize>,
    agents_in_flight: HashMap<TabKey, usize>,
}

#[derive(Clone, Copy)]
pub(in crate::app) struct DeliveryCheckpoint(usize);

/// An exact pair of counters owed by one turn.
///
/// This deliberately owns no callback or application handle and has no Drop.
/// The runtime adapter must immediately wrap every returned ticket in its
/// per-turn SettlingHandle RAII guard.
pub(in crate::app) struct DeliveryTicket {
    owner: String,
    told_agent: Option<TabKey>,
}

impl DeliveryQueue {
    pub(in crate::app) fn enqueue(&mut self, turn: PendingAgentTurn) {
        self.queued.push(turn);
    }

    pub(in crate::app) fn checkpoint(&self) -> DeliveryCheckpoint {
        DeliveryCheckpoint(self.queued.len())
    }

    pub(in crate::app) fn refuse_since(&mut self, checkpoint: DeliveryCheckpoint) {
        let earlier_len = checkpoint.0.min(self.queued.len());
        let mut appended = self.queued.split_off(earlier_len);
        appended.retain(|turn| turn.survives_refusal);
        self.queued.append(&mut appended);
    }

    /// Partition under the caller's existing AppState guard.
    ///
    /// The caller must prepare all returned turns before calling `start` and
    /// must not release the guard between this call and ticket creation.
    pub(in crate::app) fn take_ready(
        &mut self,
        mut hold: impl FnMut(&PendingAgentTurn) -> bool,
    ) -> Vec<PendingAgentTurn> {
        let agents_in_flight = &self.agents_in_flight;
        let (held, ready) = std::mem::take(&mut self.queued)
            .into_iter()
            .partition(|turn| hold(turn) || agents_in_flight.contains_key(&turn.tab_key()));
        self.queued = held;
        ready
    }

    /// Mint accounting only after AppState-dependent preparation is complete.
    /// The caller must put this ticket into a per-turn RAII guard immediately.
    pub(in crate::app) fn start(&mut self, turn: &PendingAgentTurn) -> DeliveryTicket {
        let ticket = DeliveryTicket {
            owner: turn.owner.clone(),
            told_agent: turn.says_something().then(|| turn.tab_key()),
        };
        *self
            .owners_in_flight
            .entry(ticket.owner.clone())
            .or_default() += 1;
        if let Some(agent) = &ticket.told_agent {
            *self.agents_in_flight.entry(agent.clone()).or_default() += 1;
        }
        ticket
    }

    pub(in crate::app) fn settle(&mut self, ticket: DeliveryTicket) {
        Self::drop_one(&mut self.owners_in_flight, &ticket.owner);
        if let Some(agent) = &ticket.told_agent {
            Self::drop_one(&mut self.agents_in_flight, agent);
        }
    }

    /// Re-entry happens before settling the old ticket under the same app lock.
    pub(in crate::app) fn requeue(&mut self, turn: PendingAgentTurn) {
        self.queued.push(turn);
    }

    pub(in crate::app) fn holds_owner(&self, owner: &str) -> bool {
        self.owners_in_flight.contains_key(owner)
            || self.queued.iter().any(|turn| turn.owner == owner)
    }

    pub(in crate::app) fn holds_agent(&self, key: &TabKey) -> bool {
        self.agents_in_flight.contains_key(key)
            || self
                .queued
                .iter()
                .any(|turn| turn.says_something() && turn.tab_key() == *key)
    }

    pub(in crate::app) fn has_in_flight_at_root(&self, root: &std::path::Path) -> bool {
        self.agents_in_flight.keys().any(|key| key.root == root)
    }

    pub(in crate::app) fn retain_queued(
        &mut self,
        mut keep: impl FnMut(&PendingAgentTurn) -> bool,
    ) {
        self.queued.retain(|turn| keep(turn));
    }

    /// Attach an accepted plan operation to the newest unassigned turn for its
    /// exact owner and agent. This is the one intentional queued-turn mutation;
    /// callers do not receive mutable access to the backing vector.
    pub(in crate::app) fn attach_plan_operation(
        &mut self,
        receipt: &OperationReceipt,
    ) -> Result<(), String> {
        let delivery = receipt
            .delivery
            .as_ref()
            .ok_or("thread.post: accepted plan operation has no delivery intent")?;
        let payload = delivery
            .payload
            .as_ref()
            .ok_or("thread.post: accepted plan operation has no bounded payload")?;
        let turn = self
            .queued
            .iter_mut()
            .rev()
            .find(|turn| {
                turn.operation_id.is_none()
                    && turn.owner == delivery.owner_id
                    && turn.agent_id == delivery.agent_id
            })
            .ok_or("thread.post: plan session opened without a delivery turn")?;
        turn.operation_id = Some(receipt.operation_id.clone());
        turn.conversation_id = receipt.conversation_id.clone();
        turn.model_choice = delivery.model_choice.clone();
        turn.choice_revision = delivery.choice_revision;
        turn.interrupt = delivery.interrupt;
        let exact_cold = payload.delivery_prompt(&receipt.operation_id, true);
        let exact_warm = payload.delivery_prompt(&receipt.operation_id, false);
        if let Some(say) = turn.say.as_mut() {
            // The queued lifecycle turn's warm half is its protocol-free work
            // instruction. Rebuild the cold half from that source so this
            // operation gets exactly one protocol block. The payload itself is
            // already fenced to the receipt being attached.
            say.cold = format!("{}\n\n{exact_cold}", say.warm);
            say.warm = exact_warm;
        }
        turn.wants_catch_up = false;
        turn.survives_refusal = true;
        Ok(())
    }

    #[cfg(test)]
    pub(in crate::app) fn queued_len(&self) -> usize {
        self.queued.len()
    }

    #[cfg(test)]
    pub(in crate::app) fn queued_is_empty(&self) -> bool {
        self.queued.is_empty()
    }

    pub(in crate::app) fn queued(&self) -> impl Iterator<Item = &PendingAgentTurn> {
        self.queued.iter()
    }

    #[cfg(test)]
    pub(in crate::app) fn queued_nth(&self, index: usize) -> Option<&PendingAgentTurn> {
        self.queued.get(index)
    }

    #[cfg(test)]
    pub(in crate::app) fn queued_last(&self) -> Option<&PendingAgentTurn> {
        self.queued.last()
    }

    #[cfg(test)]
    pub(in crate::app) fn clear_queued(&mut self) {
        self.queued.clear();
    }

    #[cfg(test)]
    pub(in crate::app) fn is_idle(&self) -> bool {
        self.queued.is_empty()
            && self.owners_in_flight.is_empty()
            && self.agents_in_flight.is_empty()
    }

    fn drop_one<K: std::hash::Hash + Eq>(counts: &mut HashMap<K, usize>, key: &K) {
        let Some(count) = counts.get_mut(key) else {
            return;
        };
        *count -= 1;
        if *count == 0 {
            counts.remove(key);
        }
    }
}

#[cfg(test)]
mod tests;
