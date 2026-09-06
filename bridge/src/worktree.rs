//! Worktree lifecycle: one worktree + branch per task.
//!
//! The bridge owns worktrees. A task gets an isolated branch (`build/<slug>`) and
//! a working directory cut from the project's base branch, so parallel tasks on
//! the same repo never touch each other. Removing a checkout takes its branch
//! only when the checkout itself says so — the fact is written down beside it
//! at creation, because a branch Build merely borrowed is somebody's work.
//! Issue planning has no worktree at all: its agent runs on the primary checkout.
//!
//! [`WorktreeManager`] is the one seam the orchestrator and the app talk to. How
//! a working directory is actually made — a git linked worktree, a
//! copy-on-write clone — is [`crate::isolation`]'s business, and nothing here
//! chooses between them except by passing on the isolation a caller resolved or
//! the one a checkout on disk answers for itself.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::git_process::{git_failure, run_git, run_git_with_deadline};
use crate::isolation::cow::CowBackend;
use crate::isolation::{
    checkout_name, local_branch_ref, record_branch_teardown, teardown_in_git_dir, Isolation,
    IsolationAvailability, IsolationBackend, WorktreeBackend,
};

/// The branch-name prefix for every run/task branch: `build/<slug>`.
pub const BRANCH_PREFIX: &str = "build";

/// The ref a slug becomes in Build's namespace. The one place the formula is
/// written: what a caller reserves a branch under has to be the ref
/// [`WorktreeManager::create`] then cuts, and two spellings of one rule drift
/// apart silently.
pub fn branch_name_for(slug: &str) -> String {
    format!("{BRANCH_PREFIX}/{slug}")
}

pub use crate::isolation::{branch_teardown, BranchTeardown, WorktreeError};

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

/// The branch `path` has checked out, read from the checkout itself. `None`
/// when the path is not a repository or HEAD is detached.
pub fn checked_out_branch(path: &Path) -> Option<String> {
    let repo = git2::Repository::open(path).ok()?;
    let head = repo.head().ok()?;
    if !head.is_branch() {
        return None;
    }
    head.shorthand().map(str::to_string)
}

/// Derive a filesystem- and branch-safe slug from a free-text goal.
///
/// Lowercases, collapses any run of non-alphanumerics to a single hyphen, trims
/// hyphens, truncates, and falls back to `task` if nothing survives.
pub fn slugify(goal: &str) -> String {
    let mut slug = String::new();
    let mut prev_hyphen = false;
    for ch in goal.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_hyphen = false;
        } else if !prev_hyphen && !slug.is_empty() {
            slug.push('-');
            prev_hyphen = true;
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug: String = slug.chars().take(50).collect();
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        "task".to_string()
    } else {
        slug
    }
}

/// Whether git would hold a branch under this name.
///
/// Git's own ref grammar, plus the two narrowings `git check-ref-format
/// --branch` makes that `is_valid_name` alone does not: a leading `-` would be
/// read as a flag wherever a name reaches an argv slot, and `HEAD` names the
/// pointer rather than a branch. Everything past this guard is a spelling git
/// could hold a branch under — which is the precondition
/// [`crate::gitgui::branch_origin`] needs. It says nothing about whether the
/// branch exists.
pub fn is_ref_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name != "HEAD"
        && git2::Reference::is_valid_name(&local_branch_ref(name))
}

/// Whether a caller-supplied branch name can be cut exactly as it was given.
///
/// A dispatch's `branch` is either a name or a description of one, and the two
/// are told apart here: git's own rules for a ref, narrowed to segments of
/// letters, digits, `.`, `_` and `-`. That narrowing is what makes the name safe
/// to fold into a directory as well as a ref — and it puts every sentence
/// ("Add CSV export, please") on the slugify path, where it belongs.
pub fn is_usable_branch_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 200 {
        return false;
    }
    let segments: Vec<&str> = name.split('/').collect();
    let segment_is_usable = |segment: &&str| {
        !segment.is_empty()
            && !segment.starts_with('.')
            && !segment.starts_with('-')
            && !segment.ends_with(".lock")
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    segments.iter().all(segment_is_usable) && is_ref_name(name)
}

/// Git's admin directory for a linked worktree, addressed from the repository
/// that registered it — readable whether or not the checkout is still on disk.
/// `Repository::path()` is the *caller's* admin directory, which is the linked
/// worktree's own when the project root is itself one, so the shared
/// `commondir` is what holds the `worktrees/<name>` entries.
fn admin_dir_for(repo: &git2::Repository, worktree_name: &str) -> PathBuf {
    repo.commondir().join("worktrees").join(worktree_name)
}

