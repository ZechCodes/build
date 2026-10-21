use super::*;
use crate::store::Store;
fn command() -> MutationAdmission {
    MutationAdmission {
        principal: "test".into(),
        operation_id: "11111111-1111-4111-8111-111111111111".into(),
        method: "test".into(),
        params: json!({}),
        scope: json!({}),
        owners: vec![MutationOwner {
            kind: "project".into(),
            id: "p".into(),
            generation: 1,
        }],
        resources: vec![],
        reservations: vec![],
        after: None,
        recovery_policy: "sql_only".into(),
        accepted_epoch: "boot".into(),
        acceptance: json!({}),
    }
}
#[test]
fn refused_admission_and_terminal_outbox_commit_do_not_claim_success() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let command = command();
    let writer = store.writer();
    store.fail_next_write();
    assert!(writer.admit_mutation(&command).is_err());
    assert!(load(&store.connection(), "test", &command.operation_id)
        .unwrap()
        .is_none());
    writer.admit_mutation(&command).unwrap();
    assert!(writer
        .claim_mutation("test", &command.operation_id)
        .unwrap());
    writer
        .commit_mutation("test", &command.operation_id, |tx| {
            tx.execute("INSERT INTO meta(key,value) VALUES('effect','once')", [])?;
            Ok(json!({"saved":true}))
        })
        .unwrap();
    store.fail_next_write();
    assert!(writer
        .confirm_mutation("test", &command.operation_id, json!({}))
        .is_err());
    assert_eq!(
        load(&store.connection(), "test", &command.operation_id)
            .unwrap()
            .unwrap()
            .state,
        "committed"
    );
    assert_eq!(
        store
            .connection()
            .query_row("SELECT count(*) FROM mutation_outbox", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(writer);
    drop(store);
    let store = Store::new(dir.path()).unwrap();
    let recovery = store.writer().recover_mutations().unwrap();
    assert_eq!(recovery.committed.len(), 1);
    store
        .writer()
        .confirm_mutation("test", &command.operation_id, json!({}))
        .unwrap();
    assert!(!store
        .writer()
        .claim_mutation("test", &command.operation_id)
        .unwrap());
    assert_eq!(
        store
            .connection()
            .query_row("SELECT value FROM meta WHERE key='effect'", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "once"
    );
}
#[test]
fn legacy_and_journal_share_exactly_one_writer_connection() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let guard = store.writer_for_test();
    let thread = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let result = writer.admit_mutation(&command());
        done_tx.send(result).unwrap();
    });
    started_rx.recv().unwrap();
    assert!(done_rx
        .recv_timeout(std::time::Duration::from_millis(20))
        .is_err());
    drop(guard);
    assert!(done_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap()
        .is_ok());
    thread.join().unwrap();
}

#[test]
fn malformed_terminal_data_never_replaces_recoverable_state() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let command = command();
    let writer = store.writer();
    writer.admit_mutation(&command).unwrap();
    assert!(writer
        .fail_mutation(
            "test",
            &command.operation_id,
            "unknown",
            json!({"code":"bad"})
        )
        .is_err());
    assert_eq!(
        load(&store.connection(), "test", &command.operation_id)
            .unwrap()
            .unwrap()
            .state,
        "accepted"
    );
    writer
        .claim_mutation("test", &command.operation_id)
        .unwrap();
    writer
        .commit_mutation("test", &command.operation_id, |_| Ok(json!({"saved":true})))
        .unwrap();
    assert!(writer
        .confirm_mutation(
            "test",
            &command.operation_id,
            json!({"published":"invalid"})
        )
        .is_err());
    assert!(writer
        .confirm_mutation(
            "test",
            &command.operation_id,
            json!({"refresh":[{"kind":"project"}]})
        )
        .is_err());
    assert_eq!(
        load(&store.connection(), "test", &command.operation_id)
            .unwrap()
            .unwrap()
            .state,
        "committed"
    );
    assert_eq!(
        store
            .connection()
            .query_row("SELECT count(*) FROM mutation_outbox", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let mut ambiguous = command.clone();
    ambiguous.operation_id = "22222222-2222-4222-8222-222222222222".into();
    writer.admit_mutation(&ambiguous).unwrap();
    writer
        .fail_mutation(
            "test",
            &ambiguous.operation_id,
            "unknown",
            json!({"code":"ambiguous","message":"claim","retryable":false}),
        )
        .unwrap();
    assert!(writer
        .reconcile_mutation(
            "test",
            &ambiguous.operation_id,
            json!({"state":"succeeded"})
        )
        .is_err());
    let record = load(&store.connection(), "test", &ambiguous.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(record.state, "failed");
    assert_eq!(
        record.outcome.unwrap()["refresh"],
        json!([{"kind":"project","id":"p"}])
    );
}
