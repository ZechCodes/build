//! `project.update_source`: the git half of editing one project source, run
//! off the app mutex. Everything is read and checked before anything is
//! written, so a refused edit leaves the checkout as it was.

use super::projects::{open_repo, SetRemote};
use crate::lifecycle::{Performed, WorktreeChange, WorktreeMutation};
use crate::worktree::git_origin_url;
use std::path::{Path, PathBuf};

/// One source's edit. `None` leaves a part as it is.
pub struct UpdateSource {
    /// The checkout the source stands on now.
    pub path: PathBuf,
    pub is_git: bool,
    pub base_branch: String,
    /// Another folder for the source to stand on: canonical, and already held
    /// to every other source's folder by the caller.
    pub new_path: Option<PathBuf>,
    pub new_base_branch: Option<String>,
    /// A checked url, or empty to take `origin` off.
    pub new_remote: Option<String>,
    /// Every workspace directory cut from this source. The ones with their
    /// own repository (a Rift or plain copy) keep their own `origin`, and
    /// follow the source's when they still named the one it had.
    pub workspace_checkouts: Vec<WorkspaceCheckout>,
}

/// One workspace directory cut from the source being edited.
pub struct WorkspaceCheckout {
    pub workspace_id: String,
    pub path: PathBuf,
}

/// A workspace checkout that should have followed the source's remote and
/// could not, left on the old one: git's reason, as git said it.
#[derive(Debug)]
pub struct CheckoutFailed {
    pub workspace_id: String,
    pub path: PathBuf,
    pub reason: String,
}

/// What the source stands on after the edit, how many workspace checkouts
/// had their `origin` moved with it, and which could not be.
pub struct SourceUpdated {
    pub path: PathBuf,
    pub is_git: bool,
    pub base_branch: String,
    pub checkouts_updated: usize,
    pub checkouts_failed: Vec<CheckoutFailed>,
}

/// What moving the workspace checkouts' `origin` came to.
#[derive(Default)]
struct Rewired {
    updated: usize,
    failed: Vec<CheckoutFailed>,
}

impl WorktreeMutation for UpdateSource {
    type Output = SourceUpdated;

    fn perform(self) -> Result<Performed<Self::Output>, String> {
        let (path, is_git, base_branch) = self.reopened()?;
        let rewired = match &self.new_remote {
            Some(url) => rewire_origin(&path, is_git, url, &self.workspace_checkouts)?,
            None => Rewired::default(),
        };
        Ok(Performed {
            change: WorktreeChange::nothing(),
            output: SourceUpdated {
                path,
                is_git,
                base_branch,
                checkouts_updated: rewired.updated,
                checkouts_failed: rewired.failed,
            },
        })
    }
}

impl UpdateSource {
    /// Where the source stands and on which base, proved before anything is
    /// written: a new folder is opened the way `project.add_source` opens one,
    /// and a new base must be a branch the checkout has.
    fn reopened(&self) -> Result<(PathBuf, bool, String), String> {
        if let Some(base) = &self.new_base_branch {
            refuse_unusable_branch_name(base)?;
        }
        if self.new_path.is_none() && self.new_base_branch.is_none() {
            return Ok((self.path.clone(), self.is_git, self.base_branch.clone()));
        }
        let path = self.new_path.clone().unwrap_or_else(|| self.path.clone());
        let opened = open_repo(path, self.new_base_branch.clone())?;
        if let Some(base) = &self.new_base_branch {
            refuse_missing_branch(&opened.path, opened.is_git, base)?;
        }
        Ok((opened.path, opened.is_git, opened.base))
    }
}

pub(crate) fn refuse_unusable_branch_name(base: &str) -> Result<(), String> {
    if base.starts_with('-') || !git2::Branch::name_is_valid(base).unwrap_or(false) {
        return Err(format!("{base} is not a branch name Git accepts."));
    }
    Ok(())
}

