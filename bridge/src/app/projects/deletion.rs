use crate::app::git::deferred::DeferredGitWork;
use crate::app::{require_str, AppState, DeferredGit, DeferredWork};
use serde_json::{json, Value};
use std::path::PathBuf;

struct DeleteProjectFiles {
    project_id: String,
    roots: Vec<PathBuf>,
    rift_root: PathBuf,
    retirements: Vec<crate::reaper::Retirement>,
    checkouts: Vec<(PathBuf, PathBuf)>,
}

impl DeferredGitWork for DeleteProjectFiles {
    fn run(&self, _: &Value) -> Result<Value, String> {
        for retirement in &self.retirements {
            if !retirement.wait(crate::orchestrator::CHECKOUT_REAP_WAIT) {
                return Err("Workspace cleanup stopped because a process did not exit; workspace files were preserved. Retry project deletion".into());
            }
        }
        for (source, path) in &self.checkouts {
            if path.exists() {
                crate::worktree::WorktreeManager::new(source, path.parent().unwrap_or(path))
                    .with_rift_registry_root(&self.rift_root)
                    .remove_checkout(path)
                    .map_err(|error| {
                        format!(
                            "Workspace cleanup failed at {}: {error}. Retry project deletion",
                            path.display()
                        )
                    })?;
            }
        }
        for root in &self.roots {
            match std::fs::remove_dir_all(root) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "Workspace cleanup failed at {}: {error}. Retry project deletion",
                        root.display()
                    ))
                }
            }
        }
        Ok(json!({"project_id": self.project_id, "deleted": true}))
    }

    fn invalidates_on_error(&self) -> bool {
        true
    }

    fn invalidate(&self, app: &mut AppState) {
        app.project_deletion_in_progress = false;
        if let Err(error) = app.workspaces.reload() {
            eprintln!("reload deleted workspaces: {error}");
        }
    }

    fn settle(&self, app: &mut AppState, result: Value) -> Result<Value, String> {
        app.complete_project_delete(&self.project_id)?;
        Ok(result)
    }
}

