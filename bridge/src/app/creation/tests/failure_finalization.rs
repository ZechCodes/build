use super::*;
use crate::workspace::{Workspace, WorkspaceStatus};

fn workspace_attempt(f: &Fixture, id: &str) -> MutationRecord {
    admit(
        &f.state,
        "workspace.create",
        json!({"project_id":"proj-1","name":"guarded"}),
        WriteMetadata {
            operation_id: id.into(),
            after: None,
        },
    )
    .unwrap()
}

fn unknown_with_residue() -> (Fixture, String, Workspace) {
    let f = Fixture::new();
    let id = operation(160);
    let record = workspace_attempt(&f, &id);
    assert!(f.store.writer().claim_mutation(PRINCIPAL, &id).unwrap());
    let mut workspace: Workspace =
        serde_json::from_value(record.execution["workspace"].clone()).unwrap();
    {
        let mut app = f.state.lock().unwrap();
        app.workspaces.claim_creation_workspace(&workspace).unwrap();
        workspace.status = WorkspaceStatus::Failed;
        app.workspaces
            .retain_creation_residue(workspace.clone())
            .unwrap();
    }
    std::fs::write(workspace.root.join("residue"), b"keep until cleanup").unwrap();
    let mut evidence = record.execution;
    evidence["workspace"] = json!(workspace);
    evidence["residue"] = json!({"paths":[workspace.root],"error":"retained cleanup"});
    f.store
        .writer()
        .park_creation(PRINCIPAL, &id, "unknown", evidence)
        .unwrap();
    let f = f.reopen();
    f.store.writer().recover_mutations().unwrap();
    (f, id, workspace)
}

fn assert_preparation_failure_is_retained(f: &Fixture, id: &str, workspace: &Workspace) {
    let before = f.record(id);
    let result = execute(&f.state, before.clone());
    let after = f.record(id);
    assert!(
        result.is_ok(),
        "cleanup preparation must stay nonterminal: {result:?}; {after:?}"
    );
    assert!(
        matches!(after.state.as_str(), "unknown" | "reconciling"),
        "{after:?}"
    );
    assert!(after.outcome.is_none());
    assert_eq!(after.execution, before.execution);
    assert_eq!(after.reservations, before.reservations);
    assert!(f.state.lock().unwrap().workspace_reserved(&workspace.id));
    assert_eq!(
        std::fs::read(workspace.root.join("residue")).unwrap(),
        b"keep until cleanup"
    );
    let connection = f.store.writer_for_test();
    let terminal: i64 = connection
        .query_row(
            "SELECT count(*) FROM mutation_outbox WHERE command_id=?1",
            [after.command_id],
            |row| row.get(0),
        )
        .unwrap();
    let active: i64 = connection
        .query_row(
            "SELECT count(*) FROM mutation_owner_order WHERE command_id=?1 AND active=1",
            [after.command_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(terminal, 0);
    assert_eq!(active, 1);
    drop(connection);
    assert!(admit(
        &f.state,
        "workspace.create",
        before.params,
        WriteMetadata {
            operation_id: operation(161),
            after: None,
        }
    )
    .is_err());
}

fn assert_cleanup_can_settle(f: Fixture, id: &str, workspace: &Workspace) {
    let f = f.reopen();
    f.run(id);
    let record = f.record(id);
    assert_eq!(record.state, "failed", "{record:?}");
    assert!(record.execution.get("residue").is_none());
    assert!(!workspace.root.exists());
    assert!(!f.state.lock().unwrap().workspace_reserved(&workspace.id));
}

#[test]
fn unknown_residue_cleanup_preparation_busy_keeps_fences() {
    let (f, id, workspace) = unknown_with_residue();
    f.state
        .lock()
        .unwrap()
        .reserve_workspace_for_test(&workspace.id);
    assert!(f
        .state
        .lock()
        .unwrap()
        .workspace_reclaim_reserved(&workspace.id));
    assert_preparation_failure_is_retained(&f, &id, &workspace);
    f.state
        .lock()
        .unwrap()
        .release_workspace_reservation_for_test(&workspace.id);
    assert_cleanup_can_settle(f, &id, &workspace);
}

#[test]
fn unknown_residue_cleanup_preparation_io_error_keeps_fences() {
    let (f, id, workspace) = unknown_with_residue();
    // The first preparation write is the lifecycle marker, before cleanup runs.
    f.store.fail_next_write();
    assert_preparation_failure_is_retained(&f, &id, &workspace);
    assert_cleanup_can_settle(f, &id, &workspace);
}

#[test]
fn fresh_workspace_preparation_failure_finalizes_unapplied() {
    let f = Fixture::new();
    let id = operation(162);
    let record = workspace_attempt(&f, &id);
    let workspace: Workspace =
        serde_json::from_value(record.execution["workspace"].clone()).unwrap();
    assert!(record.execution["steps"].as_array().unwrap().is_empty());
    assert!(!workspace.root.exists());
    assert!(f.store.writer().claim_mutation(PRINCIPAL, &id).unwrap());
    f.store.fail_next_write();
    f.run(&id);
    let record = f.record(&id);
    assert_eq!(record.state, "failed", "{record:?}");
    assert_eq!(record.outcome.unwrap()["effect"], "unapplied");
    assert!(!workspace.root.exists());
    assert!(!f.state.lock().unwrap().workspace_reserved(&workspace.id));
}
