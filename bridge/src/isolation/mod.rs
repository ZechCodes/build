//! How a checkout is isolated from the project it came from.
//!
//! Build gives every task its own checkout. There are two ways to make one — a
//! git linked worktree of the project repository, and a copy-on-write clone of
//! the whole project directory — and this module is where that variation
//! lives. [`Isolation`] is the choice as a value, [`Isolation::of`] reads the
//! choice back off a checkout on disk (the environment is the source of truth;
//! nothing about isolation is persisted), [`IsolationAvailability`] answers
//! whether a volume can make one, and [`IsolationBackend`] is the whole of what
//! a backend does. `WorktreeManager` is the only caller.

pub mod cow;
pub mod probe;
pub mod worktree;

use std::path::{Path, PathBuf};

use crate::git_process::GitError;

pub use worktree::WorktreeBackend;

/// Things that can go wrong managing a checkout.
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// A git command ran and failed, in git's own words.
    #[error("git command failed: {0}")]
    Command(String),
    /// Something the manager will not do, in the sentence it says it with. No
    /// git command ran, so it must not read as one having failed.
    #[error("{0}")]
    Refused(String),
    #[error("not a Build checkout: {0}")]
    NotABuildCheckout(PathBuf),
    /// An isolation this project cannot be checked out with here, in the words
    /// the controls show. No git command ran, so it says only what it means.
    #[error("{0}")]
    IsolationUnavailable(String),
}

impl From<GitError> for WorktreeError {
    fn from(error: GitError) -> Self {
        match error {
            GitError::Unstartable(io) => WorktreeError::Io(io),
            GitError::Failed(detail) => WorktreeError::Command(detail),
        }
    }
}

/// How a checkout is isolated from the project it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Isolation {
    /// A git linked worktree of the project repository (`git worktree add`).
    #[default]
    Worktree,
    /// A copy-on-write clone of the whole project directory, `.git` included.
    Cow,
}

impl Isolation {
    /// Every isolation there is.
    pub const ALL: [Isolation; 2] = [Isolation::Worktree, Isolation::Cow];

    /// The word the wire, the settings file and the controls all use.
    pub fn wire(self) -> &'static str {
        match self {
            Isolation::Worktree => "worktree",
            Isolation::Cow => "cow",
        }
    }

    /// The isolation a wire word names, or `None` when it names none.
    pub fn from_wire(name: &str) -> Option<Self> {
        Isolation::ALL
            .into_iter()
            .find(|isolation| isolation.wire() == name)
    }

    /// How the checkout at `path` is isolated, read from the checkout itself.
    /// `None` when it is not a checkout Build could have made: no `.git`, or a
    /// standalone repository without the clone marker. Two `stat`s — this runs
    /// on every poll-path use of a checkout, so it never opens git.
    pub fn of(path: &Path) -> Option<Isolation> {
        let git_dir = path.join(".git");
        let metadata = std::fs::metadata(&git_dir).ok()?;
        if metadata.is_file() {
            return Some(Isolation::Worktree);
        }
        if metadata.is_dir() && git_dir.join(COW_MARKER).exists() {
            return Some(Isolation::Cow);
        }
        None
    }
}

/// What the checkout at `path` is called: its directory's name, whatever made
/// it. Git names a linked worktree after its directory and a clone has no
/// other name, so this is the one name every backend calls a checkout by. A
/// path with no directory to be called by is no checkout, as a path with no
/// `.git` is none for [`Isolation::of`].
pub fn checkout_name(path: &Path) -> Option<String> {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

/// The ref a repository keeps a local branch under: `refs/heads/<branch>`.
/// The façade finds, cuts and deletes branches by it in the project repo and
/// a backend checks them out by it, so both sides of the seam spell it here.
pub fn local_branch_ref(branch: &str) -> String {
    format!("refs/heads/{branch}")
}

/// Git's directory for the checkout at `checkout`: the one `<checkout>/.git`
/// points at for a linked worktree, and `<checkout>/.git` itself for a
/// repository of its own — a clone, or a project's main checkout. A relative
/// `gitdir:` pointer (git 2.48+ with `worktree.useRelativePaths`) is resolved
/// against the directory holding the pointer, which is what git does with it.
///
/// The one owner of where a checkout keeps its git directory, so every marker
/// beside a checkout — the clone's and the teardown's alike — is found the same
/// way and no backend spells a marker path.
pub fn checkout_git_dir(checkout: &Path) -> Result<PathBuf, WorktreeError> {
    let pointer = checkout.join(".git");
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
        checkout.join(gitdir)
    })
}

