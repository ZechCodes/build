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
        let delivery = receipt
            .delivery
            .as_ref()
            .ok_or("operation has no delivery")?;
        if delivery.owner_id != entity_id || delivery.agent_id != agent_id {
            return Err("native receipt does not belong to this agent".into());
        }
        let address = self.resolve_conversation_address(entity_id, Some(agent_id))?;
        if receipt.conversation_id != address.conversation_id {
            return Err("native receipt conversation binding changed".into());
        }
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
        let delivery = receipt
            .delivery
            .as_ref()
            .ok_or("operation has no delivery")?;
        let payload = delivery
            .payload
            .as_ref()
            .ok_or("operation has no payload")?;
        let address =
            self.resolve_conversation_address(&delivery.owner_id, Some(&delivery.agent_id))?;
        if address.conversation_id != receipt.conversation_id {
            return Err("operation conversation binding changed".into());
        }
        let sequence = self
            .store
            .as_ref()
            .map(|store| {
                store
                    .set_operation_delivery_status(
                        &receipt.conversation_id,
                        &receipt.operation_id,
                        payload.start_sequence,
                        payload.end_sequence,
                        status,
                    )
                    .map_err(|error| format!("operation message store: {error}"))
            })
            .transpose()?;
        self.edit_agent_conversation(&delivery.owner_id, &delivery.agent_id, |thread, _| {
            thread.set_operation_delivery_status(
                &receipt.operation_id,
                payload.start_sequence,
                payload.end_sequence,
                status,
            );
            if let Some(sequence) = sequence {
                thread.advance_sequence_to(sequence);
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
        let delivery = receipt
            .delivery
            .as_ref()
            .ok_or("operation has no delivery")?;
        let address = self.resolve_conversation_address(entity_id, Some(agent_id))?;
        if receipt.method != crate::operation::THREAD_POST_METHOD
            || delivery.owner_id != entity_id
            || delivery.agent_id != agent_id
            || receipt.conversation_id != address.conversation_id
        {
            return Err("native receipt does not belong to this agent".into());
        }
        let payload = delivery
            .payload
            .as_ref()
            .ok_or("operation has no payload")?;
        let now = crate::store::now_rfc3339();
        let sequence = self
            .store
            .as_ref()
            .map(|store| {
                store
                    .acknowledge_native_operation_messages(
                        &receipt.conversation_id,
                        operation_id,
                        payload.start_sequence,
                        payload.end_sequence,
                        &now,
                    )
                    .map_err(|error| format!("operation message store: {error}"))
            })
            .transpose()?;
        self.edit_agent_conversation(entity_id, agent_id, |thread, _| {
            thread.read_native_operation_messages(
                operation_id,
                payload.start_sequence,
                payload.end_sequence,
                &now,
            );
            if let Some(sequence) = sequence {
                thread.advance_sequence_to(sequence);
            }
            Ok(serde_json::Value::Null)
        })?;
        Ok(())
    }
}
