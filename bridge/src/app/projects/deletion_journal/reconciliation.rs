//! Refresh both projections off-lock, against the current durable config epoch.
use super::*;
use crate::app::git::deferred::DeferredGitWork;
use std::sync::Mutex;

struct ReconcileDeletion {
    snapshot: crate::app::device_lifecycle::DeviceLifecycleSnapshot,
    config: Value,
    revision: crate::app::secret_storage::MembershipRevision,
    epoch: u64,
    outcome: Mutex<Option<Result<bool, String>>>,
    store: Store,
    operation: DeletionOperation,
    result: Result<Value, String>,
    #[cfg(test)]
    gate: Option<crate::app::OffLockGate>,
}
impl AppState {
    pub(super) fn queue_deletion_reconciliation(
        &mut self,
        store: Store,
        operation: DeletionOperation,
        result: Result<Value, String>,
    ) -> Result<(), String> {
        self.defer_project_deletion(ReconcileDeletion {
            snapshot: self.capture_device_lifecycle()?,
            config: self.config_value(&self.projects_dir, self.default_harness, self.isolation),
            revision: self.secret_membership_revision.clone(),
            epoch: self.secret_membership_revision.snapshot(),
            outcome: Mutex::new(None),
            store,
            operation,
            result,
            #[cfg(test)]
            gate: self.deletion_reconciliation_gate.clone(),
        });
        Ok(())
    }
}
impl ReconcileDeletion {
    fn reconcile(&self) -> Result<bool, String> {
        if !self.revision.wait_current(self.epoch) {
            return Ok(false);
        }
        let current = self
            .store
            .writer()
            .secrets_reconcile_config_if_current(&self.config, || self.revision.current(self.epoch))
            .map_err(|e| e.to_string())?;
        if !current {
            return Ok(false);
        }
        self.snapshot.reconcile(&self.store.writer())?;
        Ok(true)
    }
    fn still_current(&self, app: &AppState) -> Result<bool, String> {
        Ok(self.revision.current(self.epoch)
            && self.config
                == app.config_value(&app.projects_dir, app.default_harness, app.isolation)
            && self.snapshot.is_current(app)?)
    }
}
impl DeferredGitWork for ReconcileDeletion {
    fn run(&self, _: &Value) -> Result<Value, String> {
        #[cfg(test)]
        if let Some(gate) = &self.gate {
            gate.arrive();
        }
        *self.outcome.lock().unwrap() = Some(self.reconcile());
        Ok(Value::Null)
    }
    fn settle(&self, app: &mut AppState, _: Value) -> Result<Value, String> {
        self.operation.check(app)?;
        let outcome = self
            .outcome
            .lock()
            .unwrap()
            .clone()
            .ok_or("missing reconciliation outcome")?;
        match outcome {
            Err(error) => {
                app.secret_membership_dirty.set(true);
                app.device_lifecycle_dirty.set(true);
                app.deletion_reconciliation_failed.set(true);
                self.operation.finish(app);
                Err(format!(
                    "Project deletion reconciliation failed; retry project.delete: {error}"
                ))
            }
            Ok(reconciled) if !reconciled || !self.still_current(app)? => {
                app.queue_deletion_reconciliation(
                    self.store.clone(),
                    self.operation.clone(),
                    self.result.clone(),
                )?;
                Ok(Value::Null)
            }
            Ok(_) => {
                app.secret_membership_dirty.set(false);
                app.device_lifecycle_dirty.set(false);
                app.deletion_reconciliation_failed.set(false);
                app.publish_secret_project_routes(&self.config);
                self.operation.finish(app);
                self.result.clone()
            }
        }
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
