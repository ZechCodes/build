use crate::app::git::deferred::DeferredGitWork;
use crate::app::{model_choice_from, require_str, AppState, DeferredGit, DeferredWork};
use crate::isolation::{remove_directory_with_rift_root, Isolation};
use crate::workspace::{Workspace, WorkspaceDirectory, WorkspaceRegistry, WorkspaceSource};
use crate::worktree::{copy_directory_with_rift_root, Worktree, WorktreeManager};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

mod deletion;
mod directories;
mod git_initialization;

struct WorkspaceCreateWork {
    registry_root: PathBuf,
    rift_root: PathBuf,
    workspace_id: String,
    workspace_name: String,
    workspace: Workspace,
    unpublished: bool,
    sources: Vec<WorkspaceSource>,
    isolation: Isolation,
}

impl DeferredGitWork for WorkspaceCreateWork {
    fn invalidates_on_error(&self) -> bool {
        true
    }

    fn run(&self, _params: &Value) -> Result<Value, String> {
        let mut registry = match WorkspaceRegistry::load(&self.registry_root) {
            Ok(registry) => registry,
            Err(error) => {
                if !self.unpublished {
                    return Err(error);
                }
                let cleanup = std::fs::remove_dir_all(&self.workspace.root);
                return match cleanup {
                    Ok(()) => Err(error),
                    Err(cleanup_error) if cleanup_error.kind() == std::io::ErrorKind::NotFound => {
                        Err(error)
                    }
                    Err(cleanup_error) => Err(format!(
                        "{error}; additionally could not remove {}: {cleanup_error}",
                        self.workspace.root.display()
                    )),
                };
            }
        };
        if !self.unpublished {
            let workspace = registry.provision(
                &self.workspace_id,
                &self.sources,
                self.isolation,
                |source, destination, isolation| self.materialize(source, destination, isolation),
            )?;
            return Ok(workspace_json(&workspace));
        }
        let result = registry.provision_unpublished(
            self.workspace.clone(),
            &self.sources,
            self.isolation,
            |source, destination, isolation| self.materialize(source, destination, isolation),
        );
        let workspace = match result {
            Ok(workspace) => workspace,
            Err(failure) => {
                let (error, workspace) = *failure;
                let cleanup =
                    self.rollback(&workspace).and_then(|_| {
                        match std::fs::remove_dir_all(&workspace.root) {
                            Ok(()) => Ok(()),
                            Err(remove_error)
                                if remove_error.kind() == std::io::ErrorKind::NotFound =>
                            {
                                Ok(())
                            }
                            Err(remove_error) => Err(format!(
                                "remove incomplete workspace {}: {remove_error}",
                                workspace.root.display()
                            )),
                        }
                    });
                return match cleanup {
                    Ok(()) => Err(error),
                    Err(cleanup_error) => Err(format!(
                        "{error}; additionally could not fully roll back workspace creation: {cleanup_error}"
                    )),
                };
            }
        };
        Ok(workspace_json(&workspace))
    }

    fn invalidate(&self, app: &mut AppState) {
        if let Err(error) = app.workspaces.reload() {
            eprintln!("reload workspaces after creation: {error}");
        }
        if !self.unpublished {
            app.workspaces
                .fail_if_still_provisioning(&self.workspace_id);
        }
    }
}

/// Make one directory from one source: a checkout of the repository on a
/// branch of the workspace's own, cut from the source's base branch, or a copy
/// of the folder. The one piece of per-source work workspace creation does, so
/// a directory added to a live workspace lands the way the ones cut with it
/// did. The isolation answered is what the volume actually gave, which is not
/// always what was asked for.
pub(super) fn materialize_source(
    source: &WorkspaceSource,
    destination: &Path,
    workspace_name: &str,
    isolation: Isolation,
    rift_root: &Path,
) -> Result<(Option<String>, Isolation), String> {
    if source.is_git {
        let manager =
            WorktreeManager::new(&source.path, destination.parent().unwrap_or(destination))
                .with_rift_registry_root(rift_root);
        let effective = if manager.availability().lock_reason(isolation).is_some() {
            Isolation::Worktree
        } else {
            isolation
        };
        let checkout = manager
            .create_workspace_checkout(workspace_name, &source.base_branch, destination, effective)
            .map_err(|error| error.to_string())?;
        Ok((Some(checkout.worktree.recorded_branch), effective))
    } else {
        let resolved =
            copy_directory_with_rift_root(&source.path, destination, isolation, rift_root)
                .map_err(|error| error.to_string())?;
        Ok((None, resolved.isolation))
    }
}

