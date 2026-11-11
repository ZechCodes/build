//! Reconstruct retirement from recorded identities and the live registry on
//! every attempt. Removed tabs' unreaped receipts survive a failed attempt.
use super::*;
use crate::app::git::deferred::DeferredGitWork;

struct RetireDeletionSessions {
    operation: DeletionOperation,
    deletion: ProjectDeletion,
    retirements: Vec<crate::reaper::Retirement>,
    store: Store,
    sources: Vec<PathBuf>,
}
impl AppState {
    pub(super) fn queue_deletion_retirement(
        &mut self,
        deletion: ProjectDeletion,
        operation: DeletionOperation,
    ) -> Result<(), String> {
        operation.check(self)?;
        let plan = &deletion.plan;
        let mut retirements = self
            .project_deletion_retirements
            .get(&plan.project_id)
            .cloned()
            .unwrap_or_default();
        let keys = self.session_registry.project_deletion_keys(plan);
        for key in keys {
            if let Some(retirement) = self.retire_tab(&key, "closed") {
                retirements.push(retirement);
            }
        }
        self.revoke_project_agents(&plan.run_ids);
        self.project_deletion_retirements
            .insert(plan.project_id.clone(), retirements.clone());
        self.defer_project_deletion(RetireDeletionSessions {
            operation,
            deletion,
            retirements,
            store: self.store.clone().ok_or("missing deletion store")?,
            sources: self.deletion_sources(),
        });
        Ok(())
    }
    fn queue_recorded_cleanup(
        &mut self,
        deletion: ProjectDeletion,
        operation: DeletionOperation,
    ) -> Result<(), String> {
        operation.check(self)?;
        if deletion.phase == "prepared" {
            super::registration::queue_config_removal(self, deletion, operation)?;
        } else {
            self.defer_project_deletion(super::execution::DeleteRecordedProject::capture(
                self, deletion, operation,
            )?);
        }
        Ok(())
    }
}
impl DeferredGitWork for RetireDeletionSessions {
    fn run(&self, _: &Value) -> Result<Value, String> {
        for retirement in &self.retirements {
            if !retirement.wait(crate::orchestrator::CHECKOUT_REAP_WAIT) {
                return Err(
                    "Project deletion-pending: a process has not exited; retry project.delete"
                        .into(),
                );
            }
        }
        validate_plan(&self.store, &self.deletion, &self.sources)?;
        self.store
            .writer()
            .mark_sync_dirty("registry", "lifecycle", crate::store::sync::now_ms())
            .map_err(|e| e.to_string())?;
        if self.deletion.phase == "prepared" {
            self.store
                .writer()
                .secrets_retire_project(&self.deletion.plan.project_id)
                .map_err(|e| e.to_string())?;
        }
        Ok(Value::Null)
    }
    fn settle(&self, app: &mut AppState, _: Value) -> Result<Value, String> {
        self.operation.check(app)?;
        app.project_deletion_retirements.remove(&self.operation.id);
        app.queue_recorded_cleanup(self.deletion.clone(), self.operation.clone())?;
        Ok(Value::Null)
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
