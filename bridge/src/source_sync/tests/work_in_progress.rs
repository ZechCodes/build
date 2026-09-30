//! Work a sync must never touch: an ignored file upstream starts tracking,
//! and an operation in progress in any worktree of the repository (#268).

use super::*;

/// The base checkout and its remote, both with a second commit on `main`, so
/// an operation can start from `HEAD~1`.
fn pair_with_history() -> Pair {
    let pair = pair();
    commit(&pair.upstream, "second.txt", "second\n");
    git_in(&pair.base, &["pull", "-q", "--ff-only"]);
    pair
}

/// A rebase of whatever `repo` stands on that stops part-way, the way an
/// `edit` or a conflict leaves it: HEAD detached, `rebase-merge` in place.
fn stop_a_rebase_in(repo: &Path) {
    let rebase = git_command(repo, &["rebase", "-q", "--exec", "false", "HEAD~1"])
        .output()
        .unwrap();
    assert!(!rebase.status.success(), "the rebase was meant to stop");
    let git_dir = git_command(repo, &["rev-parse", "--absolute-git-dir"])
        .output()
        .unwrap();
    let git_dir = PathBuf::from(String::from_utf8_lossy(&git_dir.stdout).trim());
    assert!(
        git_dir.join("rebase-merge").exists(),
        "no rebase in progress"
    );
}

#[test]
fn an_ignored_file_upstream_starts_tracking_is_kept_byte_for_byte() {
    let pair = pair();
    commit(&pair.upstream, ".gitignore", "secret.env\n");
    git_in(&pair.base, &["pull", "-q", "--ff-only"]);
    std::fs::write(pair.base.join("secret.env"), "MY_LOCAL_KEY=precious\n").unwrap();
    std::fs::write(pair.upstream.join("secret.env"), "TRACKED=1\n").unwrap();
    git_in(&pair.upstream, &["add", "-f", "secret.env"]);
    git_in(&pair.upstream, &["commit", "-q", "-m", "track secret.env"]);
    let before = rev(&pair.base, "main");

    let report = sync_base(&pair.base, "main", NOW);

    assert!(skipped(&report).contains("secret.env"), "{report:?}");
    assert_eq!(rev(&pair.base, "main"), before);
    assert_eq!(
        std::fs::read(pair.base.join("secret.env")).unwrap(),
        b"MY_LOCAL_KEY=precious\n"
    );
}

/// The reviewer's repro: mid-rebase, HEAD is detached, so the base reads as
/// checked out nowhere. Moving the ref then would make `rebase --continue`
/// fail and strand the rebased commits.
#[test]
fn a_rebase_of_the_base_in_the_source_checkout_leaves_the_ref_alone() {
    let pair = pair_with_history();
    commit(&pair.upstream, "theirs.txt", "theirs\n");
    stop_a_rebase_in(&pair.base);
    let before = rev(&pair.base, "main");

    let report = sync_base(&pair.base, "main", NOW);

    assert!(skipped(&report).contains("rebase"), "{report:?}");
    assert_eq!(rev(&pair.base, "main"), before);
    assert_eq!(report.behind, 1);
}

#[test]
fn a_rebase_in_another_worktree_of_the_repository_leaves_the_ref_alone() {
    let pair = pair_with_history();
    commit(&pair.upstream, "theirs.txt", "theirs\n");
    git_in(&pair.base, &["switch", "-q", "-c", "feature"]);
    let elsewhere = pair.base.parent().unwrap().join("elsewhere");
    git_in(
        &pair.base,
        &["worktree", "add", "-q", elsewhere.to_str().unwrap(), "main"],
    );
    stop_a_rebase_in(&elsewhere);
    let before = rev(&pair.base, "main");

    let report = sync_base(&pair.base, "main", NOW);

    let reason = skipped(&report);
    assert!(reason.contains("rebase"), "{report:?}");
    assert!(reason.contains("elsewhere"), "{report:?}");
    assert_eq!(rev(&pair.base, "main"), before);
}

#[test]
fn a_bisect_in_the_source_checkout_leaves_the_ref_alone() {
    let pair = pair_with_history();
    commit(&pair.upstream, "theirs.txt", "theirs\n");
    git_in(&pair.base, &["switch", "-q", "-c", "feature"]);
    git_in(&pair.base, &["bisect", "start"]);
    let before = rev(&pair.base, "main");

    let report = sync_base(&pair.base, "main", NOW);

    assert!(skipped(&report).contains("bisect"), "{report:?}");
    assert_eq!(rev(&pair.base, "main"), before);
}

#[test]
fn a_cherry_pick_stopped_in_another_worktree_leaves_the_ref_alone() {
    let pair = pair_with_history();
    commit(&pair.upstream, "theirs.txt", "theirs\n");
    git_in(&pair.base, &["switch", "-q", "-c", "feature"]);
    commit(&pair.base, "README.md", "feature's readme\n");
    let elsewhere = pair.base.parent().unwrap().join("elsewhere");
    git_in(
        &pair.base,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            elsewhere.to_str().unwrap(),
            "main",
        ],
    );
    std::fs::write(elsewhere.join("README.md"), "a clash\n").unwrap();
    git_in(&elsewhere, &["commit", "-q", "-am", "clash"]);
    let picked = git_command(&elsewhere, &["cherry-pick", "feature"])
        .output()
        .unwrap();
    assert!(
        !picked.status.success(),
        "the cherry-pick was meant to stop"
    );
    let before = rev(&pair.base, "main");

    let report = sync_base(&pair.base, "main", NOW);

    assert!(skipped(&report).contains("cherry-pick"), "{report:?}");
    assert_eq!(rev(&pair.base, "main"), before);
}
