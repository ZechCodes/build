//! Native harness receipts update the same durable messages as conversation reads.

use super::AppState;
use crate::operation::OperationReceipt;
use crate::thread::MessageDeliveryStatus;

impl AppState {
    pub(in crate::app) fn record_native_delivery_receipt(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        token: &str,
        status: MessageDeliveryStatus,
    ) -> Result<(), String> {
        if self.record_legacy_native_receipt(entity_id, agent_id, token, status)? {
            return Ok(());
        }
        if status == MessageDeliveryStatus::Seen {
            return self.record_native_operation_seen(entity_id, agent_id, token);
        }
        let receipt = self
            .operation_receipt(token)?
            .ok_or("unknown native receipt")?;
        let (delivery_owner, delivery_agent) = receipt.delivery_target();
        if delivery_owner != entity_id || delivery_agent != agent_id {
            return Err("native receipt does not belong to this agent".into());
        }
        let address = self.resolve_conversation_address(entity_id, Some(agent_id))?;
        if receipt.conversation_id != address.conversation_id {
            return Err("native receipt conversation binding changed".into());
        }
        receipt
            .delivery_message_span()
            .ok_or("operation has no bounded message span")?;
        if status == MessageDeliveryStatus::Uncertain {
            self.transition_delivery_operation(
                token,
                receipt.status,
                crate::operation::OperationStatus::Uncertain,
                Some("agent session ended before confirming message receipt"),
            )?;
        }
        self.update_operation_messages(&receipt, status)
    }
    /// Apply a transport observation to exactly the messages accepted by this
    /// operation, including messages outside the resident conversation window.
    pub(in crate::app) fn record_operation_delivery_status(
        &mut self,
        operation_id: &str,
        status: MessageDeliveryStatus,
    ) -> Result<(), String> {
        let receipt = self
            .operation_receipt(operation_id)?
            .ok_or_else(|| format!("unknown operation_id: {operation_id}"))?;
        self.update_operation_messages(&receipt, status)
    }

    fn update_operation_messages(
        &mut self,
        receipt: &OperationReceipt,
        status: MessageDeliveryStatus,
    ) -> Result<(), String> {
        let (start_sequence, end_sequence) = receipt
            .delivery_message_span()
            .ok_or("operation has no bounded message span")?;
        let (delivery_owner, delivery_agent) = receipt.delivery_target();
        let address = self.resolve_conversation_address(delivery_owner, Some(delivery_agent))?;
        if address.conversation_id != receipt.conversation_id {
            return Err("operation conversation binding changed".into());
        }
        let sequence = self
            .store
            .as_ref()
            .map(|store| {
                store
                    .set_delivery_status(
                        &receipt.conversation_id,
                        Some(&receipt.operation_id),
                        start_sequence,
                        end_sequence,
                        status,
                    )
                    .map_err(|error| format!("operation message store: {error}"))
            })
            .transpose()?;
        if let Some(committed) = &sequence {
            self.apply_committed_thread_items(committed);
        }
        self.edit_agent_conversation(delivery_owner, delivery_agent, |thread, _| {
            thread.set_operation_delivery_status(
                &receipt.operation_id,
                start_sequence,
                end_sequence,
                status,
            );
            if let Some(sequence) = sequence {
                thread.advance_sequence_to(sequence.sequence);
            }
            Ok(serde_json::Value::Null)
        })?;
        Ok(())
    }

    /// The pump checks the captured conversation binding. Operation ownership
    /// is checked again before acknowledging any of its immutable messages.
    pub(in crate::app) fn record_native_operation_seen(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        operation_id: &str,
    ) -> Result<(), String> {
        let receipt = self
            .operation_receipt(operation_id)?
            .ok_or_else(|| format!("unknown operation_id: {operation_id}"))?;
        let (delivery_owner, delivery_agent) = receipt.delivery_target();
        let address = self.resolve_conversation_address(entity_id, Some(agent_id))?;
        if receipt.method != crate::operation::THREAD_POST_METHOD
            || delivery_owner != entity_id
            || delivery_agent != agent_id
            || receipt.conversation_id != address.conversation_id
        {
            return Err("native receipt does not belong to this agent".into());
        }
        let (start_sequence, end_sequence) = receipt
            .delivery_message_span()
            .ok_or("operation has no bounded message span")?;
        let now = crate::store::now_rfc3339();
        let sequence = self
            .store
            .as_ref()
            .map(|store| {
                store
                    .acknowledge_messages(
                        &receipt.conversation_id,
                        Some(operation_id),
                        start_sequence,
                        end_sequence,
                        &now,
                    )
                    .map_err(|error| format!("operation message store: {error}"))
            })
            .transpose()?;
        if let Some(committed) = &sequence {
            self.apply_committed_thread_items(committed);
        }
        self.edit_agent_conversation(entity_id, agent_id, |thread, _| {
            thread.read_native_operation_messages(operation_id, start_sequence, end_sequence, &now);
            if let Some(sequence) = sequence {
                thread.advance_sequence_to(sequence.sequence);
            }
            Ok(serde_json::Value::Null)
        })?;
        self.consume_task_completion_operation(entity_id, agent_id, operation_id);
        Ok(())
    }
}
