//! Correlate native input receipts with their durable reviewer operations.

use crate::app::{off_the_workers, AppState};
use crate::harness::{AgentSession, TurnReceiptSnapshot};
use crate::thread::{MessageDeliveryStatus, SessionInstance};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, Weak};

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
    let mut pump = ReceiptPump {
        state: Arc::downgrade(state),
        instance,
        acknowledged: HashSet::new(),
    };
    tokio::spawn(async move {
        loop {
            // The cumulative snapshot retains receipts emitted before this
            // pump started and across coalesced watch notifications.
            let snapshot = receipts.borrow_and_update().clone();
            let still_bound;
            (pump, still_bound) = off_the_workers(move || {
                let still_bound = pump.record(snapshot);
                (pump, still_bound)
            })
            .await;
            if !still_bound || receipts.changed().await.is_err() {
                return;
            }
        }
    });
}

/// What the receipt pump carries from one snapshot to the next. It travels to
/// the blocking pool and back with each snapshot, because recording a receipt
/// takes the app mutex.
struct ReceiptPump {
    state: Weak<Mutex<AppState>>,
    instance: SessionInstance,
    acknowledged: HashSet<(String, MessageDeliveryStatus)>,
}

impl ReceiptPump {
    /// Record every receipt in `snapshot` not yet recorded. Whether the
    /// instance is still bound to the conversation it was captured on.
    fn record(&mut self, snapshot: TurnReceiptSnapshot) -> bool {
        let Some(state) = self.state.upgrade() else {
            return false;
        };
        let instance = &self.instance;
        let mut app = state.lock().unwrap();
        // Receipts describe immutable messages, so replacing a process must
        // not discard its final acknowledgement. A changed conversation binding
        // still invalidates the captured address.
        if !app
            .resolve_conversation_address(&instance.entity_id, Some(&instance.agent_id))
            .is_ok_and(|address| address.conversation_id == instance.conversation_id)
        {
            return false;
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
            if self.acknowledged.contains(&(operation_id.clone(), status)) {
                continue;
            }
            match app.record_native_delivery_receipt(
                &instance.entity_id,
                &instance.agent_id,
                &operation_id,
                status,
            ) {
                Ok(()) => {
                    self.acknowledged.insert((operation_id, status));
                }
                Err(error) => eprintln!("native message receipt {operation_id}: {error}"),
            }
        }
        true
    }
}
