//! Correlate native input receipts with their durable reviewer operations.

use crate::app::AppState;
use crate::harness::AgentSession;
use crate::thread::{MessageDeliveryStatus, SessionInstance};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

pub(in crate::app) fn spawn_receipt_pump(
    state: &Arc<Mutex<AppState>>,
    session: &Arc<dyn AgentSession>,
    instance: Option<SessionInstance>,
) {
    let (Some(mut receipts), Some(instance)) = (session.turn_receipts(), instance) else {
        return;
    };
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    let state = Arc::downgrade(state);
    tokio::spawn(async move {
        let mut acknowledged = HashSet::new();
        loop {
            // The cumulative snapshot retains receipts emitted before this
            // pump started and across coalesced watch notifications.
            let snapshot = receipts.borrow_and_update().clone();
            {
                let Some(state) = state.upgrade() else {
                    return;
                };
                let mut app = state.lock().unwrap();
                // Receipts describe immutable messages, so replacing a process
                // must not discard its final acknowledgement. A changed
                // conversation binding still invalidates the captured address.
                if !app
                    .resolve_conversation_address(&instance.entity_id, Some(&instance.agent_id))
                    .is_ok_and(|address| address.conversation_id == instance.conversation_id)
                {
                    return;
                }
                let updates = snapshot
                    .seen_operation_ids
                    .into_iter()
                    .map(|id| (id, MessageDeliveryStatus::Seen))
                    .chain(
                        snapshot
                            .uncertain_operation_ids
                            .into_iter()
                            .map(|id| (id, MessageDeliveryStatus::Uncertain)),
                    );
                for (operation_id, status) in updates {
                    if acknowledged.contains(&(operation_id.clone(), status)) {
                        continue;
                    }
                    match app.record_native_delivery_receipt(
                        &instance.entity_id,
                        &instance.agent_id,
                        &operation_id,
                        status,
                    ) {
                        Ok(()) => {
                            acknowledged.insert((operation_id, status));
                        }
                        Err(error) => eprintln!("native message receipt {operation_id}: {error}"),
                    }
                }
            }
            if receipts.changed().await.is_err() {
                return;
            }
        }
    });
}
