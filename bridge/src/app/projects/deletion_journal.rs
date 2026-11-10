//! Deletion is admitted under the app lock; evidence and durable work run on
//! the existing deferred lane. Every continuation checks its per-project claim.
use crate::app::{AppState, DeferredGit, DeferredWork};
use crate::store::{ProjectDeletion, ProjectDeletionPlan, Store};
use serde_json::{json, Value};
use std::path::PathBuf;

mod admission;
mod execution;
mod fence;
mod inventory;
mod preparation;
mod reconciliation;
mod recovery;
mod registration;
mod retirement;
use inventory::{capture_plan_inventory, inventory_unchanged};

#[derive(Clone)]
struct DeletionOperation {
    id: String,
    token: String,
}
impl DeletionOperation {
    fn check(&self, app: &AppState) -> Result<(), String> {
        if app.project_deletion_operations.get(&self.id) != Some(&self.token) {
            return Err("project deletion claim changed; cleanup refused".into());
        }
        Ok(())
    }
    fn finish(&self, app: &mut AppState) {
        if self.check(app).is_ok() {
            app.project_deletion_operations.remove(&self.id);
            app.project_deletion_provisional.remove(&self.id);
            app.refresh_deletion_fences();
            app.publish_read_metadata();
        }
    }
}
impl AppState {
    pub(super) fn project_deletion_workspaces(
        &self,
        id: &str,
    ) -> Result<Vec<&crate::workspace::Workspace>, String> {
        let key = &self.projects.get(id).ok_or("unknown project")?.project_key;
        let prior: Vec<_> = self
            .project_deletion_cache
            .iter()
            .filter(|r| r.tombstone.as_deref() == Some("abandoned") && r.plan.project_key == *key)
            .map(|r| &r.plan)
            .collect();
        Ok(self
            .workspaces
            .list(None)
            .into_iter()
            .filter(|w| {
                w.project_id == id
                    || prior.iter().any(|p| {
                        p.workspace_ids.contains(&w.id)
                            && w.project_id == p.project_id
                            && self.projects.get(&w.project_id).is_none()
                            && !self
                                .projects
                                .parked()
                                .iter()
                                .any(|entry| entry["id"] == w.project_id)
                    })
            })
            .collect())
    }
    #[cfg(test)]
    pub(in crate::app) fn project_deletion_records(&self) -> Result<Vec<ProjectDeletion>, String> {
        Ok(self.project_deletion_cache.clone())
    }
    pub(super) fn deletion_record(&self, id: &str) -> Result<Option<ProjectDeletion>, String> {
        Ok(self
            .project_deletion_cache
            .iter()
            .rev()
            .find(|r| r.plan.project_id == id)
            .cloned())
    }
    fn claim_deletion_operation(&mut self, id: &str) -> Result<DeletionOperation, String> {
        if self.project_deletion_operations.contains_key(id) {
            return Err("project deletion is already running; retry after it finishes".into());
        }
        let operation = DeletionOperation {
            id: id.into(),
            token: uuid::Uuid::new_v4().to_string(),
        };
        self.project_deletion_operations
            .insert(operation.id.clone(), operation.token.clone());
        Ok(operation)
    }
    fn claim_deletion(&mut self, plan: &ProjectDeletionPlan) -> Result<DeletionOperation, String> {
        let operation = self.claim_deletion_operation(&plan.project_id)?;
        self.project_deletion_provisional
            .insert(plan.project_id.clone(), plan.clone());
        self.refresh_deletion_fences();
        self.publish_read_metadata();
        Ok(operation)
    }
    fn defer_project_deletion(
        &mut self,
        call: impl crate::app::git::deferred::DeferredGitWork + 'static,
    ) {
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(ScopedDeletion(call)),
            params: json!({}),
            invalidates: true,
            registry_change: true,
            #[cfg(test)]
            gate: None,
        })));
    }
    pub(super) fn retry_recorded_project_delete(
        &mut self,
        id: &str,
        params: &Value,
    ) -> Result<Option<Value>, String> {
        let Some(deletion) = self.deletion_record(id)? else {
            return Ok(None);
        };
        if deletion.tombstone.as_deref() == Some("abandoned") {
            return Ok(None);
        }
        if self
            .projects
            .get(id)
            .is_some_and(|p| p.project_key != deletion.plan.project_key)
        {
            return Err("project deletion identity mismatch; no cleanup was performed".into());
        }
        if deletion.plan.delete_tasks != (params["tasks"] == "delete") {
            return Err("project deletion mode mismatch; retry the same delete".into());
        }
        if deletion.tombstone.is_some() {
            if self.projects.get(id).is_some() {
                return Err("project deletion identity has been registered again".into());
            }
            let store = self.store.clone().ok_or("missing deletion store")?;
            let operation = self.claim_deletion_operation(id)?;
            let reply = json!({"project_id":id,"deleted":true});
            if let Err(error) =
                self.queue_deletion_reconciliation(store, operation.clone(), Ok(reply.clone()))
            {
                operation.finish(self);
                return Err(error);
            }
            return Ok(Some(reply));
        }
        let operation = self.claim_deletion(&deletion.plan)?;
        self.defer_project_deletion(preparation::PrepareDeletion::resume(
            self, deletion, operation,
        )?);
        Ok(Some(json!({"project_id":id,"deleted":true})))
    }
    fn deletion_sources(&self) -> Vec<PathBuf> {
        self.projects
            .iter()
            .flat_map(|p| {
                std::iter::once(p.repo_path.clone()).chain(p.sources.iter().map(|s| s.path.clone()))
            })
            .collect()
    }
    fn publish_deletion_records(&mut self, id: &str, records: Vec<ProjectDeletion>) {
        // Concurrent projects settle independently. An older off-lock snapshot
        // must not overwrite another project's more recent phase or tombstone.
        let newest = records.iter().map(|r| r.command_id).max().unwrap_or(0);
        self.project_deletion_cache.retain(|cached| {
            cached.plan.project_id != id
                && (cached.tombstone.is_none()
                    || cached.command_id > newest
                    || records.iter().any(|r| r.command_id == cached.command_id))
        });
        self.project_deletion_cache
            .extend(records.into_iter().filter(|r| r.plan.project_id == id));
        self.refresh_deletion_fences();
        self.publish_read_metadata();
    }
    fn apply_removed_registration(&mut self, plan: &ProjectDeletionPlan) {
        self.compaction_policies.forget_project(&plan.project_id);
        self.compaction_policies
            .forget_workspaces(&plan.workspace_ids);
        self.retain_environment_snapshot_observation(&plan.project_id);
        self.projects.remove(&plan.project_id);
        self.board.diff_mut().remove_project(&plan.project_id);
        for id in &plan.workspace_ids {
            self.workspaces.forget_memory(id);
        }
        self.note_board_lists_changed(crate::changes::BoardLists {
            projects: true,
            environments: true,
            workspaces: true,
            usage_limits: false,
        });
    }
    fn apply_finished_project(&mut self, plan: &ProjectDeletionPlan) {
        if plan.delete_tasks {
            self.task_admission_keys
                .borrow_mut()
                .retain(|_, key| key != &plan.project_key);
        }
        for id in &plan.run_ids {
            self.runs.remove(id);
            self.forget_session_owner(id);
            self.forget_run(id);
        }
        for id in &plan.plan_ids {
            self.plans.remove(id);
            self.committed_entities.forget(id);
            self.projects.unbind_entity(id);
            self.board.attention_mut().remove_entity_clocks(id);
            self.forget_live_roster_entity(id);
        }
        for id in &plan.capture_ids {
            self.captures.remove(id);
            self.publish_capture_snapshot(id);
        }
        self.forget_session_owner(&plan.project_id);
        for id in &plan.workspace_ids {
            self.workspaces.forget_memory(id);
        }
        self.project_deletion_retirements.remove(&plan.project_id);
    }
}

