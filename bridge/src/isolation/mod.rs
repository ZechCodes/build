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

pub mod worktree;

use std::path::{Path, PathBuf};
use std::process::Command;

pub use worktree::WorktreeBackend;

/// Things that can go wrong managing a checkout.
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("git command failed: {0}")]
    Command(String),
    #[error("not a Build checkout: {0}")]
    NotABuildCheckout(PathBuf),
}

/// One git command in `dir`, its output or why it failed. git splits its story
/// across streams (a conflicting merge reports "CONFLICT …" on stdout), so a
/// failure carries both.
pub(crate) fn run_git(dir: &Path, args: &[&str]) -> Result<String, WorktreeError> {
    let out = Command::new("git").args(args).current_dir(dir).output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let detail: Vec<&str> = [stderr.trim(), stdout.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
        return Err(WorktreeError::Command(format!(
            "git {args:?}: {}",
            detail.join("\n")
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
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
    /// Every isolation there is, in the order the façade walks them.
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
pub fn write_cow_marker(checkout: &Path, project: &Path) -> std::io::Result<()> {
    std::fs::write(
        checkout.join(".git").join(COW_MARKER),
        cow_marker_body(project)?,
    )
}

/// Whether the checkout at `checkout` carries a clone marker naming `project`.
pub fn cow_marker_names(checkout: &Path, project: &Path) -> bool {
    let Ok(found) = std::fs::read_to_string(checkout.join(".git").join(COW_MARKER)) else {
        return false;
    };
    cow_marker_body(project).is_ok_and(|expected| found == expected)
}

/// Why a clone cannot be made until the clone backend exists.
const COW_NOT_IN_THIS_BUILD: &str = "copy-on-write isolation is not available in this build";

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
    pub fn of(_project: &Path, _worktrees_root: &Path) -> Self {
        IsolationAvailability {
            cow: Err(COW_NOT_IN_THIS_BUILD.to_string()),
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

    /// The checkout at `path` is this backend's, belongs to `project`, and has
    /// `branch` checked out.
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

    /// Delete the checkout at `path` and this backend's own record of it, where
    /// `name` is the checkout's directory name. Absence is success.
    fn remove(&self, project: &Path, path: &Path, name: &str) -> Result<(), WorktreeError>;

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
    use std::path::PathBuf;
    use std::process::Command;

    fn init_repo() -> (tempfile::TempDir, PathBuf) {
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

    #[test]
    fn a_checkout_says_how_it_is_isolated() {
        let (dir, repo) = init_repo();

        let linked = dir.path().join("linked");
        let status = Command::new("git")
            .args(["worktree", "add", linked.to_str().unwrap(), "-b", "side"])
            .current_dir(&repo)
            .status()
            .unwrap();
        assert!(status.success());
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
        let reason = availability
            .lock_reason(Isolation::Cow)
            .expect("no clone backend in this build");
        assert_eq!(
            availability.cow.as_ref().err().map(String::as_str),
            Some(reason)
        );

        assert_eq!(
            serde_json::to_value(&availability).unwrap(),
            serde_json::json!({ "cow": false, "reason": reason }),
        );
    }
}
