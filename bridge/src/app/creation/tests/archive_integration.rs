//! New creation verbs retain archived project authority across replay and recovery.
use super::*;

fn prepared(method: &str, n: u128) -> (Fixture, String, Value) {
    let f = Fixture::new();
    let id = operation(n);
    let params = if method == "environment.create" {
        json!({"name":"Archived environment", "sources":[{"path":f._root.path().join("home")}],
            "project":{"name":"Retained project", "workspace":{}}})
    } else {
        f.params()
    };
    let record = admit(
        &f.state,
        method,
        params.clone(),
        WriteMetadata {
            operation_id: id.clone(),
            after: None,
        },
    )
    .unwrap();
    f.store.writer().claim_mutation(PRINCIPAL, &id).unwrap();
    let evidence = project_step(&f.state, &record).unwrap();
    let project = evidence["project_id"].as_str().unwrap();
    assert_eq!(project, "proj-2");
    // Legacy persisted state can predate the archive admission blocker.
    let mut app = f.state.lock().unwrap();
    app.projects.at_mut(1).archived_at = Some(1);
    app.persist();
    drop(app);
    (f, id, params)
}

fn refused_retry(method: &str, n: u128) {
    let (f, id, params) = prepared(method, n);
    let before = f.record(&id);
    let config = std::fs::read(f._root.path().join("config.json")).unwrap();
    let error = admit(
        &f.state,
        method,
        params,
        WriteMetadata {
            operation_id: id.clone(),
            after: None,
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), "project_archived");
    assert_eq!(f.record(&id).execution, before.execution);
    assert_eq!(f.record(&id).state, before.state);
    assert_eq!(
        std::fs::read(f._root.path().join("config.json")).unwrap(),
        config
    );
}

#[test]
fn archive_merge_environment_create_retry_refuses_archived_project() {
    refused_retry("environment.create", 1059001);
}

#[test]
fn archive_merge_environment_create_project_retry_refuses_archived_project() {
    refused_retry("environment.create_project", 1059002);
}

fn refused_execution(method: &str, n: u128) {
    let (f, id, _) = prepared(method, n);
    let record = f.record(&id);
    let before = record.execution.clone();
    let error = execute(&f.state, record).unwrap_err();
    assert_eq!(ApiError::classify(error).code(), "project_archived");
    assert_eq!(f.record(&id).execution, before);
    assert!(f
        .state
        .lock()
        .unwrap()
        .workspaces
        .list(Some("proj-2"))
        .is_empty());
}

#[test]
fn archive_merge_environment_create_execution_refuses_archived_project() {
    refused_execution("environment.create", 1059003);
}

#[test]
fn archive_merge_environment_create_project_execution_refuses_archived_project() {
    refused_execution("environment.create_project", 1059004);
}

#[test]
fn archive_merge_creation_recovery_retains_archived_work_and_resumes_after_restore() {
    let (f, id, _) = prepared("environment.create_project", 1059005);
    let f = f.reopen();
    f.store.writer().recover_mutations().unwrap();
    let before = f.record(&id);
    let other_id = operation(1059006);
    let other = admit(&f.state, "environment.create", json!({"sources":[{"path":f._root.path().join("home")}], "project":{"name":"Active project", "workspace":{}}}), WriteMetadata {operation_id:other_id.clone(), after:None}).unwrap();
    let mut retries = service::RetryBackoff::default();
    service::reconcile_pending(
        &f.state,
        &f.store.writer(),
        &mut retries,
        std::time::Instant::now(),
    )
    .unwrap();
    assert_eq!(f.record(&id).execution, before.execution);
    assert_eq!(f.record(&id).state, before.state);
    assert_eq!(f.record(&other.operation_id).state, "succeeded");
    let restored = f.state.lock().unwrap().project_restore("proj-2").unwrap();
    assert!(restored["project"]["archived_at"].is_null());
    service::reconcile_pending(
        &f.state,
        &f.store.writer(),
        &mut retries,
        std::time::Instant::now(),
    )
    .unwrap();
    assert_eq!(f.record(&id).state, "succeeded", "{:?}", f.record(&id));
    service::reconcile_pending(
        &f.state,
        &f.store.writer(),
        &mut retries,
        std::time::Instant::now(),
    )
    .unwrap();
    assert_eq!(
        f.state
            .lock()
            .unwrap()
            .workspaces
            .list(Some("proj-2"))
            .len(),
        1
    );
}