/// A branch with a local ref ready to be checked out: what teardown of the
/// checkout will own, and the ref this call made for it, if it made one.
struct PreparedBranch {
    teardown: BranchTeardown,
    created_ref: Option<CreatedLocalRef>,
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
struct CreatedLocalRef(String);

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

/// The one seam the orchestrator and the app talk to about materializing,
/// verifying, removing or enumerating a checkout of a single project.
///
/// It owns everything both isolations share — cutting and deleting branches,
/// choosing a unique name and directory, the checks a restore makes whatever
/// made the checkout, publish-before-read ordering — and routes the rest to a
/// backend. Creation takes the isolation the caller resolved; everything else
/// asks the checkout on disk what it is.
#[derive(Clone, Debug)]
pub struct WorktreeManager {
    repo_path: PathBuf,
    worktrees_root: PathBuf,
    worktree: WorktreeBackend,
    cow: CowBackend,
}

impl WorktreeManager {
    /// `repo_path` is the project git repo; `worktrees_root` is where task
    /// worktrees are materialized (one subdirectory per task slug). Branches
    /// are cut in the `build/` namespace.
    pub fn new(repo_path: impl Into<PathBuf>, worktrees_root: impl Into<PathBuf>) -> Self {
        WorktreeManager {
            repo_path: repo_path.into(),
            worktrees_root: worktrees_root.into(),
            worktree: WorktreeBackend,
            cow: CowBackend,
        }
    }

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
    fn prepare_existing_branch(
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
    fn checkout_branch(
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
    fn checkout_path(&self, name: &str) -> Result<PathBuf, WorktreeError> {
        std::fs::create_dir_all(&self.worktrees_root)?;
        Ok(self.worktrees_root.join(name))
    }

    /// Ask every backend to be rid of the checkout at `path`. Removal's goal is
    /// ABSENCE, and absence is success for every backend, so a checkout that is
    /// already gone and one that is still there take the same path — and a
    /// record left behind by an outside cleanup is cleared either way.
    pub fn remove_checkout(&self, path: &Path) -> Result<(), WorktreeError> {
        for backend in self.every_backend() {
            backend.remove(&self.repo_path, path)?;
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

    /// Every checkout of this project that is neither the project's own nor in
    /// `excluded` (Build-bound checkouts, which must never surface as
    /// adoptable, in whatever spelling the caller holds them: they are
    /// canonicalized here, with no lock held, so the decide phase that collects
    /// them makes no filesystem call), with a review summary each. Read-only apart
    /// from the base sync each checkout needs before its counts mean anything.
    /// A checkout whose summary cannot be computed is skipped — one broken
    /// stray must not fail the scan.
    pub fn discover(
        &self,
        base_branch: &str,
        excluded: &HashSet<PathBuf>,
    ) -> Result<Vec<ExternalWorktree>, WorktreeError> {
        let excluded: HashSet<PathBuf> = excluded.iter().map(|path| canonical_root(path)).collect();
        let mut paths: Vec<PathBuf> = Vec::new();
        for backend in self.every_backend() {
            for path in backend.discover(&self.repo_path, &self.worktrees_root)? {
                if !excluded.contains(&path) && !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
        let now = unix_now();
        let mut found: Vec<ExternalWorktree> = Vec::new();
        for path in paths {
            if let Err(error) = self.sync_base(&path, base_branch) {
                eprintln!("discover: base sync failed for {}: {error}", path.display());
            }
            if let Some(summary) = describe_checkout(&path, base_branch, now) {
                found.push(summary);
            }
        }
        found.sort_by(|a, b| {
            a.head_age_seconds
                .cmp(&b.head_age_seconds)
                .then_with(|| a.path.cmp(&b.path))
        });
        Ok(found)
    }

    /// The project's own checkout described in the shape adoption takes for any
    /// other, so one adoption path serves both. It is nobody's isolated copy —
    /// it is the repository — so it carries the default isolation.
    pub fn describe_primary(&self, base_branch: &str) -> Result<ExternalWorktree, WorktreeError> {
        let primary = std::fs::canonicalize(&self.repo_path)?;
        let isolation = Isolation::of(&primary).unwrap_or_default();
        summarize_checkout(&primary, isolation, base_branch, unix_now()).ok_or_else(|| {
            WorktreeError::Refused(format!(
                "the primary checkout at {} cannot be described — a bare or detached repository \
                 has no branch to adopt",
                primary.display()
            ))
        })
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

    /// The project repository every checkout here is cut from — what a caller
    /// reading the project's own refs, or naming the project a record belongs
    /// to, opens. Identity rather than variation: both isolations answer to
    /// the same repository.
    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    /// Which isolations this project can be checked out with on this volume.
    pub fn availability(&self) -> IsolationAvailability {
        IsolationAvailability::of(&self.repo_path, &self.worktrees_root)
    }

    // --- keyed dispatch ------------------------------------------------------

    /// One slot per isolation there is, in the order [`Isolation::ALL`] names
    /// them: the backend that makes it, or nothing when this build has none.
    /// Keyed to the enum by length, so it is the one list of backends and a new
    /// isolation cannot be added without filling in its slot here.
    fn backends(&self) -> [Option<&dyn IsolationBackend>; Isolation::ALL.len()] {
        [Some(&self.worktree), Some(&self.cow)]
    }

    /// Every backend this build has, in the order above — what the three walks
    /// that have no isolation to key on iterate.
    fn every_backend(&self) -> impl Iterator<Item = &dyn IsolationBackend> {
        self.backends().into_iter().flatten()
    }

    /// The backend that makes `isolation`, or why this volume cannot. An
    /// isolation with no backend is one [`IsolationAvailability`] locks, so the
    /// refusal is its sentence and there is no other.
    fn backend(&self, isolation: Isolation) -> Result<&dyn IsolationBackend, WorktreeError> {
        self.every_backend()
            .find(|backend| backend.kind() == isolation)
            .ok_or_else(|| {
                WorktreeError::IsolationUnavailable(
                    self.availability()
                        .lock_reason(isolation)
                        .expect(
                            "an isolation with no backend must be locked by IsolationAvailability",
                        )
                        .to_string(),
                )
            })
    }

    /// The backend that owns the checkout at `path`, which the checkout itself
    /// decides. A path that is no Build checkout names its own cause: no git
    /// command ran, so a git failure would be the wrong story.
    fn backend_of(&self, path: &Path) -> Result<&dyn IsolationBackend, WorktreeError> {
        let isolation = Isolation::of(path)
            .ok_or_else(|| WorktreeError::NotABuildCheckout(path.to_path_buf()))?;
        self.backend(isolation)
    }

    // --- internals -----------------------------------------------------------

    /// Write what teardown owns beside a checkout git has just registered, and
    /// take that registration back when the write fails.
    ///
    /// A registered checkout with no marker reads as one Build made for itself
    /// — the branch goes with it — so leaving one behind after failing to say
    /// otherwise hands somebody else's branch to the next teardown. Unwinding
    /// is best-effort because the error being returned is the one worth
    /// reporting; an unwind that fell short is reported too, since a checkout
    /// left standing unmarked is exactly what this exists to prevent.
    fn stamp_teardown_or_unwind(
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
        let mut taken_back = true;
        if let Ok(registered) = repo.find_worktree(name) {
            let mut prune = git2::WorktreePruneOptions::new();
            prune.valid(true).working_tree(true);
            if let Err(prune_error) = registered.prune(Some(&mut prune)) {
                eprintln!(
                    "worktree {name}: teardown marker unwritten and registration not pruned \
                     ({prune_error}); branch {branch} may be deleted by the next teardown"
                );
                taken_back = false;
            }
        }
        if path.exists() {
            if let Err(remove_error) = std::fs::remove_dir_all(path) {
                eprintln!(
                    "worktree {name}: teardown marker unwritten and directory not removed \
                     ({remove_error}); branch {branch} may be deleted by the next teardown"
                );
                taken_back = false;
            }
        }
        prepared.discard_created_ref(repo);
        if taken_back {
            return Err(error);
        }
        Err(WorktreeError::Command(format!(
            "could not record teardown for {name}, and its registration could not be taken \
             back: {error}"
        )))
    }

    /// Bring a branch that exists only on `remote` here: fetch exactly it,
    /// cut the local ref at what came back, and point that ref at the remote
    /// branch it came from, so the checkout is on the team's branch rather
    /// than a private copy of its name.
    fn materialise_remote_branch(
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
    fn cut_branch(
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

    // --- internals -----------------------------------------------------------

    /// The directory a named branch lands in: its segments joined by hyphens,
    /// minus Build's own namespace, which every directory here is already
    /// inside. `build/csv-export` → `csv-export`, `feature/csv-export` →
    /// `feature-csv-export`, so two namespaces never claim one directory.
    fn directory_name_for(&self, branch: &str) -> String {
        let mut segments: Vec<&str> = branch.split('/').collect();
        if segments.len() > 1 && segments[0] == BRANCH_PREFIX {
            segments.remove(0);
        }
        segments.join("-")
    }

    /// The directory a checkout can have to itself: `<stem>`, `<stem>-2`, …
    /// until nothing claims it. A name some backend holds a record of or that
    /// already exists on disk is claimed; `also_taken` adds whatever else the
    /// caller's own namespace claims.
    fn unique_checkout_name(
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
    fn checkout_name_taken(&self, name: &str) -> Result<bool, WorktreeError> {
        Ok(self.record_held(name)? || self.worktrees_root.join(name).exists())
    }

    /// Whether any backend holds a record of a checkout called `name`. A name
    /// carries no isolation, so this is one walk and every caller asks it here.
    fn record_held(&self, name: &str) -> Result<bool, WorktreeError> {
        for backend in self.every_backend() {
            if backend.holds_record(&self.repo_path, name)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether Build's own namespace already holds a branch for this slug.
    fn branch_taken(&self, repo: &git2::Repository, slug: &str) -> bool {
        repo.find_branch(&self.branch_name(slug), git2::BranchType::Local)
            .is_ok()
    }

    /// Where the checkout for `name` goes. The name a caller asks for is the
    /// name it gets unless [`create`](Self::create) has to suffix it, so this
    /// is where a checkout is expected rather than where one is.
    pub fn path_for(&self, name: &str) -> PathBuf {
        self.worktrees_root.join(name)
    }

    /// Build the branch name for a slug in Build's namespace.
    fn branch_name(&self, slug: &str) -> String {
        branch_name_for(slug)
    }

    /// Drop `branch` from the project repo. Only deletable once nothing has it
    /// checked out; already gone is done.
    fn delete_branch(&self, branch: &str) -> Result<(), WorktreeError> {
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
    fn refuse_outside_root(&self, worktree: &Worktree) -> Result<(), WorktreeError> {
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
        self.refuse_outside_root(worktree)?;
        if worktree.path.exists() {
            return self.verify_existing_checkout(worktree);
        }
        let repo = git2::Repository::open(&self.repo_path)?;
        let teardown = self.teardown_across_prune(&repo, worktree, when_unregistered)?;
        self.ensure_local_branch(&repo, &worktree.recorded_branch)?;
        let path = self.checkout_path(&worktree.name)?;
        self.backend(isolation)?
            .materialize(&self.repo_path, &worktree.recorded_branch, &path)?;
        record_branch_teardown(&path, teardown)?;
        self.verify_existing_checkout(worktree)
    }

    /// What teardown owns, taken out of the stale record before the prune that
    /// removes it. Only a record git says is *absent* leaves the environment
    /// unable to answer — then the caller's `when_unregistered` speaks, or
    /// refuses to. Every other git failure is surfaced, because a branch is
    /// deleted on the strength of this answer.
    // TODO-MERGE(Work Isolation Merge Brief §1): reading the record of a
    // checkout whose directory is gone is the tenth backend primitive,
    // `teardown_record`, not git's registry read directly.
    fn teardown_across_prune(
        &self,
        repo: &git2::Repository,
        worktree: &Worktree,
        when_unregistered: UnregisteredRestore,
    ) -> Result<BranchTeardown, WorktreeError> {
        match repo.find_worktree(&worktree.name) {
            Ok(stale) => {
                let recorded = teardown_in_git_dir(&admin_dir_for(repo, &worktree.name))?;
                let mut prune = git2::WorktreePruneOptions::new();
                prune.valid(true).working_tree(true);
                stale.prune(Some(&mut prune))?;
                Ok(recorded)
            }
            Err(error) if error.code() == git2::ErrorCode::NotFound => match when_unregistered {
                UnregisteredRestore::Write(teardown) => Ok(teardown),
                UnregisteredRestore::Refuse => Err(WorktreeError::Refused(format!(
                    "cannot restore {:?}: its worktree registration is gone, so whether \
                     teardown owns branch {:?} cannot be decided",
                    worktree.name, worktree.recorded_branch
                ))),
            },
            Err(error) => Err(error.into()),
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
                Ok(_) => self.teardown_of(&repo, worktree)?.deletes_branch(),
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
    fn publish_before_removal(&self, worktree: &Worktree) -> Result<(), WorktreeError> {
        if Isolation::of(&worktree.path).is_none() {
            return Ok(());
        }
        self.publish(&worktree.path, &worktree.recorded_branch)
    }

    /// What teardown of `worktree` owns. A checkout still on disk answers for
    /// itself, whatever made it; one that is gone is asked of the record its
    /// backend kept.
    // TODO-MERGE(Work Isolation Merge Brief §1): the vanished-checkout arm is
    // the tenth backend primitive, `teardown_record`, not git's registry.
    fn teardown_of(
        &self,
        repo: &git2::Repository,
        worktree: &Worktree,
    ) -> Result<BranchTeardown, WorktreeError> {
        if worktree.path.exists() {
            return branch_teardown(&worktree.path);
        }
        teardown_in_git_dir(&admin_dir_for(repo, &worktree.name))
    }

    /// Make sure the project repo has `branch` locally, fetching exactly it
    /// from its configured remote when it does not. The local ref is
    /// authoritative when present, and there is no fallback to the moving base:
    /// that would silently discard lineage.
    fn ensure_local_branch(
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
    fn verify_existing_checkout(&self, worktree: &Worktree) -> Result<Worktree, WorktreeError> {
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

    /// The checkout's canonical path, refused unless it resolves to somewhere
    /// under the worktrees root — a symlink or a bind mount pointing out of it
    /// is not the checkout that was recorded, whatever the recorded path spells.
    fn canonical_managed_path(&self, path: &Path) -> Result<PathBuf, WorktreeError> {
        let checkout = std::fs::canonicalize(path)?;
        if !checkout.starts_with(std::fs::canonicalize(&self.worktrees_root)?) {
            return Err(WorktreeError::Refused(
                "refusing to trust a worktree outside its canonical managed path".to_string(),
            ));
        }
        Ok(checkout)
    }

    /// What a restored checkout must be true of whatever made it: it is on the
    /// branch that was recorded for it, that branch is where the project repo
    /// says it is, and it grew out of the base it was cut from.
    fn verify_common(&self, worktree: &Worktree, checkout: &Path) -> Result<(), WorktreeError> {
        let head = head_on_recorded_branch(checkout, worktree)?;
        let project = git2::Repository::open(&self.repo_path)?;
        head_matches_project_tip(&project, head, worktree)?;
        shares_ancestry_with_base(&project, head, worktree)
    }
}

/// The commit the checkout has checked out, refused unless HEAD is the branch
/// the caller recorded for it.
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

/// Seconds since the epoch, the clock every checkout summary is aged against.
pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// Run a git subcommand in `dir`, mapping a non-zero exit to a readable error.
pub(crate) fn git_in(dir: &Path, args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// The `origin` remote URL of a repo, if it has one.
pub(crate) fn git_remote_origin(dir: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!url.is_empty()).then_some(url)
}

/// Whether two clone URLs point at the same repo, ignoring a trailing `/` or
/// `.git`. A loose check — enough to catch "already cloned" without surprises.
pub(crate) fn remotes_match(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.trim()
            .trim_end_matches('/')
            .trim_end_matches(".git")
            .to_string()
    };
    norm(a) == norm(b)
}

/// The checked-out branch name of a freshly cloned repo (its default branch).
pub(crate) fn git_default_branch(dir: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!branch.is_empty() && branch != "HEAD").then_some(branch)
}

/// Run a git subcommand in `dir` and hand back its stdout; a non-zero exit
/// becomes an error carrying whatever git said on either stream.
pub(crate) fn git_stdout(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = [stderr.trim(), stdout.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!("git {args:?}: {detail}"))
}

pub(crate) fn configured_remote_for_branch(
    repo: &git2::Repository,
    branch: &str,
) -> Option<String> {
    let config = repo.config().ok()?;
    let named = config
        .get_string(&format!("branch.{branch}.remote"))
        .ok()
        .or_else(|| config.get_string("remote.pushDefault").ok())
        .filter(|remote| remote != "." && !remote.trim().is_empty());
    if named.is_some() {
        return named;
    }
    if repo.find_remote("origin").is_ok() {
        return Some("origin".to_string());
    }
    let remotes = repo.remotes().ok()?;
    (remotes.len() == 1)
        .then(|| remotes.get(0).map(str::to_string))
        .flatten()
}

/// Fetch exactly `refspec` from `remote` into the repository at `repo_path`,
/// bounded and never prompting. A git that ran and failed says why in its own
/// words, through the one composer of a git-failure sentence.
pub(crate) fn bounded_git_fetch(
    repo_path: &Path,
    remote: &str,
    refspec: &str,
) -> Result<(), WorktreeError> {
    let args = [
        OsStr::new("fetch"),
        OsStr::new("--"),
        OsStr::new(remote),
        OsStr::new(refspec),
    ];
    let fetched = run_git_with_deadline(repo_path, &args)?;
    if !fetched.status.success() {
        return Err(git_failure(&args, &fetched).into());
    }
    Ok(())
}

/// One git worktree of the project repo that Build did not create (or no longer
/// tracks): the raw material of adoption. Pure data — discovery never mutates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalWorktree {
    /// Stable id: "wt-" + the first 12 hex chars of sha256 over the canonical
    /// absolute path (UTF-8 bytes of `path.display().to_string()`).
    pub id: String,
    /// The checkout's directory name, whatever made it — the name every
    /// backend calls it by, so adoption can build a `Worktree` that
    /// `WorktreeManager::remove` understands.
    pub name: String,
    /// Canonical absolute path of the working directory.
    pub path: PathBuf,
    /// How this checkout is isolated from the project, read from the checkout.
    pub isolation: Isolation,
    /// Checked-out branch, or None for a detached HEAD (browsable, not adoptable).
    pub branch: Option<String>,
    pub head_sha: String,
    /// HEAD commit subject (`%s`). UNTRUSTED display text.
    pub head_subject: String,
    /// Seconds since the HEAD commit's committer time (clamped at 0).
    pub head_age_seconds: u64,
    /// The HEAD commit's committer time (RFC 3339 UTC). The same fact as
    /// `head_age_seconds` told as an instant rather than a duration, because
    /// the inbox sorts and buckets by instants and an age recomputed every poll
    /// would jitter under the sort. `None` when the stamp cannot be read.
    pub head_committed_at: Option<String>,
    /// `git status --porcelain` line count — staged + unstaged + untracked.
    pub dirty_files: usize,
    /// Commits ahead of [`comparison_ref`](Self::comparison_ref). Retained for
    /// finish-action warnings that describe work the selected ref does not have.
    pub unpushed: Option<u64>,
    /// The configured upstream, or `None` when comparison falls back to the
    /// project's local base branch.
    pub upstream: Option<String>,
    /// The one ref both commit-direction counts are measured against: upstream
    /// when configured, otherwise the project's local base branch.
    pub comparison_ref: Option<String>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    /// Roll-up of `diff_against_merge_base(path, base_branch)` (§2).
    pub diffstat: crate::diff::DiffStat,
    /// The working tree's own uncommitted delta: HEAD vs the index and working
    /// directory, untracked included. What the rail shows as +/− — "what is
    /// sitting here unsaved", which is a different question from how far the
    /// branch has travelled (that is `ahead`/`behind`).
    pub uncommitted: crate::diff::DiffStat,
}

/// A git timestamp (seconds since the epoch) as RFC 3339 UTC — the one
/// timestamp format every surface of the bridge speaks.
pub fn rfc3339_from_unix(seconds: i64) -> Option<String> {
    time::OffsetDateTime::from_unix_timestamp(seconds)
        .ok()?
        .format(&time::format_description::well_known::Rfc3339)
        .ok()
}

const CHECKOUT_ID_PREFIX: &str = "wt-";
const CHECKOUT_ID_DIGITS: usize = 12;

/// The canonical form of a checkout root — the spelling every id, registry key
/// and cache entry is minted from. Falls back to the path as given when the
/// directory cannot answer (it is gone, or it does not exist yet), so a
/// vanished checkout and one still to be cut both key consistently.
pub fn canonical_root(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The canonical spelling a path WILL have once it exists: the deepest ancestor
/// that does exist, canonicalized, with the missing segments joined back on.
///
/// A checkout's id is minted from its canonical path, so the row that stands
/// for one before `git worktree add` has run has to carry the id the finished
/// checkout will — and on macOS the directory a checkout is about to be made in
/// has two literal spellings.
pub fn canonical_planned_path(path: &Path) -> PathBuf {
    let mut missing: Vec<&std::ffi::OsStr> = Vec::new();
    let mut ancestor = path;
    loop {
        if let Ok(canonical) = std::fs::canonicalize(ancestor) {
            return missing
                .iter()
                .rev()
                .fold(canonical, |resolved, segment| resolved.join(segment));
        }
        match (ancestor.parent(), ancestor.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name);
                ancestor = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// The stable external-worktree id for a canonical absolute path.
pub fn external_worktree_id(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(path.display().to_string().as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{CHECKOUT_ID_PREFIX}{}", &hex[..CHECKOUT_ID_DIGITS])
}

/// Whether `id` was minted by [`external_worktree_id`]. The one id shape whose
/// only liveness test is the scan, so the one a caller has to be able to tell
/// apart from a run, a plan or a row.
pub fn is_checkout_id(id: &str) -> bool {
    id.strip_prefix(CHECKOUT_ID_PREFIX).is_some_and(|hex| {
        hex.len() == CHECKOUT_ID_DIGITS && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

/// Branch stems that carry no meaningful goal on their own — adoption falls
/// back to the HEAD commit subject for these.
const GENERIC_BRANCH_STEMS: &[&str] = &[
    "main", "master", "dev", "develop", "wip", "tmp", "temp", "test", "testing", "scratch",
    "patch", "fix", "feature", "new", "branch",
];

/// The silently derived goal for an adopted worktree: the branch name verbatim,
/// unless the branch is generic — then the HEAD commit subject.
pub fn derive_adoption_goal(branch: &str, head_subject: &str) -> String {
    let segment = branch.rsplit('/').next().unwrap_or(branch).to_lowercase();
    let stem = strip_trailing_digit_run(&segment);
    let is_generic = stem.is_empty() || GENERIC_BRANCH_STEMS.contains(&stem.as_str());

    if !is_generic {
        return branch.to_string();
    }
    let subject = head_subject.trim();
    if !subject.is_empty() {
        subject.to_string()
    } else if !branch.is_empty() {
        branch.to_string()
    } else {
        "Adopted worktree".to_string()
    }
}

/// Strip one trailing run of ASCII digits, and the single `-`/`_` immediately
/// before that run, from a branch segment (`wip-2` -> `wip`, `test_3` -> `test`).
fn strip_trailing_digit_run(segment: &str) -> String {
    let chars: Vec<char> = segment.chars().collect();
    let mut end = chars.len();
    while end > 0 && chars[end - 1].is_ascii_digit() {
        end -= 1;
    }
    if end == chars.len() {
        return segment.to_string();
    }
    if end > 0 && (chars[end - 1] == '-' || chars[end - 1] == '_') {
        end -= 1;
    }
    chars[..end].iter().collect()
}

/// The order a checkout list is served in: freshest commit first, canonical
/// path as the tie-break. Shared, because a list amended in place has to stay
/// in the order the scan that filled it used.
pub fn sort_checkouts(checkouts: &mut [ExternalWorktree]) {
    checkouts.sort_by(|a, b| {
        a.head_age_seconds
            .cmp(&b.head_age_seconds)
            .then_with(|| a.path.cmp(&b.path))
    });
}

/// The two facts a branch listing stamps a row with about the primary
/// checkout at `repo_path`: the worktree id `run.adopt` adopts it by (the same
/// id [`find_primary_checkout`] mints) and the branch it holds. Read straight
/// off the repository — no status walk, no diffstat, no subprocess — because a
/// listing is the drain's to answer, not a description to render. `None` when
/// the checkout holds no branch: a detached or unborn HEAD, or a bare
/// repository with no working tree. A path git cannot read as a repository is
/// broken rather than branch-less, and says so through the error.
pub fn primary_checkout_holder(
    repo_path: &Path,
) -> Result<Option<(String, String)>, WorktreeError> {
    let canonical_path = std::fs::canonicalize(repo_path)?;
    let repo = git2::Repository::open(&canonical_path)?;
    if repo.is_bare() {
        return Ok(None);
    }
    let head = match repo.head() {
        Ok(head) => head,
        Err(error) if error.code() == git2::ErrorCode::UnbornBranch => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !head.is_branch() {
        return Ok(None);
    }
    Ok(head
        .shorthand()
        .map(|branch| (external_worktree_id(&canonical_path), branch.to_string())))
}

/// Summarize the Build checkout at `path` from the checkout alone, so a clone
/// and a linked worktree are described by one function. `None` when the path is
/// no Build checkout — described by nobody rather than described as something
/// it is not — or when the summary cannot be computed.
pub fn describe_checkout(path: &Path, base_branch: &str, now: i64) -> Option<ExternalWorktree> {
    summarize_checkout(path, Isolation::of(path)?, base_branch, now)
}

/// The summary itself, for a checkout whose isolation the caller already knows.
/// Every fact comes from the checkout: its own repository answers for HEAD, its
/// working tree for what is dirty, and its refs for how far the branch has
/// travelled. A checkout that cannot answer for one of them is skipped, saying
/// so — one broken stray must not fail a scan.
fn summarize_checkout(
    path: &Path,
    isolation: Isolation,
    base_branch: &str,
    now: i64,
) -> Option<ExternalWorktree> {
    let canonical = std::fs::canonicalize(path).ok()?;
    let name = checkout_name(&canonical)?;
    let repo = or_skip(
        "opening the repository",
        &canonical,
        git2::Repository::open(&canonical),
    )?;
    let head = or_skip("HEAD", &canonical, repo.head())?;
    let branch = head
        .is_branch()
        .then(|| head.shorthand().map(str::to_string))
        .flatten();
    let head_oid = head.target()?;
    let commit = or_skip("the HEAD commit", &canonical, repo.find_commit(head_oid))?;
    let dirty_files = or_skip("status", &canonical, worktree_status_line_count(&canonical))?;
    // Counts only — the board never shows this tree's patch, so never render one.
    let diffstat = or_skip(
        "the diff against the merge base",
        &canonical,
        crate::diff::stat_against_merge_base(&canonical, base_branch),
    )?;
    // What is sitting in this tree unsaved — the +/− the rail shows. Distinct
    // from the diffstat above, which is everything the branch carries.
    let uncommitted = or_skip(
        "the uncommitted diff",
        &canonical,
        crate::diff::stat_uncommitted(&canonical),
    )?;
    let comparison = branch_comparison(&repo, &commit, branch.as_deref(), base_branch);

    Some(ExternalWorktree {
        id: external_worktree_id(&canonical),
        name,
        path: canonical,
        isolation,
        branch,
        head_sha: head_oid.to_string(),
        head_subject: commit.summary().unwrap_or("").to_string(),
        head_age_seconds: (now - commit.time().seconds()).max(0) as u64,
        head_committed_at: rfc3339_from_unix(commit.time().seconds()),
        dirty_files,
        unpushed: comparison.ahead,
        upstream: comparison.upstream,
        comparison_ref: comparison.reference,
        ahead: comparison.ahead,
        behind: comparison.behind,
        diffstat,
        uncommitted,
    })
}

/// A fact a checkout summary needs, or nothing — and the one place a skipped
/// checkout says why it was skipped.
fn or_skip<T, E: std::fmt::Display>(what: &str, path: &Path, fact: Result<T, E>) -> Option<T> {
    fact.inspect_err(|error| {
        eprintln!("summarize_checkout: {what} for {}: {error}", path.display());
    })
    .ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BranchComparison {
    pub reference: Option<String>,
    pub upstream: Option<String>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
}

/// Compare HEAD in both directions with its configured upstream, falling back
/// to the local base branch only when there is no upstream.
pub(crate) fn branch_comparison(
    repo: &git2::Repository,
    head: &git2::Commit,
    branch: Option<&str>,
    base_branch: &str,
) -> BranchComparison {
    if let Some((name, oid)) = branch.and_then(|b| upstream_of(repo, b)) {
        let counts = repo.graph_ahead_behind(head.id(), oid).ok();
        return BranchComparison {
            reference: Some(name.clone()),
            upstream: Some(name),
            ahead: counts.map(|(ahead, _)| ahead as u64),
            behind: counts.map(|(_, behind)| behind as u64),
        };
    }
    let counts = resolve_commit(repo, base_branch)
        .and_then(|base| repo.graph_ahead_behind(head.id(), base.id()).ok());
    BranchComparison {
        reference: counts.map(|_| base_branch.to_string()),
        upstream: None,
        ahead: counts.map(|(ahead, _)| ahead as u64),
        behind: counts.map(|(_, behind)| behind as u64),
    }
}

/// The commit a revspec names, or `None` when it does not resolve.
fn resolve_commit<'repo>(
    repo: &'repo git2::Repository,
    revspec: &str,
) -> Option<git2::Commit<'repo>> {
    repo.revparse_single(revspec)
        .ok()
        .and_then(|object| object.peel_to_commit().ok())
}

/// A local branch's upstream, as (ref shorthand, tip) — `None` when the branch
/// tracks nothing, or its upstream ref is gone.
fn upstream_of(repo: &git2::Repository, branch: &str) -> Option<(String, git2::Oid)> {
    let upstream = repo
        .find_branch(branch, git2::BranchType::Local)
        .ok()?
        .upstream()
        .ok()?;
    let name = upstream.name().ok().flatten()?.to_string();
    let oid = upstream.get().target()?;
    Some((name, oid))
}

/// Count of non-empty `git status --porcelain` lines in `worktree_path`.
fn worktree_status_line_count(worktree_path: &Path) -> Result<usize, WorktreeError> {
    Ok(run_git(worktree_path, &["status", "--porcelain"])?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_fixture::{git_in, init_repo, init_repo_named};
    use crate::isolation::probe::cow_or_skip;
    use crate::isolation::BRANCH_TEARDOWN_MARKER;
    use std::path::Path;
    use std::process::Command;

    fn manager(dir: &tempfile::TempDir, repo: &Path) -> WorktreeManager {
        WorktreeManager::new(repo, dir.path().join("worktrees"))
    }

    /// Git's own branch-name rule, which is wider than the one Build cuts
    /// under: anything `git branch` would accept is a name git can hold a
    /// branch under, and so a name a checkout can be asked for.
    #[test]
    fn a_ref_name_is_one_git_branch_would_accept() {
        for name in [
            "build/csv-export",
            "wip@2",
            "feature/foo+bar",
            "release-1.2",
            "ünïcode",
        ] {
            assert!(is_ref_name(name), "{name:?} is a name git would take");
        }
        for name in [
            "",
            "HEAD",
            "-dashed",
            "add a csv export",
            "build/",
            "build//x",
            "build/..",
            "back\\slash",
            "star*",
            "tilde~1",
            "at@{brace}",
        ] {
            assert!(!is_ref_name(name), "{name:?} is not a branch name");
        }
    }

    #[test]
    fn a_usable_branch_name_is_one_git_and_the_filesystem_both_take() {
        for name in [
            "build/csv-export",
            "csv-export",
            "feature/api/v2",
            "release-1.2",
            "fix_the_thing",
        ] {
            assert!(is_usable_branch_name(name), "{name:?} is a branch name");
        }
        for name in [
            "",
            "add a csv export",
            "Add CSV export, please",
            "build/",
            "/build",
            "build//x",
            "-dashed",
            ".hidden",
            "build/..",
            "build/x.lock",
            "back\\slash",
            "star*",
            "tilde~1",
        ] {
            assert!(
                !is_usable_branch_name(name),
                "{name:?} is a description, not a branch name"
            );
        }
    }

    /// A name the caller gave is a name, not a description: the branch is cut
    /// exactly as asked, and the directory it lands in is derived from it.
    #[test]
    fn create_cutting_branch_cuts_the_branch_exactly_as_it_was_named() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);

        let prefixed = mgr
            .create_cutting_branch("build/csv-export", "main", Isolation::Worktree)
            .unwrap();
        assert_eq!(prefixed.worktree.recorded_branch, "build/csv-export");
        assert_eq!(prefixed.worktree.name, "csv-export");
        assert_eq!(
            prefixed.teardown,
            BranchTeardown::DeletesBranch,
            "nothing was on that name before"
        );
        assert_eq!(
            branch_teardown(&prefixed.worktree.path).unwrap(),
            BranchTeardown::DeletesBranch
        );
        assert!(prefixed.worktree.path.join("README.md").exists());

        // A name with no namespace stays with no namespace: nothing is added to
        // what the caller asked for.
        let plain = mgr
            .create_cutting_branch("hotfix", "main", Isolation::Worktree)
            .unwrap();
        assert_eq!(plain.worktree.recorded_branch, "hotfix");
        assert_eq!(plain.worktree.name, "hotfix");

        // A namespace that is not this manager's is kept whole in the directory
        // name, so two branches never share one directory.
        let foreign = mgr
            .create_cutting_branch("feature/csv-export", "main", Isolation::Worktree)
            .unwrap();
        assert_eq!(foreign.worktree.recorded_branch, "feature/csv-export");
        assert_eq!(foreign.worktree.name, "feature-csv-export");

        let r = git2::Repository::open(&repo).unwrap();
        for branch in ["build/csv-export", "hotfix", "feature/csv-export"] {
            assert!(
                r.find_branch(branch, git2::BranchType::Local).is_ok(),
                "{branch} was cut"
            );
        }
    }

    /// A branch that already exists is checked out, not cut again — dispatching
    /// onto work someone started by hand is the whole point of naming a branch.
    #[test]
    fn create_on_existing_branch_checks_out_a_branch_that_already_exists() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let r = git2::Repository::open(&repo).unwrap();
        let head = r.head().unwrap().peel_to_commit().unwrap();
        r.branch("build/started-by-hand", &head, false).unwrap();

        let added = mgr
            .create_on_existing_branch("build/started-by-hand", "main", Isolation::Worktree)
            .unwrap();

        assert_eq!(added.worktree.recorded_branch, "build/started-by-hand");
        assert_eq!(
            added.teardown,
            BranchTeardown::KeepsBranch,
            "the branch was already there, and removing this checkout must not take it"
        );
        assert_eq!(
            branch_teardown(&added.worktree.path).unwrap(),
            BranchTeardown::KeepsBranch
        );
        let checkout = git2::Repository::open(&added.worktree.path).unwrap();
        assert_eq!(
            checkout.head().unwrap().shorthand(),
            Some("build/started-by-hand")
        );

        // And teardown reads that answer for itself.
        mgr.remove(&added.worktree).unwrap();
        assert!(r
            .find_branch("build/started-by-hand", git2::BranchType::Local)
            .is_ok());
    }

    #[test]
    fn create_cutting_branch_refuses_a_name_that_is_not_a_branch_name() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let refused = mgr.create_cutting_branch("add a csv export", "main", Isolation::Worktree);
        assert!(refused.is_err(), "{refused:?}");
    }

    /// A branch only a remote carries is fetched, made local with its upstream
    /// set, and checked out — never cut fresh over the top of the work it
    /// already holds.
    #[test]
    fn create_on_existing_branch_materialises_a_branch_only_a_remote_carries() {
        let (dir, repo) = init_repo();
        push_feature_x_from_another_clone(&dir, &repo);
        let mgr = manager(&dir, &repo);

        let added = mgr
            .create_on_existing_branch("feature-x", "main", Isolation::Worktree)
            .unwrap();

        assert_eq!(added.teardown, BranchTeardown::KeepsBranch);
        assert_eq!(
            branch_teardown(&added.worktree.path).unwrap(),
            BranchTeardown::KeepsBranch
        );
        assert_eq!(
            std::fs::read_to_string(added.worktree.path.join("theirs.txt")).unwrap(),
            "their work\n",
            "the checkout carries the work the remote branch already had"
        );
        let r = git2::Repository::open(&repo).unwrap();
        let config = r.config().unwrap();
        assert_eq!(
            config.get_string("branch.feature-x.remote").unwrap(),
            "origin"
        );
        assert_eq!(
            config.get_string("branch.feature-x.merge").unwrap(),
            "refs/heads/feature-x"
        );
    }

    /// A caller that named a branch meant that branch. When nothing anywhere
    /// holds it, the answer is an error — never a fresh empty branch wearing
    /// its name.
    #[test]
    fn create_on_existing_branch_refuses_a_branch_no_ref_holds() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);

        let refused = mgr
            .create_on_existing_branch("nobody-cut-this", "main", Isolation::Worktree)
            .unwrap_err()
            .to_string();

        assert!(refused.contains("nobody-cut-this"), "{refused}");
        let r = git2::Repository::open(&repo).unwrap();
        assert!(r
            .find_branch("nobody-cut-this", git2::BranchType::Local)
            .is_err());
        assert!(!dir.path().join("worktrees/nobody-cut-this").exists());
    }

    /// The one network-facing step in a checkout: the remote no longer has the
    /// branch its tracking ref promised. The error names the remote and the
    /// branch, and nothing half-made survives it — no local ref, no directory.
    #[test]
    fn create_on_existing_branch_surfaces_a_fetch_that_no_longer_carries_the_branch() {
        let (dir, repo) = init_repo();
        let origin = push_feature_x_from_another_clone(&dir, &repo);
        git_in(&origin, &["branch", "-D", "feature-x"]);
        let mgr = manager(&dir, &repo);

        let error = mgr
            .create_on_existing_branch("feature-x", "main", Isolation::Worktree)
            .unwrap_err()
            .to_string();

        assert!(error.contains("origin"), "{error}");
        assert!(error.contains("feature-x"), "{error}");
        let r = git2::Repository::open(&repo).unwrap();
        assert!(
            r.find_branch("feature-x", git2::BranchType::Local).is_err(),
            "no half-made local ref"
        );
        assert!(
            !dir.path().join("worktrees/feature-x").exists(),
            "no half-made checkout"
        );
    }

    /// A branch this call cut for itself must not outlive a checkout that
    /// never happened: left behind as a local ref, a retry would find it,
    /// borrow it, and never let teardown delete it again.
    #[test]
    fn create_cutting_branch_takes_back_the_branch_it_cut_when_the_checkout_fails() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let worktrees = dir.path().join("worktrees");
        std::fs::create_dir_all(&worktrees).unwrap();
        std::fs::set_permissions(&worktrees, std::fs::Permissions::from_mode(0o500)).unwrap();

        let error = mgr
            .create_cutting_branch("fresh-cut", "main", Isolation::Worktree)
            .unwrap_err();

        std::fs::set_permissions(&worktrees, std::fs::Permissions::from_mode(0o700)).unwrap();
        let r = git2::Repository::open(&repo).unwrap();
        assert!(
            r.find_branch("fresh-cut", git2::BranchType::Local).is_err(),
            "the branch it cut was left behind after {error}"
        );
        assert!(!worktrees.join("fresh-cut").exists());
    }

    /// The same for a branch it made local from a remote: neither the ref nor
    /// the tracking config it wrote for it survive a checkout that failed.
    #[test]
    fn create_on_existing_branch_takes_back_a_branch_it_materialised_when_the_checkout_fails() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, repo) = init_repo();
        push_feature_x_from_another_clone(&dir, &repo);
        let mgr = manager(&dir, &repo);
        let worktrees = dir.path().join("worktrees");
        std::fs::create_dir_all(&worktrees).unwrap();
        std::fs::set_permissions(&worktrees, std::fs::Permissions::from_mode(0o500)).unwrap();

        let error = mgr
            .create_on_existing_branch("feature-x", "main", Isolation::Worktree)
            .unwrap_err();

        std::fs::set_permissions(&worktrees, std::fs::Permissions::from_mode(0o700)).unwrap();
        let r = git2::Repository::open(&repo).unwrap();
        assert!(
            r.find_branch("feature-x", git2::BranchType::Local).is_err(),
            "the branch it materialised was left behind after {error}"
        );
        let config = r.config().unwrap();
        assert!(
            config.get_string("branch.feature-x.remote").is_err(),
            "its tracking config was left behind"
        );
        assert!(
            r.find_reference("refs/remotes/origin/feature-x").is_ok(),
            "the remote-tracking ref was never this call's to take"
        );
    }

    /// The marker is all that stands between a borrowed branch and the next
    /// teardown, so a registration it cannot be written into is taken back.
    /// Left behind, that checkout would read as one Build cut the branch for,
    /// and a `worktree.finish delete` on it would take somebody else's work.
    #[test]
    fn a_checkout_whose_teardown_cannot_be_stamped_is_taken_back() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let r = git2::Repository::open(&repo).unwrap();
        let head = r.head().unwrap().peel_to_commit().unwrap();
        r.branch("theirs", &head, false).unwrap();
        let added = mgr
            .create_on_existing_branch("theirs", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        let marker = repo
            .join(".git/worktrees")
            .join(&added.name)
            .join(BRANCH_TEARDOWN_MARKER);
        std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o400)).unwrap();

        let borrowed = PreparedBranch {
            teardown: BranchTeardown::KeepsBranch,
            created_ref: None,
        };
        let error = mgr
            .stamp_teardown_or_unwind(&r, "theirs", &added.name, &added.path, &borrowed)
            .unwrap_err();

        assert!(matches!(error, WorktreeError::Io(_)), "{error:?}");
        assert!(
            r.find_branch("theirs", git2::BranchType::Local).is_ok(),
            "the branch it only borrowed survives"
        );
        assert!(
            r.find_worktree(&added.name).is_err(),
            "no registration is left over the branch without a marker"
        );
        assert!(!added.path.exists(), "no half-made checkout");
    }

    /// The same unwind, for a checkout whose branch this call cut: the branch
    /// goes with the registration, so nothing of the failed checkout is left
    /// for a retry to mistake for somebody else's work.
    #[test]
    fn a_checkout_whose_teardown_cannot_be_stamped_takes_the_branch_it_cut_with_it() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let r = git2::Repository::open(&repo).unwrap();
        let added = mgr
            .create_cutting_branch("fresh-cut", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        let marker = repo
            .join(".git/worktrees")
            .join(&added.name)
            .join(BRANCH_TEARDOWN_MARKER);
        std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o400)).unwrap();

        let cut = PreparedBranch {
            teardown: BranchTeardown::DeletesBranch,
            created_ref: Some(CreatedLocalRef("fresh-cut".to_string())),
        };
        let error = mgr
            .stamp_teardown_or_unwind(&r, "fresh-cut", &added.name, &added.path, &cut)
            .unwrap_err();

        assert!(matches!(error, WorktreeError::Io(_)), "{error:?}");
        assert!(
            r.find_branch("fresh-cut", git2::BranchType::Local).is_err(),
            "the branch it cut was left behind"
        );
        assert!(r.find_worktree(&added.name).is_err());
        assert!(!added.path.exists());
    }

    /// A bare clone of `repo` wired up as its `origin`.
    /// A `feature-x` that exists only on the remote: pushed from a second
    /// clone and fetched here, so `origin/feature-x` is known but no local
    /// `feature-x` is. Answers the bare origin it was pushed to.
    fn push_feature_x_from_another_clone(dir: &tempfile::TempDir, repo: &Path) -> PathBuf {
        let origin = bare_origin_of(dir, repo);
        let other = dir.path().join("other");
        git_in(
            dir.path(),
            &["clone", origin.to_str().unwrap(), other.to_str().unwrap()],
        );
        git_in(&other, &["config", "user.email", "o@build.ing"]);
        git_in(&other, &["config", "user.name", "O"]);
        git_in(&other, &["checkout", "-b", "feature-x"]);
        std::fs::write(other.join("theirs.txt"), "their work\n").unwrap();
        git_in(&other, &["add", "."]);
        git_in(&other, &["commit", "-m", "their work"]);
        git_in(&other, &["push", "origin", "feature-x"]);
        git_in(repo, &["fetch", "origin"]);
        origin
    }

    fn bare_origin_of(dir: &tempfile::TempDir, repo: &Path) -> PathBuf {
        let origin = dir.path().join("origin.git");
        git_in(
            dir.path(),
            &[
                "clone",
                "--bare",
                repo.to_str().unwrap(),
                origin.to_str().unwrap(),
            ],
        );
        git_in(repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
        origin
    }

    /// A checkout Build cut the branch for says so where the fact survives
    /// everything but the checkout itself: git's own admin directory for it,
    /// which git prunes when the worktree goes.
    #[test]
    fn a_checkout_build_cut_the_branch_for_says_teardown_owns_it() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr
            .create("owned", "main", Isolation::Worktree)
            .unwrap()
            .worktree;

        assert_eq!(
            branch_teardown(&wt.path).unwrap(),
            BranchTeardown::DeletesBranch
        );
        assert!(
            repo.join(".git/worktrees/owned/build-branch-teardown")
                .is_file(),
            "the fact lives in git's admin directory for the checkout"
        );
    }

    /// A checkout nobody marked is one Build did not create — a worktree made
    /// by hand and adopted — and the finish action the human chose on it
    /// speaks for its branch, exactly as it did before markers existed.
    #[test]
    fn an_unmarked_checkout_leaves_its_branch_to_the_action_chosen() {
        let (dir, repo) = init_repo();
        let by_hand = dir.path().join("by-hand");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                by_hand.to_str().unwrap(),
                "-b",
                "made-by-hand",
            ],
        );

        assert_eq!(
            branch_teardown(&by_hand).unwrap(),
            BranchTeardown::DeletesBranch
        );
    }

    /// Git 2.48+ writes a relative `gitdir:` pointer when
    /// `worktree.useRelativePaths` is set. A relative pointer is a valid
    /// pointer, resolved against the checkout that holds it.
    #[test]
    fn branch_teardown_follows_a_relative_gitdir_pointer() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr
            .create("relative-pointer", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        record_branch_teardown(&wt.path, BranchTeardown::KeepsBranch).unwrap();
        let admin = repo.join(".git/worktrees/relative-pointer");
        let relative = pathdiff_from(&wt.path, &admin);
        std::fs::write(wt.path.join(".git"), format!("gitdir: {relative}\n")).unwrap();

        assert_eq!(
            branch_teardown(&wt.path).unwrap(),
            BranchTeardown::KeepsBranch
        );
    }

    /// Guessing here deletes somebody's branch, so nothing is guessed: a
    /// pointer that resolves to no directory is an error, not an answer.
    #[test]
    fn branch_teardown_errors_rather_than_guessing_when_it_cannot_read() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr
            .create("unreadable", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        std::fs::write(wt.path.join(".git"), "gitdir: /nowhere/at/all\n").unwrap();
        assert!(branch_teardown(&wt.path).is_err());

        std::fs::write(wt.path.join(".git"), "not a pointer at all\n").unwrap();
        assert!(branch_teardown(&wt.path).is_err());

        let admin = repo.join(".git/worktrees/unreadable");
        std::fs::write(admin.join("build-branch-teardown"), "gibberish").unwrap();
        std::fs::write(
            wt.path.join(".git"),
            format!("gitdir: {}\n", admin.display()),
        )
        .unwrap();
        assert!(branch_teardown(&wt.path).is_err());

        assert!(branch_teardown(&dir.path().join("no-such-checkout")).is_err());
    }

    /// The relative path from `from` to `to`, for a test that writes the
    /// pointer git itself would write with relative paths turned on.
    fn pathdiff_from(from: &Path, to: &Path) -> String {
        let from = std::fs::canonicalize(from).unwrap();
        let to = std::fs::canonicalize(to).unwrap();
        let shared = from
            .components()
            .zip(to.components())
            .take_while(|(a, b)| a == b)
            .count();
        let ups = from.components().count() - shared;
        let mut path = PathBuf::from("../".repeat(ups).trim_end_matches('/'));
        path.extend(to.components().skip(shared));
        path.display().to_string()
    }

    #[test]
    fn slugify_is_branch_safe() {
        assert_eq!(
            slugify("Fix the typo in the README"),
            "fix-the-typo-in-the-readme"
        );
        assert_eq!(slugify("  Add OAuth!! support  "), "add-oauth-support");
        assert_eq!(slugify("***"), "task");
        assert_eq!(slugify(""), "task");
        assert!(slugify(&"x".repeat(200)).len() <= 50);
        assert!(!slugify("trailing punctuation...").ends_with('-'));
    }

    #[test]
    fn create_makes_branch_and_working_dir() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);

        let wt = mgr
            .create("fix-typo", "main", Isolation::Worktree)
            .unwrap()
            .worktree;

        assert_eq!(wt.recorded_branch, "build/fix-typo");
        assert_eq!(wt.base_branch, "main");
        assert!(wt.path.join("README.md").exists(), "worktree has the files");

        // The branch exists in the repo.
        let r = git2::Repository::open(&repo).unwrap();
        assert!(r
            .find_branch("build/fix-typo", git2::BranchType::Local)
            .is_ok());
        // And the worktree is registered.
        assert!(r.worktrees().unwrap().iter().any(|n| n == Some("fix-typo")));
    }

    #[test]
    fn two_worktrees_on_one_repo_are_independent() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);

        let a = mgr
            .create("task-a", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        let b = mgr
            .create("task-b", "main", Isolation::Worktree)
            .unwrap()
            .worktree;

        assert_ne!(a.path, b.path);
        assert!(a.path.join("README.md").exists());
        assert!(b.path.join("README.md").exists());

        // A change in one worktree's branch does not appear in the other.
        std::fs::write(a.path.join("only-a.txt"), "a").unwrap();
        assert!(!b.path.join("only-a.txt").exists());
    }

    #[test]
    fn create_disambiguates_on_collision() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let a = mgr
            .create("dup", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        let b = mgr
            .create("dup", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        let c = mgr
            .create("dup", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        assert_eq!(a.name, "dup");
        assert_eq!(b.name, "dup-2");
        assert_eq!(c.name, "dup-3");
        assert_eq!(b.recorded_branch, "build/dup-2");
        assert!(b.path.join("README.md").exists());
    }

    /// An abandoned run's work outlives the run so it can be re-attempted, so
    /// its checkout goes and its branch stays — a promise about the branch
    /// that the checkout's own teardown marker does not make.
    #[test]
    fn removing_a_checkout_while_keeping_its_branch_leaves_the_branch() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr
            .create("keep-me", "main", Isolation::Worktree)
            .unwrap()
            .worktree;

        mgr.remove_keeping_branch(&wt).unwrap();

        assert!(!wt.path.exists(), "working dir removed");
        let r = git2::Repository::open(&repo).unwrap();
        assert!(!r.worktrees().unwrap().iter().any(|n| n == Some("keep-me")));
        assert!(
            r.find_branch("build/keep-me", git2::BranchType::Local)
                .is_ok(),
            "branch kept"
        );
    }

    #[test]
    fn restore_recreates_the_original_worktree_from_its_local_branch() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr
            .create("recover-local", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        std::fs::write(wt.path.join("stage.txt"), "kept\n").unwrap();
        git_in(&wt.path, &["add", "stage.txt"]);
        git_in(&wt.path, &["commit", "-m", "stage"]);
        let head = git2::Repository::open(&wt.path)
            .unwrap()
            .head()
            .unwrap()
            .target()
            .unwrap();
        git_in(
            &repo,
            &["worktree", "remove", "--force", wt.path.to_str().unwrap()],
        );

        let restored = mgr
            .restore(
                &wt,
                UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
                Isolation::Worktree,
            )
            .unwrap();
        assert_eq!(restored, wt);
        assert_eq!(
            branch_teardown(&restored.path).unwrap(),
            BranchTeardown::DeletesBranch
        );
        assert_eq!(
            std::fs::read_to_string(wt.path.join("stage.txt")).unwrap(),
            "kept\n"
        );
        assert_eq!(
            git2::Repository::open(&wt.path)
                .unwrap()
                .head()
                .unwrap()
                .target()
                .unwrap(),
            head
        );
    }

    /// The status count runs its git child through the one runner, so a
    /// failure carries what git said and which command said it.
    #[test]
    fn a_status_count_that_fails_says_which_command_failed() {
        let dir = tempfile::tempdir().unwrap();

        let failure = worktree_status_line_count(dir.path())
            .unwrap_err()
            .to_string();

        assert!(failure.contains("status"), "{failure}");
        assert!(failure.contains("not a git repository"), "{failure}");
    }

    /// The checks a restore makes on a checkout that is still there, whatever
    /// made it: it is on the branch that was recorded for it, and that branch
    /// grew out of the base it was cut from.
    #[test]
    fn restore_refuses_a_checkout_that_left_its_recorded_branch() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr
            .create("wandered", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        git_in(&wt.path, &["checkout", "--detach"]);

        let refused = mgr
            .restore(
                &wt,
                UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
                Isolation::Worktree,
            )
            .unwrap_err()
            .to_string();

        assert!(
            refused.contains("not on the exact persisted branch"),
            "{refused}"
        );
    }

    #[test]
    fn restore_refuses_a_branch_that_shares_no_history_with_its_base() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr
            .create("unrelated", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        let empty_tree = git_output(&repo, &["hash-object", "-t", "tree", "/dev/null"]);
        let orphan = git_output(
            &repo,
            &["commit-tree", empty_tree.trim(), "-m", "unrelated"],
        );
        git_in(
            &repo,
            &[
                "update-ref",
                &format!("refs/heads/{}", wt.recorded_branch),
                orphan.trim(),
            ],
        );

        let refused = mgr
            .restore(
                &wt,
                UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
                Isolation::Worktree,
            )
            .unwrap_err()
            .to_string();

        assert!(refused.contains("no verified ancestry"), "{refused}");
    }

    /// A checkout deleted outside Build leaves git's record of it behind, and
    /// git refuses to add a worktree under a name a record still holds. The
    /// record is stale the moment the directory goes, so restore clears it
    /// before materializing.
    #[test]
    fn restore_recreates_a_checkout_deleted_outside_build() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr
            .create("hand-deleted", "main", Isolation::Worktree)
            .unwrap()
            .worktree;

        std::fs::remove_dir_all(&wt.path).unwrap();
        assert!(
            git2::Repository::open(&repo)
                .unwrap()
                .find_worktree(&wt.name)
                .is_ok(),
            "git still records the checkout somebody deleted by hand"
        );

        let restored = mgr
            .restore(
                &wt,
                UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
                Isolation::Worktree,
            )
            .unwrap();

        assert_eq!(restored, wt);
        assert!(wt.path.join("README.md").exists());
    }

    /// A directory sitting where a checkout belongs, which is no checkout at
    /// all, is refused for what it is — nothing ran git to say otherwise.
    #[test]
    fn restore_rejects_an_existing_unregistered_directory() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let path = dir.path().join("worktrees").join("forged");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("loot"), "not a worktree\n").unwrap();
        let forged = Worktree {
            name: "forged".into(),
            path,
            recorded_branch: "build/forged".into(),
            base_branch: "main".into(),
        };

        let error = mgr
            .restore(
                &forged,
                UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
                Isolation::Worktree,
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("not a Build checkout"), "{error}");
    }

    /// A symlink sitting at the managed path and pointing at a checkout outside
    /// the root is not the checkout that was recorded: whatever the path
    /// spells, what it resolves to must still be under the root.
    #[test]
    fn restore_refuses_a_managed_path_that_resolves_outside_the_root() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let elsewhere = dir.path().join("elsewhere");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                elsewhere.to_str().unwrap(),
                "-b",
                "build/escaped",
            ],
        );
        let root = dir.path().join("worktrees");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("escaped");
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        let escaped = Worktree {
            name: "escaped".into(),
            path,
            recorded_branch: "build/escaped".into(),
            base_branch: "main".into(),
        };

