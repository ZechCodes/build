use crate::git_process::run_git;
use crate::isolation::{
    local_branch_ref, record_branch_teardown, BranchTeardown, Isolation, WorktreeError,
};
use std::path::{Path, PathBuf};
/// A task's worktree: where it lives, which branch it's on, and what it was cut
/// from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// Git's internal worktree id (also the on-disk directory name) — the slug.
    pub name: String,
    /// Absolute path to the working directory.
    pub path: PathBuf,
    /// The branch this checkout was on when Build cut or adopted it. A
    /// breadcrumb, not the truth: the checkout itself decides what branch it
    /// is on (see [`Worktree::branch`]), and this name only answers when the
    /// checkout cannot — gone from disk, or detached. Restore and teardown
    /// read it deliberately: they act on the branch Build was given, not on
    /// wherever HEAD wandered since.
    pub recorded_branch: String,
    /// The branch this worktree was created from.
    pub base_branch: String,
}

impl Worktree {
    /// The branch this checkout has checked out right now, read from the
    /// working directory — the source of truth. Falls back to
    /// [`recorded_branch`](Self::recorded_branch) only when the checkout
    /// cannot answer (missing from disk, detached HEAD).
    pub fn branch(&self) -> String {
        checked_out_branch(&self.path).unwrap_or_else(|| self.recorded_branch.clone())
    }
}

/// A branch with a local ref ready to be checked out: what teardown of the
/// checkout will own, and the ref this call made for it, if it made one.
pub(super) struct PreparedBranch {
    pub(super) teardown: BranchTeardown,
    pub(super) created_ref: Option<CreatedLocalRef>,
}

impl PreparedBranch {
    fn discard_created_ref(&self, repo: &git2::Repository) {
        if let Some(created) = &self.created_ref {
            created.discard(repo);
        }
    }
}

/// A local branch a checkout-in-progress cut or materialised for itself. It
/// exists only to be checked out, so a checkout that fails takes it back —
/// left behind, a retry would find it as a plain local branch, borrow it,
/// and never let teardown delete it again.
pub(super) struct CreatedLocalRef(pub(super) String);

impl CreatedLocalRef {
    /// Best-effort, like the rest of a failed checkout's unwind: the error
    /// worth reporting is the one that stopped the checkout, and a ref that
    /// could not be taken back is logged rather than raised over it.
    fn discard(&self, repo: &git2::Repository) {
        let branch = &self.0;
        let deleted = repo
            .find_branch(branch, git2::BranchType::Local)
            .and_then(|mut local| local.delete());
        if let Err(error) = deleted {
            eprintln!(
                "branch {branch}: made for a checkout that failed and could not be taken back \
                 ({error}); it now reads as a branch Build must keep"
            );
        }
    }
}

/// A checkout added for a branch named in full, and what tearing it down owns.
/// A branch that was already there — here or on a remote — holds work nobody
/// asked Build to remove, so only a branch this call cut goes with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedBranchCheckout {
    pub worktree: Worktree,
    pub teardown: BranchTeardown,
}

/// What restore does about a checkout whose worktree registration is gone —
/// pruned by [`WorktreeManager::remove`], by `git worktree remove`, or by
/// git's own pruning. With the registration goes the record of what teardown
/// owns, and the environment can no longer answer. The caller either knows
/// (because it knows how the checkout was created) or does not, and says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnregisteredRestore {
    Write(BranchTeardown),
    Refuse,
}

fn head_on_recorded_branch(
    checkout: &Path,
    worktree: &Worktree,
) -> Result<git2::Oid, WorktreeError> {
    let repository = git2::Repository::open(checkout)?;
    let head = repository.head()?;
    if !head.is_branch() || head.shorthand() != Some(worktree.recorded_branch.as_str()) {
        return Err(WorktreeError::Refused(format!(
            "worktree is not on the exact persisted branch {:?}",
            worktree.recorded_branch
        )));
    }
    head.target()
        .ok_or_else(|| WorktreeError::Refused("worktree HEAD has no direct commit".to_string()))
}