impl AppState {
    pub(crate) fn project_delete(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        if params.get("confirm").and_then(Value::as_bool) != Some(true) {
            return Err("project.delete requires confirm: true".into());
        }
        if self.deferred_work.is_some() || self.active_deferred_filesystem_jobs > 0 {
            return Err("another filesystem operation is still running".into());
        }
        let project = self
            .projects
            .get(&project_id)
            .ok_or_else(|| format!("unknown project: {project_id}"))?;
        let mut source_paths: Vec<_> = project
            .sources
            .iter()
            .map(|source| source.path.clone())
            .collect();
        source_paths.push(project.repo_path.clone());
        let workspaces = self.workspaces.list(Some(&project_id));
        if workspaces
            .iter()
            .any(|workspace| workspace.status == crate::workspace::WorkspaceStatus::Provisioning)
        {
            return Err(
                "Wait for workspace provisioning to finish before deleting the project".into(),
            );
        }
        let run_ids: Vec<_> = self
            .runs
            .keys()
            .filter(|id| self.projects.project_id_of(id) == Some(project_id.as_str()))
            .cloned()
            .collect();
        let mut session_roots: Vec<_> = workspaces
            .iter()
            .map(|workspace| Self::canonical_root(&workspace.root))
            .collect();
        session_roots.extend(source_paths.iter().map(|path| Self::canonical_root(path)));
        session_roots.extend(
            run_ids
                .iter()
                .map(|id| Self::canonical_root(&self.runs[id].worktree.path)),
        );
        if session_roots
            .iter()
            .any(|root| self.delivery_queue.has_in_flight_at_root(root))
        {
            return Err("Stop running agents before deleting the project".into());
        }
        let mut roots: Vec<_> = workspaces
            .iter()
            .filter(|workspace| workspace.managed)
            .map(|workspace| workspace.root.clone())
            .collect();
        roots.extend(run_ids.iter().filter_map(|id| {
            let run = &self.runs[id];
            (!run.adopted
                && run
                    .worktree
                    .path
                    .starts_with(self.project_worktrees_root(&project_id)))
            .then(|| run.worktree.path.clone())
        }));
        roots.sort();
        roots.dedup();
        validate_deletion_paths(self, &roots, &workspaces)?;
        let mut checkouts: Vec<_> = workspaces
            .iter()
            .filter(|workspace| workspace.managed)
            .flat_map(|workspace| workspace.directories.iter())
            .filter(|directory| {
                directory.is_git && crate::isolation::Isolation::of(&directory.path).is_some()
            })
            .map(|directory| (directory.source_path.clone(), directory.path.clone()))
            .collect();
        checkouts.extend(run_ids.iter().filter_map(|id| {
            let path = &self.runs[id].worktree.path;
            (roots.contains(path) && crate::isolation::Isolation::of(path).is_some()).then(|| {
                (
                    source_paths.last().expect("primary source").clone(),
                    path.clone(),
                )
            })
        }));
        // Fail before touching files if the configuration is not writable.
        self.preserve_project_task_identities(&project_id)?;
        let config = self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        self.persist_config(&config)?;
        let mut retirements = Vec::new();
        for root in &session_roots {
            retirements.extend(self.retire_workspace_agents(root));
            let keys: Vec<_> = self
                .session_registry
                .tab_keys()
                .into_iter()
                .filter(|key| key.root == *root)
                .collect();
            for key in keys {
                if let Some(retirement) = self.retire_tab(&key, "closed") {
                    retirements.push(retirement);
                }
            }
        }
        self.project_deletion_in_progress = true;
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(DeleteProjectFiles {
                project_id: project_id.clone(),
                roots,
                rift_root: self.project_worktrees_root(&project_id),
                retirements,
                checkouts,
            }),
            params: params.clone(),
            invalidates: true,
            #[cfg(test)]
            gate: None,
        })));
        Ok(json!({"project_id": project_id, "deleted": true}))
    }

    fn complete_project_delete(&mut self, project_id: &str) -> Result<(), String> {
        self.projects.get(project_id).ok_or("unknown project")?;
        let run_ids: Vec<_> = self
            .runs
            .keys()
            .filter(|id| self.projects.project_id_of(id) == Some(project_id))
            .cloned()
            .collect();
        let plan_ids: Vec<_> = self
            .plans
            .keys()
            .filter(|id| self.projects.project_id_of(id) == Some(project_id))
            .cloned()
            .collect();
        for id in run_ids {
            if let Some(store) = &self.store {
                store
                    .delete_run_retaining_inbox_messages(&id, project_id)
                    .map_err(|error| {
                        format!("delete project run: {error}; retry project deletion")
                    })?;
            }
            self.runs.remove(&id);
            self.forget_session_owner(&id);
            self.forget_run(&id);
        }
        for id in plan_ids {
            if let Some(store) = &self.store {
                store.delete_plan(&id).map_err(|error| {
                    format!("delete project task: {error}; retry project deletion")
                })?;
            }
            self.plans.remove(&id);
            self.projects.unbind_entity(&id);
            self.board.attention_mut().remove_entity_clocks(&id);
            self.forget_live_roster_entity(&id);
        }
        let capture_ids: Vec<_> = self
            .captures
            .values()
            .filter(|capture| {
                capture
                    .routing
                    .as_ref()
                    .is_some_and(|routing| routing.project_id == project_id)
            })
            .map(|capture| capture.id.clone())
            .collect();
        for id in capture_ids {
            if let Some(store) = &self.store {
                store.delete_capture(&id).map_err(|error| {
                    format!("delete project capture: {error}; retry project deletion")
                })?;
            }
            self.captures.remove(&id);
        }
        let mut config =
            self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        config["projects"]
            .as_array_mut()
            .expect("project config array")
            .retain(|entry| entry["id"].as_str() != Some(project_id));
        self.persist_config(&config)?;
        if let Some(store) = &self.store {
            store
                .clear_retained_project_messages(project_id)
                .map_err(|error| {
                    format!("delete project history: {error}; retry project deletion")
                })?;
        }
        self.forget_session_owner(project_id);
        self.workspaces.forget_project(project_id);
        self.projects.remove(project_id);
        self.board.diff_mut().remove_project(project_id);
        // Both lists: the project is gone from one, and every workspace that
        // stood in it from the other.
        self.note_board_lists_changed(crate::changes::BoardLists {
            projects: true,
            workspaces: true,
            usage_limits: false,
        });
        Ok(())
    }
}

fn validate_deletion_paths(
    app: &AppState,
    roots: &[PathBuf],
    workspaces: &[&crate::workspace::Workspace],
) -> Result<(), String> {
    let protected_sources: Vec<_> = app
        .projects
        .iter()
        .flat_map(|project| {
            std::iter::once(&project.repo_path)
                .chain(project.sources.iter().map(|source| &source.path))
        })
        .map(|path| AppState::canonical_root(path))
        .collect();
    for root in roots {
        let root = AppState::canonical_root(root);
        if protected_sources
            .iter()
            .any(|source| source.starts_with(&root))
        {
            return Err("Cannot delete a workspace containing a source repository".into());
        }
    }
    for workspace in workspaces.iter().filter(|workspace| workspace.managed) {
        let root = AppState::canonical_root(&workspace.root);
        if workspace
            .directories
            .iter()
            .any(|directory| !AppState::canonical_root(&directory.path).starts_with(&root))
        {
            return Err(
                "Cannot delete a workspace whose source checkout resolves outside its managed root"
                    .into(),
            );
        }
    }
    Ok(())
}