impl WorkspaceCreateWork {
    fn materialize(
        &self,
        source: &WorkspaceSource,
        destination: &Path,
        isolation: Isolation,
    ) -> Result<(Option<String>, Isolation), String> {
        materialize_source(
            source,
            destination,
            &self.workspace_name,
            isolation,
            &self.rift_root,
        )
    }

    fn rollback(&self, workspace: &Workspace) -> Result<(), String> {
        let mut failures = Vec::new();
        for directory in workspace.directories.iter().rev() {
            if directory.status != crate::workspace::DirectoryStatus::Ready {
                continue;
            }
            let Some(source) = self
                .sources
                .iter()
                .find(|source| source.id == directory.source_id)
            else {
                continue;
            };
            let removed = if let Some(branch) = directory.branch.as_ref() {
                let manager = WorktreeManager::new(
                    &source.path,
                    directory.path.parent().unwrap_or(&directory.path),
                )
                .with_rift_registry_root(&self.rift_root);
                manager.remove(&Worktree {
                    name: directory
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    path: directory.path.clone(),
                    recorded_branch: branch.clone(),
                    base_branch: directory.base_branch.clone(),
                })
            } else {
                remove_directory_with_rift_root(
                    &source.path,
                    &directory.path,
                    directory.effective_isolation.unwrap_or(Isolation::Worktree),
                    &self.rift_root,
                )
            };
            if let Err(error) = removed {
                failures.push(error.to_string());
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }
}

impl AppState {
    /// Ensure this exact workspace root has an entity for conversations and
    /// agents. Unlike `run.adopt`, this never chooses a source checkout or
    /// writes Git metadata; multi-source and ordinary-directory workspaces are
    /// represented by their container root.
    pub(crate) fn workspace_ensure_conversation(
        &mut self,
        params: &Value,
    ) -> Result<Value, String> {
        let workspace_id = require_str(params, "workspace_id")?;
        let workspace = self
            .workspaces
            .get(&workspace_id)
            .cloned()
            .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))?;
        if workspace.status != crate::workspace::WorkspaceStatus::Ready {
            return Err(format!(
                "workspace.ensure_conversation: workspace is {:?}",
                workspace.status
            )
            .to_lowercase());
        }
        if let Some(run_id) = self.workspace_conversation_owner(&workspace) {
            return Ok(json!({
                "workspace_id": workspace_id,
                "entity_id": run_id,
                "run_id": run_id,
            }));
        }

        let model_choice = model_choice_from(params, self.default_harness)?;
        let run_id = format!("run-{}", uuid::Uuid::new_v4());
        let active = crate::orchestrator::ActiveRun::workspace_conversation(
            crate::run::RunId::new(&run_id),
            workspace.name,
            workspace.root,
            model_choice,
        );
        self.projects
            .bind_entity(run_id.clone(), workspace.project_id);
        if let Err(error) = self.finish_run_mutation(run_id.clone(), active) {
            self.runs.remove(&run_id);
            self.forget_run(&run_id);
            return Err(error);
        }
        Ok(json!({
            "workspace_id": workspace_id,
            "entity_id": run_id,
            "run_id": run_id,
        }))
    }

    pub(crate) fn workspace_list(&mut self, params: &Value) -> Result<Value, String> {
        self.adopt_legacy_workspaces();
        self.workspaces.refresh_local_capabilities();
        self.workspaces.refresh_finished_local();
        let project_id = params.get("project_id").and_then(Value::as_str);
        Ok(json!({
            "workspaces": self
                .workspaces
                .list(project_id)
                .into_iter()
                .filter(|workspace| !self.is_projects_own_checkout(workspace))
                .map(|workspace| {
                    // The conversation entity, as `workspace.get` names it: a
                    // client standing on the list files the workspace's git
                    // under it without asking for the row.
                    let owner = self.workspace_conversation_owner(workspace);
                    let mut value = workspace_json(workspace);
                    value["entity_id"] = owner.map(Value::String).unwrap_or(Value::Null);
                    value
                })
                .collect::<Vec<_>>()
        }))
    }

    /// Whether this workspace stands on the project's own checkout — the
    /// repository the project was registered at, which every workspace is cut
    /// FROM. It is adopted like any other checkout so the verbs that need a
    /// root (terminals, git init) can still name it, but it is never listed:
    /// a template is not a place to work, and nothing in the client opens it.
    /// The same root under a run's id (a primary adoption) is the same answer.
    fn is_projects_own_checkout(&self, workspace: &Workspace) -> bool {
        if workspace.managed {
            return false;
        }
        self.projects
            .get(&workspace.project_id)
            .is_some_and(|project| same_path(&project.repo_path, &workspace.root))
    }

