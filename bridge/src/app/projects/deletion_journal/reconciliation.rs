//! Preserve the original RPC result while settling device sync off the lock.
use super::*;
use crate::app::git::deferred::DeferredGitWork;

struct ReconcileDeletion {
    snapshot: crate::app::device_lifecycle::DeviceLifecycleSnapshot,
    store: Store,
    operation: DeletionOperation,
    result: Result<Value, String>,
}
impl AppState {
    pub(super) fn queue_deletion_reconciliation(
        &mut self,
        store: Store,
        operation: DeletionOperation,
        result: Result<Value, String>,
    ) -> Result<(), String> {
        let snapshot = self.capture_device_lifecycle()?;
        self.defer_project_deletion(ReconcileDeletion {
            snapshot,
            store,
            operation,
            result,
        });
        Ok(())
    }
}
impl DeferredGitWork for ReconcileDeletion {
    fn run(&self, _: &Value) -> Result<Value, String> {
        // Same failure policy as ordinary reconciliation: retain the durable
        // dirty marker for startup/the next lifecycle mutation, and report it.
        if let Err(error) = self.snapshot.reconcile(&self.store.writer()) {
            eprintln!("device sync reconciliation: {error}");
        }
        Ok(Value::Null)
    }
    fn settle(&self, app: &mut AppState, _: Value) -> Result<Value, String> {
        self.operation.check(app)?;
        if !self.snapshot.is_current(app)? {
            app.queue_deletion_reconciliation(
                self.store.clone(),
                self.operation.clone(),
                self.result.clone(),
            )?;
            return Ok(Value::Null);
        }
        app.device_lifecycle_dirty.set(false);
        self.operation.finish(app);
        self.result.clone()
    }
    fn invalidates_on_error(&self) -> bool {
        true
    }
    fn invalidate(&self, app: &mut AppState) {
        if app.deferred_work.is_none() {
            self.operation.finish(app);
        }
    }
}
