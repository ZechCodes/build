use super::*;

pub(in crate::app::tests) fn external_id(
    state: &mut AppState,
    project_id: &str,
    branch: Option<&str>,
) -> String {
    state
        .scan_external_worktrees_now(project_id)
        .unwrap()
        .into_iter()
        .find(|worktree| worktree.branch.as_deref() == branch)
        .expect("external worktree is discoverable")
        .id
}

/// Every scanned row says how its checkout is isolated, so a client can
/// tell a clone from a linked worktree without asking a second question.
#[test]
fn every_scanned_worktree_row_says_how_it_is_isolated() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    add_external_worktree(&repo, dir.path(), "labelled", "labelled");

    let rows = state.external_worktrees_json();

    let row = rows
        .rows
        .iter()
        .find(|row| row["branch"] == json!("labelled"))
        .expect("the scan finds the checkout");
    assert_eq!(row["isolation"], "worktree", "{row:?}");
}

/// A finish whose checkout vanished between the write-ahead record and the
/// destructive steps refuses while its branch still stands, rather than
/// quietly dropping work nobody has merged. Whether the branch stands is
/// the façade's answer about the project repo, not the app's own git.
#[test]
fn a_finish_merge_that_lost_its_checkout_refuses_while_the_branch_stands() {
    let (dir, repo) = init_repo();
    let worktrees = WorktreeManager::new(&repo, dir.path().join("wt"));
    let checkout =
        std::fs::canonicalize(add_external_worktree(&repo, dir.path(), "lost", "lost")).unwrap();
    let record = pending_merge_record(&repo, &checkout);
    std::fs::remove_dir_all(&checkout).unwrap();

    let refused = run_finish_git_steps(&worktrees, "main", &record).unwrap_err();

    assert!(
        refused.contains("lost its worktree before branch deletion"),
        "{refused}"
    );
    assert!(
        worktrees.branch_exists("lost").unwrap(),
        "the branch the finish refused to act on is untouched"
    );
}

/// The same finish once the branch has gone too: there is nothing left to
/// merge or delete, so the destructive half is already done.
#[test]
fn a_finish_merge_that_lost_its_checkout_and_its_branch_is_done() {
    let (dir, repo) = init_repo();
    let worktrees = WorktreeManager::new(&repo, dir.path().join("wt"));
    let checkout =
        std::fs::canonicalize(add_external_worktree(&repo, dir.path(), "gone", "gone")).unwrap();
    let record = pending_merge_record(&repo, &checkout);
    std::fs::remove_dir_all(&checkout).unwrap();
    git_in(&repo, &["worktree", "prune"]);
    git_in(&repo, &["branch", "-D", "gone"]);

    assert!(run_finish_git_steps(&worktrees, "main", &record).is_ok());
}

