use super::*;

/// Run git without asserting success — for setting up conflict/rebase
/// states whose whole point is a non-zero exit.
fn git_try(dir: &std::path::Path, args: &[&str]) {
    let _ = Command::new("git").args(args).current_dir(dir).status();
}

/// A working repo wired to a bare "origin" it already tracks (main →
/// origin/main, ahead 0 / behind 0).
pub(in crate::app::tests) fn init_repo_with_origin() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (dir, repo) = init_repo();
    let origin = dir.path().join("origin.git");
    git_in(
        dir.path(),
        &[
            "clone",
            "--bare",
            repo.to_str().unwrap(),
            origin.to_str().unwrap(),
        ],
    );
    git_in(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git_in(&repo, &["fetch", "origin"]);
    git_in(&repo, &["branch", "--set-upstream-to=origin/main", "main"]);
    (dir, repo, origin)
}

/// A second working checkout of `origin`, standing in for another dev.
fn clone_working(origin: &std::path::Path, dest: &std::path::Path) {
    git_in(
        dest.parent().unwrap(),
        &["clone", origin.to_str().unwrap(), dest.to_str().unwrap()],
    );
    git_in(dest, &["config", "user.email", "o@build.ing"]);
    git_in(dest, &["config", "user.name", "O"]);
}

#[test]
fn git_status_carries_the_repo_management_fields() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    let res = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], true, "{res:?}");
    let result = &res["result"];
    assert_eq!(result["repo_state"], "clean");
    // No remote configured → upstream/ahead/behind are null, not NaN.
    assert!(result["upstream"].is_null());
    assert!(result["ahead"].is_null());
    assert!(result["behind"].is_null());
    assert_eq!(result["stash_count"], 0);
}

#[test]
fn git_fetch_pull_push_round_trip_through_a_bare_origin() {
    let (dir, repo, origin) = init_repo_with_origin();
    let other = dir.path().join("other");
    clone_working(&origin, &other);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    // Another dev pushes a commit to origin.
    std::fs::write(other.join("remote.txt"), "remote\n").unwrap();
    git_in(&other, &["add", "remote.txt"]);
    git_in(&other, &["commit", "-m", "remote work"]);
    git_in(&other, &["push", "origin", "main"]);

    // git.fetch updates the tracking ref: we are now behind by one.
    let fetched = state.handle(req("git.fetch", json!({ "project_id": project_id })));
    assert_eq!(fetched["ok"], true, "{fetched:?}");
    assert_eq!(fetched["result"]["upstream"], "origin/main");
    assert_eq!(fetched["result"]["behind"], 1);
    assert_eq!(fetched["result"]["ahead"], 0);

    // git.pull (ff) fast-forwards the branch onto the remote commit.
    let pulled = state.handle(req("git.pull", json!({ "project_id": project_id })));
    assert_eq!(pulled["ok"], true, "{pulled:?}");
    assert_eq!(pulled["result"]["behind"], 0);
    assert!(repo.join("remote.txt").exists());

    // A local commit, then git.push publishes it to origin.
    std::fs::write(repo.join("local.txt"), "local\n").unwrap();
    git_in(&repo, &["add", "local.txt"]);
    git_in(&repo, &["commit", "-m", "local work"]);
    let ahead = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert_eq!(ahead["result"]["ahead"], 1);

    let pushed = state.handle(req("git.push", json!({ "project_id": project_id })));
    assert_eq!(pushed["ok"], true, "{pushed:?}");
    assert_eq!(pushed["result"]["ahead"], 0);
    assert_eq!(pushed["result"]["behind"], 0);

    // The other checkout can now fetch our commit — proof it reached origin.
    git_in(&other, &["fetch", "origin"]);
    let log = Command::new("git")
        .args(["log", "--oneline", "origin/main"])
        .current_dir(&other)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&log.stdout).contains("local work"));
}

