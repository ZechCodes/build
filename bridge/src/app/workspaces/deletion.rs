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

use super::branch_delete::{self, BranchDeleteFailure, BranchDeletion};
use crate::app::git::deferred::DeferredGitWork;
use crate::app::{require_str, AppState, DeferredGit, DeferredWork};
use crate::reclaim::containment::WorkspaceBoundary;
use crate::tracker::{Actor, IssueEventKind};
use crate::workspace::{Workspace, WorkspaceStatus};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// What the drain removes, resolved before the mutex was released: the
/// checkouts to unregister from their sources, and the root to walk away.
pub(super) struct DeleteWorkspaceFiles {
    workspace_id: String,
    root: PathBuf,
    boundary: WorkspaceBoundary,
    rift_root: PathBuf,
    retirements: Vec<crate::reaper::Retirement>,
    /// `(source repository, checkout inside the workspace)` per Git directory.
    checkouts: Vec<(PathBuf, PathBuf)>,
    /// The runs whose checkout lived under this root — the conversation the
    /// workspace owned, and anything adopted below it.
    run_ids: Vec<String>,
    /// The workspace as it stood when the removal was decided.
    workspace: Workspace,
    removal: Removal,
}

/// What a removal does beyond taking the files.
pub(super) enum Removal {
    /// `workspace.delete`: nothing. A plain delete is not history and records
    /// nothing.
    Delete,
    /// Done: writes the workspace into the registry's history before its
    /// files go, and deletes the branches it was asked to.
    Finish(Finishing),
    /// `workspace.reclaim`: takes the branch each directory carries once the
    /// checkouts are gone, and records on the issues how each went.
    Reclaim(Reclaiming),
}

/// What a Done does beyond a delete: where it writes the record of what it
/// finished, and the local branches it deletes once the checkouts are gone.
pub(super) struct Finishing {
    pub(super) registry_root: PathBuf,
    pub(super) branches: Vec<BranchDeletion>,
}

/// What a reclaim does beyond a delete: the branches it takes, and who
/// reclaimed, whom the issues name.
pub(super) struct Reclaiming {
    pub(super) actor: Actor,
    pub(super) branches: Vec<BranchDeletion>,
}