/// That the project repo's own tip of the recorded branch is the commit the
/// checkout is sitting on — the point of publishing before this is asked.
fn head_matches_project_tip(
    project: &git2::Repository,
    head: git2::Oid,
    worktree: &Worktree,
) -> Result<(), WorktreeError> {
    let tip = project
        .find_reference(&local_branch_ref(&worktree.recorded_branch))?
        .target()
        .ok_or_else(|| WorktreeError::Refused("persisted branch has no commit".to_string()))?;
    if head != tip {
        return Err(WorktreeError::Refused(
            "worktree HEAD does not match the persisted branch tip".to_string(),
        ));
    }
    Ok(())
}

/// That the branch and the base it was cut from share a commit. Restoring a
/// checkout whose branch grew somewhere else would hand back lineage nobody
/// cut.
fn shares_ancestry_with_base(
    project: &git2::Repository,
    head: git2::Oid,
    worktree: &Worktree,
) -> Result<(), WorktreeError> {
    let base = project
        .revparse_single(&worktree.base_branch)?
        .peel_to_commit()?
        .id();
    project.merge_base(base, head).map_err(|_| {
        WorktreeError::Refused(format!(
            "worktree branch has no verified ancestry with {:?}",
            worktree.base_branch
        ))
    })?;
    Ok(())
}

use super::command::{bounded_git_fetch, configured_remote_for_branch};
use super::identity::{checked_out_branch, is_usable_branch_name};
use super::WorktreeManager;
use crate::isolation::branch_teardown;