/// The file inside a clone's `.git` that says the clone is Build's and which
/// project it was cloned from. A clone is a repository like any other, so this
/// is the only thing that tells it apart; its contents are compared, never used
/// to build a path to act on.
pub const COW_MARKER: &str = "build-isolation";

/// The marker a clone of `project` carries: its isolation and the project's
/// canonical path, one per line.
fn cow_marker_body(project: &Path) -> std::io::Result<String> {
    let canonical = std::fs::canonicalize(project)?;
    Ok(format!(
        "{}\n{}\n",
        Isolation::Cow.wire(),
        canonical.display()
    ))
}

/// Write the marker into `checkout`, naming `project` as where it was cloned from.
pub fn write_cow_marker(checkout: &Path, project: &Path) -> Result<(), WorktreeError> {
    std::fs::write(
        checkout_git_dir(checkout)?.join(COW_MARKER),
        cow_marker_body(project)?,
    )?;
    Ok(())
}

/// Whether the checkout at `checkout` carries a clone marker naming `project`.
pub fn cow_marker_names(checkout: &Path, project: &Path) -> bool {
    let Ok(git_dir) = checkout_git_dir(checkout) else {
        return false;
    };
    let Ok(found) = std::fs::read_to_string(git_dir.join(COW_MARKER)) else {
        return false;
    };
    cow_marker_body(project).is_ok_and(|expected| found == expected)
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

/// The file, in the checkout's own git directory, that records its
/// [`BranchTeardown`]. That directory goes when the checkout does — git prunes
/// a linked worktree's entry with it, and a clone's `.git` is inside it — so
/// the fact cannot outlive what it describes.
pub(crate) const BRANCH_TEARDOWN_MARKER: &str = "build-branch-teardown";

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

/// Record what teardown of the checkout at `checkout` owns. Isolation-blind:
/// the marker lands in whatever git directory the checkout has, which for a
/// clone is its own `.git` beside [`COW_MARKER`] and for a linked worktree is
/// the entry git keeps for it in the project.
pub fn record_branch_teardown(
    checkout: &Path,
    teardown: BranchTeardown,
) -> Result<(), WorktreeError> {
    let git_dir = checkout_git_dir(checkout)?;
    std::fs::write(git_dir.join(BRANCH_TEARDOWN_MARKER), teardown.as_str())?;
    Ok(())
}

/// What teardown of the checkout at `checkout` owns, read from the checkout
/// itself — the answer whenever the checkout is still standing, whatever made
/// it.
///
/// Exactly one reading means [`BranchTeardown::DeletesBranch`]: git's admin
/// directory for the checkout was read and holds no marker, which is a
/// checkout Build did not create — one made by hand and adopted — whose
/// branch the human's chosen action speaks for. Every failure to read is
/// returned, because the alternative to an error here is deleting a ref
/// nobody asked Build to touch.
pub fn branch_teardown(checkout: &Path) -> Result<BranchTeardown, WorktreeError> {
    teardown_in_git_dir(&checkout_git_dir(checkout)?)
}

/// The same answer read out of a git directory the caller already has — the
/// entry a backend still holds for a checkout whose own directory is gone.
pub fn teardown_in_git_dir(git_dir: &Path) -> Result<BranchTeardown, WorktreeError> {
    if !git_dir.is_dir() {
        return Err(WorktreeError::Command(format!(
            "no git admin directory at {}",
            git_dir.display()
        )));
    }
    match std::fs::read_to_string(git_dir.join(BRANCH_TEARDOWN_MARKER)) {
        Ok(text) => BranchTeardown::parse(text.trim()).ok_or_else(|| {
            WorktreeError::Command(format!(
                "unreadable branch-teardown marker in {}",
                git_dir.display()
            ))
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(BranchTeardown::DeletesBranch)
        }
        Err(error) => Err(error.into()),
    }
}

/// Which isolations can be used for a project on this volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsolationAvailability {
    /// `Ok(())` when a clone can be made for this project on this volume;
    /// `Err(reason)` is the sentence the settings controls show.
    pub cow: Result<(), String>,
}

impl IsolationAvailability {
    /// What this volume can do for `project`, whose checkouts live under
    /// `worktrees_root`.
    pub fn of(project: &Path, worktrees_root: &Path) -> Self {
        IsolationAvailability {
            cow: probe::cow_availability(project, worktrees_root),
        }
    }

    /// Nothing can be cloned, for a reason no volume answered — the caller's
    /// own sentence, carried in the shape every control already reads, so the
    /// representation stays here rather than being rebuilt by whoever has a
    /// sentence of their own.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        IsolationAvailability {
            cow: Err(reason.into()),
        }
    }

    /// Why `isolation` cannot be used here, or `None` when it can. A linked
    /// worktree is never locked; a clone is locked by the probe's reason. The
    /// one owner of which isolation a volume can lock, so nothing outside this
    /// module has to name a variant to ask.
    pub fn lock_reason(&self, isolation: Isolation) -> Option<&str> {
        match isolation {
            Isolation::Worktree => None,
            Isolation::Cow => self.cow.as_ref().err().map(String::as_str),
        }
    }
}

