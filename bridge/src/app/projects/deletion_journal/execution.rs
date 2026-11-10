use super::*;
use crate::app::git::deferred::DeferredGitWork;

pub(super) struct DeleteRecordedProject {
    pub(super) store: Store,
    pub(super) deletion: ProjectDeletion,
    pub(super) retirements: Vec<crate::reaper::Retirement>,
    #[cfg(test)]
    pub(super) failure: Option<&'static str>,
}

impl DeferredGitWork for DeleteRecordedProject {
    fn run(&self, _: &Value) -> Result<Value, String> {
        for retirement in &self.retirements {
            if !retirement.wait(crate::orchestrator::CHECKOUT_REAP_WAIT) {
                return Err(
                    "Project deletion-pending: a process has not exited; retry project.delete"
                        .into(),
                );
            }
        }
        let mut deletion = self.deletion.clone();
        if !inventory_unchanged(&deletion.plan)?
            || !self
                .store
                .writer()
                .project_deletion_history_unchanged(&deletion)
                .map_err(|e| e.to_string())?
        {
            return Err("Project deletion-pending state changed; restart to abandon the stale plan, then delete again".into());
        }
        if deletion.phase == "config_removed" {
            self.fail("checkout")?;
            remove_files(&deletion.plan)?;
            deletion.phase = "files_removed".into();
            self.store
                .writer()
                .checkpoint_project_deletion(&deletion)
                .map_err(|e| e.to_string())?;
        }
        if deletion.phase == "files_removed" {
            self.fail("history")?;
            deletion.phase = "history_removed".into();
            crate::reviews::service::delete_project_history_checkpoint(
                &self.store,
                &deletion.plan.project_key,
                Some(&deletion),
            )?;
        }
        Ok(json!({"project_id":deletion.plan.project_id,"deleted":true}))
    }

    fn invalidates_on_error(&self) -> bool {
        true
    }
    fn invalidate(&self, app: &mut AppState) {
        app.project_deletion_in_progress = false;
        let _ = app.workspaces.reload();
        app.publish_read_metadata();
    }
    fn settle(&self, app: &mut AppState, result: Value) -> Result<Value, String> {
        app.project_deletion_in_progress = false;
        let mut deletion = app
            .deletion_record(&self.deletion.plan.project_id)?
            .ok_or("missing deletion record")?;
        app.finish_recorded_project(&deletion.plan)?;
        deletion.phase = "complete".into();
        deletion.tombstone = Some("deleted".into());
        self.store
            .writer()
            .checkpoint_project_deletion(&deletion)
            .map_err(|e| e.to_string())?;
        app.refresh_deletion_fences()?;
        app.publish_read_metadata();
        if let Err(error) = self.store.writer().collect_project_deletion_tombstones() {
            eprintln!("collect project deletion tombstones: {error}");
        }
        Ok(result)
    }
}

impl DeleteRecordedProject {
    fn fail(&self, phase: &str) -> Result<(), String> {
        #[cfg(test)]
        if self.failure == Some(phase) {
            return Err(format!("injected project deletion {phase} failure"));
        }
        let _ = phase;
        Ok(())
    }
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
    Ok(())
}
