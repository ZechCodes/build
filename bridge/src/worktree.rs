//! Git worktree lifecycle: one worktree + branch per task.
//!
//! The bridge owns worktrees. A task gets an isolated branch (`build/<slug>`) and
//! a working directory cut from the project's base branch, so parallel tasks on
//! the same repo never touch each other. On abandon the worktree is removed but
//! the branch is kept (abandoning stays reversible-ish); merge decides for itself.
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
    segments.iter().all(segment_is_usable)
        && git2::Reference::is_valid_name(&format!("refs/heads/{name}"))
}

/// A checkout added for a branch named in full, and whether that branch is one
/// the call cut. Tearing the checkout down deletes the branch only when the
/// answer is yes: a branch that was already there holds work nobody asked Build
/// to remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedBranchCheckout {
    pub worktree: Worktree,
    pub branch_was_cut: bool,
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
    pub fn create(&self, slug: &str, base_branch: &str) -> Result<Worktree, WorktreeError> {
        let repo = git2::Repository::open(&self.repo_path)?;
        let base_commit = repo.revparse_single(base_branch)?.peel_to_commit()?;
        std::fs::create_dir_all(&self.worktrees_root)?;

        let mut name = slug.to_string();
        let mut n = 2;
        while self.name_taken(&repo, &name) {
            name = format!("{slug}-{n}");
            n += 1;
        }
        let branch = self.branch_name(&name);

        // Cut the task branch from the tip of the base branch.
        repo.branch(&branch, &base_commit, false)?;
        let path = self.worktrees_root.join(&name);

        // Point the worktree at the branch we just created.
        let branch_ref = repo.find_reference(&format!("refs/heads/{branch}"))?;
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(&branch_ref));
        repo.worktree(&name, &path, Some(&opts))?;

        Ok(Worktree {
            name,
            path,
            recorded_branch: branch,
            base_branch: base_branch.to_string(),
        })
    }

    /// Add a worktree for the branch `branch`, spelled exactly as it was given.
    ///
    /// The counterpart to [`create`](Self::create): that one is handed a slug
    /// and owns the namespace, this one is handed the whole name and owns
    /// nothing but the directory. A branch that already exists is checked out
    /// rather than cut, so dispatching onto work started by hand reaches it.
    ///
    /// The answer says which of those two happened, because teardown turns on
    /// it: a branch that was already there is somebody's work, and removing the
    /// checkout must not take it with them.
    pub fn create_on_branch(
        &self,
        branch: &str,
        base_branch: &str,
    ) -> Result<NamedBranchCheckout, WorktreeError> {
        if !is_usable_branch_name(branch) {
            return Err(WorktreeError::Command(format!(
                "{branch:?} is not a branch name"
            )));
        }
        let repo = git2::Repository::open(&self.repo_path)?;
        std::fs::create_dir_all(&self.worktrees_root)?;

        let stem = self.directory_name_for(branch);
        let mut name = stem.clone();
        let mut n = 2;
        while repo.find_worktree(&name).is_ok() || self.worktrees_root.join(&name).exists() {
            name = format!("{stem}-{n}");
            n += 1;
        }

        let branch_ref = format!("refs/heads/{branch}");
        let branch_was_cut = repo.find_reference(&branch_ref).is_err();
        if branch_was_cut {
            let base_commit = repo.revparse_single(base_branch)?.peel_to_commit()?;
            repo.branch(branch, &base_commit, false)?;
        }
        let reference = repo.find_reference(&branch_ref)?;
        let path = self.worktrees_root.join(&name);
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(&reference));
        repo.worktree(&name, &path, Some(&opts))?;

        Ok(NamedBranchCheckout {
            worktree: Worktree {
                name,
                path,
                recorded_branch: branch.to_string(),
                base_branch: base_branch.to_string(),
            },
            branch_was_cut,
        })
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

    /// Whether a candidate name is already in use as a branch, a registered
    /// worktree, or an on-disk directory.
    fn name_taken(&self, repo: &git2::Repository, name: &str) -> bool {
        repo.find_branch(&self.branch_name(name), git2::BranchType::Local)
            .is_ok()
            || repo.find_worktree(name).is_ok()
            || self.worktrees_root.join(name).exists()
    }

    /// Build the branch name for a slug in Build's namespace.
    fn branch_name(&self, slug: &str) -> String {
        format!("{BRANCH_PREFIX}/{slug}")
    }

    /// Recreate a Build-owned checkout at its original path and branch. The
    /// local branch is authoritative when present; otherwise fetch exactly the
    /// same branch from its configured remote into a validated local ref. No
    /// fallback to the moving base is allowed because that would silently
    /// discard lineage.
    pub fn restore(&self, worktree: &Worktree) -> Result<Worktree, WorktreeError> {
        let expected_path = self.worktrees_root.join(&worktree.name);
        if worktree.path != expected_path
            || worktree.name.is_empty()
            || worktree.name.contains(['/', '\\'])
        {
            return Err(WorktreeError::Command(
                "refusing to restore a worktree outside its managed root".to_string(),
            ));
        }
        if worktree.path.exists() {
            return self.verify_existing_worktree(worktree, &expected_path);
        }
        let local_ref = format!("refs/heads/{}", worktree.recorded_branch);
        let remote_ref = format!("refs/heads/{}", worktree.recorded_branch);
        if !git2::Reference::is_valid_name(&local_ref) {
            return Err(WorktreeError::Command(format!(
                "invalid persisted branch: {:?}",
                worktree.recorded_branch
            )));
        }
        let repo = git2::Repository::open(&self.repo_path)?;
        if let Ok(stale) = repo.find_worktree(&worktree.name) {
            let mut prune = git2::WorktreePruneOptions::new();
            prune.valid(true).working_tree(true);
            stale.prune(Some(&mut prune))?;
        }
        if repo.find_reference(&local_ref).is_err() {
            let refspec = format!("+{remote_ref}:{local_ref}");
            let remote = configured_remote_for_branch(&repo, &worktree.recorded_branch)
                .unwrap_or_else(|| "origin".to_string());
            let output = bounded_git_fetch(&self.repo_path, &remote, &refspec)?;
            if !output.status.success() {
                return Err(WorktreeError::Command(format!(
                    "branch {:?} was not found locally or on configured remote: {}",
                    worktree.recorded_branch,
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
        }
        std::fs::create_dir_all(&self.worktrees_root)?;
        let branch_ref = repo.find_reference(&local_ref)?;
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(&branch_ref));
        repo.worktree(&worktree.name, &worktree.path, Some(&opts))?;
        self.verify_existing_worktree(worktree, &expected_path)
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

    /// Remove the worktree's working directory and prune git's record of it. When
    /// `keep_branch` is false the task branch is deleted too.
    pub fn remove(&self, worktree: &Worktree, keep_branch: bool) -> Result<(), WorktreeError> {
        let repo = git2::Repository::open(&self.repo_path)?;

        // Removal's goal is ABSENCE, so every step treats "already gone" as
        // done: a worktree cleaned up outside Build (`git worktree remove` by
        // hand, a reaped directory) must not block the verb that only wanted
        // it gone. Anything still present that fails to go stays an error —
        // a teardown failure is an error, not a shrug.
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

        // The branch is only deletable once it is no longer checked out.
        if !keep_branch {
            match repo.find_branch(&worktree.recorded_branch, git2::BranchType::Local) {
                Ok(mut branch) => branch.delete()?,
                Err(error) if error.code() == git2::ErrorCode::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
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

/// [`find_primary_checkout`] for a caller that has a checkout to adopt or
/// nothing to do. Read-only.
pub fn describe_primary_checkout(
    repo_path: &Path,
    base_branch: &str,
) -> Result<ExternalWorktree, WorktreeError> {
    find_primary_checkout(repo_path, base_branch)?.ok_or_else(|| {
        WorktreeError::Command(format!(
            "the primary checkout at {} cannot be described — a bare repository has no \
             working tree to adopt",
            repo_path.display()
        ))
    })
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
    fn create_on_branch_cuts_the_branch_exactly_as_it_was_named() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);

        let prefixed = mgr.create_on_branch("build/csv-export", "main").unwrap();
        assert_eq!(prefixed.worktree.recorded_branch, "build/csv-export");
        assert_eq!(prefixed.worktree.name, "csv-export");
        assert!(prefixed.branch_was_cut, "nothing was on that name before");
        assert!(prefixed.worktree.path.join("README.md").exists());

        // A name with no namespace stays with no namespace: nothing is added to
        // what the caller asked for.
        let plain = mgr.create_on_branch("hotfix", "main").unwrap();
        assert_eq!(plain.worktree.recorded_branch, "hotfix");
        assert_eq!(plain.worktree.name, "hotfix");

        // A namespace that is not this manager's is kept whole in the directory
        // name, so two branches never share one directory.
        let foreign = mgr.create_on_branch("feature/csv-export", "main").unwrap();
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
    fn create_on_branch_checks_out_a_branch_that_already_exists() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let r = git2::Repository::open(&repo).unwrap();
        let head = r.head().unwrap().peel_to_commit().unwrap();
        r.branch("build/started-by-hand", &head, false).unwrap();

        let added = mgr
            .create_on_branch("build/started-by-hand", "main")
            .unwrap();

        assert_eq!(added.worktree.recorded_branch, "build/started-by-hand");
        assert!(
            !added.branch_was_cut,
            "the branch was already there, and removing this checkout must not take it"
        );
        let checkout = git2::Repository::open(&added.worktree.path).unwrap();
        assert_eq!(
            checkout.head().unwrap().shorthand(),
            Some("build/started-by-hand")
        );

        // And teardown that keeps the branch does exactly that.
        mgr.remove(&added.worktree, /* keep_branch */ true).unwrap();
        assert!(r
            .find_branch("build/started-by-hand", git2::BranchType::Local)
            .is_ok());
    }

    #[test]
    fn create_on_branch_refuses_a_name_that_is_not_a_branch_name() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let refused = mgr.create_on_branch("add a csv export", "main");
        assert!(refused.is_err(), "{refused:?}");
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

        let wt = mgr.create("fix-typo", "main").unwrap();

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

        let a = mgr.create("task-a", "main").unwrap();
        let b = mgr.create("task-b", "main").unwrap();

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
        let a = mgr.create("dup", "main").unwrap();
        let b = mgr.create("dup", "main").unwrap();
        let c = mgr.create("dup", "main").unwrap();
        assert_eq!(a.name, "dup");
        assert_eq!(b.name, "dup-2");
        assert_eq!(c.name, "dup-3");
        assert_eq!(b.recorded_branch, "build/dup-2");
        assert!(b.path.join("README.md").exists());
    }

    #[test]
    fn abandon_removes_worktree_keeps_branch() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr.create("keep-me", "main").unwrap();

        mgr.remove(&wt, /* keep_branch */ true).unwrap();

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
        let wt = mgr.create("recover-local", "main").unwrap();
        std::fs::write(wt.path.join("stage.txt"), "kept\n").unwrap();
        git_in(&wt.path, &["add", "stage.txt"]);
        git_in(&wt.path, &["commit", "-m", "stage"]);
        let head = git2::Repository::open(&wt.path)
            .unwrap()
            .head()
            .unwrap()
            .target()
            .unwrap();
        mgr.remove(&wt, true).unwrap();

        let restored = mgr.restore(&wt).unwrap();
        assert_eq!(restored, wt);
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

        let error = mgr.restore(&forged).unwrap_err().to_string();
        assert!(error.contains("registered worktree"), "{error}");
    }

    #[test]
    fn restore_fetches_the_original_branch_when_only_origin_has_it() {
        let (dir, repo) = init_repo();
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
        git_in(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        let mgr = manager(&dir, &repo);
        let wt = mgr.create("recover-remote", "main").unwrap();
        std::fs::write(wt.path.join("remote-stage.txt"), "remote\n").unwrap();
        git_in(&wt.path, &["add", "remote-stage.txt"]);
        git_in(&wt.path, &["commit", "-m", "remote stage"]);
        git_in(&wt.path, &["push", "-u", "origin", &wt.recorded_branch]);
        mgr.remove(&wt, true).unwrap();
        git_in(&repo, &["branch", "-D", &wt.recorded_branch]);
        git_in(
            &repo,
            &[
                "update-ref",
                "-d",
                &format!("refs/remotes/origin/{}", wt.recorded_branch),
            ],
        );

        mgr.restore(&wt).unwrap();
        assert_eq!(
            std::fs::read_to_string(wt.path.join("remote-stage.txt")).unwrap(),
            "remote\n"
        );
    }

    #[test]
    fn remove_without_keep_deletes_branch() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr.create("drop-me", "main").unwrap();

        mgr.remove(&wt, /* keep_branch */ false).unwrap();

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
        let wt = manager.create("gone-slug", "main").unwrap();
        fully_vanished(&repo, &wt);

        manager.remove(&wt, false).expect("absence is the goal");
    }

    #[test]
    fn removing_a_vanished_worktree_still_deletes_a_surviving_branch() {
        // Partial carcass: dir and bookkeeping gone, branch still there — the
        // branch must still be deleted, not skipped along with the rest.
        let (dir, repo) = tests::init_repo();
        let manager = WorktreeManager::new(&repo, dir.path().join("wts"));
        let wt = manager.create("half-gone", "main").unwrap();
        std::fs::remove_dir_all(&wt.path).unwrap();
        let out = std::process::Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(out.status.success());

        manager
            .remove(&wt, false)
            .expect("carcass cleanup succeeds");
        let repo = git2::Repository::open(&repo).unwrap();
        assert!(
            repo.find_branch(&wt.recorded_branch, git2::BranchType::Local)
                .is_err(),
            "the surviving branch is deleted, not skipped"
        );
    }
}
