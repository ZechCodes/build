//! Done with the branch deleted: `branch.finish` with `action: "delete"`.
//!
//! Done removes the workspace; the local branch its checkout carried stays in
//! the source repository unless the caller asked for it to go. When it did,
//! the branch is the one the finish already resolved the workspace by, in each
//! source repository that workspace's directories carry it in — nothing else
//! is looked up by name.
//!
//! Refused before anything is touched, as one plain sentence, when the branch
//! is a default branch (`main`, `master`, the one a remote's `HEAD` names, or
//! the one the project or the directory was cut from), is checked out, rebased
//! or bisected somewhere other than the checkout Done is about to remove, or
//! holds commits no remote has. Measured again in the drain once the
//! checkout is gone, because the user's own checkout can move onto the branch
//! in between; a refusal there leaves the branch and says why in the answer,
//! since the workspace is already gone by then.
//!
//! The measurement is of one commit, not of a name: the delete names the tip
//! the checks passed and Git refuses it if the branch has moved since, so a
//! commit landing between the check and the delete is never deleted unseen.
//! `update-ref` does not refuse a branch a checkout holds, as `branch -D`
//! does, so the checkouts are read again immediately before it and once more
//! after it; a checkout that moved onto the branch in between gets the branch
//! back at the same commit.

mod checkouts;

use crate::git_process::run_git;
use crate::workspace::Workspace;
use std::path::PathBuf;

/// The `branch.finish` action that deletes the branch as well.
pub(crate) const DELETE_ACTION: &str = "delete";

/// Branch names Done never deletes, whatever the repository calls its default.
const PROTECTED_NAMES: [&str; 2] = ["main", "master"];

/// When the branch is measured: before the checkout Done removes has gone,
/// or after.
#[derive(Clone, Copy)]
enum Moment {
    /// Done's own checkout still stands on the branch and does not count.
    BeforeRemoval,
    /// Done's checkout is gone, so anything still on the branch holds it.
    AfterRemoval,
}

/// One local branch Done takes with it.
#[derive(Clone, Debug)]
pub(in crate::app) struct BranchDeletion {
    /// The source repository the branch lives in.
    repo: PathBuf,
    branch: String,
    /// The checkout Done removes. It holding the branch is not a refusal.
    checkout: PathBuf,
    /// The branches configured as this repository's default: the project's
    /// and the directory's base. Remote defaults are read from the repository.
    defaults: Vec<String>,
}

impl BranchDeletion {
    /// The branch `branch` in every Git directory of `workspace` that carries
    /// it. `defaults` are the project's configured default branches.
    pub(in crate::app) fn of(
        workspace: &Workspace,
        branch: &str,
        defaults: &[String],
    ) -> Vec<Self> {
        workspace
            .directories
            .iter()
            .filter(|directory| directory.is_git && directory.branch.as_deref() == Some(branch))
            .map(|directory| Self {
                repo: directory.source_path.clone(),
                branch: branch.to_string(),
                checkout: directory.path.clone(),
                defaults: defaults
                    .iter()
                    .chain(std::iter::once(&directory.base_branch))
                    .filter(|name| !name.is_empty())
                    .cloned()
                    .collect(),
            })
            .collect()
    }

    pub(in crate::app) fn branch(&self) -> &str {
        &self.branch
    }

    /// Why this branch cannot be deleted, as a sentence, or `None`.
    pub(in crate::app) fn refusal(&self) -> Option<String> {
        self.measure(Moment::BeforeRemoval).err()
    }

    /// The tip every check passed, `None` for a branch already gone, or why
    /// the branch stays.
    fn measure(&self, moment: Moment) -> Result<Option<String>, String> {
        let refuse = |reason: &str| {
            Err(format!(
                "Build cannot delete the branch {}: {reason}.",
                self.branch
            ))
        };
        if self.is_default() {
            return refuse("it is a default branch");
        }
        let Some(tip) = self.tip() else {
            return Ok(None);
        };
        if self.has_unpushed_commits(&tip) {
            return refuse("it has commits no remote has");
        }
        self.unheld(moment)?;
        Ok(Some(tip))
    }

    /// Delete the branch once Done's checkout is gone, measured again first.
    /// A branch already gone is deleted.
    pub(in crate::app) fn delete(&self) -> Result<(), String> {
        match self.measure(Moment::AfterRemoval)? {
            Some(tip) => self.delete_at(&tip),
            None => Ok(()),
        }
    }

