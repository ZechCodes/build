//! Config IO, cleanup, SQL and pin release all run off the app mutex.
use super::*;
use crate::app::git::deferred::DeferredGitWork;
use std::sync::Mutex;

#[derive(Default)]
struct CompletedDeletion {
    records: Option<Vec<ProjectDeletion>>,
    finished: bool,
    result: Option<Result<Value, String>>,
}
pub(super) struct DeleteRecordedProject {
    store: Store,
    deletion: ProjectDeletion,
    operation: DeletionOperation,
    sources: Vec<PathBuf>,
    failure: DeletionFailure,
    completed: Mutex<CompletedDeletion>,
    #[cfg(test)]
    cleanup_gate: Option<crate::app::OffLockGate>,
}
impl DeleteRecordedProject {
    pub(super) fn capture(
        app: &AppState,
        deletion: ProjectDeletion,
        operation: DeletionOperation,
    ) -> Result<Self, String> {
        Ok(Self {
            store: app.store.clone().ok_or("missing deletion store")?,
            deletion,
            operation,
            sources: app.deletion_sources(),
            failure: DeletionFailure::capture(app),
            completed: Mutex::new(CompletedDeletion::default()),
            #[cfg(test)]
            cleanup_gate: app.deletion_cleanup_gate.clone(),
        })
    }
    fn execute(&self) -> Result<Value, String> {
        let mut deletion = self.deletion.clone();
        validate_plan(&self.store, &deletion, &self.sources)?;
        self.store
            .writer()
            .mark_sync_dirty("registry", "lifecycle", crate::store::sync::now_ms())
            .map_err(|e| e.to_string())?;
        if deletion.phase == "prepared" {
            // The config write settled before this job was queued. Checkpoint
            // off-lock before any owned file or history cleanup.
            deletion.phase = "config_removed".into();
            self.checkpoint(&deletion)?;
        }
        if deletion.phase == "config_removed" {
            #[cfg(test)]
            if let Some(gate) = &self.cleanup_gate {
                gate.arrive();
            }
            self.failure.check("checkout")?;
            remove_files(&deletion.plan)?;
            deletion.phase = "files_removed".into();
            self.checkpoint(&deletion)?;
        }
        if deletion.phase == "files_removed" {
            self.failure.check("history")?;
            deletion.phase = "history_removed".into();
            crate::reviews::service::delete_project_history_checkpoint(
                &self.store,
                &deletion.plan.project_key,
                Some(&deletion),
            )?;
            deletion = self
                .store
                .writer()
                .project_deletions()
                .map_err(|e| e.to_string())?
                .into_iter()
                .find(|r| r.command_id == deletion.command_id)
                .ok_or("missing deletion history receipt")?;
        }
        if deletion.phase == "history_removed" {
            self.failure.check("pins")?;
            crate::reviews::history_pins::release(&deletion.pending_pins)?;
            deletion.pending_pins.clear();
            deletion.phase = "pins_released".into();
            self.checkpoint(&deletion)?;
        }
        finish_stored_project(&self.store, &deletion.plan)?;
        deletion.phase = "complete".into();
        deletion.tombstone = Some("deleted".into());
        self.checkpoint(&deletion)?;
        self.completed.lock().unwrap().finished = true;
        if let Err(error) = self.store.writer().collect_project_deletion_tombstones() {
            eprintln!("collect project deletion tombstones: {error}");
        }
        Ok(json!({"project_id":deletion.plan.project_id,"deleted":true}))
    }
    fn checkpoint(&self, deletion: &ProjectDeletion) -> Result<(), String> {
        self.store
            .writer()
            .checkpoint_project_deletion(deletion)
            .map_err(|e| e.to_string())
    }
}
impl DeferredGitWork for DeleteRecordedProject {
    fn run(&self, _: &Value) -> Result<Value, String> {
        let result = self.execute();
        let records = self
            .store
            .writer()
            .project_deletions()
            .map_err(|e| e.to_string());
        let mut completed = self.completed.lock().unwrap();
        match records {
            Ok(records) => {
                completed.records = Some(records);
                completed.result = Some(result);
            }
            Err(error) => completed.result = Some(Err(error)),
        }
        // Even a refused cleanup must publish its landed phases and reconcile
        // the registry off-lock before returning the original error.
        Ok(Value::Null)
    }
    fn settle(&self, app: &mut AppState, _: Value) -> Result<Value, String> {
        self.operation.check(app)?;
        let completed = self.completed.lock().unwrap();
        if self.deletion.phase != "prepared" {
            app.apply_removed_registration(&self.deletion.plan);
        }
        if completed.finished {
            app.apply_finished_project(&self.deletion.plan);
        }
        if let Some(records) = &completed.records {
            app.publish_deletion_records(&self.operation.id, records.clone());
        }
        app.device_lifecycle_dirty.set(true);
        let result = completed.result.clone().ok_or("missing deletion outcome")?;
        app.queue_deletion_reconciliation(self.store.clone(), self.operation.clone(), result)?;
        Ok(Value::Null)
    }
    fn invalidates_on_error(&self) -> bool {
        true
    }
    fn invalidate(&self, app: &mut AppState) {
        if self.operation.check(app).is_err() {
            return;
        }
        if app.deferred_work.is_none() {
            self.operation.finish(app);
        }
    }
}

fn finish_stored_project(store: &Store, plan: &ProjectDeletionPlan) -> Result<(), String> {
    for id in &plan.run_ids {
        store
            .delete_run_retaining_inbox_messages(id, &plan.project_id)
            .map_err(|e| e.to_string())?;
    }
    for id in &plan.plan_ids {
        store.delete_plan(id).map_err(|e| e.to_string())?;
    }
    for id in &plan.capture_ids {
        store.delete_capture(id).map_err(|e| e.to_string())?;
    }
    store
        .clear_retained_project_messages(&plan.project_id)
        .map_err(|e| e.to_string())
}
fn remove_files(plan: &ProjectDeletionPlan) -> Result<(), String> {
    for checkout in &plan.checkouts {
        let path = &checkout.path;
        crate::worktree::WorktreeManager::new(&checkout.source, path.parent().unwrap_or(path))
            .with_rift_registry_root(&checkout.registry_root)
            .remove_recorded_checkout(path, &checkout.registered_name, checkout.isolation)
            .map_err(|e| {
                format!(
                    "Project deletion-pending: workspace cleanup {}: {e}; retry project.delete",
                    path.display()
                )
            })?;
    }
    for root in &plan.roots {
        match std::fs::remove_dir_all(root) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(format!(
                    "Project deletion-pending: workspace cleanup {}: {e}; retry project.delete",
                    root.display()
                ))
            }
        }
    }
    remove_lock_markers(plan)
}
fn remove_lock_markers(plan: &ProjectDeletionPlan) -> Result<(), String> {
    for path in &plan.lock_markers {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                "Project deletion-pending: lock marker cleanup {}: {error}; retry project.delete",
                path.display()
            ))
            }
        }
    }
    Ok(())
}