impl serde::Serialize for IsolationAvailability {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut shape = serializer.serialize_struct("IsolationAvailability", 2)?;
        shape.serialize_field("cow", &self.cow.is_ok())?;
        shape.serialize_field("reason", &self.cow.as_ref().err())?;
        shape.end()
    }
}

/// Everything a backend does. Each primitive is whole: no caller sequences two
/// of them to get one outcome, and none knows about runs, plans, threads,
/// settings or naming. Branch cutting and deletion are absent by design — they
/// are project-repo work, identical for both isolations, so the façade owns them.
pub trait IsolationBackend: Send + Sync {
    /// The isolation this backend makes.
    fn kind(&self) -> Isolation;

    /// Put a checkout of `branch` (which already exists in `project`) at `path`.
    /// On any failure nothing is left at `path`.
    fn materialize(&self, project: &Path, branch: &str, path: &Path) -> Result<(), WorktreeError>;

    /// Everything only this backend can check about the checkout at `path`:
    /// that it is this backend's, that it belongs to `project`, and, where the
    /// backend is the only thing that can tell, that it is on `branch`. The
    /// checks every isolation shares — HEAD on the branch, HEAD at the
    /// project's tip, a merge-base with the base — stay in `WorktreeManager`.
    fn verify(&self, project: &Path, path: &Path, branch: &str) -> Result<(), WorktreeError>;

    /// Make the checkout's tip of `branch` the project repo's `refs/heads/<branch>`.
    fn publish(&self, project: &Path, path: &Path, branch: &str) -> Result<(), WorktreeError>;

    /// Make the project repo's tip of `base_branch` the checkout's
    /// `refs/heads/<base_branch>`, so diffs and ahead/behind against the base
    /// mean the same thing in both isolations.
    fn sync_base(
        &self,
        project: &Path,
        path: &Path,
        base_branch: &str,
    ) -> Result<(), WorktreeError>;