    /// Delete the branch only while it still points at `tip`, the commit the
    /// checks passed. Whether the branch is safe to lose was measured against
    /// every remote, not against whatever HEAD the source is on, so this is
    /// `branch -D`'s force with the commit named instead of the branch.
    fn delete_at(&self, tip: &str) -> Result<(), String> {
        self.unheld(Moment::AfterRemoval)?;
        if let Err(error) = run_git(&self.repo, &["update-ref", "-d", &self.local_ref(), tip]) {
            if self.tip().is_some_and(|now| now != tip) {
                return Err(format!(
                    "Build cannot delete the branch {}: it gained commits while Build was deleting it.",
                    self.branch
                ));
            }
            return Err(format!(
                "Build could not delete the branch {}: {}",
                self.branch,
                error.to_string().trim()
            ));
        }
        self.restore_if_held(tip)?;
        // What `branch -D` also takes: the branch's upstream and settings.
        // None is the usual case, which Git answers as a failure.
        let _ = run_git(
            &self.repo,
            &[
                "config",
                "--remove-section",
                &format!("branch.{}", self.branch),
            ],
        );
        Ok(())
    }

    fn local_ref(&self) -> String {
        format!("refs/heads/{}", self.branch)
    }

    /// The commit the branch points at, or `None` when there is no branch.
    fn tip(&self) -> Option<String> {
        run_git(
            &self.repo,
            &["rev-parse", "--verify", "--quiet", &self.local_ref()],
        )
        .ok()
        .map(|oid| oid.trim().to_string())
        .filter(|oid| !oid.is_empty())
    }

    /// `main`, `master`, a configured default, or the branch a remote's
    /// `HEAD` names.
    fn is_default(&self) -> bool {
        PROTECTED_NAMES.contains(&self.branch.as_str())
            || self.defaults.contains(&self.branch)
            || self.remote_defaults().contains(&self.branch)
    }

    /// The branch each remote's `HEAD` points at, as a local branch name:
    /// `refs/remotes/origin/HEAD -> refs/remotes/origin/trunk` is `trunk`.
    fn remote_defaults(&self) -> Vec<String> {
        let Ok(listed) = run_git(
            &self.repo,
            &[
                "for-each-ref",
                "--format=%(refname) %(symref)",
                "refs/remotes/",
            ],
        ) else {
            return Vec::new();
        };
        listed
            .lines()
            .filter_map(|line| {
                let (name, target) = line.split_once(' ')?;
                let remote = name.strip_suffix("HEAD")?;
                target.strip_prefix(remote).map(str::to_string)
            })
            .collect()
    }

    /// `Ok` while no checkout holds the branch, or why one does. Before the
    /// removal, the checkout Done removes is not counted.
    fn unheld(&self, moment: Moment) -> Result<(), String> {
        let except = match moment {
            Moment::BeforeRemoval => Some(self.checkout.as_path()),
            Moment::AfterRemoval => None,
        };
        match checkouts::held_elsewhere(&self.repo, &self.branch, except) {
            Some(reason) => Err(format!(
                "Build cannot delete the branch {}: {reason}.",
                self.branch
            )),
            None => Ok(()),
        }
    }

    /// A checkout that moved onto the branch between the look before the
    /// delete and the delete itself: the branch comes back at `tip`, the
    /// commit it was deleted at, unless something has made it again since.
    fn restore_if_held(&self, tip: &str) -> Result<(), String> {
        self.unheld(Moment::AfterRemoval).inspect_err(|_| {
            let absent = "0".repeat(tip.len());
            let _ = run_git(&self.repo, &["update-ref", &self.local_ref(), tip, &absent]);
        })
    }

