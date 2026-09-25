//! Done with the branch deleted: `branch.finish` with `action: "delete"`.
//!
//! Done removes the workspace; the local branch its checkout carried stays in
//! the source repository unless the caller asked for it to go. When it did,
//! the branch is the one the finish already resolved the workspace by, in each
//! source repository that workspace's directories carry it in — nothing else
//! is looked up by name.
//!
//! Refused before anything is touched, as one plain sentence, when the branch
//! is checked out somewhere other than the checkout Done is about to remove,
//! or holds commits no remote has. Measured again in the drain once the
//! checkout is gone, because the user's own checkout can move onto the branch
//! in between; a refusal there leaves the branch and says why in the answer,
//! since the workspace is already gone by then.

use crate::git_process::run_git;
use crate::workspace::Workspace;
use std::path::PathBuf;

/// The `branch.finish` action that deletes the branch as well.
pub(crate) const DELETE_ACTION: &str = "delete";

/// One local branch Done takes with it.
#[derive(Clone, Debug)]
pub(in crate::app) struct BranchDeletion {
    /// The source repository the branch lives in.
    repo: PathBuf,
    branch: String,
    /// The checkout Done removes. It holding the branch is not a refusal.
    checkout: PathBuf,
}

impl BranchDeletion {
    /// The branch `branch` in every Git directory of `workspace` that carries
    /// it.
    pub(in crate::app) fn of(workspace: &Workspace, branch: &str) -> Vec<Self> {
        workspace
            .directories
            .iter()
            .filter(|directory| directory.is_git && directory.branch.as_deref() == Some(branch))
            .map(|directory| Self {
                repo: directory.source_path.clone(),
                branch: branch.to_string(),
                checkout: directory.path.clone(),
            })
            .collect()
    }

    pub(in crate::app) fn branch(&self) -> &str {
        &self.branch
    }

    /// Why this branch cannot be deleted, as a sentence, or `None`.
    pub(in crate::app) fn refusal(&self) -> Option<String> {
        if !self.exists() {
            return None;
        }
        let reason = match self.checked_out_elsewhere() {
            Some(path) => format!("it is checked out at {}", path.display()),
            None if self.has_unpushed_commits() => "it has commits no remote has".to_string(),
            None => return None,
        };
        Some(format!(
            "Build cannot delete the branch {}: {reason}.",
            self.branch
        ))
    }

    /// Delete the branch, measured again first. A branch already gone is
    /// deleted.
    pub(in crate::app) fn delete(&self) -> Result<(), String> {
        if let Some(refusal) = self.refusal() {
            return Err(refusal);
        }
        if !self.exists() {
            return Ok(());
        }
        // `-D`: whether the branch is safe to lose was measured against every
        // remote above, not against whatever HEAD the source happens to be on.
        run_git(&self.repo, &["branch", "-D", "--", &self.branch])
            .map(|_| ())
            .map_err(|error| {
                format!(
                    "Build could not delete the branch {}: {}",
                    self.branch,
                    error.to_string().trim()
                )
            })
    }

    fn local_ref(&self) -> String {
        format!("refs/heads/{}", self.branch)
    }

    fn exists(&self) -> bool {
        run_git(
            &self.repo,
            &["rev-parse", "--verify", "--quiet", &self.local_ref()],
        )
        .is_ok()
    }

    /// The first checkout of the source repository standing on the branch,
    /// other than the one Done removes.
    fn checked_out_elsewhere(&self) -> Option<PathBuf> {
        let listed = run_git(&self.repo, &["worktree", "list", "--porcelain"]).ok()?;
        let wanted = format!("branch {}", self.local_ref());
        listed
            .split("\n\n")
            .filter(|block| block.lines().any(|line| line == wanted))
            .filter_map(|block| {
                block
                    .lines()
                    .find_map(|line| line.strip_prefix("worktree "))
            })
            .map(PathBuf::from)
            .find(|path| !super::same_path(path, &self.checkout))
    }

