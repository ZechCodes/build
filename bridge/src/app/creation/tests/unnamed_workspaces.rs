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
    let f = git_source_fixture();
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
    old["name_asked"] = json!(true);
    let decoded: crate::workspace::Workspace = serde_json::from_value(old).unwrap();
    assert!(!decoded.waiting_for_name);
    assert!(serde_json::to_value(decoded)
        .unwrap()
        .get("name_asked")
        .is_none());
}

fn git_source_fixture() -> Fixture {
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
    f.reopen()
}

#[test]
fn same_project_blank_workspace_execution_overlaps_its_queued_retry() {
    let f = git_source_fixture();
    let first = admit_workspace(&f, 206, None);
    let second = admit_workspace(&f, 207, Some(" "));
    let (release_first, wait_release) = std::sync::mpsc::channel();
    let (started, wait_started) = std::sync::mpsc::channel();
    let (finished, wait_finished) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let state = Arc::clone(&f.state);
        let first_thread = scope.spawn(move || {
            let _observer = observe_boundary(move |boundary| {
                if boundary == Boundary::WorkspaceStarted {
                    started.send(()).unwrap();
                    wait_release
                        .recv_timeout(std::time::Duration::from_secs(10))
                        .unwrap();
                }
            });
            execute(&state, first).unwrap();
            finished.send(()).unwrap();
        });
        let state = Arc::clone(&f.state);
        let second_thread = scope.spawn(move || {
            wait_started
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            // Enter the real executor while the first is provisionally working
            // on the same source. Project owner ordering leaves this accepted.
            execute(&state, second).unwrap();
            release_first.send(()).unwrap();
            wait_finished
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            let queued = state
                .lock()
                .unwrap()
                .store
                .as_ref()
                .unwrap()
                .writer()
                .creation_record(PRINCIPAL, &operation(207))
                .unwrap()
                .unwrap();
            execute(&state, queued).unwrap();
        });
        first_thread.join().unwrap();
        second_thread.join().unwrap();
    });
    for n in [206, 207] {
        assert_eq!(
            f.record(&operation(n)).state,
            "succeeded",
            "{:?}",
            f.record(&operation(n))
        );
    }
    let app = f.state.lock().unwrap();
    let workspaces = app.workspaces.list(Some("proj-1"));
    assert_eq!(workspaces.len(), 2);
    assert_ne!(workspaces[0].name, workspaces[1].name);
    assert!(workspaces
        .iter()
        .all(|workspace| !workspace.name.trim().is_empty()));
    assert_ne!(
        workspaces[0].directories[0].branch,
        workspaces[1].directories[0].branch
    );
    assert!(workspaces
        .iter()
        .all(|workspace| workspace.directories[0].branch.is_some()));
}

#[test]
fn two_blank_workspace_creates_overlapping_on_one_source_both_succeed() {
    let f = git_source_fixture();
    let sibling = f
        .state
        .lock()
        .unwrap()
        .dispatch(
            "project.create",
            &json!({"environment_id":"env-1","name":"Shared source sibling","sources":[]}),
        )
        .unwrap();
    let sibling = sibling["project_id"].as_str().unwrap();
    let first = admit_workspace(&f, 208, None);
    let second = admit(
        &f.state,
        "workspace.create",
        json!({"project_id":sibling,"name":" ","isolation":"worktree"}),
        WriteMetadata {
            operation_id: operation(209),
            after: None,
        },
    )
    .unwrap();
    let (started, wait_started) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let mut releases = Vec::new();
        let threads: Vec<_> = [first, second]
            .into_iter()
            .map(|record| {
                let state = Arc::clone(&f.state);
                let started = started.clone();
                let (release, wait_release) = std::sync::mpsc::channel();
                releases.push(release);
                scope.spawn(move || {
                    let _observer = observe_boundary(move |boundary| {
                        if boundary == Boundary::WorkspaceStarted {
                            started.send(()).unwrap();
                            wait_release
                                .recv_timeout(std::time::Duration::from_secs(10))
                                .unwrap();
                        }
                    });
                    execute(&state, record).unwrap();
                })
            })
            .collect();
        // Both independent project workers have entered provisioning of one
        // physical source before either may materialize its checkout.
        for _ in 0..2 {
            wait_started
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
        }
        for release in releases {
            release.send(()).unwrap();
        }
        for thread in threads {
            thread.join().unwrap();
        }
    });
    for n in [208, 209] {
        assert_eq!(
            f.record(&operation(n)).state,
            "succeeded",
            "{:?}",
            f.record(&operation(n))
        );
    }
    let app = f.state.lock().unwrap();
    let workspaces = app.workspaces.list(None);
    assert_eq!(workspaces.len(), 2);
    assert_eq!(
        workspaces[0].directories[0].source_path, workspaces[1].directories[0].source_path,
        "both executions materialize the same source"
    );
    assert_ne!(workspaces[0].name, workspaces[1].name);
    assert_ne!(
        workspaces[0].directories[0].branch,
        workspaces[1].directories[0].branch
    );
    assert!(workspaces
        .iter()
        .all(|workspace| workspace.directories[0].branch.is_some()));
}