    pub(crate) fn workspace_get(&mut self, params: &Value) -> Result<Value, String> {
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
    pub(in crate::app) fn workspace_conversation_owner(
        &self,
        workspace: &Workspace,
    ) -> Option<String> {
        if self.runs.contains_key(&workspace.id)
            && !self.is_project_conversation_owner(&workspace.id)
        {
            return Some(workspace.id.clone());
        }
        self.runs
            .iter()
            .find(|(run_id, active)| {
                !active.run.state.is_terminal()
                    && !self.is_project_conversation_owner(run_id)
                    && self.projects.project_id_of(run_id) == Some(workspace.project_id.as_str())
                    && same_path(&active.worktree.path, &workspace.root)
            })
            .map(|(run_id, _)| run_id.clone())
    }

    /// Where a run's git is read: its checkout, unless the run is a workspace
    /// conversation. That run stands on the workspace root, which is a folder of
    /// sources and no repository, so its git is the workspace's git directory —
    /// the first one, where a workspace has several. Every run-scoped read and
    /// the facts a push carries for the run answer from there, which is the
    /// entity the client files the workspace's git under.
    pub(in crate::app) fn run_git_root(&self, run_id: &str, checkout: &Path) -> PathBuf {
        if checkout.join(".git").exists() {
            return checkout.to_path_buf();
        }
        self.workspaces
            .list(None)
            .into_iter()
            .find(|workspace| {
                self.workspace_conversation_owner(workspace).as_deref() == Some(run_id)
            })
            .and_then(|workspace| {
                workspace
                    .directories
                    .iter()
                    .find(|directory| directory.is_git)
                    .map(|directory| directory.path.clone())
            })
            .unwrap_or_else(|| checkout.to_path_buf())
    }

    /// The base a run's git is measured against: its checkout's own, or — for a
    /// workspace conversation, whose checkout names none — the base branch of
    /// the project the workspace was cut from.
    pub(in crate::app) fn run_base_branch(&self, run_id: &str, own: &str) -> String {
        if !own.is_empty() {
            return own.to_string();
        }
        self.projects
            .project_id_of(run_id)
            .and_then(|project_id| self.base_for(project_id).ok())
            .unwrap_or_default()
    }

    pub(crate) fn workspace_create(&mut self, params: &Value) -> Result<Value, String> {
        if self.deferred_work.is_some() {
            return Err("another filesystem operation is still running".to_string());
        }
        let project_id = require_str(params, "project_id")?;
        let requested_name = params
            .get("name")
            .and_then(Value::as_str)
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
        let workspace = self.workspaces.prepare_with_isolation(
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
                workspace_name: workspace
                    .root
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                workspace: workspace.clone(),
                unpublished: true,
                sources,
                isolation,
            }),
            params: params.clone(),
            invalidates: true,
            #[cfg(test)]
            gate: None,
        })));
        // The record exists from here, provisioning, and the list names it.
        self.note_board_lists_changed(crate::changes::BoardLists::WORKSPACES);
        Ok(json!({ "workspace_id": workspace.id, "pending": true }))
    }

    /// Whether an agent is working in this workspace right now: a turn the
    /// bridge is carrying for it, or a live session at that root still saying
    /// it is working. The row and the verb read the same fact, so what Done
    /// offers and what Done does cannot disagree.
    pub(in crate::app) fn agent_working_at_root(&self, root: &Path) -> bool {
        self.delivery_queue.has_in_flight_at_root(root)
            || self
                .session_registry
                .agent_working_roots()
                .into_iter()
                .any(|(candidate, working)| working && Self::canonical_root(&candidate) == root)
    }

    /// Done: the workspace's work is somewhere else, so the workspace goes.
    ///
    /// Eligibility is re-measured here under the app mutex rather than trusted
    /// from the row the click came from, every agent and terminal standing in
    /// the workspace is closed, and the removal itself is the one
    /// `workspace.delete` uses — unregistering checkouts and walking a root
    /// away are both unbounded, and neither may hold the mutex. The record of
    /// what was finished is written on the way out; the live record and the
    /// files do not come back. Recovery is pulling the remote.
    pub(crate) fn workspace_finish(&mut self, params: &Value) -> Result<Value, String> {
        let workspace = self.workspace_to_remove(params)?;
        if workspace.status != crate::workspace::WorkspaceStatus::Ready {
            return Err(
                "workspace must finish provisioning successfully before it can be finished"
                    .to_string(),
            );
        }
        self.refuse_removing_what_is_not_builds(&workspace)?;
        let blockers = self.workspace_finish_blockers(&workspace);
        if !blockers.is_empty() {
            return Err(crate::workspace::finish_refusal(&blockers));
        }
        let registry_root = self.workspaces.root().to_path_buf();
        self.remove_workspace(&workspace, params, Some(registry_root));
        Ok(json!({ "workspace_id": workspace.id, "pending": true }))
    }

    /// What stands between this workspace and Done right now: an agent still
    /// working at its root, and whatever its Git directories have not put
    /// anywhere else. The same list the feed's row carries, measured again
    /// rather than read out of the last poll's cache.
    pub(in crate::app) fn workspace_finish_blockers(
        &self,
        workspace: &Workspace,
    ) -> Vec<&'static str> {
        let mut blockers = Vec::new();
        if self.agent_working_at_root(&Self::canonical_root(&workspace.root)) {
            blockers.push(crate::workspace::FINISH_BLOCKER_AGENT_WORKING);
        }
        blockers.extend(crate::workspace::workspace_directory_blockers(workspace));
        blockers
    }

    /// Rename a workspace's human-facing name.
    ///
    /// Nothing on disk moves: the root directory and the branches inside it are
    /// what terminals, worktree registrations and running agents already hold,
    /// so the only thing this touches is the label a reader sees. Answers the
    /// same detail `workspace.get` does, so the caller that renamed repaints
    /// from one read rather than guessing what the record now says.
    ///
    /// It still waits for filesystem work in flight, as every other change to a
    /// workspace does. The manifest is the one record: a job that was handed a
    /// workspace before the rename writes that workspace back when it lands,
    /// and the new name would be gone from memory and disk with no error
    /// anywhere.
    pub(crate) fn workspace_rename(&mut self, params: &Value) -> Result<Value, String> {
        let workspace_id = require_str(params, "workspace_id")?;
        let name = require_str(params, "name")?;
        if self.deferred_work.is_some() || self.active_deferred_filesystem_jobs > 0 {
            return Err("another filesystem operation is still running".to_string());
        }
        if self.workspaces.get(&workspace_id).is_none() {
            self.adopt_legacy_workspaces();
        }
        self.workspaces.rename(&workspace_id, &name)?;
        // The feed names workspaces, so every browser standing in one is
        // showing the name that just changed — and so is the list.
        self.note_board_lists_changed(crate::changes::BoardLists::WORKSPACES);
        self.workspace_get(&json!({ "workspace_id": workspace_id }))
    }

    pub(crate) fn workspace_retry(&mut self, params: &Value) -> Result<Value, String> {
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
                workspace_name: workspace
                    .root
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                workspace: workspace.clone(),
                unpublished: false,
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
    pub(crate) fn workspace_finish_legacy(&mut self, params: &Value) -> Result<Value, String> {
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
        let owned_root = direct.and_then(|run_id| {
            self.runs.get(run_id).map(|run| {
                (
                    self.projects
                        .project_id_of(run_id)
                        .unwrap_or_default()
                        .to_string(),
                    run.worktree.path.clone(),
                )
            })
        });
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
                }) || owned_root.as_ref().is_some_and(|(owner_project_id, root)| {
                    workspace.project_id == *owner_project_id && same_path(&workspace.root, root)
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
        // A durable workspace may use a run-shaped entity solely as its
        // conversation owner. Such owners deliberately have no Git base branch;
        // their root is already represented by the manifest, so do not also
        // import them as legacy one-directory workspaces. Real Git adoptions
        // retain their base branch and their run-id workspace compatibility.
        //
        // A project's conversation owner is the same shape and no workspace at
        // all: it stands in Build's own scratch directory, which no manifest
        // names, so the root test above would import it. A project is not one
        // of its own workspaces — adopting it put the project agent in its own
        // `list_workspaces`, where it found itself and sent itself work.
        let workspace_roots = self
            .workspaces
            .list(None)
            .into_iter()
            .map(|workspace| workspace.root.clone())
            .collect::<Vec<_>>();
        let runs = self
            .runs
            .values()
            .filter(|run| !self.is_project_conversation_owner(&run.run.id.0))
            .filter(|run| {
                !run.worktree.base_branch.is_empty()
                    || !workspace_roots
                        .iter()
                        .any(|root| same_path(root, &run.worktree.path))
            })
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
                if crate::workspace::is_managed_workspace_mount(&path)
                    || held_paths.iter().any(|held| same_path(held, &path))
                {
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
                    || crate::workspace::is_managed_workspace_mount(&path)
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

pub(in crate::app) fn same_path(left: &Path, right: &Path) -> bool {
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
    let mut value = json!({
        "id": workspace.id,
        "workspace_id": workspace.id,
        "project_id": workspace.project_id,
        "name": workspace.name,
        "root": workspace.root.display().to_string(),
        "status": workspace.status,
        "finished_at": workspace.archived_at,
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
    });
    if workspace.archived_at.is_none() {
        value
            .as_object_mut()
            .expect("workspace_json builds an object")
            .remove("finished_at");
    }
    value
}