/// Evidence uses captured addresses, never app callbacks. Startup calls this
/// only during owned-state construction, before AppState::shared creates a mutex.
fn validate_plan(
    store: &Store,
    deletion: &ProjectDeletion,
    sources: &[PathBuf],
) -> Result<(), String> {
    for checkout in &deletion.plan.checkouts {
        crate::isolation::WorktreeBackend
            .verify_recorded(&checkout.source, &checkout.path, &checkout.registered_name)
            .map_err(|e| e.to_string())?;
    }
    for source in sources {
        let path = AppState::canonical_root(source);
        if deletion
            .plan
            .roots
            .iter()
            .any(|root| path.starts_with(AppState::canonical_root(root)))
        {
            return Err(
                "Project deletion-pending root now contains a registered source; cleanup refused"
                    .into(),
            );
        }
    }
    if !store
        .writer()
        .project_deletion_history_unchanged(deletion)
        .map_err(|e| e.to_string())?
        || !inventory_unchanged(&deletion.plan)?
    {
        return Err("Project deletion-pending state changed; restart to abandon the stale plan, then delete again".into());
    }
    Ok(())
}
#[derive(Clone, Copy, Default)]
struct DeletionFailure {
    #[cfg(test)]
    phase: Option<&'static str>,
}
impl DeletionFailure {
    fn capture(app: &AppState) -> Self {
        let _ = app;
        Self {
            #[cfg(test)]
            phase: app.project_deletion_failure,
        }
    }
    fn check(&self, phase: &str) -> Result<(), String> {
        #[cfg(test)]
        if self.phase == Some(phase) {
            return Err(format!("injected project deletion {phase} failure"));
        }
        let _ = phase;
        Ok(())
    }
}

/// Project fences, rather than the generic device-wide filesystem exclusion,
/// protect every deletion stage. Other jobs keep their existing exclusion.
struct ScopedDeletion<T>(T);
impl<T: crate::app::git::deferred::DeferredGitWork> crate::app::git::deferred::DeferredGitWork
    for ScopedDeletion<T>
{
    fn run(&self, params: &Value) -> Result<Value, String> {
        self.0.run(params)
    }
    fn settle(&self, app: &mut AppState, result: Value) -> Result<Value, String> {
        self.0.settle(app, result)
    }
    fn invalidate(&self, app: &mut AppState) {
        self.0.invalidate(app);
    }
    fn invalidates_on_error(&self) -> bool {
        self.0.invalidates_on_error()
    }
    fn blocks_unrelated_filesystem(&self) -> bool {
        false
    }
}
