//! The `harnesses.changed` push: the harness inventory moved, so the
//! snapshot a client holds is out of date (#434).
//!
//! The inventory is a service of its own (`harness::inventory`), sweeping
//! whether or not anyone is connected; this only follows its revision for
//! each greeted session, the way `models.changed` follows the CLI readings.

use std::sync::Arc;

use super::AppState;
use crate::carrier::SessionSender;
use crate::changes::harnesses_changed_payload;
use crate::harness::inventory::Inventory;

impl AppState {
    /// Serve `harnesses.*`, and announce their changes, from `inventory`
    /// rather than the process's own.
    pub fn with_harness_inventory(mut self, inventory: Arc<Inventory>) -> Self {
        self.harness_inventory = inventory;
        self
    }

    pub(crate) fn harness_inventory(&self) -> &Arc<Inventory> {
        &self.harness_inventory
    }

    /// Tell `sender` each time the inventory's revision rises from here on.
    /// The snapshot standing now is the one it will ask for.
    pub(in crate::app) fn subscribe_harnesses_changed(&mut self, sender: &SessionSender) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        self.unsubscribe_harnesses_changed(sender.session_id());
        let mut changes = self.harness_inventory.changes();
        changes.mark_unchanged();
        let sender = sender.clone();
        let session_id = sender.session_id().to_string();
        let task = runtime.spawn(async move {
            while changes.changed().await.is_ok() {
                let revision = *changes.borrow_and_update();
                if !sender.push(harnesses_changed_payload(revision)) {
                    break;
                }
            }
        });
        self.harnesses_subscriptions.insert(session_id, task);
    }

    pub(in crate::app) fn unsubscribe_harnesses_changed(&mut self, session_id: &str) {
        if let Some(task) = self.harnesses_subscriptions.remove(session_id) {
            task.abort();
        }
    }
}
