use super::*;

fn fixture(root: &Path) -> (AppState, String, String) {
    let harness =
        crate::harness::HarnessContext::resolved(root.join("mcp.sock"), root.into()).unwrap();
    let mut app = AppState::new_unrooted_configured(root.join("worktrees"), "main", true, harness);
    let project = app.add_project(root.into(), "main".into());
    let checkout = root.join("checkout");
    std::fs::create_dir(&checkout).unwrap();
    let mut run = crate::orchestrator::ActiveRun::workspace_conversation(
        crate::run::RunId::new("owner"),
        "checkout".into(),
        checkout.clone(),
        crate::models::ModelChoice::default(),
    );
    let agent = run
        .agents
        .add(
            "agent",
            crate::models::ModelChoice::default(),
            "2026-10-20T00:00:00Z",
        )
        .id
        .clone();
    app.runs.insert("owner".into(), run);
    app.projects.bind_entity("owner".into(), project.clone());
    app.workspaces.adopt_root(
        &project,
        "alias".into(),
        "zz-valid".into(),
        checkout,
        "source-1".into(),
        false,
    );
    (app, project, agent)
}

#[test]
fn task_location_skips_an_earlier_direct_owner_with_a_different_root() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, project, agent) = fixture(tmp.path());
    app.workspaces.adopt_root(
        &project,
        "owner".into(),
        "aa-wrong-root".into(),
        tmp.path().join("wrong"),
        "source-1".into(),
        false,
    );
    // Generic reverse ownership correctly selects the direct id first; task
    // association must continue to the later workspace at the actual root.
    assert_eq!(
        app.workspace_of_conversation("owner").as_deref(),
        Some("owner")
    );
    let context = app.task_identity_context();
    assert_eq!(context.live[&agent].workspace_id.as_deref(), Some("alias"));
    assert_eq!(
        context.live[&agent].workspace_name.as_deref(),
        Some("zz-valid")
    );
    assert!(context.live[&agent].available);
    app.session_registry.retire_agent_binding("owner", &agent);
    assert!(!app.task_identity_context().live[&agent].available);
}

#[test]
fn task_location_preserves_first_matching_status_and_excludes_project_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, project, agent) = fixture(tmp.path());
    let root = app.runs["owner"].worktree.path.clone();
    app.workspaces.adopt_root(
        &project,
        "earlier".into(),
        "aa-finished".into(),
        root,
        "source-1".into(),
        false,
    );
    let mut earlier = app.workspaces.get("earlier").unwrap().clone();
    earlier.status = crate::workspace::WorkspaceStatus::Finished;
    let records = app.workspaces.root().join(".legacy");
    std::fs::create_dir_all(&records).unwrap();
    std::fs::write(
        records.join("earlier"),
        serde_json::to_vec(&earlier).unwrap(),
    )
    .unwrap();
    app.workspaces.forget("earlier");
    app.workspaces.adopt_root(
        &project,
        earlier.id,
        earlier.name,
        earlier.root,
        "source-1".into(),
        false,
    );
    let context = app.task_identity_context();
    assert_eq!(
        context.live[&agent].workspace_id.as_deref(),
        Some("earlier")
    );
    assert!(!context.live[&agent].available);
    app.runs.get_mut("owner").unwrap().worktree.path = tmp.path().into();
    app.workspaces.adopt_root(
        &project,
        "owner".into(),
        "repo".into(),
        tmp.path().into(),
        "source-1".into(),
        false,
    );
    let context = app.task_identity_context();
    assert!(context.live[&agent].workspace_id.is_none());
    assert!(!context.live[&agent].available);
}

#[test]
fn parked_capture_stops_at_the_first_matching_path_even_without_an_id() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, project, _) = fixture(tmp.path());
    app.projects.remove(&project);
    app.projects
        .retain_entity_path("owner".into(), tmp.path().display().to_string());
    app.projects.park(serde_json::json!({"path": tmp.path()}));
    app.projects
        .park(serde_json::json!({"id": project, "path": tmp.path()}));
    assert!(app.compaction_project_of("owner").is_none());
    assert!(app
        .capture_snapshot_ownership()
        .compaction_workspace("owner")
        .is_none());
}

#[cfg(unix)]
#[test]
fn captured_project_owner_keeps_non_utf8_project_path_bytes() {
    use std::os::unix::ffi::OsStringExt;
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, _, _) = fixture(tmp.path());
    let root = tmp
        .path()
        .join(std::ffi::OsString::from_vec(b"project-\xff".to_vec()));
    std::fs::create_dir(&root).unwrap();
    let project = app.add_project(root.clone(), "main".into());
    app.projects.bind_entity("owner".into(), project);
    let scratch = crate::app::projects::scratch_dir(&app.state_root, &root);
    std::fs::create_dir_all(&scratch).unwrap();
    app.runs.get_mut("owner").unwrap().worktree.path = scratch;
    assert!(app.is_project_snapshot_conversation_owner("owner"));
    assert!(app.capture_snapshot_ownership().is_project_owner("owner"));
}