impl WorktreeManager {
    /// Create `<prefix>/<slug>` from `base_branch` and materialize a checkout of
    /// it. The name is made unique (`<slug>`, `<slug>-2`, …) so re-dispatching
    /// the same goal — or leftover branches/checkouts from prior tasks — never
    /// collides.
    ///
    /// The branch is one Build cut for itself, so the answer says teardown
    /// takes it — the same fact this call writes beside the checkout, told to
    /// the caller in the shape [`create_cutting_branch`](Self::create_cutting_branch)
    /// answers in.
    pub fn create(
        &self,
        slug: &str,
        base_branch: &str,
        isolation: Isolation,
    ) -> Result<NamedBranchCheckout, WorktreeError> {
        let _creation = self.lock_creation()?;
        let backend = self.backend(isolation)?;
        let repo = git2::Repository::open(&self.repo_path)?;
        let base_commit = repo.revparse_single(base_branch)?.peel_to_commit()?;

        let name =
            self.unique_checkout_name(slug, |candidate| self.branch_taken(&repo, candidate))?;
        let branch = self.branch_name(&name);

        repo.branch(&branch, &base_commit, false)?;
        let path = self.checkout_path(&name)?;
        backend.materialize(&self.repo_path, &branch, &path)?;
        let teardown = BranchTeardown::DeletesBranch;
        record_branch_teardown(&path, teardown)?;

        Ok(NamedBranchCheckout {
            worktree: Worktree {
                name,
                path,
                recorded_branch: branch,
                base_branch: base_branch.to_string(),
            },
            teardown,
        })
    }
    /// Materialize a checkout of a branch that already exists, spelled exactly
    /// as it was given: one this repository holds is checked out as it stands,
    /// and one only a remote carries is fetched and made local with its
    /// upstream set. A name no ref anywhere backs is a mistake the caller is
    /// told about, never a fresh empty branch wearing that name.
    ///
    /// Teardown keeps such a branch: it holds work nobody asked Build to
    /// remove.
    pub fn create_on_existing_branch(
        &self,
        branch: &str,
        base_branch: &str,
        isolation: Isolation,
    ) -> Result<NamedBranchCheckout, WorktreeError> {
        let _creation = self.lock_creation()?;
        let repo = git2::Repository::open(&self.repo_path)?;
        let prepared = self
            .prepare_existing_branch(&repo, branch)?
            .ok_or_else(|| {
                WorktreeError::Command(format!(
                    "branch {branch:?} does not exist locally or on any remote"
                ))
            })?;
        self.checkout_branch(&repo, branch, base_branch, prepared, isolation)
    }
    /// Add a worktree for the branch `branch`, cutting it from `base_branch`
    /// when no ref anywhere holds it — the caller named a branch it means to
    /// start, so the name is cut exactly as given rather than re-derived.
    ///
    /// The counterpart to [`create`](Self::create): that one is handed a slug
    /// and owns the namespace, this one is handed the whole name and owns
    /// nothing but the directory. A branch that already exists is checked out
    /// rather than cut, so dispatching onto work started by hand reaches it,
    /// and the answer says which of those two happened, because teardown turns
    /// on it.
    pub fn create_cutting_branch(
        &self,
        branch: &str,
        base_branch: &str,
        isolation: Isolation,
    ) -> Result<NamedBranchCheckout, WorktreeError> {
        let _creation = self.lock_creation()?;
        let repo = git2::Repository::open(&self.repo_path)?;
        let prepared = match self.prepare_existing_branch(&repo, branch)? {
            Some(prepared) => prepared,
            None => PreparedBranch {
                teardown: BranchTeardown::DeletesBranch,
                created_ref: Some(self.cut_branch(&repo, branch, base_branch)?),
            },
        };
        self.checkout_branch(&repo, branch, base_branch, prepared, isolation)
    }
    /// Give a branch that already exists a local ref to check out, wherever it
    /// lives: one this repository holds is ready as it stands, and one only a
    /// remote carries is fetched and made local first. Either way teardown
    /// keeps it — the branch holds work nobody asked Build to remove. `None`
    /// is a name no ref anywhere backs, which each verb answers for itself.
    pub(super) fn prepare_existing_branch(
        &self,
        repo: &git2::Repository,
        branch: &str,
    ) -> Result<Option<PreparedBranch>, WorktreeError> {
        let prepared = match crate::gitgui::branch_origin(repo, branch)? {
            crate::gitgui::BranchOrigin::Local => PreparedBranch {
                teardown: BranchTeardown::KeepsBranch,
                created_ref: None,
            },
            crate::gitgui::BranchOrigin::Remote {
                remote,
                tracking_ref,
            } => PreparedBranch {
                teardown: BranchTeardown::KeepsBranch,
                created_ref: Some(self.materialise_remote_branch(
                    repo,
                    branch,
                    &remote,
                    &tracking_ref,
                )?),
            },
            crate::gitgui::BranchOrigin::Absent => return Ok(None),
        };
        Ok(Some(prepared))
    }
    /// Give a branch that is ready to be checked out a directory of its own,
    /// and stamp what teardown of it owns beside it. A ref this call made for
    /// the branch goes away again with any checkout that fails, so a retry
    /// finds the repository as it was.
    pub(super) fn checkout_branch(
        &self,
        repo: &git2::Repository,
        branch: &str,
        base_branch: &str,
        prepared: PreparedBranch,
        isolation: Isolation,
    ) -> Result<NamedBranchCheckout, WorktreeError> {
        let name = self.unique_checkout_name(&self.directory_name_for(branch), |_| false)?;
        let path = self.checkout_path(&name)?;
        if let Err(error) = self
            .backend(isolation)
            .and_then(|backend| backend.materialize(&self.repo_path, branch, &path))
        {
            prepared.discard_created_ref(repo);
            return Err(error);
        }
        self.stamp_teardown_or_unwind(repo, branch, &name, &path, &prepared)?;

        Ok(NamedBranchCheckout {
            worktree: Worktree {
                name,
                path,
                recorded_branch: branch.to_string(),
                base_branch: base_branch.to_string(),
            },
            teardown: prepared.teardown,
        })
    }
    /// Where a checkout called `name` lives, with the root it sits in already
    /// there: a backend is handed a directory to materialize, never a folder to
    /// make first.
    pub(super) fn checkout_path(&self, name: &str) -> Result<PathBuf, WorktreeError> {
        std::fs::create_dir_all(&self.worktrees_root)?;
        Ok(self.worktrees_root.join(name))
    }
    /// Remove a live checkout through its owner before clearing stale records.
    /// A provider may need the directory's marker to unregister it, or a
    /// filesystem-specific operation to remove it. Other backends only see
    /// the absent path after its owner has finished.
    pub fn remove_checkout(&self, path: &Path) -> Result<(), WorktreeError> {
        let owner = Isolation::of(path);
        if owner.is_none() && (path.join(".rift").exists() || path.join(".git").is_dir()) {
            return Err(WorktreeError::NotABuildCheckout(path.to_path_buf()));
        }
        if let Some(isolation) = owner {
            self.backend(isolation)?.remove(&self.repo_path, path)?;
        }
        for backend in self.every_backend() {
            if Some(backend.kind()) != owner {
                backend.remove(&self.repo_path, path)?;
            }
        }
        Ok(())
    }
    /// Make the checkout's tip of `branch` the project repo's — the step every
    /// read of a run branch in the project repo comes after.
    pub fn publish(&self, path: &Path, branch: &str) -> Result<(), WorktreeError> {
        self.backend_of(path)?
            .publish(&self.repo_path, path, branch)
    }
    /// Make the project's tip of `base_branch` the checkout's, so a diff or an
    /// ahead/behind count against the base means the same thing in both
    /// isolations.
    pub fn sync_base(&self, path: &Path, base_branch: &str) -> Result<(), WorktreeError> {
        self.backend_of(path)?
            .sync_base(&self.repo_path, path, base_branch)
    }
    /// Merge `branch` into `base_branch` through the project's own checkout.
    /// That checkout is the user's live one, so first verify it actually has the
    /// base branch checked out — merging into whatever happens to be at HEAD
    /// would land the work on the wrong branch (and a later push of the base
    /// branch would silently publish nothing).
    pub fn merge_into_base(
        &self,
        path: &Path,
        branch: &str,
        base_branch: &str,
    ) -> Result<(), WorktreeError> {
        self.publish(path, branch)?;
        let head = run_git(&self.repo_path, &["symbolic-ref", "--short", "HEAD"])?
            .trim()
            .to_string();
        if head != base_branch {
            return Err(WorktreeError::Refused(format!(
                "primary checkout is on {head:?}, not the base branch {base_branch:?} — \
                 check out {base_branch:?} (or commit/stash your work) and approve again"
            )));
        }
        // `--` stops option parsing so an option-shaped branch name can never be
        // read by git as a flag (defense in depth alongside the adopt-time guard).
        if let Err(merge_error) = run_git(&self.repo_path, &["merge", "--no-edit", "--", branch]) {
            // A conflict leaves the primary checkout wedged mid-merge; abort it so
            // the checkout returns to a clean base and later merges aren't poisoned.
            // Best-effort — the merge failure is the error we surface either way.
            if let Err(abort_error) = run_git(&self.repo_path, &["merge", "--abort"]) {
                eprintln!(
                    "merge_into_base {branch}: merge failed and abort also failed: {abort_error}"
                );
            }
            return Err(merge_error.into());
        }
        Ok(())
    }
    /// Clear every backend's records of checkouts that no longer exist. The one
    /// place a backend's failure becomes a log line instead of an answer:
    /// nothing a caller asked for depends on the sweep having run.
    pub fn prune(&self) {
        for backend in self.every_backend() {
            if let Err(error) = backend.prune(&self.repo_path) {
                eprintln!("prune {}: {error}", self.repo_path.display());
            }
        }
    }
    /// Whether the project repo has a local branch of this name.
    pub fn branch_exists(&self, branch: &str) -> Result<bool, WorktreeError> {
        let repo = git2::Repository::open(&self.repo_path)?;
        let found = repo.find_branch(branch, git2::BranchType::Local);
        match found {
            Ok(_) => Ok(true),
            Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
    /// Delete `branch` only while it still points at `expected_head` — a branch
    /// that moved since it was read carries work the caller never saw.
    pub fn delete_branch_at(&self, branch: &str, expected_head: &str) -> Result<(), WorktreeError> {
        run_git(
            &self.repo_path,
            &["update-ref", "-d", &local_branch_ref(branch), expected_head],
        )?;
        Ok(())
    }
    /// Put `branch` back at `sha` — the undo for a deletion whose teardown then
    /// failed.
    pub fn restore_branch(&self, branch: &str, sha: &str) -> Result<(), WorktreeError> {
        run_git(
            &self.repo_path,
            &["update-ref", &local_branch_ref(branch), sha],
        )?;
        Ok(())
    }
    /// Write what teardown owns beside a checkout git has just registered, and
    /// take that registration back when the write fails.
    ///
    /// A registered checkout with no marker reads as one Build made for itself
    /// — the branch goes with it — so leaving one behind after failing to say
    /// otherwise hands somebody else's branch to the next teardown. Unwinding
    /// is best-effort because the error being returned is the one worth
    /// reporting; an unwind that fell short is reported too, since a checkout
    /// left standing unmarked is exactly what this exists to prevent.
    pub(super) fn stamp_teardown_or_unwind(
        &self,
        repo: &git2::Repository,
        branch: &str,
        name: &str,
        path: &Path,
        prepared: &PreparedBranch,
    ) -> Result<(), WorktreeError> {
        let Err(error) = record_branch_teardown(path, prepared.teardown) else {
            return Ok(());
        };
        let taken_back = self.remove_checkout(path);
        prepared.discard_created_ref(repo);
        if let Err(unwind_error) = taken_back {
            eprintln!(
                "worktree {name}: teardown marker unwritten and the checkout not taken back \
                 ({unwind_error}); branch {branch} may be deleted by the next teardown"
            );
            return Err(WorktreeError::Command(format!(
                "could not record teardown for {name}, and the checkout could not be taken \
                 back: {error}"
            )));
        }
        Err(error)
    }
    /// Bring a branch that exists only on `remote` here: fetch exactly it,
    /// cut the local ref at what came back, and point that ref at the remote
    /// branch it came from, so the checkout is on the team's branch rather
    /// than a private copy of its name.
    pub(super) fn materialise_remote_branch(
        &self,
        repo: &git2::Repository,
        branch: &str,
        remote: &str,
        tracking_ref: &str,
    ) -> Result<CreatedLocalRef, WorktreeError> {
        bounded_git_fetch(
            &self.repo_path,
            remote,
            &format!("+refs/heads/{branch}:{tracking_ref}"),
        )
        .map_err(|error| {
            WorktreeError::Command(format!(
                "remote {remote:?} no longer carries branch {branch:?}: {error}"
            ))
        })?;
        let fetched = repo.find_reference(tracking_ref)?.peel_to_commit()?;
        let upstream = tracking_ref
            .strip_prefix("refs/remotes/")
            .ok_or_else(|| WorktreeError::Command(format!("{tracking_ref} is not a remote ref")))?;
        let mut local = repo.branch(branch, &fetched, false)?;
        let created = CreatedLocalRef(branch.to_string());
        if let Err(error) = local.set_upstream(Some(upstream)) {
            created.discard(repo);
            return Err(error.into());
        }
        Ok(created)
    }
    /// Cut `branch` from the base, for a caller that meant a name rather than
    /// a branch. This is the one place a name Build is about to fold into a
    /// directory as well as a ref has to pass the narrower rule.
    pub(super) fn cut_branch(
        &self,
        repo: &git2::Repository,
        branch: &str,
        base_branch: &str,
    ) -> Result<CreatedLocalRef, WorktreeError> {
        if !is_usable_branch_name(branch) {
            return Err(WorktreeError::Command(format!(
                "{branch:?} is not a name Build can cut a branch from"
            )));
        }
        let base_commit = repo.revparse_single(base_branch)?.peel_to_commit()?;
        repo.branch(branch, &base_commit, false)?;
        Ok(CreatedLocalRef(branch.to_string()))
    }
    /// The directory a checkout can have to itself: `<stem>`, `<stem>-2`, …
    /// until nothing claims it. A name some backend holds a record of or that
    /// already exists on disk is claimed; `also_taken` adds whatever else the
    /// caller's own namespace claims.
    pub(super) fn unique_checkout_name(
        &self,
        stem: &str,
        also_taken: impl Fn(&str) -> bool,
    ) -> Result<String, WorktreeError> {
        let mut name = stem.to_string();
        let mut suffix = 2;
        while self.checkout_name_taken(&name)? || also_taken(&name) {
            name = format!("{stem}-{suffix}");
            suffix += 1;
        }
        Ok(name)
    }
    /// Whether a checkout of this name is already there: a record some backend
    /// holds, or a directory under the worktrees root. Isolation-blind by
    /// design — two checkouts never share a name whatever made them.
    pub(super) fn checkout_name_taken(&self, name: &str) -> Result<bool, WorktreeError> {
        Ok(self.record_held(name)? || self.worktrees_root.join(name).exists())
    }
    /// Whether Build's own namespace already holds a branch for this slug.
    pub(super) fn branch_taken(&self, repo: &git2::Repository, slug: &str) -> bool {
        repo.find_branch(&self.branch_name(slug), git2::BranchType::Local)
            .is_ok()
    }
    /// Drop `branch` from the project repo. Only deletable once nothing has it
    /// checked out; already gone is done.
    pub(super) fn delete_branch(&self, branch: &str) -> Result<(), WorktreeError> {
        let repo = git2::Repository::open(&self.repo_path)?;
        let found = repo.find_branch(branch, git2::BranchType::Local);
        match found {
            Ok(mut branch) => branch.delete()?,
            Err(error) if error.code() == git2::ErrorCode::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }
    /// The path a restore is allowed to act on: the one this manager would have
    /// given the checkout. A recorded path that is not it, or a name that could
    /// climb out of the root, is refused before anything is read from disk.
    pub(super) fn refuse_outside_root(&self, worktree: &Worktree) -> Result<(), WorktreeError> {
        if worktree.path != self.worktrees_root.join(&worktree.name)
            || worktree.name.is_empty()
            || worktree.name.contains(['/', '\\'])
        {
            return Err(WorktreeError::Refused(
                "refusing to restore a worktree outside its managed root".to_string(),
            ));
        }
        Ok(())
    }
    /// Recreate a Build-owned checkout at its original path and branch, with
    /// the teardown its record carried across the prune.
    ///
    /// The recorded branch says what to restore; `isolation` says what to
    /// restore it as. A checkout that vanished carries no isolation to
    /// remember, so the caller's resolved setting is the only honest answer.
    pub fn restore(
        &self,
        worktree: &Worktree,
        when_unregistered: UnregisteredRestore,
        isolation: Isolation,
    ) -> Result<Worktree, WorktreeError> {
        let _creation = self.lock_creation()?;
        self.refuse_outside_root(worktree)?;
        if worktree.path.exists() {
            return self.verify_existing_checkout(worktree);
        }
        let repo = git2::Repository::open(&self.repo_path)?;
        let teardown = self.teardown_across_prune(worktree, when_unregistered)?;
        self.ensure_local_branch(&repo, &worktree.recorded_branch)?;
        let path = self.checkout_path(&worktree.name)?;
        self.backend(isolation)?
            .materialize(&self.repo_path, &worktree.recorded_branch, &path)?;
        record_branch_teardown(&path, teardown)?;
        self.verify_existing_checkout(worktree)
    }
    /// What teardown owns, taken out of the stale record before the sweep that
    /// clears it. A record no backend holds leaves the environment unable to
    /// answer — then the caller's `when_unregistered` speaks, or refuses to.
    /// Every failure to read one is surfaced, because a branch is deleted on
    /// the strength of this answer.
    pub(super) fn teardown_across_prune(
        &self,
        worktree: &Worktree,
        when_unregistered: UnregisteredRestore,
    ) -> Result<BranchTeardown, WorktreeError> {
        if let Some(recorded) = self.teardown_record(&worktree.name)? {
            self.prune();
            return Ok(recorded);
        }
        match when_unregistered {
            UnregisteredRestore::Write(teardown) => Ok(teardown),
            UnregisteredRestore::Refuse => Err(WorktreeError::Refused(format!(
                "cannot restore {:?}: its worktree registration is gone, so whether \
                 teardown owns branch {:?} cannot be decided",
                worktree.name, worktree.recorded_branch
            ))),
        }
    }
    /// Remove the checkout's working directory and every backend's record of
    /// it, and take the branch with it when the checkout says teardown owns it.
    ///
    /// The answer is read BEFORE anything is removed: a checkout that cannot
    /// say what its branch is owed is left standing whole rather than destroyed
    /// under a question nothing can answer afterwards. A branch that is already
    /// gone asks nothing of the marker.
    pub fn remove(&self, worktree: &Worktree) -> Result<(), WorktreeError> {
        let repo = git2::Repository::open(&self.repo_path)?;
        let deletes_branch =
            match repo.find_branch(&worktree.recorded_branch, git2::BranchType::Local) {
                Ok(_) => self.teardown_of(worktree)?.deletes_branch(),
                Err(error) if error.code() == git2::ErrorCode::NotFound => false,
                Err(error) => return Err(error.into()),
            };
        if !deletes_branch {
            self.publish_before_removal(worktree)?;
        }
        self.remove_checkout(&worktree.path)?;
        if deletes_branch {
            self.delete_branch(&worktree.recorded_branch)?;
        }
        Ok(())
    }
    /// Take the checkout away and leave its branch standing, whatever teardown
    /// would otherwise own. An abandoned run's work outlives the run so it can
    /// be re-attempted, which is a promise about the branch the checkout itself
    /// cannot make.
    pub fn remove_keeping_branch(&self, worktree: &Worktree) -> Result<(), WorktreeError> {
        self.publish_before_removal(worktree)?;
        self.remove_checkout(&worktree.path)
    }
    /// A branch that survives its checkout must carry the checkout's work, so
    /// whatever it holds reaches the project repo first. A publish that fails
    /// fails the removal. What is at the path decides only whether there is
    /// anything to publish from: a directory that is no checkout is still a
    /// directory this was pointed at, and it still goes.
    pub(super) fn publish_before_removal(&self, worktree: &Worktree) -> Result<(), WorktreeError> {
        if Isolation::of(&worktree.path).is_none() {
            return Ok(());
        }
        self.publish(&worktree.path, &worktree.recorded_branch)
    }
    /// What teardown of `worktree` owns — the façade's one rule for a question
    /// each isolation answers from somewhere else. A checkout still on disk
    /// answers for itself, whatever made it; one that is gone is asked of the
    /// records its backends keep. A checkout nothing can vouch for is refused
    /// rather than guessed at, because a branch deleted on a guess is
    /// somebody's work — and no git command ran to say so.
    pub(super) fn teardown_of(&self, worktree: &Worktree) -> Result<BranchTeardown, WorktreeError> {
        if worktree.path.exists() {
            return branch_teardown(&worktree.path);
        }
        self.teardown_record(&worktree.name)?.ok_or_else(|| {
            WorktreeError::Refused(format!(
                "{:?} is gone and nothing records whether teardown owns branch {:?}, so \
                 the branch is left standing",
                worktree.name, worktree.recorded_branch
            ))
        })
    }
    /// What any backend records about teardown of the checkout called `name`.
    /// A name carries no isolation, so this is one walk and every caller asks
    /// it here — [`record_held`](Self::record_held)'s sibling.
    pub(super) fn teardown_record(
        &self,
        name: &str,
    ) -> Result<Option<BranchTeardown>, WorktreeError> {
        for backend in self.every_backend() {
            if let Some(recorded) = backend.teardown_record(&self.repo_path, name)? {
                return Ok(Some(recorded));
            }
        }
        Ok(None)
    }
    /// Make sure the project repo has `branch` locally, fetching exactly it
    /// from its configured remote when it does not. The local ref is
    /// authoritative when present, and there is no fallback to the moving base:
    /// that would silently discard lineage.
    pub(super) fn ensure_local_branch(
        &self,
        repo: &git2::Repository,
        branch: &str,
    ) -> Result<(), WorktreeError> {
        let local_ref = local_branch_ref(branch);
        if !git2::Reference::is_valid_name(&local_ref) {
            return Err(WorktreeError::Refused(format!(
                "invalid persisted branch: {branch:?}"
            )));
        }
        if repo.find_reference(&local_ref).is_ok() {
            return Ok(());
        }
        let remote =
            configured_remote_for_branch(repo, branch).unwrap_or_else(|| "origin".to_string());
        bounded_git_fetch(
            &self.repo_path,
            &remote,
            &format!("+{local_ref}:{local_ref}"),
        )
    }
    /// The checks a restore makes on a checkout that is still there, in the
    /// order the spec names them: the path is the one this manager gave it,
    /// the checkout is its own backend's, whatever it holds reaches the
    /// project repo, and then the checks every isolation shares.
    pub(super) fn verify_existing_checkout(
        &self,
        worktree: &Worktree,
    ) -> Result<Worktree, WorktreeError> {
        let checkout = self.canonical_managed_path(&worktree.path)?;
        self.backend_of(&checkout)?.verify(
            &self.repo_path,
            &checkout,
            &worktree.recorded_branch,
        )?;
        self.publish(&checkout, &worktree.recorded_branch)?;
        self.verify_common(worktree, &checkout)?;
        Ok(worktree.clone())
    }
    /// What a restored checkout must be true of whatever made it: it is on the
    /// branch that was recorded for it, that branch is where the project repo
    /// says it is, and it grew out of the base it was cut from.
    pub(super) fn verify_common(
        &self,
        worktree: &Worktree,
        checkout: &Path,
    ) -> Result<(), WorktreeError> {
        let head = head_on_recorded_branch(checkout, worktree)?;
        let project = git2::Repository::open(&self.repo_path)?;
        head_matches_project_tip(&project, head, worktree)?;
        shares_ancestry_with_base(&project, head, worktree)
    }
}
