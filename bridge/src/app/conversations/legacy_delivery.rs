//! Native delivery for reviewer messages posted through the legacy API.

use super::{AppState, ConversationAddress};
use crate::operation::OperationPayload;
use crate::thread::{MessageDeliveryStatus, Thread, ThreadItem, ThreadMessage};

/// Local admission identity for the provider handoff. Payloads and legacy
/// receipt tokens keep their wire shape; this identity never crosses the wire.
pub(in crate::app) struct FrozenLegacyDelivery {
    address: ConversationAddress,
    pub(in crate::app) thread_id: String,
    pub(in crate::app) generation_revision: u64,
    pub(in crate::app) payload: OperationPayload,
}

impl FrozenLegacyDelivery {
    fn is_current(&self, app: &AppState, owner: &str, agent: &str) -> bool {
        let Ok(address) = app.resolve_conversation_address(owner, Some(agent)) else {
            return false;
        };
        address == self.address
            && app.conversation_at(&address).is_ok_and(|thread| {
                thread.id == self.thread_id
                    && thread.generation_revision == self.generation_revision
            })
    }

    fn apply_status(&self, thread: &mut Thread, status: MessageDeliveryStatus) {
        for message in &self.payload.messages {
            let matches = thread.items.iter().any(|item| {
                matches!(item, ThreadItem::Message(current)
                    if current.id == message.id && current.sequence == message.sequence)
            });
            if matches {
                thread.set_legacy_delivery_status(message.sequence, message.sequence, status);
            }
        }
    }
}

impl AppState {
    /// Apply an internal native-session receipt for an operationless turn.
    /// Non-legacy receipt ids are left for the operation receipt path.
    pub(in crate::app) fn record_legacy_native_receipt(
        &mut self,
        owner_id: &str,
        agent_id: &str,
        token: &str,
        status: MessageDeliveryStatus,
    ) -> Result<bool, String> {
        let Some(encoded_sequences) = token.strip_prefix("@legacy/") else {
            return Ok(false);
        };
        let sequences = parse_legacy_receipt_sequences(encoded_sequences)?;

        let address = self.resolve_conversation_address(owner_id, Some(agent_id))?;
        let now = crate::store::now_rfc3339();
        for sequence in sequences {
            if status == MessageDeliveryStatus::Seen {
                let updated_sequence = self
                    .store
                    .as_ref()
                    .map(|store| {
                        store
                            .acknowledge_messages(
                                &address.conversation_id,
                                None,
                                sequence,
                                sequence,
                                &now,
                            )
                            .map_err(|error| format!("legacy message store: {error}"))
                    })
                    .transpose()?;
                if let Some(committed) = &updated_sequence {
                    self.apply_committed_thread_items(committed);
                }
                self.edit_agent_conversation(owner_id, agent_id, |thread, _| {
                    thread.read_native_legacy_messages(sequence, sequence, &now);
                    if let Some(updated_sequence) = updated_sequence {
                        thread.advance_sequence_to(updated_sequence.sequence);
                    }
                    Ok(serde_json::Value::Null)
                })?;
            } else {
                self.record_legacy_delivery_range(
                    owner_id,
                    agent_id,
                    &address.conversation_id,
                    sequence,
                    sequence,
                    status,
                )?;
            }
        }
        Ok(true)
    }

    /// Freeze every pending legacy reviewer message from the complete durable
    /// conversation. The returned bounds are also the receipt used to mark the
    /// same messages after the provider accepts the prompt.
    #[cfg(test)]
    pub(in crate::app) fn legacy_delivery_payload(
        &self,
        owner_id: &str,
        agent_id: &str,
    ) -> Result<Option<OperationPayload>, String> {
        let address = self.resolve_conversation_address(owner_id, Some(agent_id))?;
        let thread = self.conversation_at(&address)?;
        self.legacy_payload_at(owner_id, thread)
    }

    /// Capture the canonical generation with the pending input while the app
    /// is held. Every handoff callback retains this same admission identity.
    pub(in crate::app) fn freeze_legacy_delivery(
        &self,
        owner_id: &str,
        agent_id: &str,
    ) -> Result<Option<FrozenLegacyDelivery>, String> {
        let address = self.resolve_conversation_address(owner_id, Some(agent_id))?;
        let thread = self.conversation_at(&address)?;
        Ok(self
            .legacy_payload_at(owner_id, thread)?
            .map(|payload| FrozenLegacyDelivery {
                address,
                thread_id: thread.id.clone(),
                generation_revision: thread.generation_revision,
                payload,
            }))
    }

