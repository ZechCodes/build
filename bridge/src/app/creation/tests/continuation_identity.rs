use super::admission_recovery::register_legacy;
use super::*;

fn workspace_attempt(
    f: &Fixture,
    id: &str,
    prior: Option<&str>,
) -> Result<MutationRecord, ApiError> {
    let mut params = json!({"project_id":"proj-2","name":"continued"});
    if let Some(prior) = prior {
        params["continues"] = json!(prior);
    }
    admit(
        &f.state,
        "workspace.create",
        params,
        WriteMetadata {
            operation_id: id.into(),
            after: None,
        },
    )
}

fn failed_environment_attempt() -> (Fixture, String) {
    let f = Fixture::new();
    let prior = operation(140);
    f.admit(&prior);
    f.state.lock().unwrap().config_persist_failure =
        Some(super::super::super::ConfigPersistStep::Write);
    f.run(&prior);
    let record = f.record(&prior);
    assert_eq!(record.state, "failed");
    assert_eq!(record.execution["project_id"], "proj-2");
    assert_eq!(record.outcome.unwrap()["effect"], "unapplied");
    (f.reopen(), prior)
}

// Restore an older project config while retaining the intact operation journal,
// then register a different legacy folder at the same numeric ID.
fn workspace_attempt_with_reused_id(fail: bool) -> (Fixture, String) {
    let f = Fixture::new();
    let config = f._root.path().join("config.json");
    let snapshot = std::fs::read(&config).unwrap();
    let old_home = f._root.path().join("old-project");
    std::fs::create_dir(&old_home).unwrap();
    std::fs::write(old_home.join("source"), b"old source").unwrap();
    {
        let mut app = f.state.lock().unwrap();
        assert_eq!(app.add_project(old_home.clone(), "main".into()), "proj-2");
        app.persist();
    }
    let prior = operation(141);
    workspace_attempt(&f, &prior, None).unwrap();
    if fail {
        std::fs::remove_dir_all(old_home).unwrap();
        f.run(&prior);
        assert_eq!(f.record(&prior).state, "failed");
        assert_eq!(f.record(&prior).outcome.unwrap()["effect"], "unapplied");
    }
    std::fs::write(config, snapshot).unwrap();
    let f = f.reopen();
    register_legacy(&f.state, f._root.path());
    assert_ne!(
        f.record(&prior).execution["project_key"],
        f.state
            .lock()
            .unwrap()
            .projects
            .get("proj-2")
            .unwrap()
            .project_key
    );
    (f, prior)
}

fn legacy_snapshot(f: &Fixture) -> Value {
    let app = f.state.lock().unwrap();
    app.project_json(app.projects.get("proj-2").unwrap())
}

fn assert_continuation_refused(f: &Fixture, prior: &str) {
    let marker = f._root.path().join("legacy/owned");
    std::fs::write(&marker, b"legacy-owned").unwrap();
    let legacy = legacy_snapshot(f);
    let id = operation(142);
    let result = workspace_attempt(f, &id, Some(prior));
    assert!(
        result.is_err(),
        "unrelated continuation admitted: {result:?}"
    );
    assert!(f
        .store
        .writer()
        .creation_record(PRINCIPAL, &id)
        .unwrap()
        .is_none());
    assert_eq!(legacy_snapshot(f), legacy);
    assert!(f
        .state
        .lock()
        .unwrap()
        .workspaces
        .list(Some("proj-2"))
        .is_empty());
    assert_eq!(std::fs::read(marker).unwrap(), b"legacy-owned");
}

#[test]
fn project_continuation_refuses_reused_environment_attempt_id() {
    let (f, prior) = failed_environment_attempt();
    register_legacy(&f.state, f._root.path());
    assert_continuation_refused(&f, &prior);
}

#[test]
fn project_continuation_refuses_reused_same_scope_id() {
    let (f, prior) = workspace_attempt_with_reused_id(true);
    assert_eq!(f.record(&prior).scope, json!({"project_id":"proj-2"}));
    assert_continuation_refused(&f, &prior);
}

#[test]
fn workspace_replay_refuses_changed_project_key_before_filesystem_effects() {
    let (f, prior) = workspace_attempt_with_reused_id(false);
    let record = f.record(&prior);
    let root = std::path::PathBuf::from(record.execution["workspace"]["root"].as_str().unwrap());
    assert!(!root.exists());
    let legacy = legacy_snapshot(&f);
    let marker = f._root.path().join("legacy/owned");
    std::fs::write(&marker, b"legacy-owned").unwrap();
    f.run(&prior);
    assert!(
        !root.exists(),
        "replay must refuse before creating its workspace root"
    );
    assert!(f.state.lock().unwrap().workspaces.list(None).is_empty());
    assert_eq!(legacy_snapshot(&f), legacy);
    assert_eq!(std::fs::read(marker).unwrap(), b"legacy-owned");
    let record = f.record(&prior);
    // Absence of this root alone is not proof that no external effect began.
    assert_eq!(record.state, "unknown", "{record:?}");
    assert!(record.outcome.is_none());
}

#[test]
fn environment_continuation_keeps_scope_chaining_after_unapplied_attempt() {
    let (f, prior) = failed_environment_attempt();
    let id = operation(143);
    let mut params = f.params();
    params["continues"] = json!(prior);
    admit(
        &f.state,
        "environment.create_project",
        params,
        WriteMetadata {
            operation_id: id.clone(),
            after: None,
        },
    )
    .unwrap();
    f.run(&id);
    let record = f.record(&id);
    assert_eq!(record.state, "succeeded");
    assert_ne!(
        record.execution["project_key"],
        f.record(&prior).execution["project_key"]
    );
    assert_eq!(f.state.lock().unwrap().projects.len(), 2);
}
