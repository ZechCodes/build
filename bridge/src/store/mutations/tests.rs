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

#[test]
fn settled_owner_history_is_not_loaded_when_claiming_the_next_command() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let command = command();
    writer.admit_mutation(&command).unwrap();
    writer
        .claim_mutation("test", &command.operation_id)
        .unwrap();
    writer
        .commit_mutation("test", &command.operation_id, |_| {
            Ok(json!({"large": "x".repeat(200_000)}))
        })
        .unwrap();
    writer
        .confirm_mutation("test", &command.operation_id, json!({}))
        .unwrap();
    // Poison only the retired record's decode path: scheduling must consult
    // its released owner fence without reading historical result bodies.
    store
        .connection()
        .execute(
            "UPDATE mutation_commands SET record='not-json' WHERE operation_id=?1",
            [&command.operation_id],
        )
        .unwrap();
    let mut next = command.clone();
    next.operation_id = "22222222-2222-4222-8222-222222222222".into();
    writer.admit_mutation(&next).unwrap();
    assert!(writer.claim_mutation("test", &next.operation_id).unwrap());
}

#[test]
fn canonical_commit_reserves_confirmation_envelope_space_before_effect_commit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let command = command();
    writer.admit_mutation(&command).unwrap();
    writer
        .claim_mutation("test", &command.operation_id)
        .unwrap();
    let refused = writer.commit_mutation("test", &command.operation_id, |tx| {
        tx.execute(
            "INSERT INTO meta(key,value) VALUES('oversized-effect','must-rollback')",
            [],
        )?;
        Ok(json!("x".repeat(256 * 1024 - 2)))
    });
    assert!(refused.is_err());
    assert_eq!(
        store
            .connection()
            .query_row(
                "SELECT count(*) FROM meta WHERE key='oversized-effect'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        load(&store.connection(), "test", &command.operation_id)
            .unwrap()
            .unwrap()
            .state,
        "executing"
    );
    writer
        .commit_mutation("test", &command.operation_id, |_| {
            Ok(json!("x".repeat(256 * 1024 - 1024)))
        })
        .unwrap();
    writer
        .confirm_mutation("test", &command.operation_id, json!({}))
        .unwrap();
}

#[tokio::test]
async fn caught_up_outbox_and_cursor_queries_ignore_retained_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let command = command();
    writer.admit_mutation(&command).unwrap();
    writer
        .fail_mutation(
            "test",
            &command.operation_id,
            "unapplied",
            json!({"code":"failed","message":"test","retryable":false}),
        )
        .unwrap();
    store.connection().execute_batch("WITH RECURSIVE nums(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM nums WHERE n<10000) INSERT INTO mutation_commands(principal,operation_id,state,accepted_at,terminal_at,intent,record) SELECT 'test',printf('retained-%d',n),'failed',0,0,'', '{}' FROM nums;INSERT INTO mutation_outbox(principal,command_id,revision) SELECT principal,command_id,1 FROM mutation_commands WHERE operation_id LIKE 'retained-%';").unwrap();
    let mut reader = store.read_pool().acquire().await;
    reads::OUTBOX_QUERY_STEPS.with(|steps| steps.set(0));
    reader
        .read(|read| {
            let cursor = read.mutation_cursor("test").unwrap();
            assert_eq!(cursor, 10001);
            assert!(read.mutation_outbox("test", cursor, 1).unwrap().is_empty());
        })
        .unwrap();
    let steps = reads::OUTBOX_QUERY_STEPS.with(|steps| steps.get());
    assert!(
        steps < 200,
        "caught-up queries executed {steps} SQLite VM steps"
    );
}

#[test]
fn unicode_intents_exhaust_the_store_byte_quota_before_the_command_count_quota() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection.execute_batch(JOURNAL_SCHEMA).unwrap();
    let transaction = connection.transaction().unwrap();
    let mut command = command();
    command.params = json!({"body":"🦀".repeat(15_000)});
    validate(&command).unwrap();
    let normalized = intent(&command, &[]);
    let added_bytes = normalized.len();
    let quota = 128 * 1024 * 1024;
    let count = quota / added_bytes;
    assert!(count < 4096);
    assert!(count * added_bytes <= quota);
    assert!((count + 1) * added_bytes > quota);
    // The quota query reads only intent/state/principal. Keep complete legal
    // normalized intents, without multiplying unqueried command-result bodies.
    let mut insert=transaction.prepare("INSERT INTO mutation_commands(principal,operation_id,state,accepted_at,intent,record) VALUES(?1,?2,'accepted',0,?3,'{}')").unwrap();
    for n in 0..count {
        insert
            .execute(params![
                format!("principal:{}", n / 100),
                format!("{n:08x}-0000-4000-8000-000000000000"),
                normalized
            ])
            .unwrap();
    }
    drop(insert);
    let bytes: i64 = transaction
        .query_row(
            "SELECT sum(length(CAST(intent AS BLOB))) FROM mutation_commands",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(bytes as usize, count * added_bytes);
    assert!(matches!(
        check_quota(&transaction, "new-principal", added_bytes),
        Err(StoreError::MutationBusy)
    ));
}
