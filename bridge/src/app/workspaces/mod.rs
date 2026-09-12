use crate::app::git::deferred::DeferredGitWork;
use crate::app::{require_str, AppState, DeferredGit, DeferredWork};
use crate::isolation::Isolation;
use crate::workspace::{Workspace, WorkspaceDirectory, WorkspaceRegistry, WorkspaceSource};
use crate::worktree::{copy_directory_with_rift_root, WorktreeManager};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

mod git_initialization;

struct WorkspaceCreateWork {
    registry_root: PathBuf,
    rift_root: PathBuf,
    workspace_id: String,
    workspace_name: String,
    sources: Vec<WorkspaceSource>,
    isolation: Isolation,
}

impl DeferredGitWork for WorkspaceCreateWork {
    fn invalidates_on_error(&self) -> bool {
        true
    }

    fn run(&self, _params: &Value) -> Result<Value, String> {
        let mut registry = WorkspaceRegistry::load(&self.registry_root)?;
        let workspace = registry.provision(
            &self.workspace_id,
            &self.sources,
            self.isolation,
            |source, destination, isolation| {
                if source.is_git {
                    let manager = WorktreeManager::new(
                        &source.path,
                        destination.parent().unwrap_or(destination),
                    )
                    .with_rift_registry_root(&self.rift_root);
                    let effective = if manager.availability().lock_reason(isolation).is_some() {
                        Isolation::Worktree
                    } else {
                        isolation
                    };
                    let checkout = manager
                        .create_workspace_checkout(
                            &self.workspace_name,
                            &source.base_branch,
                            destination,
                            effective,
                        )
                        .map_err(|error| error.to_string())?;
                    Ok((Some(checkout.worktree.recorded_branch), effective))
                } else {
                    let resolved = copy_directory_with_rift_root(
                        &source.path,
                        destination,
                        isolation,
                        &self.rift_root,
                    )
                    .map_err(|error| error.to_string())?;
                    Ok((None, resolved.isolation))
                }
            },
        )?;
        Ok(workspace_json(&workspace))
    }

    fn invalidate(&self, app: &mut AppState) {
        if let Err(error) = app.workspaces.reload() {
            eprintln!("reload workspaces after creation: {error}");
        }
        app.workspaces
            .fail_if_still_provisioning(&self.workspace_id);
    }
}

struct WorkspaceFinishWork {
    registry_root: PathBuf,
    workspace: Workspace,
}

impl DeferredGitWork for WorkspaceFinishWork {
    fn run(&self, _params: &Value) -> Result<Value, String> {
        let mut workspace = self.workspace.clone();
        let registry = WorkspaceRegistry::load(&self.registry_root)?;
        let result = registry.finish_existing(&mut workspace)?;
        serde_json::to_value(result).map_err(|error| error.to_string())
    }

    fn invalidate(&self, app: &mut AppState) {
        if let Err(error) = app.workspaces.reload() {
            eprintln!("reload workspaces after finish: {error}");
        }
    }
}

impl AppState {
    pub(in crate::app) fn workspace_list(&mut self, params: &Value) -> Result<Value, String> {
        self.adopt_legacy_workspaces();
        self.workspaces.refresh_local_capabilities();
        self.workspaces.refresh_finished_local();
        let project_id = params.get("project_id").and_then(Value::as_str);
        Ok(json!({
            "workspaces": self.workspaces.list(project_id).into_iter().map(workspace_json).collect::<Vec<_>>()
        }))
    }

    pub(in crate::app) fn workspace_get(&mut self, params: &Value) -> Result<Value, String> {
        self.adopt_legacy_workspaces();
        self.workspaces.refresh_local_capabilities();
        self.workspaces.refresh_finished_local();
        let id = require_str(params, "workspace_id")?;
        let workspace = self
            .workspaces
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("unknown workspace_id: {id}"))?;
        let owner = self.workspace_conversation_owner(&workspace);
        let mut value = workspace_json(&workspace);
        let Some(run_id) = owner else {
            if let Some(agent_id) = params.get("agent_id").and_then(Value::as_str) {
                return Err(format!("unknown agent_id: {agent_id}"));
            }
            value["entity_id"] = Value::Null;
            value["run_id"] = Value::Null;
            value["agents"] = json!([]);
            value["thread"] = Value::Null;
            value["run"] = Value::Null;
            return Ok(value);
        };