    /// Commits on the branch that no remote-tracking ref reaches. A repository
    /// with no remote has nowhere the work went, so every commit counts.
    /// Unreadable counts as unpushed: this is the check that keeps work.
    fn has_unpushed_commits(&self) -> bool {
        run_git(
            &self.repo,
            &[
                "rev-list",
                "--count",
                &self.local_ref(),
                "--not",
                "--remotes",
            ],
        )
        .map(|count| count.trim() != "0")
        .unwrap_or(true)
    }
}

/// The first refusal among `deletions`, or `None` when every one can go.
pub(in crate::app) fn first_refusal(deletions: &[BranchDeletion]) -> Option<String> {
    deletions.iter().find_map(BranchDeletion::refusal)
}

/// Delete every branch, and the first reason one stayed.
pub(in crate::app) fn delete_all(deletions: &[BranchDeletion]) -> Result<(), String> {
    deletions.iter().try_for_each(BranchDeletion::delete)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_fixture::{git_in, init_repo_named};
    use std::path::Path;

    fn deletion(repo: &Path, branch: &str, checkout: &Path) -> BranchDeletion {
        BranchDeletion {
            repo: repo.to_path_buf(),
            branch: branch.to_string(),
            checkout: checkout.to_path_buf(),
        }
    }

    /// `repo` with a bare `origin` that has `main`, and `feature` cut from it.
    fn pushed_feature(parent: &Path) -> PathBuf {
        let repo = init_repo_named(parent, "repo");
        let origin = parent.join("origin.git");
        git_in(
            parent,
            &["init", "--bare", "-b", "main", origin.to_str().unwrap()],
        );
        git_in(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git_in(&repo, &["push", "-q", "origin", "main"]);
        git_in(&repo, &["branch", "feature"]);
        repo
    }

    #[test]
    fn a_pushed_branch_nobody_stands_on_is_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = pushed_feature(tmp.path());
        let feature = deletion(&repo, "feature", &tmp.path().join("gone"));
        assert_eq!(feature.refusal(), None);
        feature.delete().unwrap();
        assert!(!feature.exists());
    }

    #[test]
    fn a_branch_already_gone_counts_as_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = pushed_feature(tmp.path());
        let missing = deletion(&repo, "never-was", &tmp.path().join("gone"));
        assert_eq!(missing.refusal(), None);
        missing.delete().unwrap();
    }

    #[test]
    fn a_branch_with_commits_no_remote_has_is_refused_and_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = pushed_feature(tmp.path());
        git_in(&repo, &["switch", "-q", "feature"]);
        git_in(
            &repo,
            &["commit", "-q", "--allow-empty", "-m", "local only"],
        );
        git_in(&repo, &["switch", "-q", "main"]);
        let feature = deletion(&repo, "feature", &tmp.path().join("gone"));
        let refused = feature.delete().unwrap_err();
        assert_eq!(
            refused,
            "Build cannot delete the branch feature: it has commits no remote has."
        );
        assert!(feature.exists());
    }

    #[test]
    fn a_branch_checked_out_elsewhere_is_refused_and_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = pushed_feature(tmp.path());
        let other = tmp.path().join("other");
        git_in(
            &repo,
            &["worktree", "add", "-q", other.to_str().unwrap(), "feature"],
        );
        let feature = deletion(&repo, "feature", &tmp.path().join("gone"));
        let refused = feature.delete().unwrap_err();
        assert!(
            refused.starts_with("Build cannot delete the branch feature: it is checked out at "),
            "{refused}"
        );
        assert!(refused.ends_with("other."), "{refused}");
        assert!(feature.exists());
    }

    #[test]
    fn the_checkout_done_removes_does_not_count_as_elsewhere() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = pushed_feature(tmp.path());
        let own = tmp.path().join("own");
        git_in(
            &repo,
            &["worktree", "add", "-q", own.to_str().unwrap(), "feature"],
        );
        assert_eq!(deletion(&repo, "feature", &own).refusal(), None);
    }

    #[test]
    fn the_primary_checkout_standing_on_it_is_elsewhere() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = pushed_feature(tmp.path());
        git_in(&repo, &["switch", "-q", "feature"]);
        let refused = deletion(&repo, "feature", &tmp.path().join("gone"))
            .refusal()
            .unwrap();
        assert!(refused.contains("it is checked out at "), "{refused}");
    }
}