impl DeferredGitWork for DeleteWorkspaceFiles {
    fn run(&self, _: &Value) -> Result<Value, String> {
        self.boundary.validate_removal().map_err(|error| {
            format!(
                "Workspace cleanup refused at {}: {error}. Retry the deletion",
                self.root.display()
            )
        })?;
        // Done writes down what it finished before it removes anything: a
        // removal that fails halfway must not lose the record of where the
        // work was left. It measures the Git work once more here, off the
        // mutex, so nothing that landed since the click is deleted unseen.
        let finished = match &self.removal {
            Removal::Finish(finishing) => {
                let mut workspace = self.workspace.clone();
                let registry = crate::workspace::WorkspaceRegistry::load(&finishing.registry_root)?;
                Some(registry.record_finished(&mut workspace)?)
            }
            Removal::Delete | Removal::Reclaim(_) => None,
        };
        for retirement in &self.retirements {
            if !retirement.wait(crate::orchestrator::CHECKOUT_REAP_WAIT) {
                return Err("Workspace cleanup stopped because a process did not exit; workspace files were preserved. Retry the deletion".into());
            }
        }
        self.boundary.validate_removal().map_err(|error| {
            format!(
                "Workspace cleanup refused at {}: {error}. Retry the deletion",
                self.root.display()
            )
        })?;
        // Capture the registered names while the workspace manifest still
        // exists: managed mounts include the workspace name in that key.
        let mut registrations = Vec::new();
        for (source, path) in &self.checkouts {
            self.boundary.validate_removal().map_err(|error| {
                format!(
                    "Workspace cleanup refused at {}: {error}. Retry the deletion",
                    self.root.display()
                )
            })?;
            let name = crate::isolation::checkout_name(path)
                .ok_or_else(|| "Workspace checkout has no registered name".to_string())?;
            registrations.push((source.clone(), path.clone(), name));
        }
        // A missing source would make registration cleanup impossible after
        // the files were gone, so open each source before the pinned removal.
        let repositories = registrations
            .iter()
            .map(|(source, _, _)| {
                git2::Repository::open(source).map_err(|error| {
                    format!(
                        "Workspace cleanup cannot open source {}: {error}. Retry the deletion",
                        source.display()
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let guard = self.boundary.validate_removal().map_err(|error| {
            format!(
                "Workspace cleanup refused at {}: {error}. Retry the deletion",
                self.root.display()
            )
        })?;
        let budget = crate::reclaim::Budget::new(
            u64::MAX,
            std::time::Duration::from_secs(24 * 60 * 60),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        );
        guard
            .remove_contents_preserving_manifest(&budget)
            .map_err(|error| {
                format!(
                    "Workspace cleanup failed at {}: {error}. Retry the deletion",
                    self.root.display()
                )
            })?;
        // The working trees are already gone through pinned descriptors. Let
        // libgit2 remove only its own administrative files, then ask Rift to
        // collect records whose directories have disappeared.
        for ((_, path, name), repo) in registrations.iter().zip(&repositories) {
            match repo.find_worktree(name) {
                Ok(worktree) => {
                    let mut options = git2::WorktreePruneOptions::new();
                    options.valid(true).working_tree(false);
                    worktree.prune(Some(&mut options)).map_err(|error| {
                        format!(
                            "Workspace cleanup failed at {}: {error}. Retry the deletion",
                            path.display()
                        )
                    })?;
                }
                Err(error) if error.code() == git2::ErrorCode::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "Workspace cleanup failed at {}: {error}. Retry the deletion",
                        path.display()
                    ))
                }
            }
        }
        for (source, path) in &self.checkouts {
            crate::worktree::WorktreeManager::new(source, path.parent().unwrap_or(path))
                .with_rift_registry_root(&self.rift_root)
                .prune();
        }
        guard.finish_remove_workspace(&budget).map_err(|error| {
            format!(
                "Workspace cleanup failed at {}: {error}. Retry the deletion",
                self.root.display()
            )
        })?;
        Ok(self.take_branches(finished))
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
        match &self.removal {
            Removal::Finish(finishing) if result["branch_deleted"] == true => {
                for branch in finishing.branches.iter().map(BranchDeletion::branch) {
                    app.note_branch_deleted(
                        &self.workspace.project_id,
                        &self.workspace_id,
                        branch,
                        result["branch_reason"].as_str(),
                    );
                }
            }
            Removal::Reclaim(reclaiming) => {
                let branches = result["branches"].as_array().cloned().unwrap_or_default();
                app.note_reclaimed_branches(&self.workspace, &reclaiming.actor, &branches);
            }
            Removal::Finish(_) | Removal::Delete => {}
        }
        Ok(result)
    }
}

impl DeleteWorkspaceFiles {
    /// The files are gone: take the branches the removal was asked to, and
    /// answer. Done says how its branch went beside the record of what it
    /// finished; a reclaim answers as a delete does, with what became of each
    /// branch in each repository beside it (`reclaimed_branch`).
    fn take_branches(&self, finished: Option<crate::workspace::WorkspaceFinish>) -> Value {
        match (&self.removal, finished) {
            (Removal::Finish(finishing), Some(finished)) => {
                let mut answer = json!({
                    "complete": finished.complete,
                    "repositories": finished.repositories,
                    "deleted": true,
                });
                note_branch_outcome(&mut answer, &finishing.branches);
                answer
            }
            (Removal::Reclaim(reclaiming), _) => {
                let branches: Vec<Value> = branch_delete::delete_each(&reclaiming.branches)
                    .iter()
                    .map(|(deletion, outcome)| reclaimed_branch(deletion, outcome))
                    .collect();
                json!({ "workspace_id": self.workspace_id, "deleted": true, "branches": branches })
            }
            _ => json!({ "workspace_id": self.workspace_id, "deleted": true }),
        }
    }
}

/// Delete the branches a Done was asked to take, now the checkouts holding
/// them are gone, and say how that went beside the rest of the answer. Only a
/// Done that was asked says anything about a branch.
fn note_branch_outcome(answer: &mut Value, branches: &[BranchDeletion]) {
    if branches.is_empty() {
        return;
    }
    match branch_delete::delete_all(branches) {
        Ok(()) => answer["branch_deleted"] = json!(true),
        Err(failure) => {
            answer["branch_deleted"] = json!(failure.recovery_failed());
            answer["branch_reason"] = json!(failure.reason());
        }
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
        let workspace = self.workspace_to_remove(params)?;
        let boundary = self.refuse_removing_what_is_not_builds(&workspace)?;
        if workspace.status == WorkspaceStatus::Provisioning {
            return Err(
                "Wait for workspace provisioning to finish before deleting the workspace"
                    .to_string(),
            );
        }
        if self.agent_working_at_root(&Self::canonical_root(&workspace.root)) {
            return Err("Stop running agents before deleting the workspace".to_string());
        }
        self.remove_workspace(&workspace, params, Removal::Delete, boundary)?;
        Ok(json!({ "workspace_id": workspace.id, "deleted": true }))
    }

    /// The workspace a removing verb was pointed at, with the filesystem free
    /// to remove it. Both `workspace.delete` and Done start here.
    pub(super) fn workspace_to_remove(&mut self, params: &Value) -> Result<Workspace, String> {
        let workspace_id = require_str(params, "workspace_id")?;
        // A workspace `workspace.reclaim` or the reclaim service is measuring
        // is theirs until they are done with it.
        if self.deferred_work.is_some()
            || self.active_deferred_filesystem_jobs > 0
            || self.workspace_reserved(&workspace_id)
        {
            return Err(crate::reclaim::BUSY.to_string());
        }
        if self.workspaces.get(&workspace_id).is_none() {
            self.adopt_legacy_workspaces();
        }
        self.workspaces
            .get(&workspace_id)
            .cloned()
            .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))
    }