        let mut run_params = params.clone();
        run_params["run_id"] = json!(run_id);
        let run = self.run_get(&run_params)?;
        value["entity_id"] = json!(run_id);
        value["run_id"] = json!(run_id);
        value["agents"] = run["agents"].clone();
        value["thread"] = run["thread"].clone();
        value["run"] = run;
        Ok(value)
    }

    /// The existing run that owns this workspace's root. Adopted run
    /// workspaces retain the run id directly; external-worktree workspaces
    /// need the path fallback because their stable workspace id predates the
    /// run that adopted them. Only a live run may win that fallback: a
    /// historical terminal run must never steal a newly adopted checkout's
    /// conversation.
    fn workspace_conversation_owner(&self, workspace: &Workspace) -> Option<String> {
        if self.runs.contains_key(&workspace.id) {
            return Some(workspace.id.clone());
        }
        self.runs
            .iter()
            .find(|(run_id, active)| {
                !active.run.state.is_terminal()
                    && self.projects.project_id_of(run_id) == Some(workspace.project_id.as_str())
                    && same_path(&active.worktree.path, &workspace.root)
            })
            .map(|(run_id, _)| run_id.clone())
    }

    pub(in crate::app) fn workspace_create(&mut self, params: &Value) -> Result<Value, String> {
        if self.deferred_work.is_some() {
            return Err("another filesystem operation is still running".to_string());
        }
        let project_id = require_str(params, "project_id")?;
        let requested_name = params
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or("workspace");
        let sources = self
            .sources_for(&project_id)?
            .into_iter()
            .map(|source| WorkspaceSource {
                id: source.id,
                name: source.name,
                mount: source.mount,
                path: source.path,
                is_git: source.is_git,
                base_branch: source.base_branch,
            })
            .collect::<Vec<_>>();
        if sources.is_empty() {
            return Err("workspace.create: project has no sources".to_string());
        }
        let isolation = params
            .get("isolation")
            .and_then(Value::as_str)
            .map(|value| {
                Isolation::from_wire(value).ok_or_else(|| format!("unknown isolation: {value}"))
            })
            .transpose()?
            .unwrap_or_else(|| {
                self.project(&project_id)
                    .and_then(|project| project.isolation)
                    .unwrap_or(self.isolation)
            });
        let workspace = self.workspaces.begin_with_isolation(
            &project_id,
            requested_name,
            &sources,
            isolation,
        )?;
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(WorkspaceCreateWork {
                registry_root: self.workspaces.root().to_path_buf(),
                rift_root: self.project_worktrees_root(&project_id),
                workspace_id: workspace.id.clone(),
                workspace_name: workspace.name.clone(),
                sources,
                isolation,
            }),
            params: params.clone(),
            invalidates: true,
            #[cfg(test)]
            gate: None,
        })));
        Ok(json!({ "workspace_id": workspace.id, "pending": true }))
    }

    pub(in crate::app) fn workspace_finish(&mut self, params: &Value) -> Result<Value, String> {
        if self.deferred_work.is_some() {
            return Err("another filesystem operation is still running".to_string());
        }
        let workspace_id = require_str(params, "workspace_id")?;
        if self.workspaces.get(&workspace_id).is_none() {
            self.adopt_legacy_workspaces();
        }
        if self.workspaces.get(&workspace_id).is_none() {
            return Err(format!("unknown workspace_id: {workspace_id}"));
        }
        let status = &self
            .workspaces
            .get(&workspace_id)
            .expect("checked above")
            .status;
        if !matches!(
            status,
            crate::workspace::WorkspaceStatus::Ready | crate::workspace::WorkspaceStatus::Finished
        ) {
            return Err(
                "workspace must finish provisioning successfully before it can be finished"
                    .to_string(),
            );
        }
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(WorkspaceFinishWork {
                registry_root: self.workspaces.root().to_path_buf(),
                workspace: self
                    .workspaces
                    .get(&workspace_id)
                    .expect("checked above")
                    .clone(),
            }),
            params: params.clone(),
            invalidates: true,
            #[cfg(test)]
            gate: None,
        })));
        Ok(json!({ "workspace_id": workspace_id, "pending": true }))
    }

    pub(in crate::app) fn workspace_retry(&mut self, params: &Value) -> Result<Value, String> {
        if self.deferred_work.is_some() {
            return Err("another filesystem operation is still running".to_string());
        }
        let workspace_id = require_str(params, "workspace_id")?;
        let workspace = self.workspaces.claim_retry(&workspace_id)?;
        if !workspace.managed {
            return Err("workspace.retry: adopted workspaces require no provisioning".to_string());
        }
        let sources = WorkspaceRegistry::retry_sources(&workspace);
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(WorkspaceCreateWork {
                registry_root: self.workspaces.root().to_path_buf(),
                rift_root: self.project_worktrees_root(&workspace.project_id),
                workspace_id: workspace.id.clone(),
                workspace_name: workspace.name,
                sources,
                isolation: workspace.isolation,
            }),
            params: params.clone(),
            invalidates: true,
            #[cfg(test)]
            gate: None,
        })));
        Ok(json!({ "workspace_id": workspace_id, "pending": true }))
    }

    /// Compatibility entry for the old finish routes. It resolves their
    /// branch/run/worktree identity to the adopted single-directory workspace
    /// and then uses the same non-destructive push completion flow.
    pub(in crate::app) fn workspace_finish_legacy(
        &mut self,
        params: &Value,
    ) -> Result<Value, String> {
        self.adopt_legacy_workspaces();
        if params.get("workspace_id").is_some() {
            return self.workspace_finish(params);
        }
        let direct = params
            .get("run_id")
            .or_else(|| params.get("worktree_id"))
            .and_then(Value::as_str);
        let branch = params.get("branch").and_then(Value::as_str);
        let project_id = params.get("project_id").and_then(Value::as_str);
        let workspace_id = self
            .workspaces
            .list(project_id)
            .into_iter()
            .find(|workspace| {
                direct.is_some_and(|id| {
                    workspace.id == id
                        || workspace
                            .directories
                            .iter()
                            .any(|directory| directory.id == id)
                }) || branch.is_some_and(|branch| {
                    workspace
                        .directories
                        .iter()
                        .any(|directory| directory.branch.as_deref() == Some(branch))
                })
            })
            .map(|workspace| workspace.id.clone())
            .ok_or_else(|| "finish: no matching workspace".to_string())?;
        self.workspace_finish(&json!({ "workspace_id": workspace_id }))
    }

    /// Resolve a directory selected in a workspace. Both ids are stable and
    /// the returned record contains the path and Git capability consumers need.
    pub(crate) fn resolve_workspace_directory(
        &mut self,
        workspace_id: &str,
        source_id: &str,
    ) -> Result<WorkspaceDirectory, String> {
        if self.workspaces.get(workspace_id).is_none() {
            self.adopt_legacy_workspaces();
        }
        let workspace = self
            .workspaces
            .get(workspace_id)
            .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))?;
        workspace
            .directories
            .iter()
            .find(|directory| directory.source_id == source_id || directory.id == source_id)
            .cloned()
            .ok_or_else(|| format!("unknown source_id {source_id} in workspace {workspace_id}"))
    }

    pub(crate) fn resolve_workspace_source(
        &mut self,
        workspace_id: &str,
        source_id: &str,
    ) -> Result<PathBuf, String> {
        let directory = self.resolve_workspace_directory(workspace_id, source_id)?;
        canonical_or_existing(&directory.path)
    }

    pub(crate) fn workspace_root(&mut self, workspace_id: &str) -> Result<PathBuf, String> {
        if self.workspaces.get(workspace_id).is_none() {
            self.adopt_legacy_workspaces();
        }
        let workspace = self
            .workspaces
            .get(workspace_id)
            .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))?;
        canonical_or_existing(&workspace.root)
    }

    pub(crate) fn reopen_workspace(&mut self, workspace_id: &str) -> Result<(), String> {
        self.workspaces.reopen(workspace_id)
    }

    fn adopt_legacy_workspaces(&mut self) {
        let primary = self
            .projects
            .iter()
            .filter(|project| project.sources.len() == 1)
            .map(|project| {
                let source_id = project
                    .sources
                    .first()
                    .map(|source| source.id.clone())
                    .unwrap_or_else(|| "root".to_string());
                (
                    project.id.clone(),
                    project.name.clone(),
                    project.repo_path.clone(),
                    source_id,
                    project.is_git,
                )
            })
            .collect::<Vec<_>>();
        for (project_id, name, path, source_id, is_git) in primary {
            let id = format!("legacy-{project_id}");
            self.workspaces
                .adopt_root(&project_id, id, name, path, source_id, is_git);
        }
        let runs = self
            .runs
            .values()
            .map(|run| {
                let project_id = self
                    .projects
                    .project_id_of(&run.run.id.0)
                    .unwrap_or_default()
                    .to_string();
                let source_id = self
                    .projects
                    .get(&project_id)
                    .and_then(|project| project.sources.first())
                    .map(|source| source.id.clone())
                    .unwrap_or_else(|| "root".to_string());
                (
                    project_id,
                    run.run.id.0.clone(),
                    run.worktree.name.clone(),
                    run.worktree.path.clone(),
                    source_id,
                )
            })
            .collect::<Vec<_>>();
        for (project_id, id, name, path, source_id) in runs {
            self.workspaces
                .adopt_root(&project_id, id, name, path, source_id, true);
        }
        self.adopt_external_git_worktrees();
    }

    /// Read only Git's local worktree registry. This deliberately avoids the
    /// existing discovery scan because that synchronizes refs and computes
    /// diffs; legacy adoption needs identity and paths only.
    fn adopt_external_git_worktrees(&mut self) {
        let projects = self
            .projects
            .iter()
            .filter(|project| project.is_git && project.sources.len() == 1)
            .map(|project| {
                (
                    project.id.clone(),
                    project.repo_path.clone(),
                    project.sources[0].id.clone(),
                    self.project_worktrees_root(&project.id),
                )
            })
            .collect::<Vec<_>>();
        let held_paths = self
            .workspaces
            .list(None)
            .into_iter()
            .flat_map(|workspace| {
                std::iter::once(workspace.root.clone()).chain(
                    workspace
                        .directories
                        .iter()
                        .map(|directory| directory.path.clone()),
                )
            })
            .collect::<Vec<_>>();
        for (project_id, repo_path, source_id, legacy_root) in projects {
            let Ok(repository) = git2::Repository::open(&repo_path) else {
                continue;
            };
            let Ok(names) = repository.worktrees() else {
                continue;
            };
            for name in names.iter().flatten() {
                let Ok(worktree) = repository.find_worktree(name) else {
                    continue;
                };
                let path = worktree.path().to_path_buf();
                if held_paths.iter().any(|held| same_path(held, &path)) {
                    continue;
                }
                let id = crate::worktree::external_worktree_id(&path);
                self.workspaces.adopt_root(
                    &project_id,
                    id,
                    name.to_string(),
                    path,
                    source_id.clone(),
                    true,
                );
            }
            let Ok(entries) = std::fs::read_dir(legacy_root) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if crate::isolation::Isolation::of(&path) != Some(Isolation::Rift)
                    || held_paths.iter().any(|held| same_path(held, &path))
                {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                let id = crate::worktree::external_worktree_id(&path);
                self.workspaces
                    .adopt_root(&project_id, id, name, path, source_id.clone(), true);
            }
        }
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn canonical_or_existing(path: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(path)
        .map_err(|error| format!("resolve workspace root {}: {error}", path.display()))
}

fn workspace_json(workspace: &Workspace) -> Value {
    json!({
        "id": workspace.id,
        "workspace_id": workspace.id,
        "project_id": workspace.project_id,
        "name": workspace.name,
        "root": workspace.root.display().to_string(),
        "status": workspace.status,
        "directories": workspace.directories.iter().map(|directory| json!({
            "id": directory.id,
            "source_id": directory.source_id,
            "name": directory.name,
            "path": directory.path.display().to_string(),
            "is_git": directory.is_git,
            "branch": directory.branch,
            "isolation": directory.effective_isolation,
            "status": directory.status,
            "error": directory.error,
        })).collect::<Vec<_>>()
    })
}
