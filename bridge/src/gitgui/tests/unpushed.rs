use super::*;

#[test]
fn pushed_branch_with_clean_tree_has_no_all_changes() {
    let dir = tempfile::tempdir().unwrap();
    let clone = clone_of_an_origin_carrying_feature_x(dir.path());
    let payload = unpushed_payload(&clone, None).unwrap();
    assert_eq!(payload["base"]["kind"], "push_target");
    assert_eq!(payload["base"]["label"], "origin/main");
    assert_eq!(payload["stat"]["files_changed"], 0);
}

#[test]
fn unpublished_branch_starts_at_nearest_published_history_and_includes_dirty_work() {
    let dir = tempfile::tempdir().unwrap();
    let clone = clone_of_an_origin_carrying_feature_x(dir.path());
    git_ok(&clone, &["checkout", "-q", "-b", "local-only"]);
    write(&clone, "committed.txt", "committed\n");
    git_ok(&clone, &["add", "."]);
    git_ok(&clone, &["commit", "-q", "-m", "local"]);
    write(&clone, "staged.txt", "staged\n");
    git_ok(&clone, &["add", "staged.txt"]);
    write(&clone, "f.txt", "unstaged\n");
    write(&clone, "dirty.txt", "dirty\n");

    let payload = unpushed_payload(&clone, None).unwrap();
    assert_eq!(payload["base"]["kind"], "published_ancestor");
    let patch = payload["patch"].as_str().unwrap();
    assert!(patch.contains("committed.txt"), "{patch}");
    assert!(patch.contains("staged.txt"), "{patch}");
    assert!(patch.contains("unstaged"), "{patch}");
    assert!(patch.contains("dirty.txt"), "{patch}");
}

#[test]
fn conditional_key_moves_when_the_push_tracking_ref_catches_up() {
    let dir = tempfile::tempdir().unwrap();
    let clone = clone_of_an_origin_carrying_feature_x(dir.path());
    write(&clone, "local.txt", "local\n");
    git_ok(&clone, &["add", "."]);
    git_ok(&clone, &["commit", "-q", "-m", "local"]);

    let first = unpushed_payload(&clone, None).unwrap();
    let first_key = first["diff_key"].as_str().unwrap();
    assert_eq!(
        unpushed_payload(&clone, Some(first_key)).unwrap(),
        json!({ "unchanged": true, "diff_key": first_key })
    );

    let head = git2::Repository::open(&clone)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap();
    git_ok(
        &clone,
        &["update-ref", "refs/remotes/origin/main", &head.to_string()],
    );
    let caught_up = unpushed_payload(&clone, Some(first_key)).unwrap();
    assert_ne!(caught_up["diff_key"], first_key);
    assert_eq!(caught_up["stat"]["files_changed"], 0);
}

#[test]
fn workspace_history_marks_the_unpushed_range_across_pages_and_refreshes_on_push() {
    let dir = tempfile::tempdir().unwrap();
    let clone = clone_of_an_origin_carrying_feature_x(dir.path());
    for number in 1..=3 {
        write(&clone, &format!("local-{number}.txt"), "local\n");
        git_ok(&clone, &["add", "."]);
        git_ok(&clone, &["commit", "-q", "-m", &format!("local {number}")]);
    }

    let first = log_page(&clone, Some(LogHighlight::Unpushed), 2, 0, None).unwrap();
    assert_eq!(first["commits"][0]["unpushed"], true);
    assert_eq!(first["commits"][1]["unpushed"], true);
    assert_eq!(first["more"], true);
    let older = log_page(&clone, Some(LogHighlight::Unpushed), 2, 2, None).unwrap();
    assert_eq!(older["commits"][0]["unpushed"], true);
    assert_eq!(older["commits"][1]["unpushed"], false);
    assert_eq!(older["highlight_key"], first["highlight_key"]);

    let head = git2::Repository::open(&clone)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap();
    git_ok(
        &clone,
        &["update-ref", "refs/remotes/origin/main", &head.to_string()],
    );
    let pushed = log_page(&clone, Some(LogHighlight::Unpushed), 2, 0, None).unwrap();
    assert_eq!(pushed["commits"][0]["unpushed"], false);
    assert_eq!(pushed["commits"][1]["unpushed"], false);
    assert_ne!(pushed["highlight_key"], first["highlight_key"]);
    assert_eq!(pushed["commits"][0]["hash"], first["commits"][0]["hash"]);
}

