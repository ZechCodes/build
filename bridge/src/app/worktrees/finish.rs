use crate::app::archived_worktree_json;
#[cfg(test)]
use crate::app::OffLockGate;
use crate::orchestrator::ActiveRun;
use crate::store::{
    now_rfc3339, PersistedArchivedWorktree, Store, WorktreeFinishAction, WorktreeFinishStatus,
};
use crate::worktree::{git_stdout, ExternalWorktree, WorktreeManager};
use serde_json::Value;

/// What the lock-held half of a finish decided.
pub(in crate::app) enum PlannedFinish {
    /// Answered out of memory alone — an idempotent replay of a finish that
    /// already completed. No disk, no claim, nothing to defer.
    Settled(Value),
    /// The checkout is claimed and its git work is ready to run with the mutex
    /// released.
    Deferred(Box<WorktreeFinishJob>),
}

/// One finish's git work, lifted out from under the app mutex: the forced
/// rescan, the eligibility recheck, the checkpoint, the durable intent record,
/// and the destructive steps (merge, branch delete, worktree removal). Seconds
/// to minutes on a large checkout, and none of it touching [`AppState`].
pub(in crate::app) struct WorktreeFinishJob {
    pub(in crate::app) project_id: String,
    /// The project's checkout seam, cloned off its orchestrator: every
    /// checkout, branch and scan this job touches goes through it, with the
    /// app mutex released.
    pub(in crate::app) worktrees: WorktreeManager,
    pub(in crate::app) base_branch: String,
    pub(in crate::app) worktree_id: String,
    pub(in crate::app) action: WorktreeFinishAction,
    /// Checkouts a run already owns, excluded from the scan exactly as
    /// [`AppState::external_worktrees`] excludes them.
    pub(in crate::app) excluded: std::collections::HashSet<std::path::PathBuf>,
    /// A pending record found in memory: resume it rather than preflight again.
    pub(in crate::app) resume: Option<PersistedArchivedWorktree>,
    pub(in crate::app) store: Store,
    #[cfg(test)]
    pub(in crate::app) gate: Option<OffLockGate>,
}

/// What the lock-free git work brought back for the app mutex to write down.
pub(in crate::app) struct WorktreeFinishOutcome {
    /// The fresh external scan the preflight paid for, for the cache that would
    /// otherwise pay for it again on the next poll.
    pub(in crate::app) scan: Option<Vec<ExternalWorktree>>,
    /// The archive record as it now stands on disk: Archived after a completed
    /// finish, Pending after a failed destructive step. `None` when nothing was
    /// ever written.
    pub(in crate::app) record: Option<PersistedArchivedWorktree>,
    pub(in crate::app) result: Result<Value, String>,
}

/// What [`AppState::apply_finish`] has to settle after the git work: always the
/// claim and the project's caches, plus whatever the verb that deferred it owes
/// on top.
pub(in crate::app) struct FinishEpilogue {
    /// The claim taken in the plan phase, released here.
    pub(in crate::app) worktree_id: String,
    pub(in crate::app) project_id: String,
    pub(in crate::app) kind: FinishKind,
}

/// Which verb deferred the finish, and what it still owes.
pub(in crate::app) enum FinishKind {
    /// `worktree.finish` — the archive record is the whole answer.
    Worktree,
    /// `run.finish` — retire the run behind the checkout.
    Run(RunFinishEpilogue),
    /// `branch.finish` — retire the run (if any) and settle the issue the
    /// branch was implementing.
    Branch(BranchFinishEpilogue),
}

/// The run taken off the board while its checkout is being finished, so it can
/// be retired on success or put back on failure.
pub(in crate::app) struct RunFinishEpilogue {
    pub(in crate::app) run_id: String,
    pub(in crate::app) project_id: String,
    /// The run record itself, held here rather than in `runs` — a run whose
    /// checkout is being deleted must not answer verbs meanwhile.
    pub(in crate::app) active: Box<ActiveRun>,
    /// The canonical checkout root, consulted to tell a failure that left the
    /// worktree standing (retryable) from one that did not.
    pub(in crate::app) root: std::path::PathBuf,
}

/// The issue bookkeeping `branch.finish` owes once the branch is gone.
pub(in crate::app) struct BranchFinishEpilogue {
    pub(in crate::app) branch: String,
    /// `None` for a bare checkout: there was no run behind the branch.
    pub(in crate::app) run: Option<RunFinishEpilogue>,
    /// The issue this branch implemented and landed — archived on success.
    pub(in crate::app) issue_id: Option<String>,
    /// The issue whose implementation this finish threw away — told so, and
    /// left in the inbox.
    pub(in crate::app) orphaned_issue_id: Option<String>,
}