        let refused = mgr
            .restore(
                &escaped,
                UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
                Isolation::Worktree,
            )
            .unwrap_err()
            .to_string();

        assert!(refused.contains("canonical managed path"), "{refused}");
    }

    #[test]
    fn restore_fetches_the_original_branch_when_only_origin_has_it() {
        let (dir, repo) = init_repo();
        bare_origin_of(&dir, &repo);
        let mgr = manager(&dir, &repo);
        let wt = mgr
            .create("recover-remote", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        std::fs::write(wt.path.join("remote-stage.txt"), "remote\n").unwrap();
        git_in(&wt.path, &["add", "remote-stage.txt"]);
        git_in(&wt.path, &["commit", "-m", "remote stage"]);
        git_in(&wt.path, &["push", "-u", "origin", &wt.recorded_branch]);
        git_in(
            &repo,
            &["worktree", "remove", "--force", wt.path.to_str().unwrap()],
        );
        git_in(&repo, &["branch", "-D", &wt.recorded_branch]);
        git_in(
            &repo,
            &[
                "update-ref",
                "-d",
                &format!("refs/remotes/origin/{}", wt.recorded_branch),
            ],
        );

        mgr.restore(
            &wt,
            UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
            Isolation::Worktree,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(wt.path.join("remote-stage.txt")).unwrap(),
            "remote\n"
        );
        assert_eq!(
            branch_teardown(&wt.path).unwrap(),
            BranchTeardown::DeletesBranch
        );
    }

    /// A checkout whose directory vanished but whose registration git still
    /// holds carries its own answer across the restore: the marker is read out
    /// of the admin directory before the prune takes it, and written back into
    /// the fresh one. Losing it would hand somebody else's branch to the next
    /// teardown.
    #[test]
    fn restore_carries_the_teardown_marker_a_registration_still_holds() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let r = git2::Repository::open(&repo).unwrap();
        let head = r.head().unwrap().peel_to_commit().unwrap();
        r.branch("theirs", &head, false).unwrap();
        let added = mgr
            .create_on_existing_branch("theirs", "main", Isolation::Worktree)
            .unwrap();
        std::fs::remove_dir_all(&added.worktree.path).unwrap();

        let restored = mgr
            .restore(
                &added.worktree,
                UnregisteredRestore::Refuse,
                Isolation::Worktree,
            )
            .unwrap();

        assert_eq!(
            branch_teardown(&restored.path).unwrap(),
            BranchTeardown::KeepsBranch
        );
    }

