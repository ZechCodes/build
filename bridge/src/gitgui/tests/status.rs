// Exact test bodies moved from the former inline test module.
use super::*;

#[test]
fn untracked_and_tristate_status_letters() {
    let untracked = file_status_json("new.txt", git2::Status::WT_NEW).unwrap();
    assert_eq!(untracked["staged"], "none");
    assert_eq!(untracked["index_status"], "?");
    assert_eq!(untracked["worktree_status"], "?");

    let full = file_status_json("added.txt", git2::Status::INDEX_NEW).unwrap();
    assert_eq!(full["staged"], "full");
    assert_eq!(full["index_status"], "A");
    assert_eq!(full["worktree_status"], "-");

    let partial = file_status_json(
        "both.txt",
        git2::Status::INDEX_MODIFIED | git2::Status::WT_MODIFIED,
    )
    .unwrap();
    assert_eq!(partial["staged"], "partial");
    assert_eq!(partial["index_status"], "M");
    assert_eq!(partial["worktree_status"], "M");

    let worktree_only = file_status_json("dirty.txt", git2::Status::WT_MODIFIED).unwrap();
    assert_eq!(worktree_only["staged"], "none");
    assert_eq!(worktree_only["index_status"], "-");
    assert_eq!(worktree_only["worktree_status"], "M");

    assert!(file_status_json("clean.txt", git2::Status::CURRENT).is_none());
    assert!(file_status_json("ignored.txt", git2::Status::IGNORED).is_none());
}
/// A file's key follows the WORKING TREE, not the index: the index entry
/// of a staged-then-edited file does not move when the file is edited
/// again, so keying on it would serve a stale body as current.
#[test]
fn a_second_working_tree_edit_moves_the_files_key_and_the_status_key() {
    let (_dir, repo) = crate::git_fixture::init_repo();
    std::fs::write(repo.join("README.md"), "one\n").unwrap();
    crate::git_fixture::git_in(&repo, &["add", "README.md"]);

    let staged = status_payload(&repo).unwrap();
    std::fs::write(repo.join("README.md"), "one plus two\n").unwrap();
    let edited = status_payload(&repo).unwrap();
    std::fs::write(repo.join("README.md"), "one plus two plus three\n").unwrap();
    let edited_again = status_payload(&repo).unwrap();

    let key_of = |status: &Value| {
        status["files"][0]["content_key"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_ne!(key_of(&staged), key_of(&edited));
    assert_ne!(key_of(&edited), key_of(&edited_again));
    assert_ne!(staged["status_key"], edited["status_key"]);
    assert_ne!(edited["status_key"], edited_again["status_key"]);
    assert_eq!(edited_again["status_key"].as_str().unwrap().len(), 16);
}
#[test]
fn an_mtime_only_change_moves_the_status_key() {
    let (_dir, repo) = crate::git_fixture::init_repo();
    std::fs::write(repo.join("README.md"), "changed\n").unwrap();
    let path = repo.join("README.md");
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_times(
        std::fs::FileTimes::new()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)),
    )
    .unwrap();
    let first = status_payload(&repo).unwrap();

    file.set_times(
        std::fs::FileTimes::new()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_001)),
    )
    .unwrap();
    let touched = status_payload(&repo).unwrap();

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "changed\n",
        "touching the file must not change its content"
    );
    assert_eq!(first["files"][0]["path"], touched["files"][0]["path"]);
    assert_eq!(first["files"][0]["edited_at"], 1_700_000_000_000_u64);
    assert_eq!(touched["files"][0]["edited_at"], 1_700_000_001_000_u64);
    assert_ne!(first["status_key"], touched["status_key"]);
}
#[test]
fn the_status_payload_carries_counts_and_no_patch() {
    let (_dir, repo) = crate::git_fixture::init_repo();
    std::fs::write(repo.join("README.md"), "# project\nadded\n").unwrap();
    std::fs::write(repo.join("new.txt"), "a\nb\n").unwrap();

    let status = status_payload(&repo).unwrap();

    assert!(status.get("patch").is_none(), "{status}");
    assert!(status.get("truncated").is_none(), "{status}");
    let readme = &status["files"][0];
    assert_eq!(readme["path"], "README.md");
    assert_eq!(readme["added"], 1);
    assert_eq!(readme["deleted"], 0);
    assert_eq!(readme["binary"], false);
    assert_eq!(status["files"][1]["added"], 2);
    assert_eq!(status["stat"]["insertions"], 3);
    assert_eq!(status["stat"]["files_changed"], 2);
}
#[test]
fn a_deleted_file_keys_as_deleted() {
    let (_dir, repo) = crate::git_fixture::init_repo();
    std::fs::remove_file(repo.join("README.md")).unwrap();

    let status = status_payload(&repo).unwrap();
    assert_eq!(status["files"][0]["content_key"], "deleted");
    assert_eq!(status["files"][0]["deleted"], 1);
    assert!(status["files"][0].get("edited_at").is_none());
}
#[test]
fn a_held_status_key_answers_unchanged_and_nothing_else() {
    let (_dir, repo) = crate::git_fixture::init_repo();
    std::fs::write(repo.join("README.md"), "# project\nedit\n").unwrap();

    let full = status_payload(&repo).unwrap();
    let held = full["status_key"].as_str().unwrap();

    let unchanged = status_payload_unless(&repo, Some(held)).unwrap();
    assert_eq!(unchanged, json!({ "unchanged": true, "status_key": held }));

    let stale = status_payload_unless(&repo, Some("0000000000000000")).unwrap();
    assert_eq!(stale, full);
}
#[test]
fn an_unchanged_path_answers_an_empty_patch_and_still_carries_its_key() {
    let (_dir, repo) = crate::git_fixture::init_repo();

    let answer = file_patches(&repo, &["README.md".to_string()]).unwrap();
    let file = &answer["files"][0];

    assert_eq!(file["path"], "README.md");
    assert_eq!(file["patch"], "");
    assert_eq!(file["truncated"], false);
    assert_eq!(file["content_key"].as_str().unwrap().len(), 16);
}
#[test]
fn conflicted_entries_surface_as_u_instead_of_vanishing() {
    // libgit2 reports unmerged entries with CONFLICTED alone — no INDEX_*
    // or WT_* bits — so they must not fall through the changed-bits check.
    let conflicted = file_status_json("f.txt", git2::Status::CONFLICTED).unwrap();
    assert_eq!(conflicted["staged"], "none");
    assert_eq!(conflicted["index_status"], "U");
    assert_eq!(conflicted["worktree_status"], "U");
}
#[test]
fn untracked_classification_splits_index_from_worktree() {
    // A pristine new file is untracked; once it is in the index (even with
    // a further worktree edit) it is tracked, and a plain worktree edit of
    // a committed file is tracked.
    assert!(is_untracked_status(git2::Status::WT_NEW));
    assert!(!is_untracked_status(
        git2::Status::WT_NEW | git2::Status::INDEX_NEW
    ));
    assert!(!is_untracked_status(git2::Status::WT_MODIFIED));
    assert!(!is_untracked_status(git2::Status::INDEX_MODIFIED));
}
#[test]
fn cherry_pick_conflict_maps_and_merge_abort_clears_it() {
    let dir = tempfile::tempdir().unwrap();
    init_diverged(dir.path());
    assert!(!git_run(dir.path(), &["cherry-pick", "feature"])
        .status
        .success());
    assert_eq!(repo_state(dir.path()), "cherry-picking");
    merge_abort(dir.path()).unwrap();
    assert_eq!(repo_state(dir.path()), "clean");
}
#[test]
fn revert_conflict_maps_and_merge_abort_clears_it() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    write(dir.path(), "f.txt", "second\n");
    git_ok(dir.path(), &["commit", "-q", "-am", "second"]);
    write(dir.path(), "f.txt", "third\n");
    git_ok(dir.path(), &["commit", "-q", "-am", "third"]);
    // Reverting the base->second commit tries to restore "base", but the
    // line is now "third" — a conflict, so the repo enters Revert state.
    assert!(!git_run(dir.path(), &["revert", "--no-edit", "HEAD~1"])
        .status
        .success());
    assert_eq!(repo_state(dir.path()), "reverting");
    merge_abort(dir.path()).unwrap();
    assert_eq!(repo_state(dir.path()), "clean");
}
#[test]
fn stash_pop_conflict_is_conflicted_not_merging_and_has_no_abort() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    // Stash a change to f.txt, then commit a different change to the same
    // line, so the pop's three-way merge conflicts.
    write(dir.path(), "f.txt", "stashed\n");
    git_ok(dir.path(), &["stash", "push", "-q"]);
    write(dir.path(), "f.txt", "current\n");
    git_ok(dir.path(), &["commit", "-q", "-am", "current"]);
    assert!(!git_run(dir.path(), &["stash", "pop"]).status.success());

    // repo.state() is Clean here (no MERGE_HEAD), but the UU entry makes it
    // "conflicted" — the banner must surface it.
    assert_eq!(repo_state(dir.path()), "conflicted");
    // Nothing git-abortable: merge_abort refuses rather than lying.
    let err = merge_abort(dir.path()).unwrap_err();
    assert!(err.contains("no abortable operation"), "{err}");
}
#[test]
fn map_repository_state_covers_every_flavor() {
    use git2::RepositoryState::*;
    assert_eq!(map_repository_state(Clean), "clean");
    assert_eq!(map_repository_state(Merge), "merging");
    assert_eq!(map_repository_state(Rebase), "rebasing");
    assert_eq!(map_repository_state(RebaseInteractive), "rebasing");
    assert_eq!(map_repository_state(RebaseMerge), "rebasing");
    assert_eq!(map_repository_state(CherryPick), "cherry-picking");
    assert_eq!(map_repository_state(CherryPickSequence), "cherry-picking");
    assert_eq!(map_repository_state(Revert), "reverting");
    assert_eq!(map_repository_state(RevertSequence), "reverting");
    assert_eq!(map_repository_state(Bisect), "bisecting");
    assert_eq!(map_repository_state(ApplyMailbox), "other");
    assert_eq!(map_repository_state(ApplyMailboxOrRebase), "other");
}
