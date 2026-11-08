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

pub(super) fn unknown_with_residue() -> (Fixture, String, Workspace) {
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

#[test]
fn creation_effect_evidence_migrates_legacy_records_conservatively() {
    let f = Fixture::new();
    let id = operation(163);
    let mut record = workspace_attempt(&f, &id);
    record
        .execution
        .as_object_mut()
        .unwrap()
        .remove("effect_state");
    {
        let connection = f.store.writer_for_test();
        connection
            .execute(
                "UPDATE mutation_commands SET record=?1 WHERE command_id=?2",
                rusqlite::params![json!(record).to_string(), record.command_id],
            )
            .unwrap();
        connection
            .execute("UPDATE meta SET value='25' WHERE key='schema_version'", [])
            .unwrap();
    }
    let f = f.reopen();
    let migrated = f.record(&id);
    assert_eq!(migrated.state, "unknown", "{migrated:?}");
    assert_eq!(migrated.execution["effect_state"], "started");
    assert!(migrated.outcome.is_none());
    assert_eq!(migrated.reservations, record.reservations);
    let f = f.reopen();
    assert_eq!(f.record(&id), migrated, "migration must be idempotent");
}

#[test]
fn missing_effect_evidence_never_reads_as_fresh_without_reopen() {
    let f = Fixture::new();
    let id = operation(164);
    let record = workspace_attempt(&f, &id);
    assert!(f.store.writer().claim_mutation(PRINCIPAL, &id).unwrap());
    let mut evidence = record.execution;
    evidence.as_object_mut().unwrap().remove("effect_state");
    f.store
        .writer()
        .checkpoint_creation(PRINCIPAL, &id, evidence)
        .unwrap();
    f.store.fail_next_write();
    f.run(&id);
    let record = f.record(&id);
    assert_eq!(record.state, "unknown", "{record:?}");
    assert!(record.outcome.is_none());
}

#[test]
fn creation_effect_migration_preserves_each_terminal_state() {
    for (state, effect) in [
        ("succeeded", None),
        ("failed", Some("unapplied")),
        ("failed", Some("partial")),
    ] {
        let f = Fixture::new();
        let id = operation(166);
        let mut record = workspace_attempt(&f, &id);
        record.state = state.into();
        record.outcome = Some(json!({"state":state,"effect":effect,"legacy":"unchanged"}));
        record
            .execution
            .as_object_mut()
            .unwrap()
            .remove("effect_state");
        {
            let connection = f.store.writer_for_test();
            connection
                .execute(
                    "UPDATE mutation_commands SET state=?1,record=?2 WHERE command_id=?3",
                    rusqlite::params![state, json!(record).to_string(), record.command_id],
                )
                .unwrap();
            connection
                .execute("UPDATE meta SET value='25' WHERE key='schema_version'", [])
                .unwrap();
        }
        let f = f.reopen();
        assert_eq!(f.record(&id), record);
        let f = f.reopen();
        assert_eq!(f.record(&id), record);
    }
}
