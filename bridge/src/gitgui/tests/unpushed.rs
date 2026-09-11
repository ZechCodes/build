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
