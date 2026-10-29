use build_bridge::store::{MutationAdmission, MutationOwner, MutationResource, Store, StoreError};
use serde_json::json;
fn intent(id: &str) -> MutationAdmission {
    MutationAdmission {
        principal: "device:alice".into(),
        operation_id: id.into(),
        method: "test.create".into(),
        params: json!({"body":"hello"}),
        scope: json!({"project":"p","generation":1}),
        owners: vec![MutationOwner {
            kind: "project".into(),
            id: "p".into(),
            generation: 1,
        }],
        resources: vec![],
        reservations: vec![],
        after: None,
        recovery_policy: "sql_only".into(),
        accepted_epoch: "test-boot".into(),
        acceptance: json!({"task_id":"t"}),
    }
}
const A: &str = "11111111-1111-4111-8111-111111111111";
const B: &str = "22222222-2222-4222-8222-222222222222";
#[test]
fn identity_is_durable_scoped_and_payload_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let incarnation = store.journal_incarnation().to_string();
    let original = store.writer().admit_mutation(&intent(A)).unwrap();
    drop(store);
    let store = Store::new(dir.path()).unwrap();
    assert_eq!(incarnation, store.journal_incarnation());
    assert_eq!(original, store.writer().admit_mutation(&intent(A)).unwrap());
    let mut changed = intent(A);
    changed.params = json!({"body":"different"});
    assert!(matches!(
        store.writer().admit_mutation(&changed),
        Err(StoreError::OperationConflict { .. })
    ));
    changed.principal = "device:bob".into();
    assert_ne!(
        original.command_id,
        store.writer().admit_mutation(&changed).unwrap().command_id
    );
}
#[test]
fn reservations_infer_dependencies_and_explicit_override_cannot_drop_creator() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let resource = MutationResource {
        kind: "task".into(),
        id: "t".into(),
        owner: intent(A).owners[0].clone(),
    };
    let mut creator = intent(A);
    creator.reservations.push(resource.clone());
    store.writer().admit_mutation(&creator).unwrap();
    let mut dependent = intent(B);
    dependent.resources.push(resource);
    let saved = store.writer().admit_mutation(&dependent).unwrap();
    assert_eq!(saved.dependencies, vec![A.to_string()]);
    dependent.after = Some(vec![]);
    assert!(store.writer().admit_mutation(&dependent).is_err());
    assert!(!store.writer().claim_mutation("device:alice", B).unwrap());
    let mut outsider = dependent;
    outsider.principal = "device:bob".into();
    assert!(store
        .writer()
        .admit_mutation(&outsider)
        .unwrap_err()
        .to_string()
        .contains("not_yet_available"));
}

#[tokio::test]
async fn owner_order_commit_publication_and_outbox_are_separate_durable_steps() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    writer.admit_mutation(&intent(A)).unwrap();
    writer.admit_mutation(&intent(B)).unwrap();
    assert!(!writer.claim_mutation("device:alice", B).unwrap());
    assert!(writer
        .confirm_mutation("device:alice", A, json!({}))
        .is_err());
    assert!(writer.claim_mutation("device:alice", A).unwrap());
    writer
        .commit_mutation("device:alice", A, |tx| {
            tx.execute(
                "INSERT INTO meta(key,value) VALUES('journal-test-effect','once')",
                [],
            )?;
            Ok(json!({"saved":true}))
        })
        .unwrap();
    assert!(!writer.claim_mutation("device:alice", B).unwrap());
    let recovery = writer.recover_mutations().unwrap();
    assert_eq!(recovery.committed.len(), 1);
    writer
        .confirm_mutation("device:alice", A, json!({"published":[],"refresh":[]}))
        .unwrap();
    assert!(writer.claim_mutation("device:alice", B).unwrap());
    let mut reader = store.read_pool().acquire().await;
    reader
        .read(|tx| {
            assert!(tx.mutation_status("device:bob", &[A.into()]).unwrap()[0].is_none());
            assert_eq!(tx.mutation_cursor("device:alice").unwrap(), 1);
            let events = tx.mutation_outbox("device:alice", 0, 256).unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].operation.state, "succeeded");
        })
        .unwrap();
}

