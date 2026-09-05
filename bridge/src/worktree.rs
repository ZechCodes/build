//! Git worktree lifecycle: one worktree + branch per task.
//!
//! The bridge owns worktrees. A task gets an isolated branch (`build/<slug>`) and
//! a working directory cut from the project's base branch, so parallel tasks on
//! the same repo never touch each other. Removing a checkout takes its branch
//! only when the checkout itself says so — the fact is written down beside it
//! at creation, because a branch Build merely borrowed is somebody's work.
//! Issue planning has no worktree at all: its agent runs on the primary checkout.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The branch-name prefix for every run/task branch: `build/<slug>`.
pub const BRANCH_PREFIX: &str = "build";

/// Things that can go wrong managing a worktree.
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("git command failed: {0}")]
    Command(String),
}

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
        && git2::Reference::is_valid_name(&format!("refs/heads/{name}"))
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

/// What removing a checkout does to the branch it is on.
///
/// The question is decided once, when the checkout is created, by the only
/// code that can answer it — and written down beside the checkout, because
/// every reader of it comes much later and from somewhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchTeardown {
    DeletesBranch,
    KeepsBranch,
}

/// The file, in git's admin directory for a checkout, that records its
/// [`BranchTeardown`]. Git prunes that directory with the worktree, so the
/// fact cannot outlive what it describes.
const BRANCH_TEARDOWN_MARKER: &str = "build-branch-teardown";

impl BranchTeardown {
    /// Whether removing the checkout this describes takes its branch with it.
    /// The one place the tag is turned back into the question it answers.
    pub fn deletes_branch(self) -> bool {
        self == BranchTeardown::DeletesBranch
    }

    fn as_str(self) -> &'static str {
        match self {
            BranchTeardown::DeletesBranch => "deletes-branch",
            BranchTeardown::KeepsBranch => "keeps-branch",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "deletes-branch" => Some(BranchTeardown::DeletesBranch),
            "keeps-branch" => Some(BranchTeardown::KeepsBranch),
            _ => None,
        }
    }
}

/// Record what teardown of the checkout at `worktree_path` owns.
pub fn record_branch_teardown(
    worktree_path: &Path,
    teardown: BranchTeardown,
) -> Result<(), WorktreeError> {
    let admin_dir = admin_dir_of(worktree_path)?;
    std::fs::write(admin_dir.join(BRANCH_TEARDOWN_MARKER), teardown.as_str())?;
    Ok(())
}

/// What teardown of the checkout at `worktree_path` owns.
///
/// Exactly one reading means [`BranchTeardown::DeletesBranch`]: git's admin
/// directory for the checkout was read and holds no marker, which is a
/// checkout Build did not create — one made by hand and adopted — whose
/// branch the human's chosen action speaks for. Every failure to read is
/// returned, because the alternative to an error here is deleting a ref
/// nobody asked Build to touch.
pub fn branch_teardown(worktree_path: &Path) -> Result<BranchTeardown, WorktreeError> {
    teardown_in_admin_dir(&admin_dir_of(worktree_path)?)
}

