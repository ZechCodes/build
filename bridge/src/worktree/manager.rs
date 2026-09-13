use crate::isolation::{
    Isolation, IsolationAvailability, IsolationBackend, RiftBackend, WorktreeBackend, WorktreeError,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
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
    pub(super) repo_path: PathBuf,
    pub(super) worktrees_root: PathBuf,
    worktree: WorktreeBackend,
    rift: RiftBackend,
    creation_lock: Arc<Mutex<()>>,
}

impl WorktreeManager {
    /// `repo_path` is the project git repo; `worktrees_root` is where task
    /// worktrees are materialized (one subdirectory per task slug). Branches
    /// are cut in the `build/` namespace.
    pub fn new(repo_path: impl Into<PathBuf>, worktrees_root: impl Into<PathBuf>) -> Self {
        let worktrees_root = worktrees_root.into();
        WorktreeManager {
            repo_path: repo_path.into(),
            rift: RiftBackend::new(&worktrees_root),
            worktrees_root,
            worktree: WorktreeBackend,
            creation_lock: Arc::new(Mutex::new(())),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_rift_executable(
        mut self,
        executable: impl Into<std::ffi::OsString>,
    ) -> Self {
        self.rift = RiftBackend::with_executable(&self.worktrees_root, executable);
        self
    }

    pub(super) fn lock_creation(&self) -> Result<MutexGuard<'_, ()>, WorktreeError> {
        self.creation_lock.lock().map_err(|_| {
            WorktreeError::Refused(
                "checkout creation is unavailable because its coordination lock is poisoned"
                    .to_string(),
            )
        })
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
    /// One slot per isolation there is, in the order [`Isolation::ALL`] names
    /// them: the backend that makes it, or nothing when this build has none.
    /// Keyed to the enum by length, so it is the one list of backends and a new
    /// isolation cannot be added without filling in its slot here.
    pub(super) fn backends(&self) -> [Option<&dyn IsolationBackend>; Isolation::ALL.len()] {
        [Some(&self.worktree), Some(&self.rift)]
    }
    /// Every backend this build has, in the order above — what the three walks
    /// that have no isolation to key on iterate.
    pub(super) fn every_backend(&self) -> impl Iterator<Item = &dyn IsolationBackend> {
        self.backends().into_iter().flatten()
    }
    /// The backend that makes `isolation`, or why this volume cannot. An
    /// isolation with no backend is one [`IsolationAvailability`] locks, so the
    /// refusal is its sentence and there is no other.
    pub(super) fn backend(
        &self,
        isolation: Isolation,
    ) -> Result<&dyn IsolationBackend, WorktreeError> {
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
    pub(super) fn backend_of(&self, path: &Path) -> Result<&dyn IsolationBackend, WorktreeError> {
        let isolation = Isolation::of(path)
            .ok_or_else(|| WorktreeError::NotABuildCheckout(path.to_path_buf()))?;
        self.backend(isolation)
    }
    /// Whether any backend holds a record of a checkout called `name`. A name
    /// carries no isolation, so this is one walk and every caller asks it here.
    pub(super) fn record_held(&self, name: &str) -> Result<bool, WorktreeError> {
        for backend in self.every_backend() {
            if backend.holds_record(&self.repo_path, name)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
