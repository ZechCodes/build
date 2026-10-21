use super::*;
use std::path::Path;

fn fixture(root: &Path) -> AppState {
    let context =
        crate::harness::HarnessContext::resolved(root.join("mcp.sock"), root.to_owned()).unwrap();
    AppState::new_unrooted_configured(root.join("worktrees"), "main", true, context)
}

#[test]
fn first_projection_contains_authoritative_lists_and_settings() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    app.initialize_snapshot_projection();
    let view = app.read_view.load();
    assert_eq!(view.get("board.list", "").unwrap()["projects"], json!([]));
    assert_eq!(view.get("project.list", "").unwrap()["projects"], json!([]));
    assert_eq!(
        view.get("workspace.list", "").unwrap()["workspaces"],
        json!([])
    );
    assert!(view
        .get("settings.get", "")
        .unwrap()
        .get("isolation_available")
        .is_some());
}

#[test]
fn first_projection_warms_registered_repository_observations() {
    let root = tempfile::tempdir().unwrap();
    let repo = crate::git_fixture::init_repo_named(root.path(), "repo");
    let mut app = fixture(root.path());
    let project = app.add_project(repo, "main".into());
    app.initialize_snapshot_projection();
    let view = app.read_view.load();
    let board = view.get("board.list", "").unwrap();
    assert_eq!(board["scanning"], false);
    assert_eq!(board["projects"][0]["project_id"], project);
    assert_eq!(
        view.get("project.list", "").unwrap()["projects"][0]["project_id"],
        project
    );
}

#[test]
fn publisher_preserves_unchanged_owner_chunks() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    app.initialize_snapshot_projection();
    let previous = app.read_view.load();
    app.publish_snapshot_projection();
    let next = app.read_view.load();
    assert!(previous
        .get_shared("models.list", "")
        .unwrap()
        .ptr_eq(next.get_shared("models.list", "").unwrap()));
}

#[test]
fn preparing_owner_candidate_does_not_publish_it() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    app.initialize_snapshot_projection();
    let previous = app.read_view.load();
    assert!(app.prepare_owner_snapshot_projection("unknown").is_some());
    assert!(Arc::ptr_eq(&previous, &app.read_view.load()));
}

#[test]
fn delayed_project_probe_cannot_replace_a_newer_observation() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("plain");
    std::fs::create_dir(&repo).unwrap();
    let mut app = fixture(root.path());
    let project = app.add_project(repo, "main".into());
    app.initialize_snapshot_projection();
    let delayed = app.capture_project_snapshot_refresh().render();
    let mut newer = app.read_view.load().get("project.list", "").unwrap()["projects"][0].clone();
    newer["sources"][0]["remote"] = json!("newly-observed-origin");
    app.record_project_snapshot_observation(&project, &newer);
    app.install_project_snapshot_refresh(delayed);
    app.publish_snapshot_projection();
    assert_eq!(
        app.read_view.load().get("project.list", "").unwrap()["projects"][0]["sources"][0]
            ["remote"],
        "newly-observed-origin"
    );
}

#[test]
fn owner_projection_uses_the_observed_branch_until_a_producer_refreshes_it() {
    let root = tempfile::tempdir().unwrap();
    let repo = crate::git_fixture::init_repo_named(root.path(), "branch-repo");
    let mut app = fixture(root.path());
    let project = app.add_project(repo.clone(), "main".into());
    let owner = "run-branch-observation";
    let active = crate::orchestrator::ActiveRun::workspace_conversation(
        crate::run::RunId::new(owner),
        "Branch observation".into(),
        repo.clone(),
        Default::default(),
    );
    app.projects.bind_entity(owner.into(), project);
    app.runs.insert(owner.into(), active);
    app.initialize_snapshot_projection();
    crate::git_fixture::git_in(&repo, &["checkout", "-b", "changed-after-observation"]);
    assert_eq!(
        app.runs[owner].worktree.branch(),
        "changed-after-observation"
    );
    app.publish_owner_snapshot_projection(owner);
    assert_eq!(
        app.read_view.load().get("board.list", "").unwrap()["runs"][0]["branch"],
        "main"
    );
    app.refresh_owner_snapshot_observations(owner);
    app.publish_owner_snapshot_projection(owner);
    assert_eq!(
        app.read_view.load().get("board.list", "").unwrap()["runs"][0]["branch"],
        "changed-after-observation"
    );
}

mod leaf_waits;
mod watched_tasks;
mod workspace_summaries;