    /// Stop everything standing in this workspace and hand its removal to the
    /// drain. `removal` says what else goes with it: a Done writes the record
    /// of what was finished on the way out and deletes the branches it was
    /// asked to; a reclaim takes the workspace's branches.
    pub(super) fn remove_workspace(
        &mut self,
        workspace: &Workspace,
        params: &Value,
        removal: Removal,
        boundary: WorkspaceBoundary,
    ) -> Result<(), String> {
        self.preserve_project_issue_identities(&workspace.project_id)?;
        // Done closes linked open issues only after identity preservation
        // succeeds. Do this before retiring agents, so retirement also drops
        // any notices the automatic close queues for this workspace.
        // Eligibility has been accepted; a later disk failure does not undo
        // the completed work or reopen its issues.
        if matches!(removal, Removal::Finish(_)) {
            self.close_issues_of_finished_workspace(&workspace.project_id, &workspace.id);
        }
        let root = boundary
            .validate_removal()
            .map_err(|error| format!("Workspace root is outside managed storage: {error}"))?
            .expected_root()
            .to_path_buf();
        let run_ids = self.runs_under(&root);
        let retirements = self.retire_everything_at(&root);
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(DeleteWorkspaceFiles {
                workspace_id: workspace.id.clone(),
                root: workspace.root.clone(),
                boundary,
                rift_root: self.project_worktrees_root(&workspace.project_id),
                retirements,
                checkouts: checkouts_of(workspace),
                run_ids,
                workspace: workspace.clone(),
                removal,
            }),
            params: params.clone(),
            invalidates: true,
            #[cfg(test)]
            gate: None,
        })));
        Ok(())
    }

    /// Everything that makes this workspace not Build's to remove, whether the
    /// removal is a delete or a Done. Read before a single file is touched, so
    /// a refusal costs nothing.
    ///
    /// An adopted checkout is somebody else's working copy: Build found it, it
    /// did not make it, and it does not have the record of what it was cut
    /// from to hand it back. The other two are the guards `project.delete`
    /// applies, for the same reason — a root that contains somebody's source
    /// repository, or a checkout that resolves out of the root through a link,
    /// would take work with it that was never the workspace's.
    pub(super) fn refuse_removing_what_is_not_builds(
        &self,
        workspace: &Workspace,
    ) -> Result<WorkspaceBoundary, String> {
        if !workspace.managed {
            return Err(
                "Build cannot remove an adopted checkout. Only workspaces Build created can be deleted."
                    .to_string(),
            );
        }
        let boundary = WorkspaceBoundary::new(
            self.workspaces.storage_anchor(),
            &workspace.root,
            workspace
                .directories
                .iter()
                .map(|directory| directory.path.clone())
                .collect(),
        )
        .ok_or_else(|| "Cannot delete a workspace outside its managed storage".to_string())?;
        let root = boundary
            .validate_removal()
            .map_err(|error| {
                format!("Cannot delete a workspace outside its managed storage: {error}")
            })?
            .expected_root()
            .to_path_buf();
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
        Ok(boundary)
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

    /// Close every writer at this root or anywhere below it — agents first
    /// (which also discards their queued turns), then the terminal tabs — and
    /// hand back the reaps to wait on off the mutex. Below it too: an agent or
    /// a shell rooted in one of the workspace's checkouts is writing into the
    /// files about to go.
    pub(super) fn retire_everything_at(&mut self, root: &Path) -> Vec<crate::reaper::Retirement> {
        let mut roots: Vec<PathBuf> = self
            .session_registry
            .tab_keys()
            .into_iter()
            .map(|key| key.root)
            .filter(|tab_root| tab_root.starts_with(root) && tab_root != root)
            .collect();
        roots.sort();
        roots.dedup();
        roots.insert(0, root.to_path_buf());
        self.delivery_queue
            .retain_queued(|turn| !turn.tab_key().root.starts_with(root));
        let mut retirements = Vec::new();
        for agents_root in &roots {
            retirements.extend(self.retire_workspace_agents(agents_root));
        }
        let keys: Vec<_> = self
            .session_registry
            .tab_keys()
            .into_iter()
            .filter(|key| key.root.starts_with(root))
            .collect();
        for key in keys {
            if let Some(retirement) = self.retire_tab(&key, "closed") {
                retirements.push(retirement);
            }
        }
        retirements
    }

    /// What a reclaim did with each branch the workspace carried, on every
    /// issue linking the workspace or that branch, quietly and under whoever
    /// reclaimed, like the reclaim itself: the answer's own entry for that
    /// branch and repository, as `branch_deleted` or `branch_kept`.
    fn note_reclaimed_branches(
        &mut self,
        workspace: &Workspace,
        actor: &Actor,
        branches: &[Value],
    ) {
        for entry in branches {
            let (kind, payload) = reclaimed_branch_event(workspace, entry);
            let branch = entry["branch"].as_str().unwrap_or_default();
            let issues = self.issues_linking_workspace_or_branch(
                &workspace.project_id,
                &workspace.id,
                branch,
            );
            for issue in issues {
                if let Err(error) = self.record_quiet_event(&issue.id, actor, kind, payload.clone())
                {
                    eprintln!(
                        "note reclaimed branch {branch} on #{}: {error}",
                        issue.number
                    );
                }
            }
        }
    }

    /// The files are gone: drop the runs that lived in them and the record
    /// that listed them, so `workspace.list` stops naming it.
    fn complete_workspace_delete(
        &mut self,
        workspace_id: &str,
        run_ids: &[String],
    ) -> Result<(), String> {
        let project_id = self
            .workspaces
            .get(workspace_id)
            .map(|workspace| workspace.project_id.clone());
        for run_id in run_ids {
            if let Some(store) = &self.store {
                let result = if let Some(project_id) = project_id.as_deref() {
                    store.delete_run_retaining_inbox_messages(run_id, project_id)
                } else {
                    store.delete_run(run_id)
                };
                result.map_err(|error| {
                    format!("delete workspace conversation: {error}; retry the deletion")
                })?;
            }
            self.runs.remove(run_id);
            self.forget_session_owner(run_id);
            self.forget_run(run_id);
        }
        self.workspaces.forget(workspace_id);
        self.note_board_lists_changed(crate::changes::BoardLists::WORKSPACES);
        Ok(())
    }
}