    /// Commits reachable from `tip` that no remote-tracking ref reaches. A
    /// repository with no remote has nowhere the work went, so every commit
    /// counts. Unreadable counts as unpushed: this is the check that keeps
    /// work.
    fn has_unpushed_commits(&self, tip: &str) -> bool {
        run_git(
            &self.repo,
            &["rev-list", "--count", tip, "--not", "--remotes"],
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
            defaults: Vec::new(),
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
        assert_eq!(feature.tip(), None);
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
        assert!(feature.tip().is_some());
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
        assert!(feature.tip().is_some());
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

    // A commit landing between the checks and the delete: the checks passed
    // one tip and the branch now has another, which nothing has examined.
    #[test]
    fn a_branch_that_moved_after_its_checks_is_refused_and_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = pushed_feature(tmp.path());
        let feature = deletion(&repo, "feature", &tmp.path().join("gone"));
        let measured = feature.measure(Moment::AfterRemoval).unwrap().unwrap();
        git_in(&repo, &["switch", "-q", "feature"]);
        git_in(
            &repo,
            &[
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "landed after the check",
            ],
        );
        git_in(&repo, &["switch", "-q", "main"]);
        let moved = feature.tip().unwrap();
        assert_ne!(moved, measured);
        let refused = feature.delete_at(&measured).unwrap_err();
        assert_eq!(
            refused,
            "Build cannot delete the branch feature: it gained commits while Build was deleting it."
        );
        assert_eq!(feature.tip(), Some(moved));
    }

    // A checkout moving onto the branch after the checks, at the same commit:
    // the tip still matches, so only the look immediately before the delete
    // stands between it and a checkout whose HEAD names a deleted branch.
    #[test]
    fn a_branch_checked_out_after_its_checks_is_refused_and_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = pushed_feature(tmp.path());
        let feature = deletion(&repo, "feature", &tmp.path().join("gone"));
        let measured = feature.measure(Moment::AfterRemoval).unwrap().unwrap();
        git_in(&repo, &["switch", "-q", "feature"]);
        assert_eq!(feature.tip().as_deref(), Some(measured.as_str()));
        let refused = feature.delete_at(&measured).unwrap_err();
        assert!(
            refused.starts_with("Build cannot delete the branch feature: it is checked out at "),
            "{refused}"
        );
        assert_eq!(feature.tip(), Some(measured));
    }

    // The checkout moved onto the branch between that look and the delete:
    // the branch is deleted under it, and the look after puts it back.
    #[test]
    fn a_branch_checked_out_during_its_delete_is_put_back() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = pushed_feature(tmp.path());
        let feature = deletion(&repo, "feature", &tmp.path().join("gone"));
        let measured = feature.measure(Moment::AfterRemoval).unwrap().unwrap();
        git_in(&repo, &["switch", "-q", "feature"]);
        git_in(
            &repo,
            &["update-ref", "-d", "refs/heads/feature", &measured],
        );
        assert_eq!(feature.tip(), None);
        let refused = feature.restore_if_held(&measured).unwrap_err();
        assert!(refused.contains("it is checked out at "), "{refused}");
        assert_eq!(feature.tip(), Some(measured));
    }

    #[test]
    fn deleting_takes_the_branch_settings_with_it() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = pushed_feature(tmp.path());
        git_in(
            &repo,
            &["branch", "-q", "--set-upstream-to=origin/main", "feature"],
        );
        deletion(&repo, "feature", &tmp.path().join("gone"))
            .delete()
            .unwrap();
        assert!(run_git(&repo, &["config", "--get", "branch.feature.remote"]).is_err());
    }

    #[test]
    fn main_and_master_are_never_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = pushed_feature(tmp.path());
        git_in(&repo, &["branch", "master"]);
        git_in(&repo, &["switch", "-q", "--detach"]);
        for name in ["main", "master"] {
            let protected = deletion(&repo, name, &tmp.path().join("gone"));
            assert_eq!(
                protected.delete().unwrap_err(),
                format!("Build cannot delete the branch {name}: it is a default branch.")
            );
            assert!(protected.tip().is_some());
        }
    }

    #[test]
    fn the_branch_a_remote_calls_its_default_is_never_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = pushed_feature(tmp.path());
        git_in(&repo, &["branch", "trunk"]);
        git_in(&repo, &["push", "-q", "origin", "trunk"]);
        git_in(
            &repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/trunk",
            ],
        );
        let trunk = deletion(&repo, "trunk", &tmp.path().join("gone"));
        assert_eq!(
            trunk.refusal().as_deref(),
            Some("Build cannot delete the branch trunk: it is a default branch.")
        );
        assert!(trunk.delete().is_err());
        assert!(trunk.tip().is_some());
    }

    #[test]
    fn a_configured_default_is_never_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = pushed_feature(tmp.path());
        git_in(&repo, &["push", "-q", "origin", "feature"]);
        let configured = BranchDeletion {
            defaults: vec!["feature".to_string()],
            ..deletion(&repo, "feature", &tmp.path().join("gone"))
        };
        assert!(configured.delete().is_err());
        assert!(configured.tip().is_some());
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
