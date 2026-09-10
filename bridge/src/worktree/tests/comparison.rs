use super::command::commit_file;
use super::manager::manager;
use crate::git_fixture::{git_in, init_repo};
use std::collections::HashSet;

/// A tracked branch compares both directions with its upstream. Movement on
/// the local base is irrelevant until the branch stops tracking upstream.
#[test]
fn a_tracking_branch_compares_both_directions_with_its_upstream() {
    let (dir, repo) = init_repo();
    let remote = dir.path().join("origin.git");
    git_in(&repo, &["init", "--bare", remote.to_str().unwrap()]);
    git_in(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );

    let wt_path = dir.path().join("wt-tracked");
    git_in(
        &repo,
        &[
            "worktree",
            "add",
            wt_path.to_str().unwrap(),
            "-b",
            "tracked",
        ],
    );
    git_in(&wt_path, &["push", "-u", "origin", "tracked"]);
    let other = dir.path().join("other");
    git_in(
        dir.path(),
        &[
            "clone",
            "--branch",
            "tracked",
            remote.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    git_in(&other, &["config", "user.email", "other@build.ing"]);
    git_in(&other, &["config", "user.name", "Other"]);

    // Two local commits past the shared tip, and one remote commit the local
    // branch does not have.
    commit_file(&wt_path, "a");
    commit_file(&wt_path, "b");
    commit_file(&other, "remote");
    git_in(&other, &["push", "origin", "tracked"]);
    git_in(&repo, &["fetch", "origin"]);
    // Main moves twice to prove it is not the selected comparison ref.
    commit_file(&repo, "on-main");
    commit_file(&repo, "on-main-again");

    let found = manager(&dir, &repo)
        .discover("main", &HashSet::new())
        .unwrap();

    let entry = &found[0];
    assert_eq!(entry.upstream.as_deref(), Some("origin/tracked"));
    assert_eq!(entry.comparison_ref.as_deref(), Some("origin/tracked"));
    assert_eq!(entry.ahead, Some(2), "two commits the remote lacks");
    assert_eq!(
        entry.behind,
        Some(1),
        "one remote commit is missing locally"
    );
}
/// A branch that tracks nothing has pushed nothing: every commit it carries
/// past the base is unpushed, and there is no upstream to name.
#[test]
fn an_untracked_branch_has_all_of_its_work_unpushed() {
    let (dir, repo) = init_repo();
    let wt_path = dir.path().join("wt-untracked");
    git_in(
        &repo,
        &["worktree", "add", wt_path.to_str().unwrap(), "-b", "solo"],
    );
    commit_file(&wt_path, "a");
    commit_file(&repo, "on-main");

    let found = manager(&dir, &repo)
        .discover("main", &HashSet::new())
        .unwrap();

    let entry = &found[0];
    assert_eq!(entry.upstream, None);
    assert_eq!(entry.comparison_ref.as_deref(), Some("main"));
    assert_eq!(entry.ahead, Some(1));
    assert_eq!(entry.behind, Some(1));
}
/// Nothing to report is reported as nothing — a level, pushed, clean
/// worktree has no counts rather than a row of zeroes.
#[test]
fn a_level_worktree_is_neither_stale_nor_unpushed() {
    let (dir, repo) = init_repo();
    let wt_path = dir.path().join("wt-level");
    git_in(
        &repo,
        &["worktree", "add", wt_path.to_str().unwrap(), "-b", "level"],
    );

    let found = manager(&dir, &repo)
        .discover("main", &HashSet::new())
        .unwrap();

    let entry = &found[0];
    assert_eq!(entry.comparison_ref.as_deref(), Some("main"));
    assert_eq!(entry.ahead, Some(0));
    assert_eq!(entry.behind, Some(0));
    assert_eq!(entry.uncommitted.insertions, 0);
}