/// A reclaim deleted the branch.
const BRANCH_DELETED: &str = "deleted";
/// A reclaim left the branch where it was, and says why.
const BRANCH_KEPT: &str = "kept";
/// A checkout moved onto the branch while it was deleted, and putting it back
/// failed: the ref is gone, and the reason names the commit to restore.
const BRANCH_RESTORE_FAILED: &str = "restore_failed";

/// What became of one branch in one repository, as the reclaim answers it:
/// `{ source_id, repository, branch, outcome, reason? }`.
fn reclaimed_branch(deletion: &BranchDeletion, outcome: &Result<(), BranchDeleteFailure>) -> Value {
    let word = match outcome {
        Ok(()) => BRANCH_DELETED,
        Err(failure) if failure.recovery_failed() => BRANCH_RESTORE_FAILED,
        Err(_) => BRANCH_KEPT,
    };
    let mut entry = json!({
        "source_id": deletion.source_id(),
        "repository": deletion.repo().display().to_string(),
        "branch": deletion.branch(),
        "outcome": word,
    });
    if let Err(failure) = outcome {
        entry["reason"] = json!(failure.reason());
    }
    entry
}

/// The timeline entry for one of those: the same fields, and the workspace.
/// A branch whose restoration failed is gone, so it reads as deleted, with
/// the reason.
fn reclaimed_branch_event(workspace: &Workspace, entry: &Value) -> (IssueEventKind, Value) {
    let mut payload = entry.clone();
    payload["workspace_id"] = json!(workspace.id);
    payload["workspace_name"] = json!(workspace.name);
    payload["reclaimed"] = json!(true);
    let kind = if entry["outcome"] == BRANCH_KEPT {
        IssueEventKind::BranchKept
    } else {
        IssueEventKind::BranchDeleted
    };
    (kind, payload)
}

