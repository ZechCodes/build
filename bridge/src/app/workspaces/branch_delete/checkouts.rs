//! Whether a checkout holds a branch: what `git branch -D` refuses to delete
//! under, measured here because Done deletes with `update-ref`, which refuses
//! nothing of the kind.
//!
//! A checkout holds a branch when its `HEAD` names it, or when it is in the
//! middle of rebasing or bisecting it — both detach `HEAD` and come back to
//! the branch by name, so deleting it under them strands the work.

use crate::git_process::run_git;
use std::path::{Path, PathBuf};

/// How a checkout holds the branch.
enum Hold {
    CheckedOut,
    Rebasing,
    Bisecting,
}

/// One checkout of the repository, as `git worktree list` names it.
struct Checkout {
    path: PathBuf,
    /// The ref its `HEAD` names, `None` when detached.
    head: Option<String>,
}

/// Why `branch` in `repo` is held by a checkout other than `except`, as the
/// clause after "Build cannot delete the branch X: ". A repository whose
/// checkouts cannot be read is held: this is the check that keeps work.
pub(super) fn held_elsewhere(repo: &Path, branch: &str, except: Option<&Path>) -> Option<String> {
    let Some(checkouts) = checkouts(repo) else {
        return Some("Build could not read where it is checked out".to_string());
    };
    let local_ref = format!("refs/heads/{branch}");
    checkouts
        .iter()
        .filter(|checkout| {
            except.is_none_or(|except| !super::super::same_path(&checkout.path, except))
        })
        .find_map(|checkout| {
            hold_of(checkout, branch, &local_ref).map(|hold| sentence(&hold, &checkout.path))
        })
}

fn sentence(hold: &Hold, path: &Path) -> String {
    let doing = match hold {
        Hold::CheckedOut => "checked out",
        Hold::Rebasing => "being rebased",
        Hold::Bisecting => "being bisected",
    };
    format!("it is {doing} at {}", path.display())
}

/// Every checkout of `repo`, the primary one first, or `None` when Git cannot
/// say. The primary checkout's `HEAD` is read on its own as well as from the
/// list, so a list that leaves it out still counts it.
fn checkouts(repo: &Path) -> Option<Vec<Checkout>> {
    let listed = run_git(repo, &["worktree", "list", "--porcelain"]).ok()?;
    let primary = Checkout {
        path: repo.to_path_buf(),
        head: run_git(repo, &["symbolic-ref", "--quiet", "HEAD"])
            .ok()
            .map(|head| head.trim().to_string()),
    };
    let listed = listed.split("\n\n").filter_map(|block| {
        let path = block
            .lines()
            .find_map(|line| line.strip_prefix("worktree "))?;
        let head = block
            .lines()
            .find_map(|line| line.strip_prefix("branch "))
            .map(str::to_string);
        Some(Checkout {
            path: PathBuf::from(path),
            head,
        })
    });
    Some(std::iter::once(primary).chain(listed).collect())
}

fn hold_of(checkout: &Checkout, branch: &str, local_ref: &str) -> Option<Hold> {
    if checkout.head.as_deref() == Some(local_ref) {
        return Some(Hold::CheckedOut);
    }
    in_progress(&checkout.path, branch, local_ref)
}

/// A rebase or bisect under way in `path` that started from the branch. Git
/// writes the ref a rebase returns to in `rebase-merge/head-name` (or
/// `rebase-apply/head-name`), the refs a `rebase --update-refs` rewrites in
/// `rebase-merge/update-refs`, and the branch a bisect returns to in
/// `BISECT_START`. A checkout that is gone has nothing under way.
fn in_progress(path: &Path, branch: &str, local_ref: &str) -> Option<Hold> {
    let states = run_git(
        path,
        &[
            "rev-parse",
            "--git-path",
            "rebase-merge/head-name",
            "--git-path",
            "rebase-apply/head-name",
            "--git-path",
            "rebase-merge/update-refs",
            "--git-path",
            "BISECT_START",
        ],
    )
    .ok()?;
    let read = |state: &str| {
        std::fs::read_to_string(path.join(state))
            .map(|named| named.trim().to_string())
            .ok()
    };
    let mut states = states.lines();
    let (merge, apply, updates, bisect) = (
        states.next()?,
        states.next()?,
        states.next()?,
        states.next()?,
    );
    let rebasing = [merge, apply]
        .iter()
        .any(|state| read(state).as_deref() == Some(local_ref))
        || read(updates).is_some_and(|refs| refs.lines().any(|line| line == local_ref));
    if rebasing {
        return Some(Hold::Rebasing);
    }
    (read(bisect).as_deref() == Some(branch)).then_some(Hold::Bisecting)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_fixture::{git_in, init_repo_named};

    /// `repo` with `feature`, and a second checkout `other` on it.
    fn feature_held_by_other(parent: &Path) -> (PathBuf, PathBuf) {
        let repo = init_repo_named(parent, "repo");
        git_in(&repo, &["branch", "feature"]);
        let other = parent.join("other");
        git_in(
            &repo,
            &["worktree", "add", "-q", other.to_str().unwrap(), "feature"],
        );
        (repo, other)
    }

    /// Write one of Git's own state files into `checkout`'s Git directory.
    fn write_state(checkout: &Path, state: &str, content: &str) {
        let path = run_git(checkout, &["rev-parse", "--git-path", state]).unwrap();
        let path = checkout.join(path.trim());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn a_checkout_on_the_branch_holds_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, other) = feature_held_by_other(tmp.path());
        let held = held_elsewhere(&repo, "feature", None).unwrap();
        assert!(held.starts_with("it is checked out at "), "{held}");
        assert!(held.ends_with("other"), "{held}");
        assert_eq!(held_elsewhere(&repo, "feature", Some(&other)), None);
    }

    #[test]
    fn the_primary_checkout_on_the_branch_holds_it() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo_named(tmp.path(), "repo");
        git_in(&repo, &["switch", "-q", "-c", "feature"]);
        let held = held_elsewhere(&repo, "feature", None).unwrap();
        assert!(held.starts_with("it is checked out at "), "{held}");
    }

    #[test]
    fn a_checkout_rebasing_the_branch_holds_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, other) = feature_held_by_other(tmp.path());
        git_in(&other, &["switch", "-q", "--detach"]);
        assert_eq!(held_elsewhere(&repo, "feature", None), None);
        write_state(&other, "rebase-merge/head-name", "refs/heads/feature\n");
        let held = held_elsewhere(&repo, "feature", None).unwrap();
        assert!(held.starts_with("it is being rebased at "), "{held}");
    }

    #[test]
    fn a_rebase_updating_the_branch_holds_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, other) = feature_held_by_other(tmp.path());
        git_in(&other, &["switch", "-q", "--detach"]);
        write_state(
            &other,
            "rebase-merge/update-refs",
            "refs/heads/feature\n0000000\n0000000\n",
        );
        let held = held_elsewhere(&repo, "feature", None).unwrap();
        assert!(held.starts_with("it is being rebased at "), "{held}");
    }

    #[test]
    fn a_checkout_bisecting_from_the_branch_holds_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, other) = feature_held_by_other(tmp.path());
        git_in(&other, &["switch", "-q", "--detach"]);
        write_state(&other, "BISECT_START", "feature\n");
        let held = held_elsewhere(&repo, "feature", None).unwrap();
        assert!(held.starts_with("it is being bisected at "), "{held}");
    }

    #[test]
    fn a_repository_whose_checkouts_cannot_be_read_holds_it() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            held_elsewhere(&tmp.path().join("no-repo"), "feature", None).as_deref(),
            Some("Build could not read where it is checked out")
        );
    }
}