/// The write-ahead record a `merge` finish steps from, for a checkout whose
/// branch is named after its directory.
fn pending_merge_record(
    repo: &std::path::Path,
    checkout: &std::path::Path,
) -> PersistedArchivedWorktree {
    let name = checkout
        .file_name()
        .and_then(|name| name.to_str())
        .expect("the checkout has a directory name")
        .to_string();
    PersistedArchivedWorktree {
        status: WorktreeFinishStatus::Pending,
        project_path: repo.display().to_string(),
        worktree_id: crate::worktree::external_worktree_id(checkout),
        worktree_name: name.clone(),
        worktree_path: checkout.display().to_string(),
        branch: Some(name),
        head_sha: git_stdout(checkout, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string(),
        upstream: None,
        unpushed: None,
        dirty_files: 0,
        uncommitted_files: 0,
        uncommitted_insertions: 0,
        uncommitted_deletions: 0,
        action: WorktreeFinishAction::Merge,
        archived_at: None,
    }
}

/// Stage publication is evidence in the project repo's refs, and putting
/// the checkout's branch to them first — nothing at all for a linked
/// worktree, whose refs are already the project's — changes no answer it
/// gives.
#[test]
fn stage_publication_of_a_linked_worktree_survives_publishing_first() {
    let (dir, repo) = init_repo();
    let worktrees = WorktreeManager::new(&repo, dir.path().join("wt"));
    let checkout = add_external_worktree(&repo, dir.path(), "staged", "staged");
    std::fs::write(checkout.join("stage.txt"), "shipped\n").unwrap();
    git_in(&checkout, &["add", "-A"]);
    git_in(&checkout, &["commit", "-m", "stage work"]);
    let completion_sha = git_stdout(&checkout, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();

    assert_eq!(
        classify_stage_publication(&worktrees, &checkout, "staged", "main", &completion_sha),
        StagePublication::Local,
        "nothing in the project carries the commit yet"
    );

    git_in(&repo, &["merge", "--no-edit", "--", "staged"]);

    assert_eq!(
        classify_stage_publication(&worktrees, &checkout, "staged", "main", &completion_sha),
        StagePublication::Merged,
    );
}

/// The same finish over a copy-on-write clone. A clone is its own
/// repository, so its commits reach the project only through the publish
/// the façade does first, and git's worktree registry has never heard of
/// the directory: the app's own git could neither merge the work nor take
/// the checkout away. Going through the façade, one finish path does both.
#[test]
fn a_finish_merge_of_a_clone_lands_its_work_and_takes_its_branch() {
    let (dir, repo) = init_repo();
    if !crate::isolation::probe::cow_or_skip(dir.path()) {
        return;
    }
    let worktrees = WorktreeManager::new(&repo, dir.path().join("wt"));
    let clone = worktrees
        .create_cutting_branch("landed", "main", Isolation::Cow)
        .unwrap()
        .worktree;
    std::fs::write(clone.path.join("landed.txt"), "shipped\n").unwrap();
    git_in(&clone.path, &["add", "-A"]);
    git_in(&clone.path, &["commit", "-m", "clone work"]);
    let record = pending_merge_record(&repo, &std::fs::canonicalize(&clone.path).unwrap());

    run_finish_git_steps(&worktrees, "main", &record).unwrap();

    assert!(
        repo.join("landed.txt").exists(),
        "the clone's commit is on the base branch in the project"
    );
    assert!(
        !worktrees.branch_exists("landed").unwrap(),
        "teardown owns the branch the clone was cut onto"
    );
    assert!(!clone.path.exists(), "the clone itself is gone");
}

#[test]
fn worktree_finish_cleanup_requires_clean_and_preserves_branch() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.projects[0].id.clone();
    let path = add_external_worktree(&repo, dir.path(), "cleanup", "cleanup");
    let worktree_id = external_id(&mut state, &project_id, Some("cleanup"));

    std::fs::write(path.join("dirty.txt"), "dirty\n").unwrap();
    let rejected = state.handle(req(
        "worktree.finish",
        json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "cleanup" }),
    ));
    assert_eq!(rejected["ok"], false, "{rejected:?}");
    assert!(path.exists());

    std::fs::remove_file(path.join("dirty.txt")).unwrap();
    let finished = state.handle(req(
        "worktree.finish",
        json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "cleanup" }),
    ));
    assert_eq!(finished["ok"], true, "{finished:?}");
    assert_eq!(finished["result"]["action"], "cleanup");
    assert!(!path.exists());
    assert!(git2::Repository::open(&repo)
        .unwrap()
        .find_branch("cleanup", git2::BranchType::Local)
        .is_ok());

    let repeated = state.handle(req(
        "worktree.finish",
        json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "cleanup" }),
    ));
    assert_eq!(repeated["ok"], true, "{repeated:?}");
}