/// Everything a finish action's steps may act on, resolved once. Each step
/// reads the fields its own work needs and nothing else.
pub(in crate::app) struct FinishContext<'a> {
    pub(in crate::app) worktrees: &'a WorktreeManager,
    pub(in crate::app) base_branch: &'a str,
    pub(in crate::app) record: &'a PersistedArchivedWorktree,
    pub(in crate::app) checkout: &'a std::path::Path,
}

/// What one finish action does to the repository, once its checkout has been
/// resolved.
pub(in crate::app) type FinishGitSteps = fn(&FinishContext<'_>) -> Result<(), String>;

/// How a finish action lands the work it promised to keep before the checkout
/// goes away. An action that promises nothing lands nothing.
pub(in crate::app) type FinishLanding = fn(&FinishContext<'_>, &str) -> Result<(), String>;

impl WorktreeFinishJob {
    /// The epilogue for this job, addressed to the verb that owns it.
    pub(in crate::app) fn epilogue(&self, kind: FinishKind) -> FinishEpilogue {
        FinishEpilogue {
            worktree_id: self.worktree_id.clone(),
            project_id: self.project_id.clone(),
            kind,
        }
    }

    /// Every step of a finish that touches a disk, with the app mutex released.
    pub(in crate::app) fn run(mut self) -> WorktreeFinishOutcome {
        #[cfg(test)]
        if let Some(gate) = self.gate.take() {
            gate.arrive();
        }
        let (mut record, scan) = match self.resume.take() {
            Some(record) => (record, None),
            None => {
                let scanned = match self.worktrees.discover(&self.base_branch, &self.excluded) {
                    Ok(scanned) => scanned,
                    Err(error) => {
                        return WorktreeFinishOutcome {
                            scan: None,
                            record: None,
                            result: Err(error.to_string()),
                        }
                    }
                };
                match self.preflight(&scanned) {
                    Ok(record) => (record, Some(scanned)),
                    // The scan is worth keeping even when the preflight refuses.
                    Err(error) => {
                        return WorktreeFinishOutcome {
                            scan: Some(scanned),
                            record: None,
                            result: Err(error),
                        }
                    }
                }
            }
        };

        // The durable intent, before anything destructive: a finish that dies
        // between here and the end resumes from this record.
        if let Err(error) = self.store.save_archived_worktree(&record) {
            return WorktreeFinishOutcome {
                scan,
                record: None,
                result: Err(format!("worktree finish intent store: {error}")),
            };
        }
        if let Err(error) = run_finish_git_steps(&self.worktrees, &self.base_branch, &record) {
            return WorktreeFinishOutcome {
                scan,
                record: Some(record),
                result: Err(error),
            };
        }

        let pending = record.clone();
        record.status = WorktreeFinishStatus::Archived;
        record.archived_at = Some(now_rfc3339());
        match self.store.save_archived_worktree(&record) {
            Ok(()) => WorktreeFinishOutcome {
                scan,
                result: Ok(archived_worktree_json(&record)),
                record: Some(record),
            },
            // The git is done but the archive is not recorded: the record stays
            // Pending, and the recovery sweep finishes it.
            Err(error) => WorktreeFinishOutcome {
                scan,
                record: Some(pending),
                result: Err(format!("worktree archive store: {error}")),
            },
        }
    }

    /// Resolve the requested checkout against a scan taken just now, recheck
    /// that it may be finished, checkpoint what the action would otherwise
    /// throw away, and describe it for the archive.
    pub(in crate::app) fn preflight(
        &self,
        scanned: &[ExternalWorktree],
    ) -> Result<PersistedArchivedWorktree, String> {
        let external = scanned
            .iter()
            .find(|worktree| worktree.id == self.worktree_id)
            .cloned()
            .ok_or_else(|| format!("unknown worktree_id: {}", self.worktree_id))?;

        ensure_worktree_finish_eligible(&external, self.action, &self.base_branch)?;
        let dirty_metadata = external.clone();
        if matches!(
            self.action,
            WorktreeFinishAction::Push | WorktreeFinishAction::Merge
        ) && external.dirty_files > 0
        {
            checkpoint_worktree(&external.path, self.action)?;
        }
        let head_sha = git_stdout(&external.path, &["rev-parse", "HEAD"])?
            .trim()
            .to_string();

        Ok(PersistedArchivedWorktree {
            status: WorktreeFinishStatus::Pending,
            project_path: self.worktrees.repo_path().display().to_string(),
            worktree_id: external.id,
            worktree_name: external.name,
            worktree_path: external.path.display().to_string(),
            branch: external.branch,
            head_sha,
            upstream: dirty_metadata.upstream,
            unpushed: dirty_metadata.unpushed,
            dirty_files: dirty_metadata.dirty_files,
            uncommitted_files: dirty_metadata.uncommitted.files_changed,
            uncommitted_insertions: dirty_metadata.uncommitted.insertions,
            uncommitted_deletions: dirty_metadata.uncommitted.deletions,
            action: self.action,
            archived_at: None,
        })
    }
}

impl WorktreeFinishAction {
    /// The steps this action owns — the one place a finish action decides
    /// anything from its own kind.
    pub(in crate::app) fn git_steps(self) -> FinishGitSteps {
        match self {
            WorktreeFinishAction::Cleanup => remove_finished_checkout,
            WorktreeFinishAction::Push => push_then_remove_finished_checkout,
            WorktreeFinishAction::Merge => merge_finished_checkout,
            WorktreeFinishAction::Delete => delete_finished_checkout,
        }
    }

    /// What the human called for, as the wire spells it — the word a failure
    /// names this finish by.
    pub(in crate::app) fn verb(self) -> &'static str {
        match self {
            WorktreeFinishAction::Cleanup => "cleanup",
            WorktreeFinishAction::Push => "push",
            WorktreeFinishAction::Merge => "merge",
            WorktreeFinishAction::Delete => "delete",
        }
    }
}

/// The destructive half of a finish, over the steps the chosen action owns.
/// The record is the only authority for what is acted on — a client path never
/// reaches here — and every checkout and branch it touches goes through the
/// façade, so a clone and a linked worktree are finished by one function.
pub(in crate::app) fn run_finish_git_steps(
    worktrees: &WorktreeManager,
    base_branch: &str,
    record: &PersistedArchivedWorktree,
) -> Result<(), String> {
    let checkout = validate_finish_record_path(record, worktrees.repo_path())?;
    (record.action.git_steps())(&FinishContext {
        worktrees,
        base_branch,
        record,
        checkout: &checkout,
    })
}

/// Be rid of the checkout a finish is done with. Absence is the goal, so a
/// checkout somebody already deleted is nothing to report.
pub(in crate::app) fn remove_finished_checkout(context: &FinishContext<'_>) -> Result<(), String> {
    context
        .worktrees
        .remove_checkout(context.checkout)
        .map_err(|error| error.to_string())
}

pub(in crate::app) fn push_then_remove_finished_checkout(
    context: &FinishContext<'_>,
) -> Result<(), String> {
    if !context.checkout.exists() {
        return Ok(());
    }
    crate::gitgui::push(context.checkout, false)?;
    remove_finished_checkout(context)
}

pub(in crate::app) fn merge_finished_checkout(context: &FinishContext<'_>) -> Result<(), String> {
    finish_by_landing_then_removing(context, Some(merge_finished_branch_into_base))
}

pub(in crate::app) fn delete_finished_checkout(context: &FinishContext<'_>) -> Result<(), String> {
    finish_by_landing_then_removing(context, None)
}

pub(in crate::app) fn merge_finished_branch_into_base(
    context: &FinishContext<'_>,
    branch: &str,
) -> Result<(), String> {
    context
        .worktrees
        .merge_into_base(context.checkout, branch, context.base_branch)
        .map_err(|error| error.to_string())
}

/// Land what the action promised to keep, then take the checkout away — and
/// its branch with it when the checkout says teardown owns it.
pub(in crate::app) fn finish_by_landing_then_removing(
    context: &FinishContext<'_>,
    land: Option<FinishLanding>,
) -> Result<(), String> {
    let standing = context.checkout.exists();
    let branch = finish_branch_still_present(context, standing)?;
    if !refuse_finish_that_lost_its_checkout(context, standing, branch)? {
        return Ok(());
    }
    let deleted_branch = match branch {
        Some(branch) => land_then_delete_branch_if_owned(context, branch, land)?,
        None => false,
    };
    remove_finished_checkout_restoring_branch_on_failure(context, deleted_branch)
}

/// The branch this finish acts on, if the record names one and the project repo
/// still has it. Spec §0.4's publish-before-read step is here: whatever a
/// standing checkout holds reaches the project repo first — for a clone the only
/// way its branch is there at all, for a linked worktree nothing — and only then
/// is the project repo asked. A checkout with no branch (a detached HEAD) or
/// whose branch is already gone has nothing for the finish to land or delete.
pub(in crate::app) fn finish_branch_still_present<'a>(
    context: &FinishContext<'a>,
    standing: bool,
) -> Result<Option<&'a str>, String> {
    let Some(branch) = context.record.branch.as_deref() else {
        return Ok(None);
    };
    if standing {
        context
            .worktrees
            .publish(context.checkout, branch)
            .map_err(|error| error.to_string())?;
    }
    let stands = context
        .worktrees
        .branch_exists(branch)
        .map_err(|error| error.to_string())?;
    Ok(stands.then_some(branch))
}