    /// With the registration gone, nothing on disk says whose branch this is.
    /// A caller that cannot vouch for it gets an error and an untouched
    /// repository — no re-added worktree, no fetch, no marker of either value.
    #[test]
    fn restore_refuses_when_no_registration_and_no_caller_can_vouch() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr
            .create("unvouched", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        mgr.remove(&wt).unwrap();

        let error = mgr
            .restore(&wt, UnregisteredRestore::Refuse, Isolation::Worktree)
            .unwrap_err()
            .to_string();

        assert!(error.contains("unvouched"), "{error}");
        assert!(!wt.path.exists());
        let r = git2::Repository::open(&repo).unwrap();
        assert!(r.find_worktree("unvouched").is_err());
    }

    /// A registration git cannot read is not a registration that is gone.
    /// Only absence lets the caller vouch for the branch; every other git
    /// failure is surfaced, because guessing here hands somebody's branch to
    /// the next teardown.
    #[test]
    fn restore_surfaces_a_registration_it_cannot_read_instead_of_guessing() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let r = git2::Repository::open(&repo).unwrap();
        let head = r.head().unwrap().peel_to_commit().unwrap();
        r.branch("theirs", &head, false).unwrap();
        let wt = mgr
            .create_on_existing_branch("theirs", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        std::fs::remove_dir_all(&wt.path).unwrap();
        git_in(&repo, &["update-ref", "-d", "refs/heads/theirs"]);
        std::fs::remove_file(repo.join(".git/worktrees/theirs/gitdir")).unwrap();

        let error = mgr
            .restore(
                &wt,
                UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
                Isolation::Worktree,
            )
            .unwrap_err();

        assert!(
            matches!(error, WorktreeError::Git(_)),
            "the unreadable registration is the answer, not a fetch: {error:?}"
        );
        assert!(!wt.path.exists(), "nothing was re-added");
    }

