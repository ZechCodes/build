//! The git linked-worktree backend: `git worktree add`, and the only place in
//! the bridge that speaks to git's worktree registry.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::{
    checkout_name, local_branch_ref, teardown_in_git_dir, BranchTeardown, Isolation,
    IsolationBackend, WorktreeError,
};
use crate::git_process::{git_failure, run_git, run_git_with_deadline};

/// Materializes a checkout as a git linked worktree of the project repository.
/// The project repo already holds every ref, so publishing and base-syncing are
/// nothing at all.
#[derive(Clone, Debug)]
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

    fn teardown_record(
        &self,
        project: &Path,
        name: &str,
    ) -> Result<Option<BranchTeardown>, WorktreeError> {
        // `Repository::path()` is the caller's own admin directory, which is a
        // linked worktree's when the project root is itself one, so the shared
        // `commondir` is what holds the `worktrees/<name>` entries.
        let repo = git2::Repository::open(project)?;
        let registry_entry = repo.commondir().join("worktrees").join(name);
        match repo.find_worktree(name) {
            Ok(_) => teardown_in_git_dir(&registry_entry).map(Some),
            Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

/// Create a short-lived review checkout on an existing branch. Its ownership
/// record keeps removal from deleting the branch that the user selected.
pub fn materialize_review_target(project: &Path, branch: &str, path: &Path) -> Result<(), String> {
    review_worktree_git(
        project,
        &[
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("--"),
            path.as_os_str(),
            OsStr::new(branch),
        ],
    )?;
    if let Err(error) = super::record_branch_teardown(path, BranchTeardown::KeepsBranch) {
        let cleanup = review_worktree_git(
            project,
            &[
                OsStr::new("worktree"),
                OsStr::new("remove"),
                OsStr::new("--"),
                path.as_os_str(),
            ],
        );
        return Err(match cleanup {
            Ok(()) => error.to_string(),
            Err(cleanup) => format!(
                "{error}; review checkout {} could not be removed: {cleanup}",
                path.display()
            ),
        });
    }
    Ok(())
}

/// Remove only the temporary checkout recorded as Build-owned. Never remove
/// an existing user checkout or the target branch itself.
pub fn remove_review_target(project: &Path, branch: &str, path: &Path) -> Result<(), String> {
    WorktreeBackend
        .verify(project, path, branch)
        .map_err(|error| error.to_string())?;
    if super::branch_teardown(path).map_err(|error| error.to_string())?
        != BranchTeardown::KeepsBranch
    {
        return Err(format!(
            "review checkout {} lacks its keep-branch record",
            path.display()
        ));
    }
    let repo = git2::Repository::open(path).map_err(|error| error.to_string())?;
    if repo.state() != git2::RepositoryState::Clean {
        return Err(format!(
            "review checkout {} has a Git operation in progress",
            path.display()
        ));
    }
    let expected = local_branch_ref(branch);
    if repo
        .head()
        .ok()
        .and_then(|head| head.name().map(str::to_owned))
        != Some(expected)
    {
        return Err(format!(
            "review checkout {} switched branches",
            path.display()
        ));
    }
    if !run_git(
        path,
        &[
            "status",
            "--porcelain",
            "--ignored",
            "--untracked-files=all",
        ],
    )
    .map_err(|error| error.to_string())?
    .is_empty()
    {
        return Err(format!(
            "review checkout {} has local changes",
            path.display()
        ));
    }
    review_worktree_git(
        project,
        &[
            OsStr::new("worktree"),
            OsStr::new("remove"),
            OsStr::new("--"),
            path.as_os_str(),
        ],
    )
}

fn review_worktree_git(project: &Path, args: &[&OsStr]) -> Result<(), String> {
    let output = run_git_with_deadline(project, args).map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(git_failure(args, &output).to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_fixture::{git_in, init_repo};
    use crate::isolation::{record_branch_teardown, BranchTeardown};

    /// A linked worktree's record outlives its working directory, because git
    /// keeps it in the project rather than in the checkout — so the one fact a
    /// vanished checkout cannot answer for itself is still there to read.
    #[test]
    fn a_vanished_worktree_still_answers_from_the_record_git_holds() {
        let (dir, project) = init_repo();
        let checkout = dir.path().join("borrowed");
        git_in(
            &project,
            &[
                "worktree",
                "add",
                checkout.to_str().unwrap(),
                "-b",
                "theirs",
            ],
        );
        record_branch_teardown(&checkout, BranchTeardown::KeepsBranch).unwrap();
        std::fs::remove_dir_all(&checkout).unwrap();

        assert_eq!(
            WorktreeBackend
                .teardown_record(&project, "borrowed")
                .unwrap(),
            Some(BranchTeardown::KeepsBranch),
        );
    }

    /// A registered checkout with no marker is one Build did not create, and
    /// that reading is the record's, not an absence of one.
    #[test]
    fn an_unmarked_registration_is_still_a_record() {
        let (dir, project) = init_repo();
        let checkout = dir.path().join("by-hand");
        git_in(
            &project,
            &["worktree", "add", checkout.to_str().unwrap(), "-b", "side"],
        );
        std::fs::remove_dir_all(&checkout).unwrap();

        assert_eq!(
            WorktreeBackend
                .teardown_record(&project, "by-hand")
                .unwrap(),
            Some(BranchTeardown::DeletesBranch),
        );
    }

    /// A checkout sitting directly under a worktrees root — the only shape
    /// there was before workspaces, and the shape every registry already on a
    /// user's machine was written with — is still registered, verified,
    /// discovered and torn down under its plain directory name. No migration:
    /// the workspace rule reaches only a mount whose parent holds the manifest.
    #[test]
    fn a_legacy_checkout_under_a_worktrees_root_keeps_its_directory_name() {
        let (dir, project) = init_repo();
        git_in(&project, &["branch", "csv-export"]);
        let root = dir.path().join("worktrees");
        std::fs::create_dir_all(&root).unwrap();
        let checkout = root.join("csv-export");

        WorktreeBackend
            .materialize(&project, "csv-export", &checkout)
            .unwrap();

        assert!(
            project.join(".git/worktrees/csv-export").is_dir(),
            "git registered the checkout under its own directory name"
        );
        WorktreeBackend
            .verify(&project, &checkout, "csv-export")
            .unwrap();
        assert!(WorktreeBackend
            .holds_record(&project, "csv-export")
            .unwrap());
        record_branch_teardown(&checkout, BranchTeardown::KeepsBranch).unwrap();
        assert_eq!(
            WorktreeBackend
                .teardown_record(&project, "csv-export")
                .unwrap(),
            Some(BranchTeardown::KeepsBranch),
        );
        assert_eq!(
            WorktreeBackend.discover(&project, &root).unwrap(),
            vec![std::fs::canonicalize(&checkout).unwrap()],
        );

        WorktreeBackend.remove(&project, &checkout).unwrap();

        assert!(!checkout.exists());
        assert!(!WorktreeBackend
            .holds_record(&project, "csv-export")
            .unwrap());
    }

    /// No registration is no record: git holds nothing to read, so the branch
    /// is nobody's to vouch for.
    #[test]
    fn a_name_git_never_registered_holds_no_record() {
        let (_dir, project) = init_repo();

        assert_eq!(
            WorktreeBackend
                .teardown_record(&project, "never-here")
                .unwrap(),
            None,
        );
    }
}