#[test]
fn mutation_crash_worker() {
    let Some(dir) = std::env::var_os("JOURNAL_FIXTURE_DIR") else {
        return;
    };
    let boundary = std::env::var("JOURNAL_FIXTURE_BOUNDARY").unwrap();
    let store = Store::new(&dir).unwrap();
    let writer = store.writer();
    let mut command = intent(A);
    if boundary == "claim_external" || boundary == "effect_external" {
        command.recovery_policy = "external".into();
    }
    writer.admit_mutation(&command).unwrap();
    if boundary == "accept" {
        std::process::exit(73);
    }
    assert!(writer.claim_mutation("device:alice", A).unwrap());
    if boundary.starts_with("claim_") {
        std::process::exit(73);
    }
    if boundary == "effect_external" {
        std::fs::write(std::path::Path::new(&dir).join("effect"), "once").unwrap();
        std::process::exit(73);
    }
    writer
        .commit_mutation("device:alice", A, |tx| {
            tx.execute(
                "INSERT INTO meta(key,value) VALUES('journal-test-effect','once')",
                [],
            )?;
            if boundary == "effect_sql" {
                std::process::exit(73);
            }
            Ok(json!({"saved":true}))
        })
        .unwrap();
    if boundary == "commit" {
        std::process::exit(73);
    }
    std::fs::write(std::path::Path::new(&dir).join("published"), "once").unwrap();
    if boundary == "publication" {
        std::process::exit(73);
    }
    writer
        .confirm_mutation("device:alice", A, json!({"published":[],"refresh":[]}))
        .unwrap();
    std::process::exit(73);
}

#[tokio::test]
async fn abrupt_worker_death_preserves_each_recovery_boundary_without_duplicate_effects() {
    for boundary in [
        "accept",
        "claim_sql",
        "claim_external",
        "effect_external",
        "effect_sql",
        "commit",
        "publication",
        "outbox",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "mutation_crash_worker", "--nocapture"])
            .env("JOURNAL_FIXTURE_DIR", dir.path())
            .env("JOURNAL_FIXTURE_BOUNDARY", boundary)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(73),
            "{boundary}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let store = Store::new(dir.path()).unwrap();
        let mut reader = store.read_pool().acquire().await;
        let record = reader
            .read(|tx| {
                tx.mutation_status("device:alice", &[A.into()])
                    .unwrap()
                    .remove(0)
                    .unwrap()
            })
            .unwrap();
        let expected = match boundary {
            "claim_external" | "effect_external" => "failed",
            "commit" | "publication" => "committed",
            "outbox" => "succeeded",
            _ => "accepted",
        };
        assert_eq!(record.state, expected, "{boundary}");
        let effect = rusqlite::Connection::open_with_flags(
            dir.path().join("build.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
        .query_row(
            "SELECT value FROM meta WHERE key='journal-test-effect'",
            [],
            |row| row.get::<_, String>(0),
        );
        assert_eq!(
            effect.is_ok(),
            ["commit", "publication", "outbox"].contains(&boundary),
            "{boundary}"
        );
        if expected == "failed" {
            assert_eq!(record.outcome.unwrap()["effect"], "unknown");
            assert!(!store.writer().claim_mutation("device:alice", A).unwrap());
        }
        if expected == "committed" {
            assert!(!store.writer().claim_mutation("device:alice", A).unwrap());
            store
                .writer()
                .confirm_mutation("device:alice", A, json!({"published":[],"refresh":[]}))
                .unwrap();
        }
        if ["committed", "succeeded", "failed"].contains(&expected) {
            assert_eq!(
                reader
                    .read(|tx| tx.mutation_outbox("device:alice", 0, 256).unwrap().len())
                    .unwrap(),
                1
            );
        }
    }
}

#[tokio::test]
async fn unresolved_outcomes_keep_evidence_and_owner_fences_until_reconciled() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    writer.admit_mutation(&intent(A)).unwrap();
    writer.admit_mutation(&intent(B)).unwrap();
    assert!(writer.claim_mutation("device:alice", A).unwrap());
    writer
        .fail_mutation(
            "device:alice",
            A,
            "unknown",
            json!({"code":"ambiguous","message":"external claim","retryable":false}),
        )
        .unwrap();
    assert!(!writer.claim_mutation("device:alice", B).unwrap());
    let far_future = 4_000_000_000;
    assert_eq!(writer.compact_mutations(far_future).unwrap(), 0);
    let mut reader = store.read_pool().acquire().await;
    let record = reader
        .read(|tx| {
            tx.mutation_status("device:alice", &[A.into()])
                .unwrap()
                .remove(0)
                .unwrap()
        })
        .unwrap();
    assert!(!record.expired);
    assert_eq!(record.params, json!({"body":"hello"}));
    writer.reconcile_mutation("device:alice",A,json!({"state":"failed","effect":"unapplied","error":{"code":"proven_unapplied","message":"checked","retryable":false}})).unwrap();
    assert!(writer.claim_mutation("device:alice", B).unwrap());
    assert_eq!(writer.compact_mutations(far_future).unwrap(), 1);
    let expired = writer.admit_mutation(&intent(A)).unwrap();
    assert!(expired.expired);
    assert_eq!(expired.state, "failed");
    let mut conflict = intent(A);
    conflict.params = json!({"body":"other"});
    assert!(matches!(
        writer.admit_mutation(&conflict),
        Err(StoreError::OperationConflict { .. })
    ));
    let events = reader
        .read(|tx| tx.mutation_outbox("device:alice", 0, 256).unwrap())
        .unwrap();
    assert_eq!(events.len(), 2);
    assert!(events[1].cursor > events[0].cursor);
}

