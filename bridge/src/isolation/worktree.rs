//! The git linked-worktree backend: `git worktree add`, and the only place in
//! the bridge that speaks to git's worktree registry.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::{Isolation, IsolationBackend, WorktreeError};

/// Materializes a checkout as a git linked worktree of the project repository.
/// The project repo already holds every ref, so publishing and base-syncing are
/// nothing at all.
pub struct WorktreeBackend;

impl IsolationBackend for WorktreeBackend {
    fn kind(&self) -> Isolation {
        Isolation::Worktree
    }

    fn materialize(&self, project: &Path, branch: &str, path: &Path) -> Result<(), WorktreeError> {
        let name = directory_name(path)?;
        let repo = git2::Repository::open(project)?;
        let reference = repo.find_reference(&format!("refs/heads/{branch}"))?;
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(&reference));
        repo.worktree(&name, path, Some(&options))?;
        Ok(())
    }

    fn verify(&self, project: &Path, path: &Path, _branch: &str) -> Result<(), WorktreeError> {
        let actual = std::fs::canonicalize(path)?;
        let name = directory_name(&actual)?;
        let primary = git2::Repository::open(project)?;
        let registered = primary.find_worktree(&name).map_err(|_| {
            WorktreeError::Command(format!(
                "existing path is not the registered worktree {name:?}"
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

    fn remove(&self, project: &Path, path: &Path, name: &str) -> Result<(), WorktreeError> {
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
        match repo.find_worktree(name) {
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
        let listed = git(project, &["worktree", "list", "--porcelain"])?;
        let primary = std::fs::canonicalize(project)?;
        Ok(listed
            .split("\n\n")
            .filter_map(checkout_path_in)
            .filter(|path| *path != primary)
            .collect())
    }

    fn prune(&self, project: &Path) -> Result<(), WorktreeError> {
        git(project, &["worktree", "prune"]).map(|_| ())
    }

    fn holds_record(&self, project: &Path, name: &str) -> Result<bool, WorktreeError> {
        Ok(git2::Repository::open(project)?.find_worktree(name).is_ok())
    }
}

/// A checkout's directory name — git's own name for a linked worktree, and the
/// name every isolation calls a checkout by.
fn directory_name(path: &Path) -> Result<String, WorktreeError> {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| {
            WorktreeError::Command(format!(
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

fn git(dir: &Path, args: &[&str]) -> Result<String, WorktreeError> {
    let output = Command::new("git").args(args).current_dir(dir).output()?;
    if !output.status.success() {
        return Err(WorktreeError::Command(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