/// Whether there is still a checkout to act on. A checkout that vanished
/// while a branch is still at stake is refused: the finish promised to land
/// or delete that branch from a checkout it no longer has. One that vanished
/// with no branch behind it has simply finished already.
pub(in crate::app) fn refuse_finish_that_lost_its_checkout(
    context: &FinishContext<'_>,
    standing: bool,
    branch: Option<&str>,
) -> Result<bool, String> {
    if standing {
        return Ok(true);
    }
    match branch {
        Some(_) => Err(format!(
            "worktree.finish {} lost its worktree before branch deletion",
            context.record.action.verb()
        )),
        None => Ok(false),
    }
}

/// Land the branch as the action promised, then delete it when the checkout
/// says teardown owns it. Answers whether the branch was deleted, so a
/// removal that fails afterwards knows what to put back.
///
/// The teardown is read here, before the removal prunes the admin directory
/// the answer lives in.
pub(in crate::app) fn land_then_delete_branch_if_owned(
    context: &FinishContext<'_>,
    branch: &str,
    land: Option<FinishLanding>,
) -> Result<bool, String> {
    let deletes_branch = crate::worktree::branch_teardown(context.checkout)
        .map_err(|error| error.to_string())?
        .deletes_branch();
    if let Some(land) = land {
        land(context, branch)?;
    }
    if deletes_branch {
        context
            .worktrees
            .delete_branch_at(branch, &context.record.head_sha)
            .map_err(|error| error.to_string())?;
    }
    Ok(deletes_branch)
}

