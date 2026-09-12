use super::*;

fn init_request(workspace: &Value, target: &str) -> Value {
    json!({
        "workspace_id": workspace["workspace_id"],
        "source_id": "source-1",
        "target": target,
    })
}

fn dispatch_init(state: &mut AppState, params: &Value) -> DeferredWork {
    let (dispatched, deferred) = state.dispatch_deferring("workspace.init_git", params);
    assert_eq!(dispatched.unwrap(), Value::Null);
    deferred.expect("git initialization defers its filesystem work")
}

#[test]
fn deleting_a_source_after_dispatch_does_not_recreate_or_initialize_it() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let mut state = app(tmp.path());
    let added = state.handle(req("project.add", json!({"path": source})));
    let workspace = create_workspace(
        &mut state,
        added["result"]["project_id"].as_str().unwrap(),
        "work",
    );
    let params = init_request(&workspace, "source");
    let deferred = dispatch_init(&mut state, &params);
    std::fs::remove_dir(&source).unwrap();

    let result = state
        .apply_deferred("workspace.init_git", &params, deferred.run())
        .unwrap();

    assert_eq!(result["results"][0]["status"], "failed", "{result:?}");
    assert!(
        !source.exists(),
        "the drained request recreated its old path"
    );
}

#[test]
fn replacing_a_source_after_dispatch_does_not_initialize_the_replacement() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let displaced = tmp.path().join("displaced");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("original.txt"), "original\n").unwrap();
    let mut state = app(tmp.path());
    let added = state.handle(req("project.add", json!({"path": source})));
    let workspace = create_workspace(
        &mut state,
        added["result"]["project_id"].as_str().unwrap(),
        "work",
    );
    let params = init_request(&workspace, "source");
    let deferred = dispatch_init(&mut state, &params);
    std::fs::rename(&source, &displaced).unwrap();
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("replacement.txt"), "replacement\n").unwrap();

    let result = state
        .apply_deferred("workspace.init_git", &params, deferred.run())
        .unwrap();

    assert_eq!(result["results"][0]["status"], "failed", "{result:?}");
    assert!(!source.join(".git").exists());
    assert!(!displaced.join(".git").exists());
}

#[test]
fn removing_workspace_and_project_before_settlement_keeps_both_outcomes() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let mut state = app(tmp.path());
    let added = state.handle(req("project.add", json!({"path": source})));
    let workspace = create_workspace(
        &mut state,
        added["result"]["project_id"].as_str().unwrap(),
        "work",
    );
    let params = init_request(&workspace, "both");
    let deferred = dispatch_init(&mut state, &params);
    let done = deferred.run();
    std::fs::remove_dir_all(workspace["root"].as_str().unwrap()).unwrap();
    state.workspaces.reload().unwrap();
    state.clear_projects_for_test();

    let result = state
        .apply_deferred("workspace.init_git", &params, done)
        .unwrap();

    let outcomes = result["results"].as_array().unwrap();
    assert_eq!(outcomes.len(), 2, "{result:?}");
    assert_eq!(outcomes[0]["target"], "workspace");
    assert_eq!(outcomes[0]["status"], "failed");
    assert_eq!(outcomes[1]["target"], "source");
    assert_eq!(outcomes[1]["status"], "failed");
    assert_eq!(result["workspace"], Value::Null);
    assert_eq!(result["source"], Value::Null);
}

#[test]
fn source_config_persistence_failure_is_retryable_after_git_was_created() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let config = tmp.path().join("config.json");
    std::fs::create_dir(&source).unwrap();
    let mut state = app(tmp.path()).with_config(&config).unwrap();
    let added = state.handle(req("project.add", json!({"path": source})));
    let workspace = create_workspace(
        &mut state,
        added["result"]["project_id"].as_str().unwrap(),
        "work",
    );
    let params = init_request(&workspace, "source");
    state.config_persist_failure = Some(ConfigPersistStep::Write);

    let failed = state.handle(req("workspace.init_git", params.clone()));

    assert_eq!(failed["ok"], true, "{failed:?}");
    assert_eq!(failed["result"]["results"][0]["status"], "failed");
    assert!(source.join(".git").is_dir());
    assert_eq!(
        failed["result"]["source"]["is_git"], true,
        "the in-memory registry reflects Git even when its config write failed"
    );

    state.config_persist_failure = None;
    let retried = state.handle(req("workspace.init_git", params));
    assert_eq!(retried["ok"], true, "{retried:?}");
    assert_eq!(
        retried["result"]["results"][0]["status"],
        "already_initialized"
    );
    assert_eq!(retried["result"]["source"]["is_git"], true);
}

#[test]
fn both_targets_at_the_same_canonical_path_share_one_initialization_outcome() {
    let tmp = tempfile::tempdir().unwrap();
    let source = init_repo_named(tmp.path(), "source");
    let before = git2::Repository::open(&source)
        .unwrap()
        .head()
        .unwrap()
        .target();
    let mut state = app(tmp.path());
    let added = state.handle(req("project.add", json!({"path": source})));
    let project_id = added["result"]["project_id"].as_str().unwrap();
    let workspace = state.workspaces.adopt_root(
        project_id,
        "adopted".to_string(),
        "adopted".to_string(),
        source.clone(),
        "source-1".to_string(),
        true,
    );
    let params = json!({
        "workspace_id": workspace.id,
        "source_id": "source-1",
        "target": "both",
    });

    let initialized = state.handle(req("workspace.init_git", params));

    assert_eq!(initialized["ok"], true, "{initialized:?}");
    assert_eq!(
        initialized["result"]["results"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        initialized["result"]["results"][0]["status"],
        "already_initialized"
    );
    assert_eq!(
        initialized["result"]["results"][1]["status"],
        "already_initialized"
    );
    assert_eq!(
        git2::Repository::open(&source)
            .unwrap()
            .head()
            .unwrap()
            .target(),
        before
    );
}

#[test]
fn repeated_both_keeps_the_sources_configured_base_when_head_differs() {
    let tmp = tempfile::tempdir().unwrap();
    let source = init_repo_named(tmp.path(), "source");
    git_in(&source, &["branch", "release"]);
    git_in(&source, &["checkout", "-b", "develop"]);
    let mut state = app(tmp.path());
    let added = state.handle(req(
        "project.add",
        json!({"path": source, "base_branch": "release"}),
    ));
    let project_id = added["result"]["project_id"].as_str().unwrap();
    let listed = state.handle(req("workspace.list", json!({"project_id": project_id})));
    let workspace = &listed["result"]["workspaces"][0];
    let workspace_only = init_request(workspace, "workspace");
    let params = init_request(workspace, "both");

    let first = state.handle(req("workspace.init_git", workspace_only));
    let second = state.handle(req("workspace.init_git", params));

    assert_eq!(first["result"]["source"]["base_branch"], "release");
    assert_eq!(second["result"]["source"]["base_branch"], "release");
    assert_eq!(
        second["result"]["workspace"]["directories"][0]["branch"],
        "develop"
    );
}