/// Every Git directory of the workspace that is a real isolated checkout, as
/// `(what it was cut from, where it landed)`. A directory that is only a copy
/// has nothing to unregister and is removed with the root.
fn checkouts_of(workspace: &Workspace) -> Vec<(PathBuf, PathBuf)> {
    workspace
        .directories
        .iter()
        .filter(|directory| {
            directory.is_git
                && (directory.effective_isolation.is_some()
                    || crate::isolation::Isolation::of(&directory.path).is_some())
        })
        .map(|directory| (directory.source_path.clone(), directory.path.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(outcome: Result<(), BranchDeleteFailure>) -> Value {
        let deletion = BranchDeletion::for_tests(Path::new("/repos/second"), "source-2", "build/x");
        reclaimed_branch(&deletion, &outcome)
    }

    /// Each outcome, by its word, naming the repository it happened in.
    #[test]
    fn a_reclaimed_branch_says_its_repository_and_what_became_of_it() {
        assert_eq!(
            said(Ok(())),
            json!({
                "source_id": "source-2",
                "repository": "/repos/second",
                "branch": "build/x",
                "outcome": "deleted",
            })
        );
        let kept = said(Err(BranchDeleteFailure::Refused(
            "Build cannot delete the branch build/x: it has commits no remote has.".to_string(),
        )));
        assert_eq!(kept["outcome"], "kept");
        assert_eq!(
            kept["reason"],
            "Build cannot delete the branch build/x: it has commits no remote has."
        );
    }

    /// Restoration that failed left the ref gone: the answer says so, and the
    /// issue reads it as deleted, with the reason naming what to restore.
    #[test]
    fn a_failed_restoration_is_reported_as_such_and_logged_as_deleted() {
        let failed = said(Err(BranchDeleteFailure::RecoveryFailed(
            "Build could not restore the deleted branch build/x at abc123".to_string(),
        )));
        assert_eq!(failed["outcome"], "restore_failed");
        assert!(failed["reason"].as_str().unwrap().contains("abc123"));

        let workspace: Workspace = serde_json::from_value(json!({
            "id": "ws-1", "project_id": "proj-1", "name": "pair",
            "root": "/ws/pair", "status": "ready", "directories": [],
        }))
        .unwrap();
        let (kind, payload) = reclaimed_branch_event(&workspace, &failed);
        assert_eq!(kind, IssueEventKind::BranchDeleted);
        assert_eq!(payload["outcome"], "restore_failed");
        assert_eq!(payload["repository"], "/repos/second");
        assert_eq!(payload["workspace_name"], "pair");
        assert_eq!(payload["reclaimed"], true);

        let (kind, _) = reclaimed_branch_event(
            &workspace,
            &said(Err(BranchDeleteFailure::Refused("no".into()))),
        );
        assert_eq!(kind, IssueEventKind::BranchKept);
    }
}