    /// A checkout Build cut a branch for takes that branch with it, and no
    /// caller has to say so — the checkout does.
    #[test]
    fn removing_a_checkout_build_cut_takes_its_branch() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr
            .create("drop-me", "main", Isolation::Worktree)
            .unwrap()
            .worktree;

        mgr.remove(&wt).unwrap();

        assert!(!wt.path.exists());
        let r = git2::Repository::open(&repo).unwrap();
        assert!(
            r.find_branch("build/drop-me", git2::BranchType::Local)
                .is_err(),
            "branch deleted"
        );
    }

    #[test]
    fn external_worktree_id_is_stable_and_prefixed() {
        let a = PathBuf::from("/Users/zech/Projects/8ly/Build");
        let b = PathBuf::from("/Users/zech/Projects/8ly/Build-hotfix");

        let id_a1 = external_worktree_id(&a);
        let id_a2 = external_worktree_id(&a);
        let id_b = external_worktree_id(&b);

        assert_eq!(id_a1, id_a2);
        assert_ne!(id_a1, id_b);
        assert!(id_a1.starts_with("wt-"));
        assert_eq!(id_a1.len(), 15);
    }

    /// `Ok(None)` is reserved for a repository that really has no working
    /// tree. A path git cannot read as a repository at all is broken, and
    /// saying so is what keeps a caller from reading "nothing holds this
    /// branch" off a repository that answered nothing.
    #[test]
    fn find_primary_checkout_errors_on_a_directory_that_is_not_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let not_a_repo = dir.path().join("plain");
        std::fs::create_dir(&not_a_repo).unwrap();

        let found = WorktreeManager::new(&not_a_repo, dir.path().join("worktrees"))
            .describe_primary("main");

        assert!(
            found.is_err(),
            "a directory git cannot read is broken, not checkout-less: {found:?}"
        );
    }

    /// The listing stamps a row with two facts about the primary checkout —
    /// the id `run.adopt` adopts it by and the branch it holds — and pays for
    /// nothing else: no status walk, no diffstat, no subprocess.
    #[test]
    fn primary_checkout_holder_names_the_branch_by_the_id_adoption_uses() {
        let (_dir, repo) = init_repo();

        let holder = primary_checkout_holder(&repo).unwrap();

        let described = manager(&_dir, &repo).describe_primary("main").unwrap();
        assert_eq!(holder, Some((described.id, "main".to_string())));
    }

    #[test]
    fn primary_checkout_holder_is_none_when_head_is_detached() {
        let (_dir, repo) = init_repo();
        git_in(&repo, &["checkout", "--detach"]);

        assert_eq!(primary_checkout_holder(&repo).unwrap(), None);
    }

    #[test]
    fn primary_checkout_holder_errors_on_a_directory_that_is_not_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let not_a_repo = dir.path().join("plain");
        std::fs::create_dir(&not_a_repo).unwrap();

        assert!(primary_checkout_holder(&not_a_repo).is_err());
    }

    #[test]
    fn discovery_lists_a_user_worktree_and_skips_the_primary() {
        let (dir, repo) = init_repo();
        let wt_path = dir.path().join("wt-a");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "-b",
                "hotfix/thing",
            ],
        );
        std::fs::write(wt_path.join("dirty.txt"), "dirty\n").unwrap();

        let excluded = std::collections::HashSet::new();
        let found = manager(&dir, &repo).discover("main", &excluded).unwrap();

        assert_eq!(found.len(), 1);
        let entry = &found[0];
        assert_eq!(entry.branch, Some("hotfix/thing".to_string()));
        assert_eq!(entry.dirty_files, 1);
        assert!(!entry.head_subject.is_empty());
        assert_eq!(entry.name, "wt-a");
        assert!(entry.id.starts_with("wt-"));

        let primary_canonical = std::fs::canonicalize(&repo).unwrap();
        assert!(found.iter().all(|w| w.path != primary_canonical));
    }

    #[test]
    fn one_checkout_describes_itself_the_way_the_scan_describes_it() {
        let (dir, repo) = init_repo();
        let wt_path = dir.path().join("wt-one");
        git_in(
            &repo,
            &["worktree", "add", wt_path.to_str().unwrap(), "-b", "solo"],
        );
        std::fs::write(wt_path.join("dirty.txt"), "dirty\n").unwrap();

        let scanned = manager(&dir, &repo)
            .discover("main", &HashSet::new())
            .unwrap();
        let described = describe_checkout(&wt_path, "main", unix_now()).unwrap();

        // `head_age_seconds` is a reading of the clock, not a property of the
        // checkout: two reads straddling a second boundary differ by one.
        let described = ExternalWorktree {
            head_age_seconds: scanned[0].head_age_seconds,
            ..described
        };
        assert_eq!(
            described, scanned[0],
            "a checkout described on its own must be the entry a scan would have found"
        );
    }

    #[test]
    fn a_checkout_outside_the_repository_cannot_be_described() {
        let (dir, _repo) = init_repo();
        let stranger = dir.path().join("not-a-worktree");
        std::fs::create_dir_all(&stranger).unwrap();

        let described = describe_checkout(&stranger, "main", unix_now());

        assert!(described.is_none(), "{described:?}");
    }

    #[test]
    fn discovery_separates_what_is_uncommitted_from_what_the_branch_carries() {
        let (dir, repo) = init_repo();
        let wt_path = dir.path().join("wt-mixed");
        git_in(
            &repo,
            &["worktree", "add", wt_path.to_str().unwrap(), "-b", "mixed"],
        );
        // One committed line, then two uncommitted ones on top of it.
        std::fs::write(wt_path.join("committed.txt"), "one\n").unwrap();
        git_in(&wt_path, &["add", "committed.txt"]);
        git_in(&wt_path, &["commit", "-m", "committed work"]);
        std::fs::write(wt_path.join("dirty.txt"), "two\nthree\n").unwrap();

        let found = manager(&dir, &repo)
            .discover("main", &HashSet::new())
            .unwrap();

        let entry = &found[0];
        // The branch delta carries both; the uncommitted stat only what is
        // sitting in the tree unsaved.
        assert_eq!(entry.diffstat.insertions, 3);
        assert_eq!(entry.uncommitted.insertions, 2);
        assert_eq!(entry.uncommitted.files_changed, 1);
    }

    fn git_output(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Commit `name` in `dir` as a new file of the same name.
    fn commit_file(dir: &Path, name: &str) {
        std::fs::write(dir.join(format!("{name}.txt")), "x\n").unwrap();
        git_in(dir, &["add", "."]);
        git_in(dir, &["commit", "-m", name]);
    }

    /// A tracked branch compares both directions with its upstream. Movement on
    /// the local base is irrelevant until the branch stops tracking upstream.
    #[test]
    fn a_tracking_branch_compares_both_directions_with_its_upstream() {
        let (dir, repo) = init_repo();
        let remote = dir.path().join("origin.git");
        git_in(&repo, &["init", "--bare", remote.to_str().unwrap()]);
        git_in(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );

        let wt_path = dir.path().join("wt-tracked");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "-b",
                "tracked",
            ],
        );
        git_in(&wt_path, &["push", "-u", "origin", "tracked"]);
        let other = dir.path().join("other");
        git_in(
            dir.path(),
            &[
                "clone",
                "--branch",
                "tracked",
                remote.to_str().unwrap(),
                other.to_str().unwrap(),
            ],
        );
        git_in(&other, &["config", "user.email", "other@build.ing"]);
        git_in(&other, &["config", "user.name", "Other"]);

        // Two local commits past the shared tip, and one remote commit the local
        // branch does not have.
        commit_file(&wt_path, "a");
        commit_file(&wt_path, "b");
        commit_file(&other, "remote");
        git_in(&other, &["push", "origin", "tracked"]);
        git_in(&repo, &["fetch", "origin"]);
        // Main moves twice to prove it is not the selected comparison ref.
        commit_file(&repo, "on-main");
        commit_file(&repo, "on-main-again");

        let found = manager(&dir, &repo)
            .discover("main", &HashSet::new())
            .unwrap();

        let entry = &found[0];
        assert_eq!(entry.upstream.as_deref(), Some("origin/tracked"));
        assert_eq!(entry.comparison_ref.as_deref(), Some("origin/tracked"));
        assert_eq!(entry.ahead, Some(2), "two commits the remote lacks");
        assert_eq!(
            entry.behind,
            Some(1),
            "one remote commit is missing locally"
        );
    }

    /// A branch that tracks nothing has pushed nothing: every commit it carries
    /// past the base is unpushed, and there is no upstream to name.
    #[test]
    fn an_untracked_branch_has_all_of_its_work_unpushed() {
        let (dir, repo) = init_repo();
        let wt_path = dir.path().join("wt-untracked");
        git_in(
            &repo,
            &["worktree", "add", wt_path.to_str().unwrap(), "-b", "solo"],
        );
        commit_file(&wt_path, "a");
        commit_file(&repo, "on-main");

        let found = manager(&dir, &repo)
            .discover("main", &HashSet::new())
            .unwrap();

        let entry = &found[0];
        assert_eq!(entry.upstream, None);
        assert_eq!(entry.comparison_ref.as_deref(), Some("main"));
        assert_eq!(entry.ahead, Some(1));
        assert_eq!(entry.behind, Some(1));
    }

    /// Nothing to report is reported as nothing — a level, pushed, clean
    /// worktree has no counts rather than a row of zeroes.
    #[test]
    fn a_level_worktree_is_neither_stale_nor_unpushed() {
        let (dir, repo) = init_repo();
        let wt_path = dir.path().join("wt-level");
        git_in(
            &repo,
            &["worktree", "add", wt_path.to_str().unwrap(), "-b", "level"],
        );

        let found = manager(&dir, &repo)
            .discover("main", &HashSet::new())
            .unwrap();

        let entry = &found[0];
        assert_eq!(entry.comparison_ref.as_deref(), Some("main"));
        assert_eq!(entry.ahead, Some(0));
        assert_eq!(entry.behind, Some(0));
        assert_eq!(entry.uncommitted.insertions, 0);
    }

    #[test]
    fn discovery_excludes_bound_paths() {
        let (dir, repo) = init_repo();
        let wt_path = dir.path().join("wt-bound");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "-b",
                "bound/thing",
            ],
        );

        let mut excluded = std::collections::HashSet::new();
        excluded.insert(std::fs::canonicalize(&wt_path).unwrap());
        let found = manager(&dir, &repo).discover("main", &excluded).unwrap();

        assert!(found.is_empty());
    }

    #[test]
    fn a_bound_path_excludes_its_checkout_in_whatever_spelling_it_arrives() {
        let (dir, repo) = init_repo();
        let wt_path = dir.path().join("wt-bound");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "-b",
                "bound/spelled-otherwise",
            ],
        );
        let another_spelling = dir.path().join("wt-bound-link");
        std::os::unix::fs::symlink(&wt_path, &another_spelling).unwrap();

        let mut excluded = std::collections::HashSet::new();
        excluded.insert(another_spelling);
        let found = manager(&dir, &repo).discover("main", &excluded).unwrap();

        assert!(
            found.is_empty(),
            "the scan canonicalizes what it is told to exclude, so no caller has to: {found:?}"
        );
    }

    #[test]
    fn discovery_reports_detached_head() {
        let (dir, repo) = init_repo();
        let wt_path = dir.path().join("wt-d");
        git_in(
            &repo,
            &["worktree", "add", "--detach", wt_path.to_str().unwrap()],
        );

        let excluded = std::collections::HashSet::new();
        let found = manager(&dir, &repo).discover("main", &excluded).unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].branch, None);
    }

    #[test]
    fn discovery_cache_invalidation_sees_new_head() {
        // Not a cache test (Layer 1 owns no cache) — confirms a fresh scan after
        // a new commit reflects the moved HEAD, the property the app-layer cache
        // invalidation relies on.
        let (dir, repo) = init_repo();
        let wt_path = dir.path().join("wt-c");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "-b",
                "feature/thing",
            ],
        );
        let excluded = std::collections::HashSet::new();
        let before = manager(&dir, &repo).discover("main", &excluded).unwrap();
        let sha_before = before[0].head_sha.clone();

        std::fs::write(wt_path.join("more.txt"), "more\n").unwrap();
        git_in(&wt_path, &["add", "more.txt"]);
        git_in(&wt_path, &["commit", "-m", "more work"]);

        let after = manager(&dir, &repo).discover("main", &excluded).unwrap();
        assert_ne!(before[0].head_sha, after[0].head_sha);
        assert_ne!(sha_before, after[0].head_sha);
    }

    #[test]
    fn derive_adoption_goal_pinned_cases() {
        assert_eq!(
            derive_adoption_goal("hotfix/login-redirect", "irrelevant"),
            "hotfix/login-redirect"
        );
        assert_eq!(
            derive_adoption_goal("wip", "Fix the thing"),
            "Fix the thing"
        );
        assert_eq!(
            derive_adoption_goal("wip-2", "some subject"),
            "some subject"
        );
        assert_eq!(
            derive_adoption_goal("zech/test_3", "some subject"),
            "some subject"
        );
        assert_eq!(
            derive_adoption_goal("feature", "some subject"),
            "some subject"
        );
        assert_eq!(derive_adoption_goal("", ""), "Adopted worktree");
    }

    /// The scan is the union of every backend's, minus the project's own
    /// checkout and the paths the caller has already bound, and each entry says
    /// how it is isolated.
    #[test]
    fn discover_lists_every_checkout_but_the_project_and_the_excluded() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let listed = dir.path().join("wt-listed");
        let bound = dir.path().join("wt-bound");
        git_in(
            &repo,
            &["worktree", "add", listed.to_str().unwrap(), "-b", "listed"],
        );
        git_in(
            &repo,
            &["worktree", "add", bound.to_str().unwrap(), "-b", "bound"],
        );
        let mut excluded = HashSet::new();
        excluded.insert(std::fs::canonicalize(&bound).unwrap());

        let found = mgr.discover("main", &excluded).unwrap();

        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].name, "wt-listed");
        assert_eq!(found[0].branch.as_deref(), Some("listed"));
        assert_eq!(found[0].isolation, Isolation::Worktree);
    }

    /// The scan is the union of both backends' walks: a clone and a linked
    /// worktree of the same project both surface, each saying what it is, and
    /// each is base-synced first — for a clone that is a real fetch from the
    /// project, so a base that moved after the clone was made is visible in
    /// its counts.
    #[test]
    fn discover_lists_a_clone_and_a_linked_worktree_of_the_same_project() {
        let (dir, repo) = init_repo();
        if !cow_or_skip(dir.path()) {
            return;
        }
        let mgr = manager(&dir, &repo);
        let clone = mgr
            .create("cloned", "main", Isolation::Cow)
            .unwrap()
            .worktree;
        let linked = dir.path().join("worktrees").join("wt-linked");
        git_in(
            &repo,
            &["worktree", "add", linked.to_str().unwrap(), "-b", "linked"],
        );
        commit_file(&repo, "moved-base");

        let found = mgr.discover("main", &HashSet::new()).unwrap();

        let isolations: Vec<(String, Isolation)> = found
            .iter()
            .map(|checkout| (checkout.name.clone(), checkout.isolation))
            .collect();
        assert_eq!(found.len(), 2, "{isolations:?}");
        assert!(
            isolations.contains(&("cloned".to_string(), Isolation::Cow)),
            "{isolations:?}"
        );
        assert!(
            isolations.contains(&("wt-linked".to_string(), Isolation::Worktree)),
            "{isolations:?}"
        );
        let cloned = found
            .iter()
            .find(|checkout| checkout.path == std::fs::canonicalize(&clone.path).unwrap())
            .expect("the clone is on the board");
        assert_eq!(
            cloned.behind,
            Some(1),
            "the clone's row does not reflect the base sync the scan ran"
        );
    }

    /// One broken stray must not fail the scan. A checkout git still lists but
    /// that is no Build checkout any more is refused a base sync and described
    /// by nobody; one git itself gives up on is never listed; and the healthy
    /// one beside them is found all the same.
    #[test]
    fn discover_skips_a_broken_stray_and_keeps_the_rest() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let healthy = dir.path().join("wt-healthy");
        let hollowed = dir.path().join("wt-hollowed");
        let pointerless = dir.path().join("wt-pointerless");
        for (path, branch) in [
            (&healthy, "healthy"),
            (&hollowed, "hollowed"),
            (&pointerless, "pointerless"),
        ] {
            git_in(
                &repo,
                &["worktree", "add", path.to_str().unwrap(), "-b", branch],
            );
        }
        std::fs::remove_file(hollowed.join(".git")).unwrap();
        std::fs::create_dir(hollowed.join(".git")).unwrap();
        std::fs::remove_file(pointerless.join(".git")).unwrap();

        let found = mgr.discover("main", &HashSet::new()).unwrap();

        assert_eq!(
            found.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(),
            vec!["wt-healthy"],
            "{found:?}"
        );
    }

    /// A checkout describes itself, branch and all — and a directory that is no
    /// Build checkout is described by nobody.
    #[test]
    fn a_detached_checkout_is_described_without_a_branch() {
        let (dir, repo) = init_repo();
        let detached = dir.path().join("wt-detached");
        git_in(
            &repo,
            &["worktree", "add", "--detach", detached.to_str().unwrap()],
        );

        let described = describe_checkout(&detached, "main", unix_now())
            .expect("a linked worktree describes itself");

        assert_eq!(described.branch, None);
        assert_eq!(described.name, "wt-detached");
        assert_eq!(described.isolation, Isolation::Worktree);
        assert!(
            describe_checkout(&repo, "main", unix_now()).is_none(),
            "the project's own checkout is nobody's isolated copy"
        );
    }

    /// Removal's goal is absence: a checkout already gone still takes git's
    /// record of it with it, and a kept branch is left whole.
    #[test]
    fn removing_a_checkout_that_is_already_gone_still_clears_its_record() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr
            .create("vanished", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        std::fs::remove_dir_all(&wt.path).unwrap();

        mgr.remove_keeping_branch(&wt).unwrap();

        let r = git2::Repository::open(&repo).unwrap();
        assert!(
            !r.worktrees().unwrap().iter().any(|n| n == Some("vanished")),
            "the record went with the directory"
        );
        assert!(
            r.find_branch(&wt.recorded_branch, git2::BranchType::Local)
                .is_ok(),
            "the kept branch is untouched"
        );
    }

    /// A checkout is known by its directory, so renaming one leaves git's
    /// registry naming something that is not there: removal takes the directory
    /// it was pointed at, and the sweep clears what the rename stranded.
    #[test]
    fn a_checkout_renamed_after_registration_is_removed_by_its_directory() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr
            .create("was-here", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        let renamed = dir.path().join("worktrees").join("now-here");
        std::fs::rename(&wt.path, &renamed).unwrap();

        mgr.remove_checkout(&renamed).unwrap();

        assert!(!renamed.exists(), "the directory it was pointed at is gone");
        let stale = git2::Repository::open(&repo).unwrap();
        assert!(
            stale
                .worktrees()
                .unwrap()
                .iter()
                .any(|n| n == Some("was-here")),
            "the rename stranded the record"
        );

        mgr.prune();

        let swept = git2::Repository::open(&repo).unwrap();
        assert!(
            !swept
                .worktrees()
                .unwrap()
                .iter()
                .any(|n| n == Some("was-here")),
            "the sweep clears it"
        );
    }

    /// The merge runs through the project's own checkout, which must be on the
    /// base branch — merging into whatever is at HEAD would land the work
    /// somewhere nobody asked for.
    #[test]
    fn merge_into_base_refuses_a_primary_checkout_on_another_branch() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr
            .create("mergeable", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        commit_file(&wt.path, "landed");
        git_in(&repo, &["checkout", "-b", "elsewhere"]);

        let refused = mgr
            .merge_into_base(&wt.path, &wt.recorded_branch, "main")
            .unwrap_err()
            .to_string();

        assert!(refused.contains("not the base branch"), "{refused}");
        assert!(
            !refused.contains("git command failed"),
            "the merge never ran, so the refusal must not read as git's failure: {refused}"
        );
        git_in(&repo, &["checkout", "main"]);
        mgr.merge_into_base(&wt.path, &wt.recorded_branch, "main")
            .unwrap();
        assert!(
            repo.join("landed.txt").exists(),
            "the work is on the base branch"
        );
    }

    /// Deleting a branch by an expected head is how a teardown promises it took
    /// only what it read; a branch that moved since keeps its work.
    #[test]
    fn a_branch_is_deleted_only_while_it_still_points_where_it_was_read() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr
            .create("ref-ops", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        commit_file(&wt.path, "moved-on");
        let branch_tip = tip_of(&repo, &format!("refs/heads/{}", wt.recorded_branch));
        let base_tip = tip_of(&repo, "refs/heads/main");
        mgr.remove_checkout(&wt.path).unwrap();

        assert!(mgr.branch_exists(&wt.recorded_branch).unwrap());
        assert!(!mgr.branch_exists("build/never-cut").unwrap());

        assert!(
            mgr.delete_branch_at(&wt.recorded_branch, &base_tip)
                .is_err(),
            "a branch that is not where it was read keeps its work"
        );
        assert!(mgr.branch_exists(&wt.recorded_branch).unwrap());

        mgr.delete_branch_at(&wt.recorded_branch, &branch_tip)
            .unwrap();
        assert!(!mgr.branch_exists(&wt.recorded_branch).unwrap());

        mgr.restore_branch(&wt.recorded_branch, &branch_tip)
            .unwrap();
        assert_eq!(
            tip_of(&repo, &format!("refs/heads/{}", wt.recorded_branch)),
            branch_tip,
            "the undo puts it back exactly where it was"
        );
    }

    /// The sha a ref points at, as git spells it.
    fn tip_of(repo: &Path, reference: &str) -> String {
        git2::Repository::open(repo)
            .unwrap()
            .find_reference(reference)
            .unwrap()
            .target()
            .unwrap()
            .to_string()
    }

    /// Availability now comes from the probe: a linked worktree is never
    /// locked, and a clone is locked exactly when this volume cannot make one,
    /// in the probe's own words. The manager owns no reason of its own.
    #[test]
    fn a_clones_availability_is_the_probes_answer() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let availability = mgr.availability();

        assert_eq!(availability.lock_reason(Isolation::Worktree), None);
        assert_eq!(
            availability.lock_reason(Isolation::Cow),
            availability.cow.as_ref().err().map(String::as_str),
            "the clone lock is exactly the probe's failure sentence"
        );
    }

    /// Every backend sits at the isolation it makes, in the order the enum
    /// names them — and the clone backend arrived in this stage, so no slot is
    /// empty and every isolation resolves to a backend.
    #[test]
    fn each_backend_sits_at_the_isolation_it_makes() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);

        for (isolation, slot) in Isolation::ALL.into_iter().zip(mgr.backends()) {
            let backend = slot.expect("every isolation has a backend in this build");
            assert_eq!(backend.kind(), isolation);
            assert_eq!(mgr.backend(isolation).unwrap().kind(), isolation);
        }
        assert_eq!(
            mgr.backends()
                .map(|slot| slot.map(|backend| backend.kind())),
            [Some(Isolation::Worktree), Some(Isolation::Cow)],
        );
    }

    /// The whole of a clone create through the façade: the branch is cut in the
    /// project repo, the working directory is a copy-on-write clone on that
    /// branch, and `Isolation::of` reads it back as a clone.
    #[test]
    fn create_materializes_a_clone_end_to_end() {
        let (dir, repo) = init_repo();
        if !cow_or_skip(dir.path()) {
            return;
        }
        let mgr = manager(&dir, &repo);

        let wt = mgr
            .create("cloned", "main", Isolation::Cow)
            .unwrap()
            .worktree;

        assert_eq!(wt.recorded_branch, "build/cloned");
        assert_eq!(wt.base_branch, "main");
        assert_eq!(Isolation::of(&wt.path), Some(Isolation::Cow));
        assert!(wt.path.join("README.md").exists(), "the clone is warm");
        let r = git2::Repository::open(&repo).unwrap();
        assert!(
            r.find_branch("build/cloned", git2::BranchType::Local)
                .is_ok(),
            "the branch was cut in the project repo"
        );
        assert_eq!(
            git2::Repository::open(&wt.path)
                .unwrap()
                .head()
                .unwrap()
                .shorthand(),
            Some("build/cloned"),
            "the clone is on the cut branch"
        );
    }

    /// A named branch that already exists is checked out into a clone, not cut
    /// again — dispatching a clone onto work started by hand reaches it.
    #[test]
    fn create_on_branch_clones_onto_an_existing_branch() {
        let (dir, repo) = init_repo();
        if !cow_or_skip(dir.path()) {
            return;
        }
        let mgr = manager(&dir, &repo);
        let r = git2::Repository::open(&repo).unwrap();
        let head = r.head().unwrap().peel_to_commit().unwrap();
        r.branch("build/started-by-hand", &head, false).unwrap();

        let added = mgr
            .create_on_existing_branch("build/started-by-hand", "main", Isolation::Cow)
            .unwrap();

        assert_eq!(
            added.teardown,
            BranchTeardown::KeepsBranch,
            "the branch was already there, not cut again"
        );
        assert_eq!(Isolation::of(&added.worktree.path), Some(Isolation::Cow));
        assert_eq!(
            git2::Repository::open(&added.worktree.path)
                .unwrap()
                .head()
                .unwrap()
                .shorthand(),
            Some("build/started-by-hand"),
        );
    }

    /// A clone deleted from disk is recreated on its recorded branch as a
    /// clone: the caller's resolved isolation says what to recreate it as, and
    /// the branch (published on removal) says where.
    #[test]
    fn restore_recreates_a_deleted_clone_on_its_recorded_branch() {
        let (dir, repo) = init_repo();
        if !cow_or_skip(dir.path()) {
            return;
        }
        let mgr = manager(&dir, &repo);
        let wt = mgr
            .create("recover-clone", "main", Isolation::Cow)
            .unwrap()
            .worktree;
        commit_file(&wt.path, "clone-stage");
        mgr.remove_keeping_branch(&wt).unwrap();
        assert!(!wt.path.exists(), "the clone was removed");

        let restored = mgr
            .restore(
                &wt,
                UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
                Isolation::Cow,
            )
            .unwrap();

        assert_eq!(restored, wt);
        assert_eq!(Isolation::of(&wt.path), Some(Isolation::Cow));
        assert!(
            wt.path.join("clone-stage.txt").exists(),
            "the recreated clone carries the published work"
        );
    }

    /// An existing clone passes restore's verify; a clone whose marker names a
    /// different project is refused for what it is — it is not this project's.
    #[test]
    fn restore_verifies_a_clone_and_rejects_one_of_another_project() {
        let (dir, repo) = init_repo();
        if !cow_or_skip(dir.path()) {
            return;
        }
        let mgr = manager(&dir, &repo);

        let mine = mgr.create("mine", "main", Isolation::Cow).unwrap().worktree;
        assert_eq!(
            mgr.restore(
                &mine,
                UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
                Isolation::Cow
            )
            .unwrap(),
            mine
        );

        let other = init_repo_named(dir.path(), "other");
        let intruder_path = dir.path().join("worktrees").join("intruder");
        CowBackend
            .materialize(&other, "main", &intruder_path)
            .unwrap();
        let intruder = Worktree {
            name: "intruder".into(),
            path: intruder_path,
            recorded_branch: "main".into(),
            base_branch: "main".into(),
        };

        let refused = mgr
            .restore(
                &intruder,
                UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
                Isolation::Cow,
            )
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("not a copy-on-write clone of this project"),
            "{refused}"
        );
    }

    /// A clone is a directory, and a directory already under the worktrees root
    /// makes its name taken whatever made it — so a second create disambiguates.
    #[test]
    fn a_clone_directory_makes_its_name_taken() {
        let (dir, repo) = init_repo();
        if !cow_or_skip(dir.path()) {
            return;
        }
        let mgr = manager(&dir, &repo);

        let first = mgr.create("dup", "main", Isolation::Cow).unwrap().worktree;
        let second = mgr.create("dup", "main", Isolation::Cow).unwrap().worktree;

        assert_eq!(first.name, "dup");
        assert_eq!(
            second.name, "dup-2",
            "the existing clone directory made the name taken"
        );
        assert_ne!(first.path, second.path);
    }

    /// A path that never held a checkout is absent already, and absence is
    /// success for every backend — the clone backend included.
    #[test]
    fn remove_checkout_on_a_missing_path_succeeds() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let ghost = dir.path().join("worktrees").join("never-existed");

        mgr.remove_checkout(&ghost)
            .expect("absence is success for every backend");
    }

    /// A directory that is not a checkout is still a directory the teardown was
    /// pointed at: there is nothing to publish from, and it goes.
    #[test]
    fn removing_a_directory_that_is_no_longer_a_checkout_still_clears_it() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr
            .create("half-made", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        std::fs::remove_file(wt.path.join(".git")).unwrap();
        assert_eq!(Isolation::of(&wt.path), None, "no longer a checkout");

        mgr.remove_keeping_branch(&wt)
            .expect("a directory nobody can publish from is still removable");

        assert!(!wt.path.exists(), "the directory is gone");
        assert!(
            git2::Repository::open(&repo)
                .unwrap()
                .find_branch(&wt.recorded_branch, git2::BranchType::Local)
                .is_ok(),
            "the kept branch is untouched"
        );
    }
}

