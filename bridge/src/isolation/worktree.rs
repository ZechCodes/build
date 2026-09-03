//! The git linked-worktree backend: `git worktree add`, and the only place in
//! the bridge that speaks to git's worktree registry.

use std::path::{Path, PathBuf};

use super::{checkout_name, local_branch_ref, Isolation, IsolationBackend, WorktreeError};
use crate::git_process::run_git;

/// Materializes a checkout as a git linked worktree of the project repository.
/// The project repo already holds every ref, so publishing and base-syncing are
/// nothing at all.
pub struct WorktreeBackend;

impl IsolationBackend for WorktreeBackend {
    fn kind(&self) -> Isolation {
        Isolation::Worktree
    }

    fn materialize(&self, project: &Path, branch: &str, path: &Path) -> Result<(), WorktreeError> {
        let name = registered_name(path)?;
        let repo = git2::Repository::open(project)?;
        let reference = repo.find_reference(&local_branch_ref(branch))?;
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(&reference));
        repo.worktree(&name, path, Some(&options))?;
        Ok(())
    }

    fn verify(&self, project: &Path, path: &Path, _branch: &str) -> Result<(), WorktreeError> {
        let actual = std::fs::canonicalize(path)?;
        let name = registered_name(&actual)?;
        let primary = git2::Repository::open(project)?;
        let registered = primary.find_worktree(&name).map_err(|_| {
            WorktreeError::Refused(format!(
                "existing path is not the registered worktree {name:?}"
            ))
        })?;
        if std::fs::canonicalize(registered.path())? != actual {
            return Err(WorktreeError::Refused(
                "registered worktree path does not match the persisted path".to_string(),
            ));
        }
        let checkout = git2::Repository::open(&actual)?;
        if std::fs::canonicalize(checkout.commondir())?
            != std::fs::canonicalize(primary.commondir())?
        {
            return Err(WorktreeError::Refused(
                "existing path belongs to a different git common directory".to_string(),
            ));
        }
        Ok(())
    }

    fn publish(&self, _project: &Path, _path: &Path, _branch: &str) -> Result<(), WorktreeError> {
        Ok(())
    }

    fn sync_base(
        &self,
        _project: &Path,
        _path: &Path,
        _base_branch: &str,
    ) -> Result<(), WorktreeError> {
        Ok(())
    }

    fn remove(&self, project: &Path, path: &Path) -> Result<(), WorktreeError> {
        let name = registered_name(path)?;
        // The project repo is opened before the directory goes: a teardown that
        // cannot reach the registry would leave a record naming a directory it
        // can no longer prune, so it refuses while there is still nothing lost.
        let repo = git2::Repository::open(project)?;
        if path.exists() {
            std::fs::remove_dir_all(path)?;
        }
        // find_worktree on pruned bookkeeping surfaces as NotFound — sometimes
        // via a baffling "could not find '.git/shallow' to stat" — and either
        // spelling means the same thing: nothing left to prune.
        match repo.find_worktree(&name) {
            Ok(registered) => {
                let mut options = git2::WorktreePruneOptions::new();
                options.valid(true).working_tree(true);
                registered.prune(Some(&mut options))?;
            }
            Err(error) if error.code() == git2::ErrorCode::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    fn discover(
        &self,
        project: &Path,
        _worktrees_root: &Path,
    ) -> Result<Vec<PathBuf>, WorktreeError> {
        let listed = run_git(project, &["worktree", "list", "--porcelain"])?;
        let primary = std::fs::canonicalize(project)?;
        Ok(listed
            .split("\n\n")
            .filter_map(checkout_path_in)
            .filter(|path| *path != primary)
            .collect())
    }

    fn prune(&self, project: &Path) -> Result<(), WorktreeError> {
        run_git(project, &["worktree", "prune"])?;
        Ok(())
    }

    fn holds_record(&self, project: &Path, name: &str) -> Result<bool, WorktreeError> {
        Ok(git2::Repository::open(project)?.find_worktree(name).is_ok())
    }
}

/// The name git's registry knows the checkout at `path` by, refused when the
/// path has no directory to be called by.
fn registered_name(path: &Path) -> Result<String, WorktreeError> {
    checkout_name(path).ok_or_else(|| {
        WorktreeError::Refused(format!(
            "checkout path has no directory name: {}",
            path.display()
        ))
    })
}

/// The canonical path in one `git worktree list --porcelain` block, or `None`
/// for a block that names no live checkout (bare, prunable, or gone from disk).
fn checkout_path_in(block: &str) -> Option<PathBuf> {
    let mut path = None;
    for line in block.trim().lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(rest));
        } else if line == "bare" || line.starts_with("prunable") {
            return None;
        }
    }
    std::fs::canonicalize(path?).ok()
}
