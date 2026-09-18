//! `workspace.delete`: stop everything standing in one workspace, hand its
//! checkouts back to the repositories they were cut from, then remove the root
//! and the record.
//!
//! The same three steps `project.delete` takes over a whole project's
//! workspaces at once (`app/projects/deletion.rs`), narrowed to one — and
//! narrowed deliberately rather than shared, because the project verb also
//! rewrites the config, unbinds that project's issues and captures, and drops
//! the project itself, none of which a single workspace owns.
//!
//! Split the same way every filesystem verb here is: the decide half runs under
//! the app mutex and does nothing but refuse (an adopted checkout, a workspace
//! still provisioning, an agent mid-turn) and gather what the work needs; the
//! removal itself runs off the mutex through the deferred drain, because
//! unregistering worktrees and walking a tree away are both unbounded.

use crate::app::git::deferred::DeferredGitWork;
use crate::app::{require_str, AppState, DeferredGit, DeferredWork};
use crate::workspace::{Workspace, WorkspaceStatus};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// What the drain removes, resolved before the mutex was released: the
/// checkouts to unregister from their sources, and the root to walk away.
struct DeleteWorkspaceFiles {
    workspace_id: String,
    root: PathBuf,
    rift_root: PathBuf,
    retirements: Vec<crate::reaper::Retirement>,
    /// `(source repository, checkout inside the workspace)` per Git directory.
    checkouts: Vec<(PathBuf, PathBuf)>,
    /// The runs whose checkout lived under this root — the conversation the
    /// workspace owned, and anything adopted below it.
    run_ids: Vec<String>,
}

impl DeferredGitWork for DeleteWorkspaceFiles {
    fn run(&self, _: &Value) -> Result<Value, String> {
        for retirement in &self.retirements {
            if !retirement.wait(crate::orchestrator::CHECKOUT_REAP_WAIT) {
                return Err("Workspace cleanup stopped because a process did not exit; workspace files were preserved. Retry the deletion".into());
            }
        }
        // Unregister before removing: a worktree walked away without telling
        // its repository leaves an administrative record pointing at nothing,
        // which makes the same name unusable next time.
        for (source, path) in &self.checkouts {
            if path.exists() {
                crate::worktree::WorktreeManager::new(source, path.parent().unwrap_or(path))
                    .with_rift_registry_root(&self.rift_root)
                    .remove_checkout(path)
                    .map_err(|error| {
                        format!(
                            "Workspace cleanup failed at {}: {error}. Retry the deletion",
                            path.display()
                        )
                    })?;
            }
        }
        match std::fs::remove_dir_all(&self.root) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Workspace cleanup failed at {}: {error}. Retry the deletion",
                    self.root.display()
                ))
            }
        }
        Ok(json!({ "workspace_id": self.workspace_id, "deleted": true }))
    }

    /// A partial removal is still a move: the in-memory index has to agree
    /// with whatever is left on disk before the next read is answered.
    fn invalidates_on_error(&self) -> bool {
        true
    }

    fn invalidate(&self, app: &mut AppState) {
        if let Err(error) = app.workspaces.reload() {
            eprintln!("reload workspaces after delete: {error}");
        }
    }

    fn settle(&self, app: &mut AppState, result: Value) -> Result<Value, String> {
        app.complete_workspace_delete(&self.workspace_id, &self.run_ids)?;
        Ok(result)
    }
}

