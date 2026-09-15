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

/// Restart does not resume or settle a legacy finish journal. Its record stays
/// pending for compatibility, and recovery performs no checkout lifecycle work.
#[test]
fn restart_preserves_a_pending_legacy_finish_record() {
    let (dir, repo) = init_repo();
    let state = qa_state(&repo, dir.path());
    let missing_checkout = dir.path().join("already-gone");
    let record = PersistedArchivedWorktree {
        status: WorktreeFinishStatus::Pending,
        project_path: repo.display().to_string(),
        worktree_id: crate::worktree::external_worktree_id(&missing_checkout),
        worktree_name: "already-gone".to_string(),
        worktree_path: missing_checkout.display().to_string(),
        branch: None,
        head_sha: git_stdout(&repo, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string(),
        upstream: None,
        unpushed: None,
        dirty_files: 0,
        uncommitted_files: 0,
        uncommitted_insertions: 0,
        uncommitted_deletions: 0,
        action: WorktreeFinishAction::Cleanup,
        archived_at: None,
    };
    state
        .require_store()
        .unwrap()
        .save_archived_worktree(&record)
        .unwrap();
    drop(state);

    let restored = qa_state(&repo, dir.path());
    let preserved = restored
        .board
        .archived(&record.worktree_id)
        .expect("the pending legacy journal remains loaded");
    assert_eq!(preserved.status, WorktreeFinishStatus::Pending);
    assert_eq!(preserved.archived_at, None);
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
