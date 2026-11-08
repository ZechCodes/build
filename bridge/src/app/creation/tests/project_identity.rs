use super::admission_recovery::{interrupted_reallocated_creation, register_legacy};
use super::*;

fn assert_legacy_fence(f: &Fixture) {
    let mut app = f.state.lock().unwrap();
    let real = app.workspace_create(&json!({"project_id":"proj-3","name":"conflict"}));
    assert!(real.is_err(), "landed project must stay fenced: {real:?}");
    let unrelated = app.workspace_create(&json!({"project_id":"proj-2","name":"unrelated"}));
    assert!(
        unrelated.is_ok(),
        "legacy project must stay usable: {unrelated:?}"
    );
    assert!(app.workspaces.list(Some("proj-3")).is_empty());
}

fn workspace_attempt(f: &Fixture, project_id: &str, id: u128) -> MutationRecord {
    admit(
        &f.state,
        "workspace.create",
        json!({"project_id":project_id,"name":"competing"}),
        WriteMetadata {
            operation_id: operation(id),
            after: None,
        },
    )
    .unwrap()
}

fn assert_claim_fence(f: &Fixture) {
    let real = workspace_attempt(f, "proj-3", 120);
    let unrelated = workspace_attempt(f, "proj-2", 121);
    assert!(
        !f.store
            .writer()
            .claim_mutation(PRINCIPAL, &real.operation_id)
            .unwrap(),
        "landed project must stay fenced"
    );
    assert!(
        f.store
            .writer()
            .claim_mutation(PRINCIPAL, &unrelated.operation_id)
            .unwrap(),
        "legacy project must stay usable"
    );
}

#[test]
fn landed_project_blocks_legacy_provisioning_before_checkpoint() {
    let (f, _) = interrupted_reallocated_creation();
    assert_legacy_fence(&f);
}

#[test]
fn reopened_landed_project_blocks_legacy_provisioning_before_reconciliation() {
    let (f, _) = interrupted_reallocated_creation();
    let f = f.reopen();
    f.store.writer().recover_mutations().unwrap();
    assert_legacy_fence(&f);
}

#[test]
fn landed_project_blocks_project_claim_before_checkpoint() {
    let (f, _) = interrupted_reallocated_creation();
    assert_claim_fence(&f);
}

#[test]
fn reopened_landed_project_blocks_project_claim_before_reconciliation() {
    let (f, _) = interrupted_reallocated_creation();
    let f = f.reopen();
    f.store.writer().recover_mutations().unwrap();
    assert_claim_fence(&f);
}

fn failed_post_project_checkpoint() -> (Fixture, String) {
    let f = Fixture::new();
    let id = operation(122);
    f.admit(&id);
    let state = f.state.clone();
    let root = f._root.path().to_path_buf();
    let store = f.store.clone();
    let _observer = observe_boundary(move |boundary| match boundary {
        Boundary::ProjectCheckpointed => register_legacy(&state, &root),
        Boundary::ProjectPersisted => store.fail_next_write(),
        _ => {}
    });
    f.run(&id);
    let record = f.record(&id);
    assert_eq!(record.state, "failed");
    assert_eq!(record.outcome.as_ref().unwrap()["effect"], "partial");
    assert_eq!(
        record.outcome.as_ref().unwrap()["error"]["details"]["project"]["project_id"],
        "proj-3"
    );
    assert_eq!(record.execution["project_id"], "proj-3");
    assert_eq!(record.execution["steps"][0]["step"], "project");
    assert_eq!(record.execution["steps"][0]["state"], "done");
    (f, id)
}

#[test]
fn post_project_checkpoint_failure_allows_real_project_continuation() {
    let (f, prior) = failed_post_project_checkpoint();
    let id = operation(123);
    admit(
        &f.state,
        "workspace.create",
        json!({"project_id":"proj-3","name":"continued","continues":prior}),
        WriteMetadata {
            operation_id: id.clone(),
            after: None,
        },
    )
    .unwrap();
    f.run(&id);
    assert_eq!(f.record(&id).state, "succeeded");
    assert_eq!(
        f.state
            .lock()
            .unwrap()
            .workspaces
            .list(Some("proj-3"))
            .len(),
        1
    );
}

#[test]
fn post_project_checkpoint_failure_allows_real_project_discard() {
    let (f, id) = failed_post_project_checkpoint();
    let mut app = f.state.lock().unwrap();
    app.project_discard_unused(&json!({"project_id":"proj-3","creation_operation_id":id}))
        .unwrap();
    assert!(app.projects.get("proj-3").is_none());
    assert!(app.projects.get("proj-2").is_some());
}

#[test]
fn final_identity_checkpoint_failure_prevents_project_persist() {
    let f = Fixture::new();
    let id = operation(124);
    f.admit(&id);
    let state = f.state.clone();
    let root = f._root.path().to_path_buf();
    let store = f.store.clone();
    let _observer = observe_boundary(move |boundary| {
        if boundary == Boundary::ProjectCheckpointed {
            register_legacy(&state, &root);
            store.fail_next_write();
        }
    });
    f.run(&id);
    assert_eq!(f.state.lock().unwrap().projects.len(), 2);
    assert_eq!(f.record(&id).outcome.unwrap()["effect"], "unapplied");
}