#[test]
fn worktree_finish_store_failure_happens_before_worktree_removal() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.projects[0].id.clone();
    let path = add_external_worktree(&repo, dir.path(), "store-failure", "store-failure");
    let worktree_id = external_id(&mut state, &project_id, Some("store-failure"));
    state
        .store
        .as_ref()
        .expect("the qa state has a store")
        .fail_next_write();

    let failed = state.handle(req(
        "worktree.finish",
        json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "cleanup" }),
    ));

    assert_eq!(failed["ok"], false, "{failed:?}");
    assert!(path.exists(), "store failure must precede removal");
    assert!(git2::Repository::open(&repo)
        .unwrap()
        .find_branch("store-failure", git2::BranchType::Local)
        .is_ok());
}

#[test]
fn worktree_finish_merge_checkpoints_dirty_work_deletes_branch_and_archives() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.projects[0].id.clone();
    let path = add_external_worktree(&repo, dir.path(), "merge-me", "merge-me");
    std::fs::write(path.join("feature.txt"), "finished\n").unwrap();
    let worktree_id = external_id(&mut state, &project_id, Some("merge-me"));

    let finished = state.handle(req(
        "worktree.finish",
        json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "merge" }),
    ));
    assert_eq!(finished["ok"], true, "{finished:?}");
    assert!(repo.join("feature.txt").is_file());
    assert!(!path.exists());
    assert!(git2::Repository::open(&repo)
        .unwrap()
        .find_branch("merge-me", git2::BranchType::Local)
        .is_err());
    let log = Command::new("git")
        .args(["log", "--format=%s", "-2"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&log.stdout).contains("Build checkpoint before merge"));

    let archive = state.handle(req("archive.list", json!({ "project_id": project_id })));
    let worktree = &archive["result"]["worktrees"][0];
    assert_eq!(worktree["worktree_id"], worktree_id);
    assert_eq!(worktree["dirty_files"], 1);
    assert_eq!(worktree["action"], "merge");
}

#[test]
fn worktree_finish_push_requires_tracking_then_checkpoints_pushes_and_keeps_branch() {
    let (dir, repo, _origin) = init_repo_with_origin();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.projects[0].id.clone();
    let path = add_external_worktree(&repo, dir.path(), "push-me", "push-me");
    let worktree_id = external_id(&mut state, &project_id, Some("push-me"));

    let rejected = state.handle(req(
        "worktree.finish",
        json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "push" }),
    ));
    assert_eq!(rejected["ok"], false, "{rejected:?}");
    assert!(rejected["error"].as_str().unwrap().contains("upstream"));

    git_in(&path, &["push", "-u", "origin", "push-me"]);
    std::fs::write(path.join("pushed.txt"), "published\n").unwrap();
    let finished = state.handle(req(
        "worktree.finish",
        json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "push" }),
    ));
    assert_eq!(finished["ok"], true, "{finished:?}");
    assert!(!path.exists());
    assert!(git2::Repository::open(&repo)
        .unwrap()
        .find_branch("push-me", git2::BranchType::Local)
        .is_ok());
    let remote_subject = Command::new("git")
        .args(["log", "--format=%s", "-1", "origin/push-me"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&remote_subject.stdout).contains("Build checkpoint before push")
    );
}

#[test]
fn worktree_finish_delete_accepts_dirty_detached_head_without_deleting_a_branch() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.projects[0].id.clone();
    let path = add_external_worktree(&repo, dir.path(), "detached", "detached-source");
    git_in(&path, &["checkout", "--detach"]);
    std::fs::write(path.join("discarded.txt"), "discard me\n").unwrap();
    let worktree_id = external_id(&mut state, &project_id, None);

    let finished = state.handle(req(
        "worktree.finish",
        json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "delete" }),
    ));
    assert_eq!(finished["ok"], true, "{finished:?}");
    assert!(finished["result"]["branch"].is_null());
    assert!(!path.exists());
    assert!(git2::Repository::open(&repo)
        .unwrap()
        .find_branch("detached-source", git2::BranchType::Local)
        .is_ok());

    let attached_path =
        add_external_worktree(&repo, dir.path(), "attached-delete", "attached-delete");
    std::fs::write(attached_path.join("discarded.txt"), "discard me too\n").unwrap();
    let attached_id = external_id(&mut state, &project_id, Some("attached-delete"));
    let attached = state.handle(req(
        "worktree.finish",
        json!({ "project_id": project_id, "worktree_id": attached_id, "action": "delete" }),
    ));
    assert_eq!(attached["ok"], true, "{attached:?}");
    assert!(git2::Repository::open(&repo)
        .unwrap()
        .find_branch("attached-delete", git2::BranchType::Local)
        .is_err());
}