impl AppState {
    /// Delete one workspace: its agents and terminals stop, its checkouts are
    /// unregistered, its root is removed, and its record leaves the listing.
    ///
    /// Answers the completion shape straight away, exactly as `project.delete`
    /// does — the acknowledgement and the drain's real answer are one type, so
    /// a client reads the same value whichever half it sees.
    pub(crate) fn workspace_delete(&mut self, params: &Value) -> Result<Value, String> {
        let workspace_id = require_str(params, "workspace_id")?;
        if self.deferred_work.is_some() || self.active_deferred_filesystem_jobs > 0 {
            return Err("another filesystem operation is still running".to_string());
        }
        if self.workspaces.get(&workspace_id).is_none() {
            self.adopt_legacy_workspaces();
        }
        let workspace = self
            .workspaces
            .get(&workspace_id)
            .cloned()
            .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))?;
        self.refuse_undeletable_workspace(&workspace)?;
        let root = Self::canonical_root(&workspace.root);
        let run_ids = self.runs_under(&root);
        let retirements = self.retire_everything_at(&root);
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(DeleteWorkspaceFiles {
                workspace_id: workspace_id.clone(),
                root: workspace.root.clone(),
                rift_root: self.project_worktrees_root(&workspace.project_id),
                retirements,
                checkouts: checkouts_of(&workspace),
                run_ids,
            }),
            params: params.clone(),
            invalidates: true,
            #[cfg(test)]
            gate: None,
        })));
        Ok(json!({ "workspace_id": workspace_id, "deleted": true }))
    }

    /// Everything that makes this workspace not Build's to remove. Read before
    /// a single file is touched, so a refusal costs nothing.
    fn refuse_undeletable_workspace(&self, workspace: &Workspace) -> Result<(), String> {
        if !workspace.managed {
            return Err(
                "workspace.delete: adopted checkouts are not Build's to remove".to_string(),
            );
        }
        if workspace.status == WorkspaceStatus::Provisioning {
            return Err(
                "Wait for workspace provisioning to finish before deleting the workspace"
                    .to_string(),
            );
        }
        let root = Self::canonical_root(&workspace.root);
        if self.delivery_queue.has_in_flight_at_root(&root) {
            return Err("Stop running agents before deleting the workspace".to_string());
        }
        // The same two guards `project.delete` applies, for the same reason: a
        // root that contains somebody's source repository, or a checkout that
        // resolves out of the root through a link, would take work with it that
        // was never the workspace's.
        if self
            .projects
            .iter()
            .flat_map(|project| {
                std::iter::once(&project.repo_path)
                    .chain(project.sources.iter().map(|source| &source.path))
            })
            .any(|path| Self::canonical_root(path).starts_with(&root))
        {
            return Err("Cannot delete a workspace containing a source repository".to_string());
        }
        if workspace
            .directories
            .iter()
            .any(|directory| !Self::canonical_root(&directory.path).starts_with(&root))
        {
            return Err(
                "Cannot delete a workspace whose source checkout resolves outside its managed root"
                    .to_string(),
            );
        }
        Ok(())
    }

    /// The runs whose checkout stands inside `root`: the conversation the
    /// workspace owns, and anything adopted below it. Resolved now, because
    /// once the tree is gone there is no path left to match on.
    fn runs_under(&self, root: &Path) -> Vec<String> {
        self.runs
            .iter()
            .filter(|(_, active)| Self::canonical_root(&active.worktree.path).starts_with(root))
            .map(|(run_id, _)| run_id.clone())
            .collect()
    }

    /// Close every writer at this root — agents first (which also discards
    /// their queued turns), then the terminal tabs — and hand back the reaps
    /// to wait on off the mutex.
    fn retire_everything_at(&mut self, root: &Path) -> Vec<crate::reaper::Retirement> {
        let mut retirements = self.retire_workspace_agents(root);
        let keys: Vec<_> = self
            .session_registry
            .tab_keys()
            .into_iter()
            .filter(|key| key.root == root)
            .collect();
        for key in keys {
            if let Some(retirement) = self.retire_tab(&key, "closed") {
                retirements.push(retirement);
            }
        }
        retirements
    }

    /// The files are gone: drop the runs that lived in them and the record
    /// that listed them, so `workspace.list` stops naming it.
    fn complete_workspace_delete(
        &mut self,
        workspace_id: &str,
        run_ids: &[String],
    ) -> Result<(), String> {
        for run_id in run_ids {
            if let Some(store) = &self.store {
                store.delete_run(run_id).map_err(|error| {
                    format!("delete workspace conversation: {error}; retry the deletion")
                })?;
            }
            self.runs.remove(run_id);
            self.forget_run(run_id);
        }
        self.workspaces.forget(workspace_id);
        Ok(())
    }
}

/// Every Git directory of the workspace that is a real isolated checkout, as
/// `(what it was cut from, where it landed)`. A directory that is only a copy
/// has nothing to unregister and is removed with the root.
fn checkouts_of(workspace: &Workspace) -> Vec<(PathBuf, PathBuf)> {
    workspace
        .directories
        .iter()
        .filter(|directory| {
            directory.is_git && crate::isolation::Isolation::of(&directory.path).is_some()
        })
        .map(|directory| (directory.source_path.clone(), directory.path.clone()))
        .collect()
}
