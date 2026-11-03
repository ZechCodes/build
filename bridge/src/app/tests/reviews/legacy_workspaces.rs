//! A cold workspace carries retained legacy identity without task-link guesses.
use super::*;

fn seeded() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    AppState,
    String,
    String,
    Value,
) {
    let home = tempfile::tempdir().unwrap();
    let (repository, mut app, project) = tracked(home.path());
    let workspace_id = workspace(&mut app, &project, "legacy workspace marker");
    let task = filed(&mut app, &project, "Unlinked legacy review");
    assert!(task["links"]["workspace_ids"]
        .as_array()
        .unwrap()
        .is_empty());
    legacy_review(
        &mut app,
        json!({"task_id":task["id"],"workspace_id":workspace_id,"expected_version":0}),
    );
    (home, repository, app, project, workspace_id, task)
}

fn assert_marker(row: &Value) {
    assert_eq!(row["legacy_review"], true, "{row}");
    let typed: crate::api::v1::workspace::WorkspaceRow =
        serde_json::from_value(row.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap()["legacy_review"], true);
}

fn assert_workspace_rows(app: &mut AppState, workspace: &str) {
    assert_marker(
        &app.workspace_get(&json!({"workspace_id":workspace}))
            .unwrap(),
    );
    let list = app.workspace_list(&json!({})).unwrap();
    assert_marker(
        list["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["workspace_id"] == workspace)
            .unwrap(),
    );
}

fn assert_projected_rows(app: &AppState, workspace: &str) {
    let view = app.read_view.load();
    assert_marker(&view.get("workspace.get", workspace).unwrap());
    let list = view.get("workspace.list", "").unwrap();
    assert_marker(
        list["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["workspace_id"] == workspace)
            .unwrap(),
    );
}

#[test]
fn legacy_workspace_marker_is_authoritative_on_cold_get_list_and_restart() {
    let (home, repository, mut app, project, workspace, _) = seeded();
    assert_workspace_rows(&mut app, &workspace);
    drop(app);
    let mut app = super::super::project_agent::rooted(home.path())
        .with_task_store(home.path().join("store"))
        .unwrap();
    let restored =
        super::super::project_agent::added_project(&mut app, &repository.path().join("repo"));
    assert_eq!(restored, project);
    app.initialize_snapshot_projection();
    assert_projected_rows(&app, &workspace);
    assert_workspace_rows(&mut app, &workspace);
}

#[test]
fn legacy_workspace_marker_survives_projection_completion_and_ordinary_writes() {
    let (_home, _repository, mut app, _, workspace, task) = seeded();
    app.initialize_snapshot_projection();
    assert_projected_rows(&app, &workspace);
    review_call(
        &mut app,
        "tasks.review.complete",
        json!({"task_id":task["id"],"expected_version":1,"description":"Close legacy review"}),
    );
    review_call(
        &mut app,
        "tasks.comment",
        json!({"task_id":task["id"],"body":"Ordinary task discussion remains open"}),
    );
    review_call(
        &mut app,
        "workspace.set_locked",
        json!({"workspace_id":workspace,"locked":true}),
    );
    assert_projected_rows(&app, &workspace);
    let refreshed = app.refresh_snapshot_observations().load();
    app.install_workspace_review_refresh(refreshed);
    app.publish_all_snapshot_projection();
    assert_projected_rows(&app, &workspace);
    assert_workspace_rows(&mut app, &workspace);
}

#[test]
fn legacy_workspace_marker_omits_plain_and_pr_workspaces_and_fences_stale_absence() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracked(home.path());
    let workspace = workspace(&mut app, &project, "marker absence");
    assert!(app
        .workspace_get(&json!({"workspace_id":workspace}))
        .unwrap()
        .get("legacy_review")
        .is_none());
    let stale = app.workspace_review_reader().load(&workspace).unwrap();
    let task = filed(&mut app, &project, "Retained legacy");
    legacy_review(
        &mut app,
        json!({"task_id":task["id"],"workspace_id":workspace,"expected_version":0}),
    );
    app.refresh_workspace_review_observation(&workspace);
    app.install_workspace_review_observation(&workspace, stale);
    let mut observed = json!({});
    app.snapshot_workspace_review(&workspace, &mut observed);
    assert_eq!(
        observed["legacy_review"], true,
        "stale absence replaced retained identity"
    );
    app.warm_snapshot_observations_for_test();
    app.publish_all_snapshot_projection();
    assert_projected_rows(&app, &workspace);
    let pr_workspace = super::workspace(&mut app, &project, "pr only marker");
    wire_actions::open(&mut app, &pr_workspace);
    let pr = app
        .workspace_get(&json!({"workspace_id":pr_workspace}))
        .unwrap();
    assert!(pr.get("active_review").is_some());
    assert!(pr.get("legacy_review").is_none(), "{pr}");
    assert_projected_rows(&app, &workspace);
}
