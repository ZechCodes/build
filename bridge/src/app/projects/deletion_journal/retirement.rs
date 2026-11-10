//! Reconstruct retirement from recorded identities and the live registry on
//! every attempt. Removed tabs' unreaped receipts survive a failed attempt.
use super::*;
use crate::app::git::deferred::DeferredGitWork;

struct RetireDeletionSessions {
    operation: DeletionOperation,
    deletion: ProjectDeletion,
    retirements: Vec<crate::reaper::Retirement>,
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
        });
        Ok(())
    }
    fn queue_recorded_cleanup(
        &mut self,
        deletion: ProjectDeletion,
        operation: DeletionOperation,
    ) -> Result<(), String> {
        operation.check(self)?;
        if self.deletion_config_write {
            return Err("configuration write in progress; retry project deletion".into());
        }
        let mut config =
            self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        config["projects"]
            .as_array_mut()
            .expect("project array")
            .retain(|p| p["id"] != deletion.plan.project_id);
        let mut policies = self.compaction_policies.clone();
        policies.forget_project(&deletion.plan.project_id);
        policies.forget_workspaces(&deletion.plan.workspace_ids);
        policies.write_config(&mut config);
        let call =
            super::execution::DeleteRecordedProject::capture(self, deletion, operation, config)?;
        self.deletion_config_write = true;
        self.defer_project_deletion(call);
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