#[test]
fn git_push_sets_the_upstream_on_the_first_push() {
    let (dir, repo) = init_repo();
    let origin = dir.path().join("origin.git");
    git_in(
        dir.path(),
        &["init", "--bare", "-b", "main", origin.to_str().unwrap()],
    );
    git_in(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    // No upstream yet.
    let before = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert!(before["result"]["upstream"].is_null());

    let pushed = state.handle(req("git.push", json!({ "project_id": project_id })));
    assert_eq!(pushed["ok"], true, "{pushed:?}");
    assert_eq!(pushed["result"]["upstream"], "origin/main");
    assert_eq!(pushed["result"]["ahead"], 0);
}

#[test]
fn git_push_force_uses_force_with_lease_after_a_rewrite() {
    let (dir, repo, _origin) = init_repo_with_origin();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    // Publish a commit, then rewrite it so local diverges from origin.
    std::fs::write(repo.join("x.txt"), "one\n").unwrap();
    git_in(&repo, &["add", "x.txt"]);
    git_in(&repo, &["commit", "-m", "first"]);
    assert_eq!(
        state.handle(req("git.push", json!({ "project_id": project_id })))["ok"],
        true
    );
    std::fs::write(repo.join("x.txt"), "two\n").unwrap();
    git_in(&repo, &["commit", "-a", "--amend", "-m", "rewritten"]);

    // A plain push is rejected (non-fast-forward); force-with-lease wins.
    let plain = state.handle(req("git.push", json!({ "project_id": project_id })));
    assert_eq!(plain["ok"], false, "{plain:?}");
    let forced = state.handle(req(
        "git.push",
        json!({ "project_id": project_id, "force": true }),
    ));
    assert_eq!(forced["ok"], true, "{forced:?}");
}

#[test]
fn git_push_refuses_a_detached_head() {
    let (dir, repo, _origin) = init_repo_with_origin();
    git_in(&repo, &["checkout", "--detach", "HEAD"]);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    let res = state.handle(req("git.push", json!({ "project_id": project_id })));
    assert_eq!(res["ok"], false, "{res:?}");
    assert_eq!(res["error"], "cannot push a detached HEAD");
}

#[test]
fn git_pull_ff_only_refuses_divergent_history() {
    let (dir, repo, origin) = init_repo_with_origin();
    let other = dir.path().join("other");
    clone_working(&origin, &other);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    std::fs::write(other.join("theirs.txt"), "theirs\n").unwrap();
    git_in(&other, &["add", "theirs.txt"]);
    git_in(&other, &["commit", "-m", "theirs"]);
    git_in(&other, &["push", "origin", "main"]);

    std::fs::write(repo.join("mine.txt"), "mine\n").unwrap();
    git_in(&repo, &["add", "mine.txt"]);
    git_in(&repo, &["commit", "-m", "mine"]);

    assert_eq!(
        state.handle(req("git.fetch", json!({ "project_id": project_id })))["ok"],
        true
    );
    let pulled = state.handle(req("git.pull", json!({ "project_id": project_id })));
    assert_eq!(pulled["ok"], false, "{pulled:?}");
    assert!(
        pulled["error"].as_str().unwrap().contains("fast-forward")
            || pulled["error"].as_str().unwrap().contains("fast forward"),
        "{pulled:?}"
    );
}

#[test]
fn git_pull_conflict_leaves_a_visible_merging_state() {
    let (dir, repo, origin) = init_repo_with_origin();
    let other = dir.path().join("other");
    clone_working(&origin, &other);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    // Both sides edit README differently; the other side lands first.
    std::fs::write(other.join("README.md"), "# theirs\n").unwrap();
    git_in(&other, &["commit", "-am", "theirs"]);
    git_in(&other, &["push", "origin", "main"]);
    std::fs::write(repo.join("README.md"), "# mine\n").unwrap();
    git_in(&repo, &["commit", "-am", "mine"]);
    assert_eq!(
        state.handle(req("git.fetch", json!({ "project_id": project_id })))["ok"],
        true
    );

    let pulled = state.handle(req(
        "git.pull",
        json!({ "project_id": project_id, "mode": "merge" }),
    ));
    assert_eq!(pulled["ok"], false, "{pulled:?}");

    // The conflict is legible in the very next status: merging + a U file.
    let status = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert_eq!(status["result"]["repo_state"], "merging");
    let readme = file_entry(&status["result"], "README.md");
    assert_eq!(readme["index_status"], "U");
    assert_eq!(readme["worktree_status"], "U");
}

#[test]
fn worktree_diff_reports_existing_file_mtimes_and_omits_deletions() {
    let (dir, repo) = init_repo();
    let checkout = add_external_worktree(&repo, dir.path(), "timestamped", "timestamped");
    std::fs::write(checkout.join("new.txt"), "new\n").unwrap();
    std::fs::remove_file(checkout.join("README.md")).unwrap();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let worktree_id = state
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|worktree| worktree.branch.as_deref() == Some("timestamped"))
        .unwrap()
        .id;

    let result = state.handle(req(
        "worktree.diff",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    ));
    assert_eq!(result["ok"], true, "{result:?}");
    let edited_at = result["result"]["file_edited_at"].as_object().unwrap();
    assert!(edited_at["new.txt"].as_u64().unwrap() > 0);
    assert!(edited_at.get("README.md").is_none());

    let diff_key = result["result"]["diff_key"].as_str().unwrap().to_string();
    let unchanged = state.handle(req(
        "worktree.diff",
        json!({
            "project_id": project_id, "worktree_id": worktree_id, "if_diff_key": diff_key,
        }),
    ));
    assert_eq!(
        unchanged["result"],
        json!({ "unchanged": true, "diff_key": diff_key }),
        "{unchanged:?}"
    );

    std::fs::write(checkout.join("new.txt"), "newer\n").unwrap();
    let changed = state.handle(req(
        "worktree.diff",
        json!({
            "project_id": project_id, "worktree_id": worktree_id, "if_diff_key": diff_key,
        }),
    ));
    assert_ne!(changed["result"]["diff_key"], diff_key, "{changed:?}");
    assert!(changed["result"]["patch"]
        .as_str()
        .unwrap()
        .contains("newer"));
}