#[cfg(test)]
mod vanished_worktree_removal {
    use super::*;
    use crate::git_fixture::{git_in, init_repo};

    /// The state an outside cleanup leaves behind: directory removed,
    /// bookkeeping pruned, branch deleted.
    fn fully_vanished(repo: &Path, wt: &Worktree) {
        std::fs::remove_dir_all(&wt.path).unwrap();
        git_in(repo, &["worktree", "prune"]);
        git_in(repo, &["branch", "-D", &wt.recorded_branch]);
    }

    #[test]
    fn removing_an_already_vanished_worktree_succeeds() {
        // The defect this guards: a Build worktree cleaned up outside Build
        // (dir, bookkeeping AND branch gone) made remove() fail on git2's
        // baffling "could not find '.git/shallow' to stat" from find_worktree,
        // which blocked the plan approve that only wanted the worktree gone.
        // Removal's goal is absence; finding absence is success.
        let (dir, repo) = init_repo();
        let manager = WorktreeManager::new(&repo, dir.path().join("wts"));
        let wt = manager
            .create("gone-slug", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        fully_vanished(&repo, &wt);

        manager.remove(&wt).expect("absence is the goal");
    }

    #[test]
    fn removing_a_vanished_worktree_leaves_a_branch_it_can_no_longer_vouch_for() {
        // Partial carcass: dir and bookkeeping gone, branch still there. The
        // bookkeeping is where the checkout recorded whose branch that is, so
        // with it gone the branch is reported and left alone rather than
        // deleted on a guess.
        let (dir, repo) = init_repo();
        let manager = WorktreeManager::new(&repo, dir.path().join("wts"));
        let wt = manager
            .create("half-gone", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        std::fs::remove_dir_all(&wt.path).unwrap();
        git_in(&repo, &["worktree", "prune"]);

        let error = manager.remove(&wt).unwrap_err();

        assert!(
            format!("{error}").contains("half-gone"),
            "the error names the checkout that can no longer answer: {error}"
        );
        let repo = git2::Repository::open(&repo).unwrap();
        assert!(
            repo.find_branch(&wt.recorded_branch, git2::BranchType::Local)
                .is_ok(),
            "a branch nothing can vouch for is left standing"
        );
    }

    /// The marker decides whether the branch goes, so it is read before
    /// anything is destroyed: a checkout that cannot answer is left whole —
    /// directory, registration and branch — rather than torn down under a
    /// question nothing can be asked again afterwards.
    #[test]
    fn removing_a_checkout_that_cannot_answer_destroys_nothing() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, repo) = init_repo();
        let manager = WorktreeManager::new(&repo, dir.path().join("wts"));
        let r = git2::Repository::open(&repo).unwrap();
        let head = r.head().unwrap().peel_to_commit().unwrap();
        r.branch("theirs", &head, false).unwrap();
        let wt = manager
            .create_on_existing_branch("theirs", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        let admin_dir = repo.join(".git/worktrees").join(&wt.name);
        std::fs::set_permissions(&admin_dir, std::fs::Permissions::from_mode(0o000)).unwrap();

        let error = manager.remove(&wt).unwrap_err();

        std::fs::set_permissions(&admin_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            matches!(error, WorktreeError::Io(_)),
            "the unreadable marker is the answer: {error:?}"
        );
        assert!(wt.path.exists(), "the checkout is left standing");
        assert!(
            r.find_worktree(&wt.name).is_ok(),
            "and so is its registration"
        );
        assert!(
            r.find_branch("theirs", git2::BranchType::Local).is_ok(),
            "and the branch it was only borrowing"
        );
    }

    #[test]
    fn removal_refuses_before_deleting_when_the_project_repo_is_unreachable() {
        // A teardown that cannot reach git's registry would delete the
        // directory and strand a record naming it; it must refuse while
        // nothing is lost yet.
        let (dir, repo) = init_repo();
        let root = dir.path().join("wts");
        let wt = WorktreeManager::new(&repo, &root)
            .create("kept-slug", "main", Isolation::Worktree)
            .unwrap()
            .worktree;
        let unreachable = WorktreeManager::new(dir.path().join("not-a-repo"), &root);

        unreachable
            .remove_checkout(&wt.path)
            .expect_err("no registry to clear, so nothing is deleted");
        assert!(wt.path.exists(), "the checkout survives a refused teardown");
    }
}
