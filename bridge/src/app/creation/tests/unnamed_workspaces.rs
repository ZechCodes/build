use super::*;

fn admit_workspace(f: &Fixture, n: u128, name: Option<&str>) -> MutationRecord {
    let mut params = json!({"project_id":"proj-1","isolation":"worktree"});
    if let Some(name) = name {
        params["name"] = json!(name);
    }
    admit(
        &f.state,
        "workspace.create",
        params,
        WriteMetadata {
            operation_id: operation(n),
            after: None,
        },
    )
    .unwrap()
}

#[test]
fn two_blank_workspaces_get_distinct_labels_and_branches() {
    let f = Fixture::new();
    {
        let app = f.state.lock().unwrap();
        let home = &app.projects.get("proj-1").unwrap().home_path;
        let repo = git2::Repository::init(home).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("source")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@example.com").unwrap();
        repo.commit(Some("refs/heads/main"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
        repo.set_head("refs/heads/main").unwrap();
        drop(tree);
        drop(repo);
    }
    // Reload the project now that its configured source has Git metadata.
    let f = f.reopen();
    // Both are admitted before either executes, as concurrent client calls are.
    let a = admit_workspace(&f, 201, Some(""));
    let b = admit_workspace(&f, 202, Some(""));
    execute(&f.state, a).unwrap();
    execute(&f.state, b).unwrap();
    assert_eq!(
        f.record(&operation(201)).state,
        "succeeded",
        "{:?}",
        f.record(&operation(201)).outcome
    );
    assert_eq!(
        f.record(&operation(202)).state,
        "succeeded",
        "{:?}",
        f.record(&operation(202)).outcome
    );
    let app = f.state.lock().unwrap();
    let workspaces = app.workspaces.list(Some("proj-1"));
    assert_eq!(workspaces.len(), 2);
    assert!(workspaces.iter().all(|w| !w.name.trim().is_empty()));
    assert_ne!(workspaces[0].name, workspaces[1].name);
    let branches: Vec<_> = workspaces
        .iter()
        .map(|w| w.directories[0].branch.as_deref().unwrap())
        .collect();
    assert_ne!(branches[0], branches[1]);
}

#[test]
fn a_blank_workspace_waits_for_a_name_across_a_reload() {
    let f = Fixture::new();
    for (n, name) in [(203, None), (204, Some("   "))] {
        let admitted = admit_workspace(&f, n, name);
        execute(&f.state, admitted).unwrap();
    }
    let f = f.reopen();
    let app = f.state.lock().unwrap();
    let workspaces = app.workspaces.list(Some("proj-1"));
    assert_eq!(workspaces.len(), 2);
    for workspace in workspaces {
        assert_eq!(
            serde_json::to_value(workspace).unwrap()["waiting_for_name"],
            true
        );
        assert!(!workspace.name.trim().is_empty());
    }
}

#[test]
fn a_named_or_agent_created_workspace_never_waits_for_a_name() {
    let f = Fixture::new();
    let named = admit_workspace(&f, 205, Some("Release"));
    execute(&f.state, named).unwrap();
    let mut app = f.state.lock().unwrap();
    let ensured = app
        .dispatch(
            "project.ensure_conversation",
            &json!({"project_id":"proj-1"}),
        )
        .unwrap();
    let owner = ensured["run_id"].as_str().unwrap();
    let agent = app.project_pin(owner).unwrap().to_string();
    app.agent_action(
        owner,
        &agent,
        crate::mcp::BridgeAction::CreateWorkspace {
            name: "Agent work".into(),
            isolation: Some("worktree".into()),
        },
    )
    .unwrap();
    assert!(app
        .workspaces
        .list(Some("proj-1"))
        .iter()
        .any(|workspace| workspace.created_by_agent));
    for workspace in app.workspaces.list(Some("proj-1")) {
        assert_eq!(
            serde_json::to_value(workspace).unwrap()["waiting_for_name"],
            false
        );
    }
}

#[test]
fn older_and_adopted_workspace_records_are_already_named() {
    let f = Fixture::new();
    let mut app = f.state.lock().unwrap();
    let home = app.projects.get("proj-1").unwrap().home_path.clone();
    let adopted = app.workspaces.adopt_root(
        "proj-1",
        "adopted".into(),
        "Checkout".into(),
        home,
        "source".into(),
        false,
    );
    let mut old = serde_json::to_value(&adopted).unwrap();
    assert_eq!(old["waiting_for_name"], false);
    old.as_object_mut().unwrap().remove("waiting_for_name");
    old.as_object_mut().unwrap().remove("name_asked");
    let decoded: crate::workspace::Workspace = serde_json::from_value(old).unwrap();
    assert!(!decoded.waiting_for_name);
    assert!(!decoded.name_asked);
}