#[test]
fn git_stash_and_pop_round_trip() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    std::fs::write(repo.join("README.md"), "# edited\n").unwrap();
    std::fs::write(repo.join("fresh.txt"), "fresh\n").unwrap();

    // Stash includes the untracked file (-u), leaving a clean tree.
    let stashed = state.handle(req("git.stash", json!({ "project_id": project_id })));
    assert_eq!(stashed["ok"], true, "{stashed:?}");
    assert_eq!(stashed["result"]["stash_count"], 1);
    assert!(stashed["result"]["files"].as_array().unwrap().is_empty());
    assert!(!repo.join("fresh.txt").exists());

    // Pop restores both, and the stash stack is empty again.
    let popped = state.handle(req("git.stash_pop", json!({ "project_id": project_id })));
    assert_eq!(popped["ok"], true, "{popped:?}");
    assert_eq!(popped["result"]["stash_count"], 0);
    assert!(repo.join("fresh.txt").exists());

    // Popping an empty stack is git's error, passed through.
    let empty = state.handle(req("git.stash_pop", json!({ "project_id": project_id })));
    assert_eq!(empty["ok"], false, "{empty:?}");
}

#[test]
fn git_discard_reverts_tracked_and_deletes_untracked() {
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    // A tracked edit (staged) and a fresh untracked file.
    std::fs::write(repo.join("README.md"), "# tampered\n").unwrap();
    git_in(&repo, &["add", "README.md"]);
    std::fs::write(repo.join("junk.txt"), "junk\n").unwrap();

    let res = state.handle(req(
        "git.discard",
        json!({ "project_id": project_id, "paths": ["README.md", "junk.txt"] }),
    ));
    assert_eq!(res["ok"], true, "{res:?}");

    // Tracked file is back to its committed content, in both index and tree.
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "# project\n"
    );
    assert!(!has_file_entry(&res["result"], "README.md"));
    // Untracked file is gone from disk.
    assert!(!repo.join("junk.txt").exists());
    assert!(!has_file_entry(&res["result"], "junk.txt"));
}

#[test]
fn git_discard_rejects_traversal_and_symlink_escapes() {
    let (dir, repo) = init_repo();
    // A secret outside the worktree, and an untracked symlink pointing at it.
    let secret = dir.path().join("secret.txt");
    std::fs::write(&secret, "top secret\n").unwrap();
    std::os::unix::fs::symlink(&secret, repo.join("leak")).unwrap();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    // Lexical traversal is refused before any git call.
    let traversal = state.handle(req(
        "git.discard",
        json!({ "project_id": project_id, "paths": ["../secret.txt"] }),
    ));
    assert_eq!(traversal["ok"], false, "{traversal:?}");

    // The symlink's components look Normal, so only the canonical fence
    // catches it — and the outside secret must survive.
    let symlink = state.handle(req(
        "git.discard",
        json!({ "project_id": project_id, "paths": ["leak"] }),
    ));
    assert_eq!(symlink["ok"], false, "{symlink:?}");
    assert!(
        secret.exists(),
        "the fence must not delete outside the worktree"
    );
}

#[test]
fn git_merge_abort_handles_each_repo_state() {
    // Clean: nothing to abort.
    let (dir, repo) = init_repo();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let clean = state.handle(req("git.merge_abort", json!({ "project_id": project_id })));
    assert_eq!(clean["ok"], false, "{clean:?}");
    assert_eq!(clean["error"], "no abortable operation in progress");

    // Merging: abort returns to a clean state.
    git_in(&repo, &["checkout", "-b", "topic"]);
    std::fs::write(repo.join("README.md"), "# topic\n").unwrap();
    git_in(&repo, &["commit", "-am", "topic"]);
    git_in(&repo, &["checkout", "main"]);
    std::fs::write(repo.join("README.md"), "# mainline\n").unwrap();
    git_in(&repo, &["commit", "-am", "mainline"]);
    git_try(&repo, &["merge", "topic"]);
    let aborted = state.handle(req("git.merge_abort", json!({ "project_id": project_id })));
    assert_eq!(aborted["ok"], true, "{aborted:?}");
    assert_eq!(aborted["result"]["repo_state"], "clean");

    // Rebasing: a conflicting rebase leaves a rebasing state to abort.
    git_in(&repo, &["checkout", "topic"]);
    git_try(&repo, &["rebase", "main"]);
    let status = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert_eq!(status["result"]["repo_state"], "rebasing");
    let rebase_aborted = state.handle(req("git.merge_abort", json!({ "project_id": project_id })));
    assert_eq!(rebase_aborted["ok"], true, "{rebase_aborted:?}");
    assert_eq!(rebase_aborted["result"]["repo_state"], "clean");
}