/// A base branch is one the checkout has, locally or as `origin`'s: a commit
/// or an expression would resolve too, and is not a branch to cut from.
fn refuse_missing_branch(path: &Path, is_git: bool, base: &str) -> Result<(), String> {
    if !is_git {
        return Err("A folder that is not a Git repository has no base branch.".to_string());
    }
    let repository = git2::Repository::open(path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let found = [
        format!("refs/heads/{base}"),
        format!("refs/remotes/origin/{base}"),
    ]
    .iter()
    .any(|name| repository.find_reference(name).is_ok());
    if !found {
        return Err(format!("{} has no branch named {base}.", path.display()));
    }
    Ok(())
}

/// Point the source's checkout at `url`, then every workspace checkout of its
/// own that still named the source's old `origin`. A checkout whose `origin`
/// was changed by hand is left alone. A checkout git cannot rewrite is
/// reported, not rolled back: the source's change stands, and so do the
/// checkouts that moved.
fn rewire_origin(
    path: &Path,
    is_git: bool,
    url: &str,
    workspace_checkouts: &[WorkspaceCheckout],
) -> Result<Rewired, String> {
    if !is_git {
        return Err("A folder that is not a Git repository has no remote.".to_string());
    }
    let previous = git_origin_url(path);
    set_origin(path, url)?;
    let mut rewired = Rewired::default();
    if git_origin_url(path) == previous {
        return Ok(rewired);
    }
    let following = workspace_checkouts.iter().filter(|checkout| {
        holds_its_own_repository(&checkout.path) && git_origin_url(&checkout.path) == previous
    });
    for checkout in following {
        match set_origin(&checkout.path, url) {
            Ok(()) => rewired.updated += 1,
            Err(reason) => rewired.failed.push(CheckoutFailed {
                workspace_id: checkout.workspace_id.clone(),
                path: checkout.path.clone(),
                reason,
            }),
        }
    }
    Ok(rewired)
}

/// Write `url` (or take `origin` off when it is empty), then read it back:
/// taking a remote off does not report git's refusal, so what `origin` names
/// afterwards is the answer.
fn set_origin(path: &Path, url: &str) -> Result<(), String> {
    SetRemote {
        repo_path: path.to_path_buf(),
        url: url.to_string(),
    }
    .perform()?;
    let wanted = (!url.is_empty()).then_some(url);
    if git_origin_url(path).as_deref() != wanted {
        return Err(format!(
            "Git did not change the remote in {}.",
            path.display()
        ));
    }
    Ok(())
}

/// A git worktree's `.git` is a file pointing into the source's repository,
/// whose config (and `origin`) it shares; a copy's is a directory of its own.
/// A `.git` that is a symlink is not followed: it could lead into the source's
/// repository or anywhere else, and nothing of its own is written there.
fn holds_its_own_repository(checkout: &Path) -> bool {
    std::fs::symlink_metadata(checkout.join(".git")).is_ok_and(|meta| meta.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_fixture::{git_in, init_repo_named};

    fn update(path: &Path) -> UpdateSource {
        UpdateSource {
            path: path.to_path_buf(),
            is_git: true,
            base_branch: "main".to_string(),
            new_path: None,
            new_base_branch: None,
            new_remote: None,
            workspace_checkouts: Vec::new(),
        }
    }

    fn copy_of(source: &Path, into: &Path) -> PathBuf {
        git_in(
            into.parent().unwrap(),
            &[
                "clone",
                "-q",
                source.to_str().unwrap(),
                into.to_str().unwrap(),
            ],
        );
        crate::git_fixture::configure_repo(into);
        into.to_path_buf()
    }

    #[test]
    fn a_remote_moves_the_checkout_and_the_copies_that_still_named_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let source = init_repo_named(dir.path(), "code");
        git_in(
            &source,
            &["remote", "add", "origin", "git@example.com:old/code.git"],
        );
        let following = copy_of(&source, &dir.path().join("rift-a"));
        git_in(
            &following,
            &[
                "remote",
                "set-url",
                "origin",
                "git@example.com:old/code.git",
            ],
        );
        let by_hand = copy_of(&source, &dir.path().join("rift-b"));
        git_in(
            &by_hand,
            &[
                "remote",
                "set-url",
                "origin",
                "git@example.com:fork/code.git",
            ],
        );
        let worktree = dir.path().join("worktree");
        git_in(
            &source,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "w",
                worktree.to_str().unwrap(),
            ],
        );

        let updated = UpdateSource {
            new_remote: Some("git@example.com:new/code.git".to_string()),
            workspace_checkouts: checkouts(&[&following, &by_hand, &worktree]),
            ..update(&source)
        }
        .perform()
        .unwrap()
        .output;

        assert_eq!(updated.checkouts_updated, 1);
        assert!(updated.checkouts_failed.is_empty());
        let new = Some("git@example.com:new/code.git".to_string());
        assert_eq!(git_origin_url(&source), new);
        assert_eq!(git_origin_url(&following), new);
        assert_eq!(
            git_origin_url(&by_hand).as_deref(),
            Some("git@example.com:fork/code.git")
        );
        assert_eq!(
            git_origin_url(&worktree),
            new,
            "a worktree shares the source's config"
        );
    }

    fn checkouts(paths: &[&PathBuf]) -> Vec<WorkspaceCheckout> {
        paths
            .iter()
            .enumerate()
            .map(|(index, path)| WorkspaceCheckout {
                workspace_id: format!("ws-{index}"),
                path: path.to_path_buf(),
            })
            .collect()
    }

    #[test]
    fn a_checkout_git_cannot_rewrite_is_reported_and_the_rest_still_move() {
        let dir = tempfile::tempdir().unwrap();
        let source = init_repo_named(dir.path(), "code");
        git_in(
            &source,
            &["remote", "add", "origin", "git@example.com:old/code.git"],
        );
        let stuck = copy_of(&source, &dir.path().join("rift-a"));
        let moving = copy_of(&source, &dir.path().join("rift-b"));
        for checkout in [&stuck, &moving] {
            git_in(
                checkout,
                &[
                    "remote",
                    "set-url",
                    "origin",
                    "git@example.com:old/code.git",
                ],
            );
        }
        // Another git holds the config's lock, so set-url cannot write it.
        std::fs::write(stuck.join(".git/config.lock"), "").unwrap();

        let updated = UpdateSource {
            new_remote: Some("git@example.com:new/code.git".to_string()),
            workspace_checkouts: checkouts(&[&stuck, &moving]),
            ..update(&source)
        }
        .perform()
        .unwrap()
        .output;

        assert_eq!(updated.checkouts_updated, 1);
        assert_eq!(updated.checkouts_failed.len(), 1);
        let failed = &updated.checkouts_failed[0];
        assert_eq!(failed.workspace_id, "ws-0");
        assert_eq!(failed.path, stuck);
        assert!(!failed.reason.is_empty());
        assert_eq!(
            git_origin_url(&stuck).as_deref(),
            Some("git@example.com:old/code.git")
        );
        assert_eq!(
            git_origin_url(&moving).as_deref(),
            Some("git@example.com:new/code.git")
        );
        assert_eq!(
            git_origin_url(&source).as_deref(),
            Some("git@example.com:new/code.git"),
            "a checkout that failed does not undo the source's own change"
        );
    }

    #[test]
    fn a_checkout_that_keeps_its_remote_when_it_is_taken_off_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let source = init_repo_named(dir.path(), "code");
        let url = "git@example.com:old/code.git";
        git_in(&source, &["remote", "add", "origin", url]);
        let stuck = copy_of(&source, &dir.path().join("rift-a"));
        git_in(&stuck, &["remote", "set-url", "origin", url]);
        std::fs::write(stuck.join(".git/config.lock"), "").unwrap();

        let updated = UpdateSource {
            new_remote: Some(String::new()),
            workspace_checkouts: checkouts(&[&stuck]),
            ..update(&source)
        }
        .perform()
        .unwrap()
        .output;

        assert_eq!(git_origin_url(&source), None);
        assert_eq!(updated.checkouts_updated, 0);
        assert_eq!(updated.checkouts_failed.len(), 1);
        assert_eq!(git_origin_url(&stuck).as_deref(), Some(url));
    }

    #[test]
    fn a_checkout_whose_git_directory_is_a_symlink_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let source = init_repo_named(dir.path(), "code");
        git_in(
            &source,
            &["remote", "add", "origin", "git@example.com:old/code.git"],
        );
        let elsewhere = copy_of(&source, &dir.path().join("elsewhere"));
        git_in(
            &elsewhere,
            &[
                "remote",
                "set-url",
                "origin",
                "git@example.com:old/code.git",
            ],
        );
        let linked = dir.path().join("linked");
        std::fs::create_dir(&linked).unwrap();
        std::os::unix::fs::symlink(elsewhere.join(".git"), linked.join(".git")).unwrap();

        let updated = UpdateSource {
            new_remote: Some("git@example.com:new/code.git".to_string()),
            workspace_checkouts: checkouts(&[&linked]),
            ..update(&source)
        }
        .perform()
        .unwrap()
        .output;

        assert_eq!(updated.checkouts_updated, 0);
        assert_eq!(
            git_origin_url(&elsewhere).as_deref(),
            Some("git@example.com:old/code.git")
        );
    }

    #[test]
    fn a_remote_is_added_where_there_was_none_and_taken_off_with_an_empty_url() {
        let dir = tempfile::tempdir().unwrap();
        let source = init_repo_named(dir.path(), "code");

        UpdateSource {
            new_remote: Some("https://example.com/code.git".to_string()),
            ..update(&source)
        }
        .perform()
        .unwrap();
        assert_eq!(
            git_origin_url(&source).as_deref(),
            Some("https://example.com/code.git")
        );

        UpdateSource {
            new_remote: Some(String::new()),
            ..update(&source)
        }
        .perform()
        .unwrap();
        assert_eq!(git_origin_url(&source), None);
    }

    #[test]
    fn a_base_branch_must_be_a_branch_the_checkout_has() {
        let dir = tempfile::tempdir().unwrap();
        let source = init_repo_named(dir.path(), "code");
        git_in(&source, &["branch", "develop"]);

        let moved = UpdateSource {
            new_base_branch: Some("develop".to_string()),
            ..update(&source)
        }
        .perform()
        .unwrap()
        .output;
        assert_eq!(moved.base_branch, "develop");

        for refused in ["nope", "HEAD~0", "-develop", "a..b"] {
            let answer = UpdateSource {
                new_base_branch: Some(refused.to_string()),
                ..update(&source)
            }
            .perform();
            assert!(answer.is_err(), "{refused}");
        }
    }

    #[test]
    fn a_refused_base_leaves_the_remote_unwritten() {
        let dir = tempfile::tempdir().unwrap();
        let source = init_repo_named(dir.path(), "code");

        let answer = UpdateSource {
            new_base_branch: Some("nope".to_string()),
            new_remote: Some("https://example.com/code.git".to_string()),
            ..update(&source)
        }
        .perform();

        assert!(answer.is_err());
        assert_eq!(git_origin_url(&source), None);
    }

    #[test]
    fn a_new_folder_is_opened_and_its_base_read() {
        let dir = tempfile::tempdir().unwrap();
        let source = init_repo_named(dir.path(), "code");
        let elsewhere = init_repo_named(dir.path(), "elsewhere");
        let plain = dir.path().join("plain");
        std::fs::create_dir(&plain).unwrap();

        let moved = UpdateSource {
            new_path: Some(elsewhere.clone()),
            ..update(&source)
        }
        .perform()
        .unwrap()
        .output;
        assert_eq!(moved.path, std::fs::canonicalize(&elsewhere).unwrap());
        assert!(moved.is_git);
        assert_eq!(moved.base_branch, "main");

        let folder = UpdateSource {
            new_path: Some(plain),
            ..update(&source)
        }
        .perform()
        .unwrap()
        .output;
        assert!(!folder.is_git);
    }
}