#[tokio::test]
async fn failed_creator_fails_dependents_and_preserves_reserved_identity() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let resource = MutationResource {
        kind: "task".into(),
        id: "t".into(),
        owner: intent(A).owners[0].clone(),
    };
    let mut creator = intent(A);
    creator.reservations.push(resource.clone());
    writer.admit_mutation(&creator).unwrap();
    let mut dependent = intent(B);
    dependent.resources.push(resource);
    writer.admit_mutation(&dependent).unwrap();
    writer
        .fail_mutation(
            "device:alice",
            A,
            "unapplied",
            json!({"code":"refused","message":"failed","retryable":false}),
        )
        .unwrap();
    assert!(!writer.claim_mutation("device:alice", B).unwrap());
    let mut reader = store.read_pool().acquire().await;
    let record = reader
        .read(|tx| {
            tx.mutation_status("device:alice", &[B.into()])
                .unwrap()
                .remove(0)
                .unwrap()
        })
        .unwrap();
    assert_eq!(record.state, "failed");
    assert_eq!(
        record.outcome.unwrap()["error"]["code"],
        "dependency_failed"
    );
    assert!(reader
        .read(|tx| tx.mutation_reservation("device:bob", "task", "t").unwrap())
        .unwrap()
        .is_none());
}

#[test]
fn dependencies_remain_stable_when_creator_publishes_and_other_principals_can_use_canonical_id() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let resource = MutationResource {
        kind: "task".into(),
        id: "t".into(),
        owner: intent(A).owners[0].clone(),
    };
    let mut creator = intent(A);
    creator.reservations.push(resource.clone());
    writer.admit_mutation(&creator).unwrap();
    let mut dependent = intent(B);
    dependent.resources.push(resource);
    let saved = writer.admit_mutation(&dependent).unwrap();
    assert!(writer.claim_mutation("device:alice", A).unwrap());
    writer
        .commit_mutation("device:alice", A, |_| Ok(json!({"task_id":"t"})))
        .unwrap();
    writer
        .confirm_mutation("device:alice", A, json!({}))
        .unwrap();
    assert_eq!(saved, writer.admit_mutation(&dependent).unwrap());
    dependent.principal = "device:bob".into();
    assert!(writer
        .admit_mutation(&dependent)
        .unwrap()
        .dependencies
        .is_empty());
}

#[test]
fn canonical_uuid_and_normalized_json_fence_retries() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let mut command = intent(A);
    command.params = serde_json::from_str("{\"b\":2,\"a\":1}").unwrap();
    let saved = writer.admit_mutation(&command).unwrap();
    command.params = serde_json::from_str("{\"a\":1,\"b\":2}").unwrap();
    assert_eq!(saved, writer.admit_mutation(&command).unwrap());
    command.operation_id = "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA".into();
    assert!(writer.admit_mutation(&command).is_err());
    command = intent(B);
    command.owners[0].generation = 2;
    assert!(writer.admit_mutation(&command).is_ok());
    assert!(writer.claim_mutation("device:alice", B).unwrap());
}

#[test]
fn schema_seventeen_gains_empty_journal_without_changing_existing_domain_records() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    drop(store);
    // Build a disposable pre-journal fixture; no rollback executable is used.
    let fixture = rusqlite::Connection::open(dir.path().join("build.db")).unwrap();
    fixture.execute_batch("DROP TABLE mutation_outbox;DROP TABLE mutation_reservations;DROP TABLE mutation_dependencies;DROP TABLE mutation_owner_order;DROP TABLE mutation_commands;DELETE FROM meta WHERE key='journal_incarnation';UPDATE meta SET value='17' WHERE key='schema_version';INSERT INTO meta(key,value) VALUES('fixture-domain-record','preserved');").unwrap();
    drop(fixture);
    let store = Store::new(dir.path()).unwrap();
    assert!(uuid::Uuid::parse_str(store.journal_incarnation()).is_ok());
    store.writer().admit_mutation(&intent(A)).unwrap();
    drop(store);
    let database = rusqlite::Connection::open_with_flags(
        dir.path().join("build.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        database
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        build_bridge::store::SCHEMA_VERSION.to_string()
    );
    assert_eq!(
        database
            .query_row(
                "SELECT value FROM meta WHERE key='fixture-domain-record'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "preserved"
    );
}