fn teardown_in_admin_dir(admin_dir: &Path) -> Result<BranchTeardown, WorktreeError> {
    if !admin_dir.is_dir() {
        return Err(WorktreeError::Command(format!(
            "no git admin directory at {}",
            admin_dir.display()
        )));
    }
    match std::fs::read_to_string(admin_dir.join(BRANCH_TEARDOWN_MARKER)) {
        Ok(text) => BranchTeardown::parse(text.trim()).ok_or_else(|| {
            WorktreeError::Command(format!(
                "unreadable branch-teardown marker in {}",
                admin_dir.display()
            ))
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(BranchTeardown::DeletesBranch)
        }
        Err(error) => Err(error.into()),
    }
}

/// Git's admin directory for a linked worktree, addressed from the repository
/// that registered it — readable whether or not the checkout is still on disk.
/// `Repository::path()` is the *caller's* admin directory, which is the linked
/// worktree's own when the project root is itself one, so the shared
/// `commondir` is what holds the `worktrees/<name>` entries.
fn admin_dir_for(repo: &git2::Repository, worktree_name: &str) -> PathBuf {
    repo.commondir().join("worktrees").join(worktree_name)
}

/// Git's admin directory for a checkout: the one `<worktree>/.git` points at
/// for a linked worktree, and `<repo>/.git` itself for a main checkout. A
/// relative `gitdir:` pointer (git 2.48+ with `worktree.useRelativePaths`) is
/// resolved against the directory holding the pointer, which is what git does
/// with it.
fn admin_dir_of(worktree_path: &Path) -> Result<PathBuf, WorktreeError> {
    let pointer = worktree_path.join(".git");
    if pointer.is_dir() {
        return Ok(pointer);
    }
    let text = std::fs::read_to_string(&pointer)?;
    let gitdir = text
        .lines()
        .find_map(|line| line.strip_prefix("gitdir:"))
        .map(str::trim)
        .filter(|gitdir| !gitdir.is_empty())
        .ok_or_else(|| {
            WorktreeError::Command(format!("{} names no git directory", pointer.display()))
        })?;
    let gitdir = Path::new(gitdir);
    Ok(if gitdir.is_absolute() {
        gitdir.to_path_buf()
    } else {
        worktree_path.join(gitdir)
    })
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

/// Owns worktree creation and teardown for a single project repository.
pub struct WorktreeManager {
    repo_path: PathBuf,
    worktrees_root: PathBuf,
}

impl WorktreeManager {
    /// `repo_path` is the project git repo; `worktrees_root` is where task
    /// worktrees are materialized (one subdirectory per task slug). Branches
    /// are cut in the `build/` namespace.
    pub fn new(repo_path: impl Into<PathBuf>, worktrees_root: impl Into<PathBuf>) -> Self {
        WorktreeManager {
            repo_path: repo_path.into(),
            worktrees_root: worktrees_root.into(),
        }
    }

    /// Create `<prefix>/<slug>` from `base_branch` and add a worktree for it. The
    /// name is made unique (`<slug>`, `<slug>-2`, …) so re-dispatching the same
    /// goal — or leftover branches/worktrees from prior tasks — never collides.
    ///
    /// The branch is one Build cut for itself, so the answer says teardown
    /// takes it — the same fact this call writes beside the checkout, told to
    /// the caller in the shape [`create_cutting_branch`](Self::create_cutting_branch)
    /// answers in.
    pub fn create(
        &self,
        slug: &str,
        base_branch: &str,
    ) -> Result<NamedBranchCheckout, WorktreeError> {
        let repo = git2::Repository::open(&self.repo_path)?;
        let base_commit = repo.revparse_single(base_branch)?.peel_to_commit()?;
        std::fs::create_dir_all(&self.worktrees_root)?;

        let name =
            self.unique_checkout_name(&repo, slug, |candidate| self.branch_taken(&repo, candidate));
        let branch = self.branch_name(&name);

        repo.branch(&branch, &base_commit, false)?;
        let path = self.worktrees_root.join(&name);

        let branch_ref = repo.find_reference(&format!("refs/heads/{branch}"))?;
        self.add_checkout_on_ref(&repo, &name, &path, &branch_ref)?;
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

    /// Add a worktree for a branch that already exists, spelled exactly as it
    /// was given: one this repository holds is checked out as it stands, and
    /// one only a remote carries is fetched and made local with its upstream
    /// set. A name no ref anywhere backs is a mistake the caller is told
    /// about, never a fresh empty branch wearing that name.
    ///
    /// Teardown keeps such a branch: it holds work nobody asked Build to
    /// remove.
    pub fn create_on_existing_branch(
        &self,
        branch: &str,
        base_branch: &str,
    ) -> Result<NamedBranchCheckout, WorktreeError> {
        let repo = git2::Repository::open(&self.repo_path)?;
        match crate::gitgui::branch_origin(&repo, branch)? {
            crate::gitgui::BranchOrigin::Local => {}
            crate::gitgui::BranchOrigin::Remote {
                remote,
                tracking_ref,
            } => self.materialise_remote_branch(&repo, branch, &remote, &tracking_ref)?,
            crate::gitgui::BranchOrigin::Absent => {
                return Err(WorktreeError::Command(format!(
                    "branch {branch:?} does not exist locally or on any remote"
                )))
            }
        }
        self.checkout_branch(&repo, branch, base_branch, BranchTeardown::KeepsBranch)
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
    ) -> Result<NamedBranchCheckout, WorktreeError> {
        let repo = git2::Repository::open(&self.repo_path)?;
        let teardown = match crate::gitgui::branch_origin(&repo, branch)? {
            crate::gitgui::BranchOrigin::Local => BranchTeardown::KeepsBranch,
            crate::gitgui::BranchOrigin::Remote {
                remote,
                tracking_ref,
            } => {
                self.materialise_remote_branch(&repo, branch, &remote, &tracking_ref)?;
                BranchTeardown::KeepsBranch
            }
            crate::gitgui::BranchOrigin::Absent => {
                self.cut_branch(&repo, branch, base_branch)?;
                BranchTeardown::DeletesBranch
            }
        };
        self.checkout_branch(&repo, branch, base_branch, teardown)
    }

    /// Give a branch that is ready to be checked out a directory of its own,
    /// and stamp what teardown of it owns beside it.
    fn checkout_branch(
        &self,
        repo: &git2::Repository,
        branch: &str,
        base_branch: &str,
        teardown: BranchTeardown,
    ) -> Result<NamedBranchCheckout, WorktreeError> {
        std::fs::create_dir_all(&self.worktrees_root)?;
        let name = self.unique_checkout_name(repo, &self.directory_name_for(branch), |_| false);
        let reference = repo.find_reference(&format!("refs/heads/{branch}"))?;
        let path = self.worktrees_root.join(&name);
        self.add_checkout_on_ref(repo, &name, &path, &reference)?;
        self.stamp_teardown_or_unwind(repo, &name, &path, teardown)?;

        Ok(NamedBranchCheckout {
            worktree: Worktree {
                name,
                path,
                recorded_branch: branch.to_string(),
                base_branch: base_branch.to_string(),
            },
            teardown,
        })
    }

    /// Register a worktree at `path` with its HEAD on `reference`.
    fn add_checkout_on_ref(
        &self,
        repo: &git2::Repository,
        name: &str,
        path: &Path,
        reference: &git2::Reference<'_>,
    ) -> Result<(), WorktreeError> {
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(reference));
        repo.worktree(name, path, Some(&opts))?;
        Ok(())
    }

    /// Write what teardown owns beside a checkout git has just registered, and
    /// take that registration back when the write fails.
    ///
    /// A registered checkout with no marker reads as one Build made for itself
    /// — the branch goes with it — so leaving one behind after failing to say
    /// otherwise hands somebody else's branch to the next teardown. Unwinding
    /// is best-effort because the error being returned is the one worth
    /// reporting.
    fn stamp_teardown_or_unwind(
        &self,
        repo: &git2::Repository,
        name: &str,
        path: &Path,
        teardown: BranchTeardown,
    ) -> Result<(), WorktreeError> {
        let Err(error) = record_branch_teardown(path, teardown) else {
            return Ok(());
        };
        if let Ok(registered) = repo.find_worktree(name) {
            let mut prune = git2::WorktreePruneOptions::new();
            prune.valid(true).working_tree(true);
            let _ = registered.prune(Some(&mut prune));
        }
        let _ = std::fs::remove_dir_all(path);
        Err(error)
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
    ) -> Result<(), WorktreeError> {
        let output = bounded_git_fetch(
            &self.repo_path,
            remote,
            &format!("+refs/heads/{branch}:{tracking_ref}"),
        )?;
        if !output.status.success() {
            return Err(WorktreeError::Command(format!(
                "remote {remote:?} no longer carries branch {branch:?}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let fetched = repo.find_reference(tracking_ref)?.peel_to_commit()?;
        let mut local = repo.branch(branch, &fetched, false)?;
        let upstream = tracking_ref
            .strip_prefix("refs/remotes/")
            .ok_or_else(|| WorktreeError::Command(format!("{tracking_ref} is not a remote ref")))?;
        local.set_upstream(Some(upstream))?;
        Ok(())
    }

    /// Cut `branch` from the base, for a caller that meant a name rather than
    /// a branch. This is the one place a name Build is about to fold into a
    /// directory as well as a ref has to pass the narrower rule.
    fn cut_branch(
        &self,
        repo: &git2::Repository,
        branch: &str,
        base_branch: &str,
    ) -> Result<(), WorktreeError> {
        if !is_usable_branch_name(branch) {
            return Err(WorktreeError::Command(format!(
                "{branch:?} is not a name Build can cut a branch from"
            )));
        }
        let base_commit = repo.revparse_single(base_branch)?.peel_to_commit()?;
        repo.branch(branch, &base_commit, false)?;
        Ok(())
    }

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
    /// until nothing claims it. A name git has registered a worktree under or
    /// that already exists on disk is claimed; `also_taken` adds whatever else
    /// the caller's own namespace claims.
    fn unique_checkout_name(
        &self,
        repo: &git2::Repository,
        stem: &str,
        also_taken: impl Fn(&str) -> bool,
    ) -> String {
        let mut name = stem.to_string();
        let mut n = 2;
        while repo.find_worktree(&name).is_ok()
            || self.worktrees_root.join(&name).exists()
            || also_taken(&name)
        {
            name = format!("{stem}-{n}");
            n += 1;
        }
        name
    }

    /// Whether Build's own namespace already holds a branch for this slug.
    fn branch_taken(&self, repo: &git2::Repository, slug: &str) -> bool {
        repo.find_branch(&self.branch_name(slug), git2::BranchType::Local)
            .is_ok()
    }

    /// Build the branch name for a slug in Build's namespace.
    fn branch_name(&self, slug: &str) -> String {
        format!("{BRANCH_PREFIX}/{slug}")
    }

    /// Recreate a Build-owned checkout at its original path and branch, with
    /// the teardown its registration recorded carried across the prune.
    pub fn restore(
        &self,
        worktree: &Worktree,
        when_unregistered: UnregisteredRestore,
    ) -> Result<Worktree, WorktreeError> {
        let expected_path = self.managed_path_for(worktree)?;
        if worktree.path.exists() {
            return self.verify_existing_worktree(worktree, &expected_path);
        }
        let repo = git2::Repository::open(&self.repo_path)?;
        let teardown = self.teardown_across_prune(&repo, worktree, when_unregistered)?;
        self.ensure_local_branch_ref(&repo, worktree)?;
        self.readd_on_recorded_branch(&repo, worktree, teardown)?;
        self.verify_existing_worktree(worktree, &expected_path)
    }

    /// Where a restorable checkout must live: inside the managed root, under a
    /// name that is one directory component, on a branch spelling git would
    /// hold. Anything else is refused before a single ref is read or written.
    fn managed_path_for(&self, worktree: &Worktree) -> Result<PathBuf, WorktreeError> {
        let expected_path = self.worktrees_root.join(&worktree.name);
        if worktree.path != expected_path
            || worktree.name.is_empty()
            || worktree.name.contains(['/', '\\'])
        {
            return Err(WorktreeError::Command(
                "refusing to restore a worktree outside its managed root".to_string(),
            ));
        }
        if !git2::Reference::is_valid_name(&recorded_ref(worktree)) {
            return Err(WorktreeError::Command(format!(
                "invalid persisted branch: {:?}",
                worktree.recorded_branch
            )));
        }
        Ok(expected_path)
    }

    /// What teardown owns, taken out of the stale registration before the
    /// prune that removes it. Only a registration git says is *absent* leaves
    /// the environment unable to answer — then the caller's
    /// `when_unregistered` speaks, or refuses to. Every other git failure is
    /// surfaced, because a branch is deleted on the strength of this answer.
    fn teardown_across_prune(
        &self,
        repo: &git2::Repository,
        worktree: &Worktree,
        when_unregistered: UnregisteredRestore,
    ) -> Result<BranchTeardown, WorktreeError> {
        match repo.find_worktree(&worktree.name) {
            Ok(stale) => {
                let recorded = teardown_in_admin_dir(&admin_dir_for(repo, &worktree.name))?;
                let mut prune = git2::WorktreePruneOptions::new();
                prune.valid(true).working_tree(true);
                stale.prune(Some(&mut prune))?;
                Ok(recorded)
            }
            Err(error) if error.code() == git2::ErrorCode::NotFound => match when_unregistered {
                UnregisteredRestore::Write(teardown) => Ok(teardown),
                UnregisteredRestore::Refuse => Err(WorktreeError::Command(format!(
                    "cannot restore {:?}: its worktree registration is gone, so whether \
                     teardown owns branch {:?} cannot be decided",
                    worktree.name, worktree.recorded_branch
                ))),
            },
            Err(error) => Err(error.into()),
        }
    }

    /// Give the recorded branch a local ref, fetching exactly it from the
    /// branch's configured remote when only the remote still carries it. There
    /// is no fallback to the moving base: that would silently discard lineage.
    fn ensure_local_branch_ref(
        &self,
        repo: &git2::Repository,
        worktree: &Worktree,
    ) -> Result<(), WorktreeError> {
        let local_ref = recorded_ref(worktree);
        if repo.find_reference(&local_ref).is_ok() {
            return Ok(());
        }
        let remote = configured_remote_for_branch(repo, &worktree.recorded_branch)
            .unwrap_or_else(|| "origin".to_string());
        let output = bounded_git_fetch(
            &self.repo_path,
            &remote,
            &format!("+{local_ref}:{local_ref}"),
        )?;
        if !output.status.success() {
            return Err(WorktreeError::Command(format!(
                "branch {:?} was not found locally or on configured remote: {}",
                worktree.recorded_branch,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    /// Put the checkout back on its recorded branch and stamp the teardown it
    /// came in with into the fresh admin directory.
    fn readd_on_recorded_branch(
        &self,
        repo: &git2::Repository,
        worktree: &Worktree,
        teardown: BranchTeardown,
    ) -> Result<(), WorktreeError> {
        std::fs::create_dir_all(&self.worktrees_root)?;
        let branch_ref = repo.find_reference(&recorded_ref(worktree))?;
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(&branch_ref));
        repo.worktree(&worktree.name, &worktree.path, Some(&opts))?;
        record_branch_teardown(&worktree.path, teardown)
    }

    fn verify_existing_worktree(
        &self,
        worktree: &Worktree,
        expected_path: &Path,
    ) -> Result<Worktree, WorktreeError> {
        let actual = std::fs::canonicalize(&worktree.path)?;
        let expected = std::fs::canonicalize(expected_path)?;
        if actual != expected {
            return Err(WorktreeError::Command(
                "refusing to trust a worktree outside its canonical managed path".to_string(),
            ));
        }
        let primary = git2::Repository::open(&self.repo_path)?;
        let registered = primary.find_worktree(&worktree.name).map_err(|_| {
            WorktreeError::Command(format!(
                "existing path is not the registered worktree {:?}",
                worktree.name
            ))
        })?;
        if std::fs::canonicalize(registered.path())? != actual {
            return Err(WorktreeError::Command(
                "registered worktree path does not match the persisted path".to_string(),
            ));
        }
        let checkout = git2::Repository::open(&actual)?;
        if std::fs::canonicalize(checkout.commondir())?
            != std::fs::canonicalize(primary.commondir())?
        {
            return Err(WorktreeError::Command(
                "existing path belongs to a different git common directory".to_string(),
            ));
        }
        let head = checkout.head()?;
        if !head.is_branch() || head.shorthand() != Some(worktree.recorded_branch.as_str()) {
            return Err(WorktreeError::Command(format!(
                "worktree is not on the exact persisted branch {:?}",
                worktree.recorded_branch
            )));
        }
        let head_oid = head.target().ok_or_else(|| {
            WorktreeError::Command("worktree HEAD has no direct commit".to_string())
        })?;
        let branch_oid = primary
            .find_reference(&format!("refs/heads/{}", worktree.recorded_branch))?
            .target()
            .ok_or_else(|| WorktreeError::Command("persisted branch has no commit".to_string()))?;
        if head_oid != branch_oid {
            return Err(WorktreeError::Command(
                "worktree HEAD does not match the persisted branch tip".to_string(),
            ));
        }
        let base_oid = primary
            .revparse_single(&worktree.base_branch)?
            .peel_to_commit()?
            .id();
        primary.merge_base(base_oid, head_oid).map_err(|_| {
            WorktreeError::Command(format!(
                "worktree branch has no verified ancestry with {:?}",
                worktree.base_branch
            ))
        })?;
        Ok(worktree.clone())
    }

    /// Remove the worktree's working directory, prune git's record of it, and
    /// take the branch with it when the checkout says teardown owns it.
    ///
    /// The answer is read from the registration in the main repository — which
    /// this call is about to prune, and which still holds it when the working
    /// directory is already gone — so no caller has to sequence the read for
    /// itself. It is read *first*: a checkout that cannot say what its branch
    /// is owed is left standing whole rather than destroyed under a question
    /// nothing can answer afterwards. A branch that is already gone asks
    /// nothing of the marker.
    pub fn remove(&self, worktree: &Worktree) -> Result<(), WorktreeError> {
        let repo = git2::Repository::open(&self.repo_path)?;
        let deletes_branch = match repo
            .find_branch(&worktree.recorded_branch, git2::BranchType::Local)
        {
            Ok(_) => teardown_in_admin_dir(&admin_dir_for(&repo, &worktree.name))?.deletes_branch(),
            Err(error) if error.code() == git2::ErrorCode::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        self.unregister(&repo, worktree)?;
        if deletes_branch {
            repo.find_branch(&worktree.recorded_branch, git2::BranchType::Local)?
                .delete()?;
        }
        Ok(())
    }

    /// Take the checkout away and leave its branch standing, whatever teardown
    /// would otherwise own. An abandoned run's work outlives the run so it can
    /// be re-attempted, which is a promise about the branch the checkout
    /// itself cannot make.
    pub fn remove_keeping_branch(&self, worktree: &Worktree) -> Result<(), WorktreeError> {
        let repo = git2::Repository::open(&self.repo_path)?;
        self.unregister(&repo, worktree)
    }

    /// Working directory gone, git's record of it pruned.
    ///
    /// The goal is ABSENCE, so every step treats "already gone" as done: a
    /// worktree cleaned up outside Build (`git worktree remove` by hand, a
    /// reaped directory) must not block the verb that only wanted it gone.
    /// Anything still present that fails to go stays an error — a teardown
    /// failure is an error, not a shrug.
    fn unregister(
        &self,
        repo: &git2::Repository,
        worktree: &Worktree,
    ) -> Result<(), WorktreeError> {
        if worktree.path.exists() {
            std::fs::remove_dir_all(&worktree.path)?;
        }
        // find_worktree on pruned bookkeeping surfaces as NotFound — sometimes
        // via a baffling "could not find '.git/shallow' to stat" — and either
        // spelling means the same thing: nothing left to prune.
        match repo.find_worktree(&worktree.name) {
            Ok(gwt) => {
                let mut prune = git2::WorktreePruneOptions::new();
                prune.valid(true).working_tree(true);
                gwt.prune(Some(&mut prune))?;
            }
            Err(error) if error.code() == git2::ErrorCode::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }
}

/// The full ref a worktree's recorded branch is spelled under.
fn recorded_ref(worktree: &Worktree) -> String {
    format!("refs/heads/{}", worktree.recorded_branch)
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

pub(crate) fn bounded_git_fetch(
    repo_path: &Path,
    remote: &str,
    refspec: &str,
) -> Result<std::process::Output, WorktreeError> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let mut child = std::process::Command::new("git")
        .arg("fetch")
        .arg("--")
        .arg(remote)
        .arg(refspec)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "Never")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(repo_path)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(WorktreeError::Command(format!(
                "timed out fetching persisted ref {refspec:?} from remote {remote:?}"
            )));
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut pipe) = child.stdout.take() {
        pipe.read_to_end(&mut stdout)?;
    }
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_end(&mut stderr)?;
    }
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// One git worktree of the project repo that Build did not create (or no longer
/// tracks): the raw material of adoption. Pure data — discovery never mutates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalWorktree {
    /// Stable id: "wt-" + the first 12 hex chars of sha256 over the canonical
    /// absolute path (UTF-8 bytes of `path.display().to_string()`).
    pub id: String,
    /// Git's internal worktree name (`repo.find_worktree(name)` works) — kept so
    /// adoption can build a `Worktree` that `WorktreeManager::remove` understands.
    pub name: String,
    /// Canonical absolute path of the working directory.
    pub path: PathBuf,
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

/// The stable external-worktree id for a canonical absolute path.
pub fn external_worktree_id(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(path.display().to_string().as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("wt-{}", &hex[..12])
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

/// Enumerate every git worktree of `repo_path` that is neither the primary
/// checkout nor in `excluded_paths` (canonical paths of Build-bound worktrees —
/// runs, which must never surface as adoptable),
/// with a review summary per worktree. Read-only. A worktree whose summary
/// cannot be computed (corrupt checkout, no merge base with the base branch)
/// is skipped with an eprintln! — one broken stray must not fail the scan.
pub fn discover_external_worktrees(
    repo_path: &Path,
    base_branch: &str,
    excluded_paths: &HashSet<PathBuf>,
) -> Result<Vec<ExternalWorktree>, WorktreeError> {
    let primary_canonical = std::fs::canonicalize(repo_path)?;
    let target = ScanTarget::External {
        primary: &primary_canonical,
        excluded: excluded_paths,
    };
    let mut found = describe_checkouts(repo_path, base_branch, &target)?;
    found.sort_by(|a, b| {
        a.head_age_seconds
            .cmp(&b.head_age_seconds)
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(found)
}

/// The primary checkout described in the shape adoption takes for an external
/// worktree, or `None` when the repository has no working tree there to
/// describe — a bare repository is the whole of that case. A repository that
/// cannot be listed at all is broken rather than checkout-less, and says so
/// through the error. Read-only.
pub fn find_primary_checkout(
    repo_path: &Path,
    base_branch: &str,
) -> Result<Option<ExternalWorktree>, WorktreeError> {
    let primary_canonical = std::fs::canonicalize(repo_path)?;
    let target = ScanTarget::Primary {
        primary: &primary_canonical,
    };
    Ok(describe_checkouts(repo_path, base_branch, &target)?.pop())
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

/// Which of the repository's checkouts a scan describes. The membership test
/// runs BEFORE any summary is computed: a summary costs several git
/// invocations per checkout and the external scan runs on a poll.
enum ScanTarget<'a> {
    External {
        primary: &'a Path,
        excluded: &'a HashSet<PathBuf>,
    },
    Primary {
        primary: &'a Path,
    },
}

impl ScanTarget<'_> {
    fn admits(&self, canonical_path: &Path) -> bool {
        match self {
            ScanTarget::External { primary, excluded } => {
                canonical_path != *primary && !excluded.contains(canonical_path)
            }
            ScanTarget::Primary { primary } => canonical_path == *primary,
        }
    }

    fn is_primary(&self, canonical_path: &Path) -> bool {
        matches!(self, ScanTarget::Primary { primary } if canonical_path == *primary)
    }
}

/// `git worktree list --porcelain`, parsed into the summaries `target` admits.
fn describe_checkouts(
    repo_path: &Path,
    base_branch: &str,
    target: &ScanTarget<'_>,
) -> Result<Vec<ExternalWorktree>, WorktreeError> {
    let output = std::process::Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_path)
        .output()?;
    if !output.status.success() {
        return Err(WorktreeError::Command(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let repo = git2::Repository::open(repo_path)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    Ok(stdout
        .split("\n\n")
        .filter_map(|block| parse_worktree_block(block, &repo, target, base_branch, now))
        .collect())
}

/// Parse one `git worktree list --porcelain` block into an [`ExternalWorktree`],
/// or `None` if it should be skipped (bare/prunable, outside `target`, gone from
/// disk, or a summary that could not be computed — each case logs its own
/// `eprintln!` except the deliberately silent structural skips).
fn parse_worktree_block(
    block: &str,
    repo: &git2::Repository,
    target: &ScanTarget<'_>,
    base_branch: &str,
    now: i64,
) -> Option<ExternalWorktree> {
    let block = block.trim();
    if block.is_empty() {
        return None;
    }

    let mut path = None;
    let mut head_sha = None;
    let mut branch = None;
    let mut detached = false;
    let mut bare = false;
    let mut prunable = false;
    for line in block.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            head_sha = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("branch refs/heads/") {
            branch = Some(rest.to_string());
        } else if line == "detached" {
            detached = true;
        } else if line == "bare" {
            bare = true;
        } else if line.starts_with("prunable") {
            prunable = true;
        }
    }
    if bare || prunable {
        return None;
    }
    let path = path?;
    if !path.exists() {
        return None;
    }
    let canonical_path = std::fs::canonicalize(&path).ok()?;
    if !target.admits(&canonical_path) {
        return None;
    }
    let head_sha = head_sha?;
    if !detached && branch.is_none() {
        // Malformed block: neither a branch nor an explicit detached marker.
        return None;
    }

    // Git names only LINKED worktrees, so the primary checkout has none. A
    // name exists to make `WorktreeManager::remove` work, and the primary is
    // never removed (it is the repository), so its directory stands in.
    let name = if target.is_primary(&canonical_path) {
        canonical_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "primary".to_string())
    } else {
        resolve_worktree_name(repo, &canonical_path).or_else(|| {
            eprintln!(
                "discover_external_worktrees: no git worktree name for {}",
                canonical_path.display()
            );
            None
        })?
    };

    let head_oid = git2::Oid::from_str(&head_sha)
        .inspect_err(|e| {
            eprintln!(
                "discover_external_worktrees: bad HEAD sha for {}: {e}",
                canonical_path.display()
            );
        })
        .ok()?;
    let commit = repo
        .find_commit(head_oid)
        .inspect_err(|e| {
            eprintln!(
                "discover_external_worktrees: no commit {head_sha} for {}: {e}",
                canonical_path.display()
            );
        })
        .ok()?;
    let head_subject = commit.summary().unwrap_or("").to_string();
    let head_age_seconds = (now - commit.time().seconds()).max(0) as u64;
    let head_committed_at = rfc3339_from_unix(commit.time().seconds());

    let dirty_files = worktree_status_line_count(&canonical_path)
        .inspect_err(|e| {
            eprintln!(
                "discover_external_worktrees: status failed for {}: {e}",
                canonical_path.display()
            );
        })
        .ok()?;

    // Counts only — the board never shows this tree's patch, so never render one.
    let diffstat = crate::diff::stat_against_merge_base(&canonical_path, base_branch)
        .inspect_err(|e| {
            eprintln!(
                "discover_external_worktrees: diff failed for {}: {e}",
                canonical_path.display()
            );
        })
        .ok()?;

    // What is sitting in this tree unsaved — the +/− the rail shows. Distinct
    // from the diffstat above, which is everything the branch carries.
    let uncommitted = crate::diff::stat_uncommitted(&canonical_path)
        .inspect_err(|e| {
            eprintln!(
                "discover_external_worktrees: uncommitted diff failed for {}: {e}",
                canonical_path.display()
            );
        })
        .ok()?;

    let comparison = branch_comparison(repo, &commit, branch.as_deref(), base_branch);

    Some(ExternalWorktree {
        id: external_worktree_id(&canonical_path),
        name,
        path: canonical_path,
        branch,
        head_sha,
        head_subject,
        head_age_seconds,
        head_committed_at,
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

/// Match a canonicalized worktree path against git's own worktree registry to
/// recover the name `WorktreeManager` and `repo.find_worktree` expect.
fn resolve_worktree_name(repo: &git2::Repository, canonical_path: &Path) -> Option<String> {
    let names = repo.worktrees().ok()?;
    for name in names.iter().flatten() {
        let Ok(candidate) = repo.find_worktree(name) else {
            continue;
        };
        if std::fs::canonicalize(candidate.path()).ok().as_deref() == Some(canonical_path) {
            return Some(name.to_string());
        }
    }
    None
}

/// Count of non-empty `git status --porcelain` lines in `worktree_path`.
fn worktree_status_line_count(worktree_path: &Path) -> Result<usize, WorktreeError> {
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(worktree_path)
        .output()?;
    if !output.status.success() {
        return Err(WorktreeError::Command(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    /// Init a repo on `main` with one commit, returning (tempdir, repo_path).
    pub(super) fn init_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "test@build.ing"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(repo.join("README.md"), "# project\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "initial"]);
        (dir, repo)
    }

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
            .create_cutting_branch("build/csv-export", "main")
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
        let plain = mgr.create_cutting_branch("hotfix", "main").unwrap();
        assert_eq!(plain.worktree.recorded_branch, "hotfix");
        assert_eq!(plain.worktree.name, "hotfix");

        // A namespace that is not this manager's is kept whole in the directory
        // name, so two branches never share one directory.
        let foreign = mgr
            .create_cutting_branch("feature/csv-export", "main")
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
            .create_on_existing_branch("build/started-by-hand", "main")
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
        let refused = mgr.create_cutting_branch("add a csv export", "main");
        assert!(refused.is_err(), "{refused:?}");
    }

    /// A branch only a remote carries is fetched, made local with its upstream
    /// set, and checked out — never cut fresh over the top of the work it
    /// already holds.
    #[test]
    fn create_on_existing_branch_materialises_a_branch_only_a_remote_carries() {
        let (dir, repo) = init_repo();
        let origin = bare_origin_of(&dir, &repo);
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
        git_in(&repo, &["fetch", "origin"]);
        let mgr = manager(&dir, &repo);

        let added = mgr.create_on_existing_branch("feature-x", "main").unwrap();

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
            .create_on_existing_branch("nobody-cut-this", "main")
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
        let origin = bare_origin_of(&dir, &repo);
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
        git_in(&repo, &["fetch", "origin"]);
        git_in(&origin, &["branch", "-D", "feature-x"]);
        let mgr = manager(&dir, &repo);

        let error = mgr
            .create_on_existing_branch("feature-x", "main")
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
            .create_on_existing_branch("theirs", "main")
            .unwrap()
            .worktree;
        let marker = repo
            .join(".git/worktrees")
            .join(&added.name)
            .join(BRANCH_TEARDOWN_MARKER);
        std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o400)).unwrap();

        let error = mgr
            .stamp_teardown_or_unwind(&r, &added.name, &added.path, BranchTeardown::KeepsBranch)
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

    /// A bare clone of `repo` wired up as its `origin`.
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
        let wt = mgr.create("owned", "main").unwrap().worktree;

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
        let wt = mgr.create("relative-pointer", "main").unwrap().worktree;
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
        let wt = mgr.create("unreadable", "main").unwrap().worktree;
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

        let wt = mgr.create("fix-typo", "main").unwrap().worktree;

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

        let a = mgr.create("task-a", "main").unwrap().worktree;
        let b = mgr.create("task-b", "main").unwrap().worktree;

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
        let a = mgr.create("dup", "main").unwrap().worktree;
        let b = mgr.create("dup", "main").unwrap().worktree;
        let c = mgr.create("dup", "main").unwrap().worktree;
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
        let wt = mgr.create("keep-me", "main").unwrap().worktree;

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
        let wt = mgr.create("recover-local", "main").unwrap().worktree;
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
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("registered worktree"), "{error}");
    }

    #[test]
    fn restore_fetches_the_original_branch_when_only_origin_has_it() {
        let (dir, repo) = init_repo();
        bare_origin_of(&dir, &repo);
        let mgr = manager(&dir, &repo);
        let wt = mgr.create("recover-remote", "main").unwrap().worktree;
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
        let added = mgr.create_on_existing_branch("theirs", "main").unwrap();
        std::fs::remove_dir_all(&added.worktree.path).unwrap();

        let restored = mgr
            .restore(&added.worktree, UnregisteredRestore::Refuse)
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
        let wt = mgr.create("unvouched", "main").unwrap().worktree;
        mgr.remove(&wt).unwrap();

        let error = mgr
            .restore(&wt, UnregisteredRestore::Refuse)
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
            .create_on_existing_branch("theirs", "main")
            .unwrap()
            .worktree;
        std::fs::remove_dir_all(&wt.path).unwrap();
        git_in(&repo, &["update-ref", "-d", "refs/heads/theirs"]);
        std::fs::remove_file(repo.join(".git/worktrees/theirs/gitdir")).unwrap();

        let error = mgr
            .restore(
                &wt,
                UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
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
        let wt = mgr.create("drop-me", "main").unwrap().worktree;

        mgr.remove(&wt).unwrap();

        assert!(!wt.path.exists());
        let r = git2::Repository::open(&repo).unwrap();
        assert!(
            r.find_branch("build/drop-me", git2::BranchType::Local)
                .is_err(),
            "branch deleted"
        );
    }

    fn git_in(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
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

        let found = find_primary_checkout(&not_a_repo, "main");

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

        let described = find_primary_checkout(&repo, "main").unwrap().unwrap();
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
        let found = discover_external_worktrees(&repo, "main", &excluded).unwrap();

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

        let found = discover_external_worktrees(&repo, "main", &HashSet::new()).unwrap();

        let entry = &found[0];
        // The branch delta carries both; the uncommitted stat only what is
        // sitting in the tree unsaved.
        assert_eq!(entry.diffstat.insertions, 3);
        assert_eq!(entry.uncommitted.insertions, 2);
        assert_eq!(entry.uncommitted.files_changed, 1);
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

        let found = discover_external_worktrees(&repo, "main", &HashSet::new()).unwrap();

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

        let found = discover_external_worktrees(&repo, "main", &HashSet::new()).unwrap();

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

        let found = discover_external_worktrees(&repo, "main", &HashSet::new()).unwrap();

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
        let found = discover_external_worktrees(&repo, "main", &excluded).unwrap();

        assert!(found.is_empty());
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
        let found = discover_external_worktrees(&repo, "main", &excluded).unwrap();

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
        let before = discover_external_worktrees(&repo, "main", &excluded).unwrap();
        let sha_before = before[0].head_sha.clone();

        std::fs::write(wt_path.join("more.txt"), "more\n").unwrap();
        git_in(&wt_path, &["add", "more.txt"]);
        git_in(&wt_path, &["commit", "-m", "more work"]);

        let after = discover_external_worktrees(&repo, "main", &excluded).unwrap();
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
}

#[cfg(test)]
mod vanished_worktree_removal {
    use super::*;

    /// The state an outside cleanup leaves behind: directory removed,
    /// bookkeeping pruned, branch deleted.
    fn fully_vanished(repo: &Path, wt: &Worktree) {
        std::fs::remove_dir_all(&wt.path).unwrap();
        for args in [
            vec!["worktree", "prune"],
            vec!["branch", "-D", &wt.recorded_branch],
        ] {
            let out = std::process::Command::new("git")
                .args(&args)
                .current_dir(repo)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        }
    }

    #[test]
    fn removing_an_already_vanished_worktree_succeeds() {
        // The defect this guards: a Build worktree cleaned up outside Build
        // (dir, bookkeeping AND branch gone) made remove() fail on git2's
        // baffling "could not find '.git/shallow' to stat" from find_worktree,
        // which blocked the plan approve that only wanted the worktree gone.
        // Removal's goal is absence; finding absence is success.
        let (dir, repo) = tests::init_repo();
        let manager = WorktreeManager::new(&repo, dir.path().join("wts"));
        let wt = manager.create("gone-slug", "main").unwrap().worktree;
        fully_vanished(&repo, &wt);

        manager.remove(&wt).expect("absence is the goal");
    }

    #[test]
    fn removing_a_vanished_worktree_leaves_a_branch_it_can_no_longer_vouch_for() {
        // Partial carcass: dir and bookkeeping gone, branch still there. The
        // bookkeeping is where the checkout recorded whose branch that is, so
        // with it gone the branch is reported and left alone rather than
        // deleted on a guess.
        let (dir, repo) = tests::init_repo();
        let manager = WorktreeManager::new(&repo, dir.path().join("wts"));
        let wt = manager.create("half-gone", "main").unwrap().worktree;
        std::fs::remove_dir_all(&wt.path).unwrap();
        let out = std::process::Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(out.status.success());

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

        let (dir, repo) = tests::init_repo();
        let manager = WorktreeManager::new(&repo, dir.path().join("wts"));
        let r = git2::Repository::open(&repo).unwrap();
        let head = r.head().unwrap().peel_to_commit().unwrap();
        r.branch("theirs", &head, false).unwrap();
        let wt = manager
            .create_on_existing_branch("theirs", "main")
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
}