#[test]
fn workspace_history_uses_published_ancestor_or_root_like_all_changes() {
    let dir = tempfile::tempdir().unwrap();
    let clone = clone_of_an_origin_carrying_feature_x(dir.path());
    git_ok(&clone, &["checkout", "-q", "-b", "local-only"]);
    write(&clone, "local.txt", "local\n");
    git_ok(&clone, &["add", "."]);
    git_ok(&clone, &["commit", "-q", "-m", "local"]);

    let unpublished = log_page(&clone, Some(LogHighlight::Unpushed), 10, 0, None).unwrap();
    assert_eq!(unpublished["commits"][0]["unpushed"], true);
    assert_eq!(unpublished["commits"][1]["unpushed"], false);

    let local_dir = tempfile::tempdir().unwrap();
    init_repo(local_dir.path());
    let never_pushed =
        log_page(local_dir.path(), Some(LogHighlight::Unpushed), 10, 0, None).unwrap();
    assert_eq!(never_pushed["commits"][0]["unpushed"], true);
}

#[test]
fn diverged_push_target_uses_merge_base_and_does_not_render_remote_only_work_as_a_revert() {
    let dir = tempfile::tempdir().unwrap();
    let clone = clone_of_an_origin_carrying_feature_x(dir.path());
    git_ok(&clone, &["checkout", "-q", "-b", "topic"]);
    write(&clone, "local.txt", "local\n");
    git_ok(&clone, &["add", "."]);
    git_ok(&clone, &["commit", "-q", "-m", "local"]);
    git_ok(&clone, &["checkout", "-q", "-b", "remote-side", "main"]);
    write(&clone, "remote-only.txt", "remote\n");
    git_ok(&clone, &["add", "."]);
    git_ok(&clone, &["commit", "-q", "-m", "remote"]);
    let remote_tip = git2::Repository::open(&clone)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap();
    git_ok(
        &clone,
        &[
            "update-ref",
            "refs/remotes/origin/topic",
            &remote_tip.to_string(),
        ],
    );
    git_ok(&clone, &["checkout", "-q", "topic"]);

    let payload = unpushed_payload(&clone, None).unwrap();
    assert_eq!(payload["base"]["label"], "origin/topic");
    let patch = payload["patch"].as_str().unwrap();
    assert!(patch.contains("local.txt"), "{patch}");
    assert!(!patch.contains("remote-only.txt"), "{patch}");
}

#[test]
fn branch_push_remote_wins_over_push_default_and_upstream_remote() {
    let dir = tempfile::tempdir().unwrap();
    let clone = clone_of_an_origin_carrying_feature_x(dir.path());
    let origin = dir.path().join("origin.git");
    git_ok(
        &clone,
        &["remote", "add", "default-fork", origin.to_str().unwrap()],
    );
    git_ok(
        &clone,
        &["remote", "add", "branch-fork", origin.to_str().unwrap()],
    );
    git_ok(&clone, &["fetch", "-q", "default-fork"]);
    git_ok(&clone, &["fetch", "-q", "branch-fork"]);
    git_ok(&clone, &["config", "remote.pushDefault", "default-fork"]);
    git_ok(&clone, &["config", "branch.main.pushRemote", "branch-fork"]);

    let payload = unpushed_payload(&clone, None).unwrap();
    assert_eq!(payload["base"]["label"], "branch-fork/main");
}

#[test]
fn work_summary_counts_local_commits_and_the_final_tree_delta_once() {
    let dir = tempfile::tempdir().unwrap();
    let clone = clone_of_an_origin_carrying_feature_x(dir.path());
    write(&clone, "committed.txt", "one\ntwo\n");
    git_ok(&clone, &["add", "."]);
    git_ok(&clone, &["commit", "-q", "-m", "local one"]);
    write(&clone, "committed.txt", "one\ntwo\nthree\n");
    git_ok(&clone, &["commit", "-q", "-am", "local two"]);
    write(&clone, "untracked.txt", "four\n");

    let summary = work_summary(&clone).unwrap();

    assert_eq!(summary.pushes, 2);
    assert_eq!(summary.behind, 0);
    assert_eq!(summary.additions, 4);
    assert_eq!(summary.deletions, 0);
}

