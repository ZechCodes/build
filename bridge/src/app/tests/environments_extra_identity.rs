//! Identity consumers reached through archive and ownership projections.
use super::*;
use crate::app::projects::test_support::register_sibling;
use crate::store::{WorktreeFinishAction, WorktreeFinishStatus};

fn sibling_state(root: &Path, repo: &Path) -> (AppState, String, String) {
    let mut app = qa_state_before_admission(repo, root);
    let migrated = app.project_at(0).id.clone();
    let sibling = register_sibling(&mut app, &migrated);
    (app, migrated, sibling)
}

fn archived_record(key: &str, worktree: &str) -> PersistedArchivedWorktree {
    PersistedArchivedWorktree {
        status: WorktreeFinishStatus::Archived,
        project_path: key.into(),
        worktree_id: worktree.into(),
        worktree_name: worktree.into(),
        worktree_path: worktree.into(),
        branch: Some("same-branch".into()),
        head_sha: "a".repeat(40),
        upstream: None,
        unpushed: None,
        dirty_files: 0,
        uncommitted_files: 0,
        uncommitted_insertions: 0,
        uncommitted_deletions: 0,
        action: WorktreeFinishAction::Cleanup,
        archived_at: Some("2026-11-08T00:00:00Z".into()),
    }
}

#[test]
fn environments_extra_identity_archives_join_and_dedupe_by_key() {
    let (dir, repo) = init_repo();
    let (mut app, migrated, sibling) = sibling_state(dir.path(), &repo);
    let records = [
        (&migrated, "legacy-worktree"),
        (&sibling, "sibling-worktree"),
    ]
    .map(|(project, id)| archived_record(&app.project(project).unwrap().project_key, id));
    app.board.replace_archived(
        records
            .into_iter()
            .map(|row| (row.worktree_id.clone(), row))
            .collect(),
    );
    let mut active = crate::orchestrator::ActiveRun::workspace_conversation(
        crate::run::RunId::new("sibling-run"),
        "same-branch".into(),
        dir.path().join("sibling-worktree"),
        ModelChoice::default(),
    );
    active.run.state = RunState::Archived;
    app.runs.insert("sibling-run".into(), active);
    app.projects
        .bind_entity("sibling-run".into(), sibling.clone());
    let items = app.archived_list()["items"].as_array().unwrap().clone();
    assert_eq!(
        items.len(),
        2,
        "the sibling run merges its own retained record only: {items:?}"
    );
    let legacy = items
        .iter()
        .find(|row| row["worktree_id"] == "legacy-worktree")
        .unwrap();
    assert_eq!(legacy["project_id"], migrated);
    let sibling_row = items
        .iter()
        .find(|row| row["run_id"] == "sibling-run")
        .unwrap();
    assert_eq!(sibling_row["project_id"], sibling);
    assert_eq!(sibling_row["worktree_id"], "sibling-worktree");
}

#[test]
fn environments_extra_identity_ownership_recognizes_sibling_scratch_and_parked_key() {
    let (dir, repo) = init_repo();
    let (mut app, migrated, sibling) = sibling_state(dir.path(), &repo);
    let key = app.project(&sibling).unwrap().project_key.clone();
    // Compute the final keyed scratch independently, so this fails before scratch routing changes.
    let suffix = key.rsplit('/').next().unwrap();
    let digest = sha256_hex(key.as_bytes());
    let root = app.state_root.join("project-scratch").join(format!(
        "{}-{}",
        crate::app::projects::safe_mount_name(suffix),
        &digest[..12]
    ));
    std::fs::create_dir_all(&root).unwrap();
    let active = crate::orchestrator::ActiveRun::workspace_conversation(
        crate::run::RunId::new("sibling-owner"),
        "sibling".into(),
        root.clone(),
        ModelChoice::default(),
    );
    app.runs.insert("sibling-owner".into(), active);
    app.projects
        .bind_entity("sibling-owner".into(), sibling.clone());
    assert!(app
        .capture_snapshot_ownership()
        .is_project_owner("sibling-owner"));
    app.projects
        .retain_entity_path("sibling-owner".into(), key.clone());
    app.projects.unbind_live_entity("sibling-owner");
    app.projects
        .park(json!({"id":sibling, "project_key":key, "path":repo}));
    // A migrated sibling on the same home must never claim the orphaned owner's key.
    app.projects
        .park(json!({"id":migrated, "project_key":repo, "path":repo}));
    let context = app.capture_snapshot_ownership();
    assert!(context.is_project_owner("sibling-owner"));
    let checkout = dir.path().join("parked-checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    let active = app.runs.get_mut("sibling-owner").unwrap();
    active.worktree.path = checkout.clone();
    active.run.state = RunState::Building;
    app.workspaces.adopt_root(
        &sibling,
        "parked-workspace".into(),
        "parked workspace".into(),
        checkout,
        "source-1".into(),
        false,
    );
    assert_eq!(
        app.capture_snapshot_ownership()
            .compaction_workspace("sibling-owner"),
        Some("parked-workspace"),
        "an unbound sibling run resolves its parked project's key, not the shared home"
    );
}