#[test]
fn archive_merge_new_project_can_share_an_environment_with_archived_project() {
    let f = Fixture::new();
    assert!(f.state.lock().unwrap().project_archive("proj-1").is_ok());
    let before = f
        .state
        .lock()
        .unwrap()
        .project("proj-1")
        .unwrap()
        .project_key
        .clone();
    let id = operation(1059007);
    f.admit(&id);
    f.run(&id);
    assert_eq!(f.record(&id).state, "succeeded");
    let app = f.state.lock().unwrap();
    assert_eq!(app.project("proj-1").unwrap().project_key, before);
    assert!(app.project("proj-1").unwrap().archived_at.is_some());
    assert_eq!(app.workspaces.list(Some("proj-1")).len(), 0);
    assert_eq!(app.workspaces.list(Some("proj-2")).len(), 1);
}

#[test]
fn archive_merge_environment_discard_keeps_archived_project_and_environment() {
    let (f, id, _) = prepared("environment.create", 1059008);
    let record = f.record(&id);
    let environment = record.execution["environment_id"].as_str().unwrap();
    f.store.writer().finish_creation(PRINCIPAL, &id, json!({"state":"failed", "effect":"partial", "error":{"code":"test", "message":"interrupted before workspace", "retryable":false}})).unwrap();
    let before = std::fs::read(f._root.path().join("config.json")).unwrap();
    let mut app = f.state.lock().unwrap();
    assert_eq!(
        app.environment_discard_unused(
            &json!({"environment_id":environment, "creation_operation_id":id})
        )
        .unwrap_err(),
        "environment has projects"
    );
    assert!(app.project("proj-2").unwrap().archived_at.is_some());
    assert!(app.projects.environment(environment).is_some());
    assert_eq!(
        std::fs::read(f._root.path().join("config.json")).unwrap(),
        before
    );
}

fn temporarily_move_project_home(f: &Fixture) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = f
        .state
        .lock()
        .unwrap()
        .projects
        .get("proj-1")
        .unwrap()
        .home_path
        .clone();
    let aside = home.with_extension("aside");
    std::fs::rename(&home, &aside).unwrap();
    (home, aside)
}

fn admit_fresh_workspace_retry(
    f: &Fixture,
    project: &str,
    prior: &str,
    n: u128,
) -> Result<MutationRecord, ApiError> {
    admit(
        &f.state,
        "workspace.create",
        json!({"project_id":project,"name":"retry","continues":prior}),
        WriteMetadata {
            operation_id: operation(n),
            after: None,
        },
    )
}

fn assert_archived_retry_refused_until_restore(f: &Fixture, project: &str, prior: &str, n: u128) {
    let failed = f.record(prior);
    f.state.lock().unwrap().project_archive(project).unwrap();
    let err = admit_fresh_workspace_retry(f, project, prior, n).unwrap_err();
    assert_eq!(err.code(), "project_archived");
    assert!(f
        .store
        .writer()
        .creation_record(PRINCIPAL, &operation(n))
        .unwrap()
        .is_none());
    assert_eq!(f.record(prior).revision, failed.revision);
    assert!(f
        .state
        .lock()
        .unwrap()
        .workspaces
        .list(Some(project))
        .is_empty());
    f.state.lock().unwrap().project_restore(project).unwrap();
    admit_fresh_workspace_retry(f, project, prior, n).unwrap();
    f.run(&operation(n));
    let done = f.record(&operation(n));
    assert_eq!(done.state, "succeeded", "{done:?}");
    assert_eq!(f.record(prior).state, "failed");
    let app = f.state.lock().unwrap();
    let rows = app.workspaces.list(Some(project));
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].status, crate::workspace::WorkspaceStatus::Ready);
}

#[test]
fn failed_workspace_creation_fresh_retry_refused_until_project_restored() {
    let f = Fixture::new();
    let id = operation(1079001);
    admit(
        &f.state,
        "workspace.create",
        json!({"project_id":"proj-1","name":"guarded"}),
        WriteMetadata {
            operation_id: id.clone(),
            after: None,
        },
    )
    .unwrap();
    let (home, aside) = temporarily_move_project_home(&f);
    f.run(&id);
    std::fs::rename(&aside, &home).unwrap();
    let failed = f.record(&id);
    assert_eq!(failed.state, "failed", "{failed:?}");
    assert_archived_retry_refused_until_restore(&f, "proj-1", &id, 1079002);
}

#[test]
fn failed_first_workspace_fresh_retry_refused_until_project_restored() {
    let f = Fixture::new();
    let id = operation(1079003);
    f.admit(&id);
    let (home, aside) = temporarily_move_project_home(&f);
    f.run(&id);
    std::fs::rename(&aside, &home).unwrap();
    let failed = f.record(&id);
    assert_eq!(failed.state, "failed", "{failed:?}");
    assert_eq!(failed.execution["project_id"], "proj-2");
    assert_archived_retry_refused_until_restore(&f, "proj-2", &id, 1079004);
}