#[test]
fn work_summary_counts_the_push_target_commits_missing_from_head() {
    let dir = tempfile::tempdir().unwrap();
    let clone = clone_of_an_origin_carrying_feature_x(dir.path());
    git_ok(
        &clone,
        &["checkout", "-q", "-b", "remote-side", "origin/main"],
    );
    write(&clone, "remote.txt", "remote\n");
    git_ok(&clone, &["add", "."]);
    git_ok(&clone, &["commit", "-q", "-m", "remote"]);
    let remote_tip = git2::Repository::open(&clone)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap();
    git_ok(&clone, &["checkout", "-q", "main"]);
    git_ok(
        &clone,
        &[
            "update-ref",
            "refs/remotes/origin/main",
            &remote_tip.to_string(),
        ],
    );

    let remote_ahead = work_summary(&clone).unwrap();
    assert_eq!(remote_ahead.pushes, 0);
    assert_eq!(remote_ahead.behind, 1);
    assert!(
        remote_ahead.clean,
        "remote-only work does not dirty this checkout"
    );

    write(&clone, "local.txt", "local\n");
    git_ok(&clone, &["add", "."]);
    git_ok(&clone, &["commit", "-q", "-m", "local"]);
    let diverged = work_summary(&clone).unwrap();
    assert_eq!(diverged.pushes, 1);
    assert_eq!(diverged.behind, 1);
    assert!(!diverged.clean);

    git_ok(&clone, &["checkout", "-q", "-b", "local-only"]);
    let without_destination = work_summary(&clone).unwrap();
    assert_eq!(without_destination.behind, 0);
}

#[test]
fn work_summary_does_not_discover_a_repository_below_the_requested_path() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    init_repo(&nested);

    assert!(work_summary(dir.path()).is_err());
}

#[test]
fn work_summary_rejects_a_bare_repository_without_a_working_tree() {
    let dir = tempfile::tempdir().unwrap();
    git_ok(dir.path(), &["init", "-q", "--bare"]);

    assert!(work_summary(dir.path()).is_err());
}

#[test]
fn work_summary_rejects_the_git_directory_of_a_nonbare_repository() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());

    assert!(work_summary(&dir.path().join(".git")).is_err());
}

#[test]
fn aggregate_work_summary_combines_two_repositories_and_dirty_work() {
    let dir = tempfile::tempdir().unwrap();
    let first_root = dir.path().join("first");
    let second_root = dir.path().join("second");
    std::fs::create_dir(&first_root).unwrap();
    std::fs::create_dir(&second_root).unwrap();
    let first = clone_of_an_origin_carrying_feature_x(&first_root);
    let second = clone_of_an_origin_carrying_feature_x(&second_root);
    for (repo, name) in [(&first, "first.txt"), (&second, "second.txt")] {
        write(repo, name, "committed\n");
        git_ok(repo, &["add", "."]);
        git_ok(repo, &["commit", "-q", "-m", "local"]);
        git_ok(
            repo,
            &["checkout", "-q", "-b", "remote-side", "origin/main"],
        );
        write(repo, "remote.txt", "remote\n");
        git_ok(repo, &["add", "."]);
        git_ok(repo, &["commit", "-q", "-m", "remote"]);
        let remote_tip = git2::Repository::open(repo)
            .unwrap()
            .head()
            .unwrap()
            .target()
            .unwrap();
        git_ok(repo, &["checkout", "-q", "main"]);
        git_ok(
            repo,
            &[
                "update-ref",
                "refs/remotes/origin/main",
                &remote_tip.to_string(),
            ],
        );
        write(repo, "untracked.txt", "dirty\n");
    }

    let summary = aggregate_work_summary(&[first, second]).unwrap();

    assert_eq!(summary.pushes, 2);
    assert_eq!(summary.behind, 2);
    assert_eq!(summary.additions, 4);
    assert_eq!(summary.deletions, 0);
    assert!(!summary.clean);
}

#[test]
fn zero_line_untracked_work_is_not_clean() {
    let dir = tempfile::tempdir().unwrap();
    let repo = clone_of_an_origin_carrying_feature_x(dir.path());
    write(&repo, "empty.bin", "");

    let summary = work_summary(&repo).unwrap();

    assert_eq!(summary.additions, 0);
    assert_eq!(summary.deletions, 0);
    assert!(
        !summary.clean,
        "status, not line counts, decides cleanliness"
    );
}

/// A repository with no remote has no publication base at all, so EVERY
/// commit it holds is unpublished — which is the whole history of a project
/// a human has never pushed anywhere. The pushed commit list is capped at
/// the commits a client keeps, newest first, so a `git` item's size is a
/// constant rather than the length of the repository.
#[test]
fn a_checkout_with_no_publication_base_pushes_a_capped_commit_list() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("local-only");
    std::fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    let above_the_cap = crate::changes::UNPUSHED_COMMITS_MAX + 3;
    for number in 1..=above_the_cap {
        git_ok(
            &repo,
            &[
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                &format!("local {number}"),
            ],
        );
    }

    let summary = unpushed_summary(&repo).unwrap();

    assert_eq!(summary["base"]["kind"], "empty", "{summary:?}");
    let commits = summary["commits"].as_array().unwrap();
    assert_eq!(commits.len(), crate::changes::UNPUSHED_COMMITS_MAX);
    assert_eq!(
        commits[0]["subject"],
        format!("local {above_the_cap}"),
        "newest first: {summary:?}"
    );
}
