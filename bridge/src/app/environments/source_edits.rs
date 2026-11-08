//! Environment source updates share the project's serialized filesystem mutation.
use crate::app::projects::source_update::{
    checked_source_edit, remote_mutation_paths, LockedSourceUpdate, SerializedSourceUpdate,
    SourceEdit, SourceUpdateScope,
};
use crate::app::projects::{canonical_source_path, ProjectSource};
use crate::app::{expand_tilde, require_str, AppState};
use crate::lifecycle::{PendingState, SourceUpdated, UpdateSource, WorkspaceCheckout};
use serde_json::{json, Value};

struct SourceAddress {
    environment: String,
    source: ProjectSource,
    edit: SourceEdit,
    repositories: Vec<std::path::PathBuf>,
}

impl AppState {
    pub(crate) fn environment_update_source(&mut self, params: &Value) -> Result<Value, String> {
        self.mark_device_lifecycle_dirty()?;
        let id = require_str(params, "environment_id")?;
        let source_id = require_str(params, "source_id")?;
        let source = self.environment_source(&id, &source_id)?.clone();
        let path = params
            .get("path")
            .and_then(Value::as_str)
            .map(|path| self.environment_source_move(&id, &source, path))
            .transpose()?;
        let edit = checked_source_edit(params, &source, path)?;
        if !edit.needs_git() {
            return self.settle_environment_source_edit(
                SourceAddress {
                    environment: id,
                    source: source.clone(),
                    edit,
                    repositories: vec![],
                },
                SourceUpdated {
                    path: source.path,
                    is_git: source.is_git,
                    base_branch: source.base_branch,
                    checkouts_updated: 0,
                    checkouts_failed: vec![],
                },
                vec![],
                vec![],
            );
        }
        let moves = edit.path.as_ref().is_some_and(|path| *path != source.path);
        let projects = if moves {
            self.environment_project_ids(&id)
        } else {
            vec![]
        };
        let mutation = UpdateSource {
            path: source.path.clone(),
            is_git: source.is_git,
            base_branch: source.base_branch.clone(),
            new_path: edit.path.clone(),
            new_base_branch: edit.base_branch.clone(),
            new_remote: edit.remote.clone(),
            workspace_checkouts: self.environment_checkouts_cut_from(&id, &source),
        };
        let repositories = remote_mutation_paths(&mutation);
        let mutation = SerializedSourceUpdate {
            mutation,
            captured: SourceUpdateScope::Environment {
                id: id.clone(),
                source: source.clone(),
            },
            state: self.source_validation_handle(),
            environments: None,
            configuration_probe: None,
        };
        let title = self.projects.environment(&id).unwrap().name.clone();
        let address = SourceAddress {
            environment: id,
            source: source.clone(),
            edit,
            repositories,
        };
        let held = projects.clone();
        let deferred = self.defer_project(
            source.path,
            title,
            PendingState::Updating,
            mutation,
            move |app: &mut AppState, result: Result<LockedSourceUpdate, String>| match result {
                Ok(updated) => {
                    let (updated, locks) = updated.into_parts();
                    app.settle_environment_source_edit(address, updated, locks, held)
                }
                Err(error) => app.observe_failed_environment_edit(address, error, held),
            },
        )?;
        for project in projects {
            self.source_moves.begin(&project);
        }
        Ok(deferred)
    }
    fn environment_source_move(
        &self,
        id: &str,
        source: &ProjectSource,
        path: &str,
    ) -> Result<std::path::PathBuf, String> {
        let candidate = canonical_source_path(&expand_tilde(path.trim()))?;
        self.validate_environment_source_path(id, Some(&source.id), &candidate)?;
        if candidate == source.path {
            return Ok(candidate);
        }
        if self
            .projects
            .environment(id)
            .unwrap()
            .sources
            .first()
            .is_some_and(|s| s.id == source.id)
        {
            return Err("The first folder is the environment's home and cannot move. Add the other folder as a source instead.".into());
        }
        self.refuse_environment_source_move(id, source)?;
        Ok(candidate)
    }
    fn environment_checkouts_cut_from(
        &self,
        id: &str,
        source: &ProjectSource,
    ) -> Vec<WorkspaceCheckout> {
        let projects = self.environment_project_ids(id);
        self.workspaces
            .list(None)
            .into_iter()
            .filter(|w| projects.contains(&w.project_id))
            .flat_map(|w| w.directories.iter().map(move |d| (&w.id, d)))
            .filter(|(_, d)| {
                d.source_id == source.id && d.source_path == source.path && d.path != source.path
            })
            .map(|(id, d)| WorkspaceCheckout {
                workspace_id: id.clone(),
                path: d.path.clone(),
            })
            .collect()
    }
    fn settle_environment_source_edit(
        &mut self,
        address: SourceAddress,
        updated: SourceUpdated,
        locks: Vec<crate::source_sync::SyncLock>,
        held: Vec<String>,
    ) -> Result<Value, String> {
        let validation = (|| {
            self.validate_environment_source_path(
                &address.environment,
                Some(&address.source.id),
                &updated.path,
            )?;
            let current = self.environment_source(&address.environment, &address.source.id)?;
            if current.path != address.source.path {
                return Err("source changed while it was being edited".into());
            }
            Ok::<_, String>(())
        })();
        if let Err(error) = validation {
            return self.observe_failed_environment_edit(address, error, held);
        }
        let moves = address
            .edit
            .path
            .as_ref()
            .is_some_and(|path| *path != address.source.path);
        let check_id = address.environment.clone();
        let check_source = address.source.id.clone();
        let id = address.environment.clone();
        let source_id = address.source.id.clone();
        let sync_base = address.edit.sync_base;
        let checkouts_updated = updated.checkouts_updated;
        let failed = updated.checkouts_failed.iter().map(|f| json!({"workspace_id":f.workspace_id,"path":f.path.display().to_string(),"reason":f.reason})).collect::<Vec<_>>();
        self.commit_environment_configuration_checked(
            &address.environment,
            |projects| {
                let source = projects
                    .environment_mut(&address.environment)
                    .ok_or("unknown environment")?
                    .sources
                    .iter_mut()
                    .find(|s| s.id == address.source.id)
                    .ok_or("source changed while it was being edited")?;
                source.path = updated.path;
                source.is_git = updated.is_git;
                source.base_branch = updated.base_branch;
                if let Some(name) = address.edit.name {
                    source.name = name;
                }
                if let Some(sync) = sync_base {
                    source.sync_base = Some(sync);
                }
                Ok(())
            },
            None,
            address.repositories,
            move |app, result| {
                for project in held {
                    app.source_moves.end(&project);
                }
                let mut row = result?;
                if sync_base == Some(true) {
                    app.request_environment_source_sync(&id, &source_id);
                }
                row["checkouts_updated"] = checkouts_updated.into();
                row["checkouts_failed"] = json!(failed);
                drop(locks);
                Ok(row)
            },
            move |app| {
                if moves {
                    app.refuse_environment_source_move(
                        &check_id,
                        app.environment_source(&check_id, &check_source)?,
                    )?;
                }
                Ok(())
            },
        )
    }
    fn observe_failed_environment_edit(
        &mut self,
        address: SourceAddress,
        error: String,
        held: Vec<String>,
    ) -> Result<Value, String> {
        self.commit_environment_configuration_with(
            &address.environment,
            |_| Ok(()),
            None,
            address.repositories,
            move |app, _| {
                for project in held {
                    app.source_moves.end(&project);
                }
                Err(error)
            },
        )
    }
    pub(crate) fn environment_sync_source(&mut self, params: &Value) -> Result<Value, String> {
        let id = require_str(params, "environment_id")?;
        let source_id = require_str(params, "source_id")?;
        let source = self.environment_source(&id, &source_id)?;
        if !source.is_git {
            return Err(
                "Build cannot sync a folder that is not a Git repository: it has no base branch."
                    .into(),
            );
        }
        if params
            .get("source_path")
            .and_then(Value::as_str)
            .is_some_and(|p| p != source.path.to_string_lossy())
        {
            return Err(crate::app::projects::MOVED_GIT_SOURCE.into());
        }
        self.request_environment_source_sync(&id, &source_id);
        Ok(json!({"pending":true}))
    }
}