#[test]
fn worktree_finish_branch_delete_failure_is_retryable_and_not_archived() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.projects[0].id.clone();
    let path = add_external_worktree(&repo, dir.path(), "delete-retry", "delete-retry");
    std::fs::write(path.join("discarded.txt"), "discard me\n").unwrap();
    let worktree_id = external_id(&mut state, &project_id, Some("delete-retry"));
    let lock = repo.join(".git/refs/heads/delete-retry.lock");
    std::fs::write(&lock, "locked\n").unwrap();

    let failed = state.handle(req(
        "worktree.finish",
        json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "delete" }),
    ));
    assert_eq!(failed["ok"], false, "{failed:?}");
    assert!(
        path.exists(),
        "branch failure must leave a retryable worktree"
    );
    assert_eq!(
        external_id(&mut state, &project_id, Some("delete-retry")),
        worktree_id,
        "the rail must still resolve the original attached worktree"
    );
    let archive = state.handle(req("archive.list", json!({ "project_id": project_id })));
    assert!(archive["result"]["worktrees"]
        .as_array()
        .unwrap()
        .is_empty());

    std::fs::remove_file(lock).unwrap();
    let finished = state.handle(req(
        "worktree.finish",
        json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "delete" }),
    ));
    assert_eq!(finished["ok"], true, "{finished:?}");
    assert!(!path.exists());
    assert!(git2::Repository::open(&repo)
        .unwrap()
        .find_branch("delete-retry", git2::BranchType::Local)
        .is_err());
}

#[test]
fn worktree_finish_never_accepts_paths_and_git_failure_does_not_archive() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.projects[0].id.clone();
    let path = add_external_worktree(&repo, dir.path(), "conflict", "conflict");
    std::fs::write(path.join("README.md"), "feature\n").unwrap();
    git_in(&path, &["commit", "-am", "feature"]);
    std::fs::write(repo.join("README.md"), "mainline\n").unwrap();
    git_in(&repo, &["commit", "-am", "mainline"]);
    let worktree_id = external_id(&mut state, &project_id, Some("conflict"));

    let forged = state.handle(req(
        "worktree.finish",
        json!({
            "project_id": project_id,
            "worktree_id": "wt-not-real",
            "path": path,
            "action": "delete"
        }),
    ));
    assert_eq!(forged["ok"], false, "{forged:?}");
    assert!(path.exists());

    let failed = state.handle(req(
        "worktree.finish",
        json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "merge" }),
    ));
    assert_eq!(failed["ok"], false, "{failed:?}");
    assert!(path.exists(), "a failed merge must not remove the worktree");
    let archive = state.handle(req("archive.list", json!({ "project_id": project_id })));
    assert!(archive["result"]["worktrees"]
        .as_array()
        .unwrap()
        .is_empty());

    let primary_id = crate::worktree::external_worktree_id(&std::fs::canonicalize(&repo).unwrap());
    let primary = state.handle(req(
        "worktree.finish",
        json!({
            "project_id": project_id,
            "worktree_id": primary_id,
            "action": "delete"
        }),
    ));
    assert_eq!(primary["ok"], false, "{primary:?}");
    assert!(repo.join("README.md").exists());
}
