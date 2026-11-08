use super::*;

#[test]
fn committed_identity_uses_captured_association_and_retains_missing_history() {
    let agent = "agent-committed";
    let mut context = TaskIdentityContext::default();
    context
        .workspaces
        .insert(PathBuf::from("/current"), "current-workspace".into());
    let mut retained = unknown_identity(agent);
    retained.workspace_id = Some("retained-workspace".into());
    retained.name = Some("Retained name".into());
    retained.available = true;
    let mut durable = unknown_identity(agent);
    durable.workspace_name = Some("Current checkout".into());
    for (path, expected) in [
        (Some("/current"), "current-workspace"),
        (Some("/absent"), "retained-workspace"),
        (None, "retained-workspace"),
    ] {
        let current = context.committed_identity(durable.clone(), path);
        let merged = retain_known_fields(&retained, current);
        let identity = context.identity(agent, &merged);
        assert_eq!(identity.workspace_id.as_deref(), Some(expected));
        assert_eq!(identity.name.as_deref(), Some("Retained name"));
        assert_eq!(identity.workspace_name.as_deref(), Some("Current checkout"));
        assert!(!identity.available);
    }
}

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
    app.projects.bind_entity("owner".into(), project.clone());
    // Freeze the pre-M0.2 scratch format from the native path, not its lossy key.
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project");
    let digest = crate::app::sha256_hex(root.as_os_str().as_encoded_bytes());
    let scratch = app.state_root.join("project-scratch").join(format!(
        "{}-{}",
        crate::app::projects::safe_mount_name(name),
        &digest[..12]
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    app.runs.get_mut("owner").unwrap().worktree.path = scratch.clone();
    let key = app.project(&project).unwrap().project_key.clone();
    assert!(app.is_project_snapshot_conversation_owner("owner"));
    assert!(app.capture_snapshot_ownership().is_project_owner("owner"));
    assert!(app.is_project_conversation_owner("owner"));
    assert!(app.is_project_scratch(&key, &scratch));
    assert_eq!(
        app.project_snapshot_scratch_path("owner"),
        Some(scratch.clone())
    );
    assert!(app.project_snapshot_scratch_paths().contains(&scratch));
    assert_eq!(
        app.project_conversation_run(&project).as_deref(),
        Some("owner")
    );
    assert_eq!(
        app.project_snapshot_conversation_run(&project).as_deref(),
        Some("owner")
    );
    assert_eq!(app.conversation_owner_ref("owner").unwrap().id, project);
    let conversation = app
        .project_ensure_conversation(&serde_json::json!({"project_id":project}))
        .unwrap();
    assert_eq!(
        conversation["entity_id"], "owner",
        "legacy scratch must not mint a second owner"
    );
    assert_eq!(app.runs.len(), 1);

    let sibling = crate::app::projects::test_support::register_sibling(&mut app, &project);
    let sibling_key = app.project(&sibling).unwrap().project_key.clone();
    let keyed_scratch = crate::app::projects::scratch_dir(&app.state_root, &sibling_key);
    let conversation = app
        .project_ensure_conversation(&serde_json::json!({"project_id":sibling}))
        .unwrap();
    let sibling_owner = conversation["entity_id"].as_str().unwrap();
    assert_eq!(app.runs[sibling_owner].worktree.path, keyed_scratch);
    assert_ne!(
        keyed_scratch, scratch,
        "a UUID sibling still hashes its opaque key"
    );

    // A second native path can have the same lossy spelling. Retained lookup
    // must keep project_by_key's first-match behavior in captured ownership too.
    let other_root = tmp
        .path()
        .join(std::ffi::OsString::from_vec(b"project-\xfe".to_vec()));
    std::fs::create_dir(&other_root).unwrap();
    let other = app.add_project(other_root, "main".into());
    assert_eq!(app.project(&other).unwrap().project_key, key);
    app.projects.retain_entity_path("owner".into(), key);
    app.projects.unbind_live_entity("owner");
    assert!(app.is_project_conversation_owner("owner"));
    assert!(app.capture_snapshot_ownership().is_project_owner("owner"));
}