pub(in crate::app) fn remove_finished_checkout_restoring_branch_on_failure(
    context: &FinishContext<'_>,
    deleted_branch: bool,
) -> Result<(), String> {
    let Err(remove_error) = remove_finished_checkout(context) else {
        return Ok(());
    };
    if !deleted_branch {
        return Err(remove_error);
    }
    let Some(branch) = context.record.branch.as_deref() else {
        return Err(remove_error);
    };
    match context
        .worktrees
        .restore_branch(branch, &context.record.head_sha)
    {
        Ok(()) => Err(remove_error),
        Err(restore_error) => Err(format!(
            "{remove_error}; restoring branch {branch:?} after removal failure also failed: \
             {restore_error}"
        )),
    }
}

pub(in crate::app) fn parse_worktree_finish_action(
    action: &str,
) -> Result<WorktreeFinishAction, String> {
    match action {
        "cleanup" => Ok(WorktreeFinishAction::Cleanup),
        "push" => Ok(WorktreeFinishAction::Push),
        "merge" => Ok(WorktreeFinishAction::Merge),
        "delete" => Ok(WorktreeFinishAction::Delete),
        other => Err(format!(
            "unknown worktree finish action {other:?} — expected cleanup, push, merge, or delete"
        )),
    }
}

pub(in crate::app) fn ensure_worktree_finish_eligible(
    worktree: &ExternalWorktree,
    action: WorktreeFinishAction,
    base_branch: &str,
) -> Result<(), String> {
    let repo = git2::Repository::open(&worktree.path).map_err(|error| error.to_string())?;
    let conflicted = repo
        .statuses(Some(
            git2::StatusOptions::new()
                .include_untracked(true)
                .recurse_untracked_dirs(true),
        ))
        .map_err(|error| error.to_string())?
        .iter()
        .any(|entry| entry.status().is_conflicted());
    match action {
        WorktreeFinishAction::Cleanup => {
            if worktree.dirty_files > 0
                || conflicted
                || repo.state() != git2::RepositoryState::Clean
            {
                return Err(
                    "worktree.finish cleanup requires no uncommitted, staged, untracked, or conflicted changes"
                        .to_string(),
                );
            }
        }
        WorktreeFinishAction::Push => {
            if worktree.upstream.is_none() {
                return Err("worktree.finish push requires an upstream/tracking branch".to_string());
            }
            if conflicted || repo.state() != git2::RepositoryState::Clean {
                return Err(
                    "worktree.finish push cannot checkpoint conflicted git state".to_string(),
                );
            }
        }
        WorktreeFinishAction::Merge => {
            let branch = worktree
                .branch
                .as_deref()
                .ok_or("worktree.finish merge requires an attached branch")?;
            if branch == base_branch {
                return Err(format!(
                    "worktree.finish merge requires a branch different from base {base_branch:?}"
                ));
            }
            if conflicted || repo.state() != git2::RepositoryState::Clean {
                return Err(
                    "worktree.finish merge cannot checkpoint conflicted git state".to_string(),
                );
            }
        }
        WorktreeFinishAction::Delete => {}
    }
    Ok(())
}