    fn legacy_payload_at(
        &self,
        owner_id: &str,
        thread: &Thread,
    ) -> Result<Option<OperationPayload>, String> {
        let messages = self.waiting_legacy_messages(thread)?;
        let Some(first) = messages.first() else {
            return Ok(None);
        };
        let start_sequence = first.sequence;
        let end_sequence = messages
            .last()
            .expect("a first legacy message implies a last message")
            .sequence;
        Ok(Some(self.payload_for_reader(
            owner_id,
            OperationPayload {
                start_sequence,
                end_sequence,
                messages,
                prior_context: String::new(),
                ask_to_name: false,
                tells_sender_context: false,
            },
        )))
    }

    /// Every message a legacy delivery still owes, from the whole
    /// conversation: off the resident tail when that is all of it, and
    /// otherwise asked of the store, which decodes only what waits.
    fn waiting_legacy_messages(&self, thread: &Thread) -> Result<Vec<ThreadMessage>, String> {
        if thread.total_item_count() == thread.items.len() as u64 {
            return Ok(thread
                .items
                .iter()
                .filter_map(|item| match item {
                    ThreadItem::Message(message) if message.awaits_legacy_delivery() => {
                        Some(message.clone())
                    }
                    _ => None,
                })
                .collect());
        }
        self.history_store()?
            .waiting_legacy_messages(&thread.agent.id)
            .map_err(|error| format!("conversation store: {error}"))
    }

    /// Record one provider transition for the frozen sparse message set,
    /// including rows outside the resident tail. Stale callbacks leave the
    /// replacement history unchanged.
    pub(in crate::app) fn record_legacy_delivery_status(
        &mut self,
        owner_id: &str,
        agent_id: &str,
        frozen: &FrozenLegacyDelivery,
        status: MessageDeliveryStatus,
    ) -> Result<(), String> {
        if frozen.payload.messages.is_empty() || !frozen.is_current(self, owner_id, agent_id) {
            return Ok(());
        }
        let payload = &frozen.payload;
        let committed = match &self.store {
            Some(store) => {
                let committed = store
                    .set_frozen_legacy_delivery_status(
                        &frozen.address.conversation_id,
                        &frozen.thread_id,
                        frozen.generation_revision,
                        &payload.messages,
                        status,
                    )
                    .map_err(|error| format!("legacy message store: {error}"))?;
                let Some(committed) = committed else {
                    return Ok(());
                };
                self.apply_committed_thread_items(&committed);
                Some(committed)
            }
            None => None,
        };
        self.edit_agent_conversation(owner_id, agent_id, |thread, _| {
            frozen.apply_status(thread, status);
            if let Some(committed) = committed {
                thread.advance_sequence_to(committed.sequence);
            }
            Ok(serde_json::Value::Null)
        })?;
        Ok(())
    }

    fn record_legacy_delivery_range(
        &mut self,
        owner_id: &str,
        agent_id: &str,
        conversation_id: &str,
        start_sequence: u64,
        end_sequence: u64,
        status: MessageDeliveryStatus,
    ) -> Result<(), String> {
        let sequence = self
            .store
            .as_ref()
            .map(|store| {
                store
                    .set_delivery_status(
                        conversation_id,
                        None,
                        start_sequence,
                        end_sequence,
                        status,
                    )
                    .map_err(|error| format!("legacy message store: {error}"))
            })
            .transpose()?;
        if let Some(committed) = &sequence {
            self.apply_committed_thread_items(committed);
        }
        self.edit_agent_conversation(owner_id, agent_id, |thread, _| {
            thread.set_legacy_delivery_status(start_sequence, end_sequence, status);
            if let Some(sequence) = sequence {
                thread.advance_sequence_to(sequence.sequence);
            }
            Ok(serde_json::Value::Null)
        })?;
        Ok(())
    }
}

fn parse_legacy_receipt_sequences(encoded: &str) -> Result<Vec<u64>, String> {
    let sequences = encoded
        .split(',')
        .map(|value| {
            value
                .parse::<u64>()
                .ok()
                .filter(|sequence| *sequence > 0)
                .ok_or_else(|| "invalid legacy native receipt sequence".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if sequences.is_empty() || sequences.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err("legacy native receipt sequences must be strictly increasing".into());
    }
    Ok(sequences)
}
