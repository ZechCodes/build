use super::*;
use crate::operation::OperationReceipt;
use crate::secrets::{GrantScope, SecretContext, SecretPostBinding, SecretRecord, StageContext};

fn setup(store: &Store) -> (PersistedRun, OperationReceipt, SecretPostBinding, String) {
    let mut record = run_record("task-1", None, NOW);
    store.save_run(&record).unwrap();
    store
        .writer_for_test()
        .execute(
            "INSERT INTO device_project_ids(path,project_id) VALUES('/repo','test-project')",
            [],
        )
        .unwrap();
    let agent = record.agents[0].id.clone();
    let generation = record.agents[0].thread.id.clone();
    let id = format!("sec_{}", uuid::Uuid::new_v4());
    let secret = SecretRecord {
        context: SecretContext {
            id: id.clone(),
            device_id: "test-device".into(),
            creator: "device_user".into(),
            name: "TEST_TOKEN".into(),
            ttl_seconds: Some(60),
            grant: GrantScope::Agent {
                agent_id: agent.clone(),
                conversation_id: agent.clone(),
                generation: generation.clone(),
            },
            stage: StageContext {
                owner_id: record.id.clone(),
                conversation_id: agent.clone(),
                generation: generation.clone(),
                draft_id: None,
            },
        },
        created_at: 100,
        attached_at: None,
        expires_at: None,
        revoked_at: None,
        revocation_reason: None,
        revision: 1,
        message_operation_id: None,
        upload_request_id: uuid::Uuid::new_v4().to_string(),
        upload_digest: "a".repeat(64),
        blob_present: true,
    };
    store.writer().secrets_stage(&secret).unwrap();
    let claim = SecretPostBinding {
        ids: vec![id.clone()],
        device_id: "test-device".into(),
        creator: "device_user".into(),
        project_id: "test-project".into(),
        owner_id: record.id.clone(),
        agent_id: agent.clone(),
        conversation_id: agent,
        generation,
        draft_id: None,
        attached_at: 200,
    };
    record.agents[0]
        .thread
        .post_user("use test token", None, NOW);
    let mut receipt = queued_operation("secret-atomic", 1);
    let mut preview = secret;
    preview.attached_at = Some(200);
    preview.expires_at = Some(260);
    preview.revision += 1;
    let metadata = crate::thread::MessageSecret::from_record(&preview).unwrap();
    let ThreadItem::Message(message) = record.agents[0].thread.items.last_mut().unwrap() else {
        panic!("message");
    };
    message.secrets = vec![metadata];
    receipt
        .delivery
        .as_mut()
        .unwrap()
        .payload
        .as_mut()
        .unwrap()
        .messages = vec![message.clone()];
    (record, receipt, claim, id)
}

#[test]
fn secret_atomic_operation_rechecks_generation_before_saving_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let (record, receipt, claim, id) = setup(&store);
    let mut changed = record.clone();
    changed.agents[0].thread.id = "replacement-thread".into();
    changed.agents[0].thread.generation_revision += 1;
    changed.agents[0].thread.items.clear();
    store.save_run(&changed).unwrap();
    assert!(store
        .save_run_accepting_operation_with_secrets(&record, &receipt, Some(&claim))
        .is_err());
    assert!(store.operation(&receipt.operation_id).unwrap().is_none());
    assert!(store
        .writer()
        .secret_record(&id)
        .unwrap()
        .unwrap()
        .attached_at
        .is_none());
    let raw: String = store
        .writer_for_test()
        .query_row(
            "SELECT record FROM agents WHERE id=?1",
            [&claim.agent_id],
            |row| row.get(0),
        )
        .unwrap();
    let saved: crate::agent::Agent = serde_json::from_str(&raw).unwrap();
    assert_eq!(saved.thread.id, "replacement-thread");
}

#[test]
fn secret_atomic_operation_rolls_back_at_message_and_receipt_boundaries() {
    for table in ["thread_items", "operations"] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let (record, receipt, claim, id) = setup(&store);
        store.writer_for_test().execute_batch(&format!("CREATE TRIGGER reject_secret_admission BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'injected persistence boundary'); END")).unwrap();
        assert!(
            store
                .save_run_accepting_operation_with_secrets(&record, &receipt, Some(&claim))
                .is_err(),
            "{table}"
        );
        assert!(store.operation(&receipt.operation_id).unwrap().is_none());
        assert!(store.thread_items(&claim.agent_id).unwrap().is_empty());
        assert!(store
            .writer()
            .secret_record(&id)
            .unwrap()
            .unwrap()
            .attached_at
            .is_none());
        assert_eq!(
            store
                .writer_for_test()
                .query_row("SELECT COUNT(*) FROM secret_bindings", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        store
            .writer_for_test()
            .execute_batch("DROP TRIGGER reject_secret_admission")
            .unwrap();
        assert!(store
            .save_run_accepting_operation_with_secrets(&record, &receipt, Some(&claim))
            .is_ok());
    }
}

#[test]
fn secret_atomic_operation_refuses_ended_recipients_and_changed_project_membership() {
    for ended in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let (record, receipt, claim, id) = setup(&store);
        if ended {
            let mut changed = record.clone();
            changed.agents[0].state = crate::agent::AgentLifecycle::Ended;
            store.save_run(&changed).unwrap();
        } else {
            store.writer_for_test().execute("UPDATE device_project_ids SET project_id='other-project' WHERE project_id='test-project'", []).unwrap();
        }
        assert!(store
            .save_run_accepting_operation_with_secrets(&record, &receipt, Some(&claim))
            .is_err());
        assert!(store.operation(&receipt.operation_id).unwrap().is_none());
        assert!(store
            .writer()
            .secret_record(&id)
            .unwrap()
            .unwrap()
            .attached_at
            .is_none());
    }
}

#[test]
fn secret_atomic_operation_cold_replay_preserves_binding_and_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let (record, receipt, mut claim, id) = setup(&store);
    let accepted = store
        .save_run_accepting_operation_with_secrets(&record, &receipt, Some(&claim))
        .unwrap();
    drop(store);
    let restarted = Store::new(dir.path()).unwrap();
    claim.attached_at = 500;
    let retried = restarted
        .save_run_accepting_operation_with_secrets(&record, &receipt, Some(&claim))
        .unwrap();
    assert_eq!(retried, accepted);
    let secret = restarted.writer().secret_record(&id).unwrap().unwrap();
    assert_eq!(secret.attached_at, Some(200));
    assert_eq!(secret.expires_at, Some(260));
    let connection = restarted.writer_for_test();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM secret_final_bindings WHERE secret_id=?1",
                [&id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM secret_audit WHERE secret_id=?1 AND outcome='attached'",
                [&id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}
