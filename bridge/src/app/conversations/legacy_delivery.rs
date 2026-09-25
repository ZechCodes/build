//! Native delivery for reviewer messages posted through the legacy API.

use super::AppState;
use crate::operation::OperationPayload;
use crate::thread::{MessageDeliveryStatus, Thread, ThreadItem, ThreadMessage};

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
                            .acknowledge_native_legacy_messages(
                                &address.conversation_id,
                                sequence,
                                sequence,
                                &now,
                            )
                            .map_err(|error| format!("legacy message store: {error}"))
                    })
                    .transpose()?;
                self.edit_agent_conversation(owner_id, agent_id, |thread, _| {
                    thread.read_native_legacy_messages(sequence, sequence, &now);
                    if let Some(updated_sequence) = updated_sequence {
                        thread.advance_sequence_to(updated_sequence);
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
    pub(in crate::app) fn legacy_delivery_payload(
        &self,
        owner_id: &str,
        agent_id: &str,
    ) -> Result<Option<OperationPayload>, String> {
        let address = self.resolve_conversation_address(owner_id, Some(agent_id))?;
        let thread = self.conversation_at(&address)?;
        let messages = self.waiting_legacy_messages(thread)?;
        let Some(first) = messages.first() else {
            return Ok(None);
        };
        let start_sequence = first.sequence;
        let end_sequence = messages
            .last()
            .expect("a first legacy message implies a last message")
            .sequence;
        Ok(Some(OperationPayload {
            start_sequence,
            end_sequence,
            messages,
            prior_context: String::new(),
            ask_to_name: false,
            tells_sender_context: false,
        }))
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

    /// Record provider acceptance for the exact legacy message range rendered
    /// into a prompt, including messages outside the resident thread window.
    pub(in crate::app) fn record_legacy_delivery_status(
        &mut self,
        owner_id: &str,
        agent_id: &str,
        payload: &OperationPayload,
        status: MessageDeliveryStatus,
    ) -> Result<(), String> {
        let address = self.resolve_conversation_address(owner_id, Some(agent_id))?;
        if !payload.messages.is_empty() {
            for message in &payload.messages {
                self.record_legacy_delivery_range(
                    owner_id,
                    agent_id,
                    &address.conversation_id,
                    message.sequence,
                    message.sequence,
                    status,
                )?;
            }
            return Ok(());
        }
        self.record_legacy_delivery_range(
            owner_id,
            agent_id,
            &address.conversation_id,
            payload.start_sequence,
            payload.end_sequence,
            status,
        )
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
                    .set_legacy_delivery_status(
                        conversation_id,
                        start_sequence,
                        end_sequence,
                        status,
                    )
                    .map_err(|error| format!("legacy message store: {error}"))
            })
            .transpose()?;
        self.edit_agent_conversation(owner_id, agent_id, |thread, _| {
            thread.set_legacy_delivery_status(start_sequence, end_sequence, status);
            if let Some(sequence) = sequence {
                thread.advance_sequence_to(sequence);
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