    /// Delete the checkout at `path` and this backend's own record of it. A
    /// checkout is named by its directory, so the name is the path's own and no
    /// caller can pass one that disagrees with it. Absence is success.
    fn remove(&self, project: &Path, path: &Path) -> Result<(), WorktreeError>;

    /// Canonical paths of every checkout of `project` this backend can find
    /// under `worktrees_root` or in its own records, the project's own checkout
    /// excluded.
    fn discover(
        &self,
        project: &Path,
        worktrees_root: &Path,
    ) -> Result<Vec<PathBuf>, WorktreeError>;

    /// Clear this backend's stale records of checkouts that no longer exist.
    fn prune(&self, project: &Path) -> Result<(), WorktreeError>;

    /// Whether this backend holds a record of a checkout called `name`.
    fn holds_record(&self, project: &Path, name: &str) -> Result<bool, WorktreeError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_fixture::{git_in, init_repo};

    #[test]
    fn a_local_branch_ref_lives_under_refs_heads() {
        assert_eq!(local_branch_ref("main"), "refs/heads/main");
        assert_eq!(local_branch_ref("build/slug"), "refs/heads/build/slug");
    }

    #[test]
    fn a_checkout_says_how_it_is_isolated() {
        let (dir, repo) = init_repo();

        let linked = dir.path().join("linked");
        git_in(
            &repo,
            &["worktree", "add", linked.to_str().unwrap(), "-b", "side"],
        );
        assert_eq!(Isolation::of(&linked), Some(Isolation::Worktree));

        // A standalone repository is nobody's checkout until it carries the marker.
        let clone = dir.path().join("clone");
        std::fs::create_dir_all(clone.join(".git")).unwrap();
        assert_eq!(Isolation::of(&clone), None);
        write_cow_marker(&clone, &repo).unwrap();
        assert_eq!(Isolation::of(&clone), Some(Isolation::Cow));
        assert!(cow_marker_names(&clone, &repo));
        assert!(!cow_marker_names(&clone, dir.path()));

        assert_eq!(
            Isolation::of(&repo),
            None,
            "the project is not a checkout Build made"
        );
        assert_eq!(Isolation::of(&dir.path().join("nothing-here")), None);
    }

    /// A checkout is called by its directory, whatever made it — and a path
    /// with no directory to be called by is no checkout at all.
    #[test]
    fn a_checkout_is_named_by_its_directory() {
        assert_eq!(
            checkout_name(Path::new("/tmp/worktrees/csv-export")),
            Some("csv-export".to_string())
        );
        assert_eq!(
            checkout_name(Path::new("worktrees/csv-export/")),
            Some("csv-export".to_string())
        );
        assert_eq!(checkout_name(Path::new("/")), None);
        assert_eq!(checkout_name(Path::new("")), None);
    }

    #[test]
    fn the_wire_word_round_trips_and_nothing_else_parses() {
        for isolation in Isolation::ALL {
            assert_eq!(Isolation::from_wire(isolation.wire()), Some(isolation));
        }
        assert_eq!(Isolation::from_wire("Worktree"), None);
        assert_eq!(Isolation::from_wire("clone"), None);
        assert_eq!(Isolation::from_wire(""), None);
        assert_eq!(Isolation::default(), Isolation::Worktree);
    }

    #[test]
    fn availability_locks_only_the_isolation_the_volume_cannot_make() {
        let (dir, repo) = init_repo();
        let availability = IsolationAvailability::of(&repo, &dir.path().join("worktrees"));

        assert_eq!(availability.lock_reason(Isolation::Worktree), None);
        // A clone is locked exactly when the probe could not make one, in the
        // probe's own words — no platform assumption either way.
        assert_eq!(
            availability.lock_reason(Isolation::Cow),
            availability.cow.as_ref().err().map(String::as_str),
        );
        assert_eq!(
            serde_json::to_value(&availability).unwrap(),
            serde_json::json!({
                "cow": availability.cow.is_ok(),
                "reason": availability.cow.as_ref().err(),
            }),
        );
    }
}