pub(in crate::app) fn checkpoint_worktree(
    worktree_path: &std::path::Path,
    action: WorktreeFinishAction,
) -> Result<(), String> {
    git_stdout(worktree_path, &["add", "-A", "--", "."])?;
    let staged = git_stdout(worktree_path, &["diff", "--cached", "--name-only"])?;
    if staged.trim().is_empty() {
        return Ok(());
    }
    let message = match action {
        WorktreeFinishAction::Push => "Build checkpoint before push",
        WorktreeFinishAction::Merge => "Build checkpoint before merge",
        WorktreeFinishAction::Cleanup | WorktreeFinishAction::Delete => {
            unreachable!("only push/merge checkpoint")
        }
    };
    git_stdout(worktree_path, &["commit", "-m", message]).map(|_| ())
}

pub(in crate::app) fn validate_finish_record_path(
    record: &PersistedArchivedWorktree,
    project_path: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    if record.project_path != project_path.display().to_string() {
        return Err("worktree.finish record belongs to another project".to_string());
    }
    let worktree_path = std::path::PathBuf::from(&record.worktree_path);
    let canonical_project = std::fs::canonicalize(project_path)
        .map_err(|error| format!("worktree.finish project path: {error}"))?;
    let resolved_worktree = if worktree_path.exists() {
        std::fs::canonicalize(&worktree_path)
            .map_err(|error| format!("worktree.finish worktree path: {error}"))?
    } else {
        worktree_path.clone()
    };
    if resolved_worktree == canonical_project {
        return Err("worktree.finish never acts on the primary checkout".to_string());
    }
    if crate::worktree::external_worktree_id(&resolved_worktree) != record.worktree_id {
        return Err(
            "worktree.finish record id no longer matches its server-resolved path".to_string(),
        );
    }
    Ok(resolved_worktree)
}

/// Whether a Pending finish record's git is in fact already done — the boot
/// question that turns an interrupted finish into an archived one. It is asked
/// before any project is registered, so the record's own two paths are the
/// only thing that can say which repository to put it to.
pub(in crate::app) fn finish_git_steps_are_complete(record: &PersistedArchivedWorktree) -> bool {
    if std::path::Path::new(&record.worktree_path).exists() {
        return false;
    }
    match record.action {
        WorktreeFinishAction::Cleanup | WorktreeFinishAction::Push => true,
        WorktreeFinishAction::Merge | WorktreeFinishAction::Delete => {
            record.branch.as_deref().is_none_or(|branch| {
                worktrees_of_record(record)
                    .branch_exists(branch)
                    .is_ok_and(|exists| !exists)
            })
        }
    }
}

/// The checkout seam for the project a finish record names, rooted where that
/// record's own checkout stood — the two paths a record carries, and all a
/// completeness check needs to ask the project repo about its branches.
pub(in crate::app) fn worktrees_of_record(record: &PersistedArchivedWorktree) -> WorktreeManager {
    let checkout = std::path::Path::new(&record.worktree_path);
    WorktreeManager::new(
        std::path::PathBuf::from(&record.project_path),
        checkout.parent().unwrap_or(checkout),
    )
}

/// What Done asks of a run before it archives the run's worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum FinishRequirement {
    /// `run.finish`: the run reached a review gate — completed work.
    CompletedWork,
    /// `branch.finish`: nothing. Done on a branch deletes it, and what that
    /// costs is reported as warnings on the row (see
    /// [`crate::branch::branch_finish_warnings`]) for the user to confirm
    /// through. The bridge does not second-guess a confirmed decision.
    Unconditional,
}
