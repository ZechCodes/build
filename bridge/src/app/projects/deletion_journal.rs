//! Recoverable deletion. The record owns every destructive address; current
//! configuration only proves identity and removes the registration.
use crate::app::{AppState, DeferredGit, DeferredWork};
use crate::store::{ProjectDeletion, ProjectDeletionPlan, Store};
use serde_json::{json, Value};
use std::path::PathBuf;

mod execution;
mod fence;
use execution::DeleteRecordedProject;
mod inventory;
mod recovery;
use inventory::{capture_inventory, inventory_unchanged};

impl AppState {
    /// A rollback may re-add a legacy key under a new numeric ID. A fresh
    /// consent may capture its preserved manifests; a retry never does this.
    pub(super) fn project_deletion_workspaces(
        &self,
        id: &str,
    ) -> Result<Vec<&crate::workspace::Workspace>, String> {
        let key = &self.projects.get(id).ok_or("unknown project")?.project_key;
        let prior: Vec<_> = self
            .project_deletion_records()?
            .into_iter()
            .filter(|r| r.tombstone.as_deref() == Some("abandoned") && r.plan.project_key == *key)
            .map(|r| r.plan)
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

    pub(super) fn begin_recorded_project_delete(
        &mut self,
        project_id: &str,
        delete_tasks: bool,
        roots: Vec<PathBuf>,
        checkouts: Vec<(PathBuf, PathBuf)>,
        run_ids: Vec<String>,
        session_roots: Vec<PathBuf>,
    ) -> Result<Value, String> {
        self.deletion_fail("record")?;
        let project = self.projects.get(project_id).ok_or("unknown project")?;
        let config = self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        let plan = ProjectDeletionPlan {
            project_id: project_id.into(),
            project_key: project.project_key.clone(),
            project_config: config["projects"]
                .as_array()
                .expect("project array")
                .iter()
                .find(|p| p["id"] == project_id)
                .cloned()
                .ok_or("project config missing")?,
            environment_config: config["environments"]
                .as_array()
                .expect("environment array")
                .iter()
                .find(|e| e["id"] == project.environment_id)
                .cloned()
                .ok_or("project environment missing")?,
            delete_tasks,
            inventory: capture_inventory(&roots, &self.project_worktrees_root(project_id))?,
            roots,
            checkouts: checkouts
                .into_iter()
                .map(|(source, path)| {
                    Ok(crate::store::ProjectDeletionCheckout {
                        registered_name: crate::isolation::checkout_name(&path)
                            .ok_or("checkout has no registration name")?,
                        registry_root: self
                            .workspaces
                            .list(None)
                            .iter()
                            .find(|w| w.directories.iter().any(|d| d.path == path))
                            .map(|w| self.project_worktrees_root(&w.project_id))
                            .unwrap_or_else(|| self.project_worktrees_root(project_id)),
                        isolation: crate::isolation::Isolation::of(&path)
                            .ok_or("checkout has no isolation owner")?,
                        source,
                        path,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
            rift_root: self.project_worktrees_root(project_id),
            run_ids,
            plan_ids: self
                .plans
                .keys()
                .filter(|id| self.projects.project_id_of(id) == Some(project_id))
                .cloned()
                .collect(),
            capture_ids: self
                .captures
                .values()
                .filter(|c| {
                    c.routing
                        .as_ref()
                        .is_some_and(|r| r.project_id == project_id)
                })
                .map(|c| c.id.clone())
                .collect(),
            workspace_ids: self
                .project_deletion_workspaces(project_id)?
                .iter()
                .map(|w| w.id.clone())
                .collect(),
        };
        let store = self
            .store
            .as_ref()
            .ok_or("project deletion needs a durable store")?
            .clone();
        let deletion = store
            .writer()
            .begin_project_deletion(plan)
            .map_err(|e| e.to_string())?;
        self.refresh_deletion_fences()?;
        self.publish_read_metadata();
        self.resume_recorded_project_delete(deletion, session_roots)
    }

    pub(super) fn deletion_record(&self, id: &str) -> Result<Option<ProjectDeletion>, String> {
        Ok(self
            .project_deletion_records()?
            .into_iter()
            .rev()
            .find(|r| r.plan.project_id == id))
    }

    pub(in crate::app) fn project_deletion_records(&self) -> Result<Vec<ProjectDeletion>, String> {
        self.store
            .as_ref()
            .map(|store| {
                store
                    .writer()
                    .project_deletions()
                    .map_err(|e| e.to_string())
            })
            .unwrap_or_else(|| Ok(vec![]))
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
        if let Some(project) = self.projects.get(id) {
            if project.project_key != deletion.plan.project_key {
                return Err("project deletion identity mismatch; no cleanup was performed".into());
            }
        }
        if deletion.plan.delete_tasks != (params["tasks"] == "delete") {
            return Err("project deletion mode mismatch; retry the same delete".into());
        }
        if deletion.tombstone.is_some() {
            if self.projects.get(id).is_some() {
                return Err("project deletion identity has been registered again".into());
            }
            return Ok(Some(json!({"project_id":id,"deleted":true})));
        }
        self.validate_deletion_plan(&deletion)?;
        self.resume_recorded_project_delete(deletion, vec![])
            .map(Some)
    }

    fn resume_recorded_project_delete(
        &mut self,
        mut deletion: ProjectDeletion,
        session_roots: Vec<PathBuf>,
    ) -> Result<Value, String> {
        self.mark_device_lifecycle_dirty()?;
        let store = self
            .store
            .as_ref()
            .ok_or("project deletion needs a durable store")?
            .clone();
        if deletion.phase == "prepared" {
            self.remove_recorded_registration(&deletion.plan)?;
            deletion.phase = "config_removed".into();
            store
                .writer()
                .checkpoint_project_deletion(&deletion)
                .map_err(|e| e.to_string())?;
        }
        let retirements = self.retire_project_roots(&session_roots);
        self.revoke_project_agents(&deletion.plan.run_ids);
        self.project_deletion_in_progress = true;
        let id = deletion.plan.project_id.clone();
        self.publish_read_metadata();
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(DeleteRecordedProject {
                store,
                deletion,
                retirements,
                #[cfg(test)]
                failure: self.project_deletion_failure,
            }),
            params: json!({}),
            invalidates: true,
            registry_change: true,
            #[cfg(test)]
            gate: None,
        })));
        Ok(json!({"project_id":id,"deleted":true}))
    }

    fn remove_recorded_registration(&mut self, plan: &ProjectDeletionPlan) -> Result<(), String> {
        self.deletion_fail("config")?;
        let mut config =
            self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        config["projects"]
            .as_array_mut()
            .expect("project array")
            .retain(|p| p["id"] != plan.project_id);
        let mut policies = self.compaction_policies.clone();
        policies.forget_project(&plan.project_id);
        policies.forget_workspaces(&plan.workspace_ids);
        policies.write_config(&mut config);
        self.store
            .as_ref()
            .ok_or("missing deletion store")?
            .writer()
            .secrets_retire_project(&plan.project_id)
            .map_err(|e| e.to_string())?;
        self.persist_config(&config)?;
        self.compaction_policies = policies;
        self.retain_environment_snapshot_observation(&plan.project_id);
        self.projects.remove(&plan.project_id);
        self.board.diff_mut().remove_project(&plan.project_id);
        self.workspaces.forget_project(&plan.project_id);
        self.note_board_lists_changed(crate::changes::BoardLists {
            projects: true,
            environments: true,
            workspaces: true,
            usage_limits: false,
        });
        Ok(())
    }

    fn finish_recorded_project(&mut self, plan: &ProjectDeletionPlan) -> Result<(), String> {
        let store = self.store.as_ref().ok_or("missing deletion store")?.clone();
        for id in &plan.run_ids {
            store
                .delete_run_retaining_inbox_messages(id, &plan.project_id)
                .map_err(|e| e.to_string())?;
            self.runs.remove(id);
            self.forget_session_owner(id);
            self.forget_run(id);
        }
        for id in &plan.plan_ids {
            store.delete_plan(id).map_err(|e| e.to_string())?;
            self.plans.remove(id);
            self.committed_entities.forget(id);
            self.projects.unbind_entity(id);
            self.board.attention_mut().remove_entity_clocks(id);
            self.forget_live_roster_entity(id);
        }
        for id in &plan.capture_ids {
            store.delete_capture(id).map_err(|e| e.to_string())?;
            self.captures.remove(id);
            self.publish_capture_snapshot(id);
        }
        store
            .clear_retained_project_messages(&plan.project_id)
            .map_err(|e| e.to_string())?;
        self.forget_session_owner(&plan.project_id);
        for id in &plan.workspace_ids {
            self.workspaces.forget(id);
        }
        Ok(())
    }

    fn validate_deletion_plan(&self, deletion: &ProjectDeletion) -> Result<(), String> {
        for checkout in &deletion.plan.checkouts {
            crate::isolation::WorktreeBackend
                .verify_recorded(&checkout.source, &checkout.path, &checkout.registered_name)
                .map_err(|e| e.to_string())?;
        }
        for source in self.projects.iter().flat_map(|p| p.sources.iter()) {
            let path = Self::canonical_root(&source.path);
            if deletion
                .plan
                .roots
                .iter()
                .any(|root| path.starts_with(Self::canonical_root(root)))
            {
                return Err("Project deletion-pending root now contains a registered source; cleanup refused".into());
            }
        }
        if !self
            .store
            .as_ref()
            .ok_or("missing deletion store")?
            .writer()
            .project_deletion_history_unchanged(deletion)
            .map_err(|e| e.to_string())?
            || !inventory_unchanged(&deletion.plan)?
        {
            return Err("Project deletion-pending state changed; restart to abandon the stale plan, then delete again".into());
        }
        Ok(())
    }

    pub(in crate::app) fn deletion_fail(&self, phase: &str) -> Result<(), String> {
        #[cfg(test)]
        if self.project_deletion_failure == Some(phase) {
            return Err(format!("injected project deletion {phase} failure"));
        }
        let _ = phase;
        Ok(())
    }
}
