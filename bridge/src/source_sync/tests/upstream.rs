//! What the base follows is what git says it follows: `branch.<base>.remote`
//! and `branch.<base>.merge`, not names assumed from the base's own (#268).

use super::*;

#[test]
fn a_base_that_tracks_a_differently_named_branch_follows_that_branch() {
    let pair = pair();
    git_in(&pair.upstream, &["branch", "-q", "trunk"]);
    git_in(&pair.base, &["fetch", "-q", "origin"]);
    git_in(&pair.base, &["branch", "-q", "-u", "origin/trunk", "main"]);
    git_in(&pair.upstream, &["switch", "-q", "trunk"]);
    commit(&pair.upstream, "on-trunk.txt", "trunk\n");
    git_in(&pair.upstream, &["switch", "-q", "main"]);
    commit(&pair.upstream, "on-main.txt", "main\n");
    commit(&pair.upstream, "more-main.txt", "main\n");

    let report = sync_base(&pair.base, "main", NOW);

    assert_eq!(report.outcome, SyncOutcome::FastForwarded { commits: 1 });
    assert_eq!(rev(&pair.base, "main"), rev(&pair.upstream, "trunk"));
    assert!(pair.base.join("on-trunk.txt").exists());
}

#[test]
fn a_base_whose_tracked_branch_is_gone_says_which_branch() {
    let pair = pair();
    git_in(&pair.upstream, &["branch", "-q", "trunk"]);
    git_in(&pair.base, &["fetch", "-q", "origin"]);
    git_in(&pair.base, &["branch", "-q", "-u", "origin/trunk", "main"]);
    git_in(&pair.upstream, &["branch", "-q", "-D", "trunk"]);

    let report = sync_base(&pair.base, "main", NOW);

    assert_eq!(skipped(&report), "origin has no branch trunk.");
}

#[test]
fn a_base_that_tracks_another_remote_follows_that_remote() {
    let pair = pair();
    let fork = pair.base.parent().unwrap().join("fork");
    git_in(
        pair.base.parent().unwrap(),
        &[
            "clone",
            "-q",
            "--bare",
            pair.upstream.to_str().unwrap(),
            "fork",
        ],
    );
    git_in(
        &pair.base,
        &["remote", "add", "upstream", pair.upstream.to_str().unwrap()],
    );
    git_in(
        &pair.base,
        &["remote", "set-url", "origin", fork.to_str().unwrap()],
    );
    git_in(&pair.base, &["fetch", "-q", "upstream"]);
    git_in(&pair.base, &["branch", "-q", "-u", "upstream/main", "main"]);
    commit(&pair.upstream, "theirs.txt", "theirs\n");

    let report = sync_base(&pair.base, "main", NOW);

    assert_eq!(report.outcome, SyncOutcome::FastForwarded { commits: 1 });
    assert_eq!(rev(&pair.base, "main"), rev(&pair.upstream, "main"));
}

/// `remote.pushDefault` says where pushes go, not what the base follows: a
/// base with no upstream set fetches from `origin`, as `git pull` would.
#[test]
fn a_push_default_is_not_what_the_base_follows() {
    let pair = pair();
    git_in(&pair.base, &["branch", "-q", "--unset-upstream", "main"]);
    let fork = pair.base.parent().unwrap().join("fork");
    git_in(
        pair.base.parent().unwrap(),
        &[
            "clone",
            "-q",
            "--bare",
            pair.upstream.to_str().unwrap(),
            "fork",
        ],
    );
    git_in(
        &pair.base,
        &["remote", "add", "fork", fork.to_str().unwrap()],
    );
    git_in(&pair.base, &["config", "remote.pushDefault", "fork"]);
    commit(&pair.upstream, "theirs.txt", "theirs\n");

    let report = sync_base(&pair.base, "main", NOW);

    assert_eq!(report.outcome, SyncOutcome::FastForwarded { commits: 1 });
    assert_eq!(rev(&pair.base, "main"), rev(&pair.upstream, "main"));
}
