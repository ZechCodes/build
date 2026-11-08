use crate::secrets::{GrantScope, SecretContext, SecretError, SecretRecord, StageContext};
use crate::store::Store;
use std::collections::{HashMap, HashSet};

const NOW: i64 = 1_800_000_000;

pub(super) fn record(creator: &str, grant: GrantScope) -> SecretRecord {
    SecretRecord {
        context: SecretContext {
            id: format!("sec_{}", uuid::Uuid::new_v4()),
            device_id: "synthetic-device".into(),
            creator: creator.into(),
            name: "TEST_ONLY_TOKEN".into(),
            ttl_seconds: Some(60),
            grant,
            stage: StageContext {
                owner_id: "synthetic-owner".into(),
                conversation_id: "synthetic-conversation".into(),
                generation: "synthetic-generation".into(),
                draft_id: None,
            },
        },
        created_at: NOW,
        attached_at: None,
        expires_at: None,
        revoked_at: None,
        revocation_reason: None,
        revision: 1,
        message_operation_id: None,
        upload_request_id: uuid::Uuid::new_v4().to_string(),
        upload_digest: "ab".repeat(32),
        blob_present: true,
    }
}

pub(super) fn agent_grant() -> GrantScope {
    GrantScope::Agent {
        agent_id: "synthetic-agent".into(),
        conversation_id: "synthetic-conversation".into(),
        generation: "synthetic-generation".into(),
    }
}
fn project_grant() -> GrantScope {
    GrantScope::Project {
        project_id: "synthetic-project".into(),
    }
}

#[test]
fn metadata_survives_restart_and_migrates_existing_database_without_history_changes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    store
        .writer_for_test()
        .execute("INSERT INTO meta VALUES ('synthetic-marker','kept')", [])
        .unwrap();
    drop(store);
    let db = rusqlite::Connection::open(dir.path().join("build.db")).unwrap();
    db.execute_batch("DROP TABLE secret_records; DROP TABLE secret_clock; UPDATE meta SET value='23' WHERE key='schema_version';").unwrap();
    drop(db);
    let store = Store::new(dir.path()).unwrap();
    assert!(!store.writer().secrets_have_records().unwrap());
    let rec = record("synthetic-user", agent_grant());
    store.writer().secrets_stage(&rec).unwrap();
    drop(store);
    let store = Store::new(dir.path()).unwrap();
    assert_eq!(
        store.writer().secret_record(&rec.context.id).unwrap(),
        Some(rec)
    );
    let marker: String = store
        .writer_for_test()
        .query_row(
            "SELECT value FROM meta WHERE key='synthetic-marker'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(marker, "kept");
    let version: String = store
        .writer_for_test()
        .query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(version, "26");
}

#[test]
fn quota_is_per_creator_and_device_and_retained_blob_quota_is_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    for _ in 0..100 {
        writer
            .secrets_stage(&record("synthetic-user", agent_grant()))
            .unwrap();
    }
    assert!(matches!(
        writer.secrets_stage(&record("synthetic-user", agent_grant())),
        Err(SecretError::Limit)
    ));
    writer
        .secrets_stage(&record("synthetic-other-user", agent_grant()))
        .unwrap();
    for index in 101..1000 {
        writer
            .secrets_stage(&record(&format!("synthetic-user-{index}"), agent_grant()))
            .unwrap();
    }
    assert!(matches!(
        writer.secrets_stage(&record("synthetic-user-overflow", project_grant())),
        Err(SecretError::Limit)
    ));
    let first = writer.secret_records().unwrap().remove(0);
    writer.secrets_revoke(&first.context.id, NOW + 1).unwrap();
    assert!(matches!(
        writer.secrets_stage(&record("synthetic-user-overflow", project_grant())),
        Err(SecretError::Limit)
    ));
    writer.secrets_mark_deleted(&first.context.id).unwrap();
    writer
        .secrets_stage(&record("synthetic-user-overflow", project_grant()))
        .unwrap();
}

#[test]
fn attach_is_all_or_nothing_sets_deadlines_and_replays_original_even_after_revoke() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let first = record("synthetic-user", agent_grant());
    let mut second = record("synthetic-user", project_grant());
    second.context.ttl_seconds = None;
    for rec in [&first, &second] {
        writer.secrets_stage(rec).unwrap();
    }
    assert!(writer
        .secrets_attach(
            &[first.context.id.clone(), "sec_invalid".into()],
            "synthetic-operation",
            NOW + 5
        )
        .is_err());
    assert_eq!(
        writer
            .secret_record(&first.context.id)
            .unwrap()
            .unwrap()
            .attached_at,
        None
    );
    let ids = vec![first.context.id.clone(), second.context.id.clone()];
    let attached = writer
        .secrets_attach(&ids, "synthetic-operation", NOW + 5)
        .unwrap();
    assert_eq!(attached[0].expires_at, Some(NOW + 65));
    assert_eq!(attached[1].expires_at, None);
    assert_eq!(attached[0].attached_at, Some(NOW + 5));
    assert!(writer
        .secrets_attach(&ids, "other-operation", NOW + 6)
        .is_err());
    writer.secrets_revoke(&first.context.id, NOW + 10).unwrap();
    let replay = writer
        .secrets_attach(&ids, "synthetic-operation", NOW + 1000)
        .unwrap();
    assert_eq!(replay[0].attached_at, Some(NOW + 5));
    assert_eq!(replay[0].expires_at, Some(NOW + 65));
}

#[test]
fn stage_boundary_and_ttl_overflow_refuse_without_partial_claim() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let first = record("synthetic-user", agent_grant());
    writer.secrets_stage(&first).unwrap();
    assert!(writer
        .secrets_attach(
            std::slice::from_ref(&first.context.id),
            "operation",
            NOW + 86400
        )
        .is_err());
    let mut second = record("synthetic-user", agent_grant());
    second.context.ttl_seconds = Some(i64::MAX);
    writer.secrets_stage(&second).unwrap();
    assert!(writer
        .secrets_attach(
            std::slice::from_ref(&second.context.id),
            "operation",
            NOW + 1
        )
        .is_err());
    assert_eq!(
        writer
            .secret_record(&second.context.id)
            .unwrap()
            .unwrap()
            .attached_at,
        None
    );
}

#[test]
fn clock_highwater_survives_restart_and_never_moves_backwards() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    assert!(store.writer().secret_clock_observe(NOW).unwrap());
    assert_eq!(store.writer().secret_clock_highwater().unwrap(), NOW);
    assert!(!store.writer().secret_clock_observe(NOW - 1).unwrap());
    drop(store);
    let store = Store::new(dir.path()).unwrap();
    assert!(!store.writer().secret_clock_observe(NOW - 1).unwrap());
    assert!(store.writer().secret_clock_observe(NOW).unwrap());
}

#[test]
fn reset_and_project_delete_have_independent_scope_lifetimes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let agent = record("synthetic-user", agent_grant());
    let project = record("synthetic-user", project_grant());
    writer.secrets_stage(&agent).unwrap();
    writer.secrets_stage(&project).unwrap();
    store
        .reset_conversation(
            "synthetic-conversation",
            &[],
            &HashMap::new(),
            &HashSet::new(),
        )
        .unwrap();
    assert!(writer
        .secret_record(&agent.context.id)
        .unwrap()
        .unwrap()
        .revoked_at
        .is_some());
    assert_eq!(
        writer
            .secret_record(&project.context.id)
            .unwrap()
            .unwrap()
            .revoked_at,
        None
    );
    let agent_revocations: i64 = writer
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM secret_audit WHERE secret_id=?1 AND outcome='revoked'",
            [&agent.context.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(agent_revocations, 1);
    store
        .clear_retained_project_messages("synthetic-project")
        .unwrap();
    assert!(writer
        .secret_record(&project.context.id)
        .unwrap()
        .unwrap()
        .revoked_at
        .is_some());
    let project_revocations: i64 = writer
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM secret_audit WHERE secret_id=?1 AND outcome='revoked'",
            [&project.context.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(project_revocations, 1);
}

#[test]
fn metadata_rejects_tainted_stage_and_request_reuse() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let rec = record("synthetic-user", agent_grant());
    writer.secrets_stage(&rec).unwrap();
    let mut other = rec.clone();
    other.context.id = format!("sec_{}", uuid::Uuid::new_v4());
    assert!(writer.secrets_stage(&other).is_err());
    let mut bad = record("synthetic-user", agent_grant());
    bad.expires_at = Some(NOW + 1);
    assert!(writer.secrets_stage(&bad).is_err());
    assert_eq!(writer.secret_records().unwrap().len(), 1);
}

#[test]
fn removing_roster_member_or_owner_retires_agent_grants_and_preserves_project_grants() {
    for remove_owner in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let writer = store.writer();
        let agent = crate::agent::Agent::new(
            "synthetic-agent",
            "synthetic-owner",
            crate::models::ModelChoice::default(),
            1,
            "2026-11-04T00:00:00Z",
        );
        store
            .in_transaction(|tx| {
                Store::write_agents(tx, "synthetic-owner", std::slice::from_ref(&agent))
            })
            .unwrap();
        let a = record("synthetic-user", agent_grant());
        let p = record("synthetic-user", project_grant());
        writer.secrets_stage(&a).unwrap();
        writer.secrets_stage(&p).unwrap();
        if remove_owner {
            store.delete_run("synthetic-owner").unwrap();
        } else {
            store
                .in_transaction(|tx| Store::write_agents(tx, "synthetic-owner", &[]))
                .unwrap();
        }
        assert!(writer
            .secret_record(&a.context.id)
            .unwrap()
            .unwrap()
            .revoked_at
            .is_some());
        assert_eq!(
            writer
                .secret_record(&p.context.id)
                .unwrap()
                .unwrap()
                .revoked_at,
            None
        );
        let revocations: i64 = writer
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM secret_audit WHERE secret_id=?1 AND outcome='revoked'",
                [&a.context.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(revocations, 1);
    }
}

#[test]
fn lifecycle_audit_rows_are_deleted_with_their_secret_tombstone() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let rec = record("synthetic-user", agent_grant());
    writer.secrets_stage(&rec).unwrap();
    writer.secrets_revoke(&rec.context.id, NOW + 1).unwrap();
    writer.secrets_mark_deleted(&rec.context.id).unwrap();
    let lifecycle_rows = |writer: &crate::store::StoreWriter| {
        writer
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM secret_audit WHERE secret_id=?1 AND outcome IN ('created','attached','expired','revoked')",
                [&rec.context.id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
    };
    assert_eq!(lifecycle_rows(&writer), 2);
    writer
        .connection()
        .execute("DELETE FROM secret_records WHERE id=?1", [&rec.context.id])
        .unwrap();
    assert_eq!(lifecycle_rows(&writer), 0);
}

#[test]
fn attachment_hook_shares_operation_transaction_and_rolls_back_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let rec = record("synthetic-user", agent_grant());
    writer.secrets_stage(&rec).unwrap();
    {
        let mut connection = writer.connection();
        connection
            .pragma_update(None, "synchronous", "FULL")
            .unwrap();
        let tx = connection.transaction().unwrap();
        tx.execute(
            "INSERT INTO meta (key,value) VALUES ('synthetic-operation-marker','not-committed')",
            [],
        )
        .unwrap();
        super::attach_in(
            &tx,
            std::slice::from_ref(&rec.context.id),
            "synthetic-operation",
            NOW + 1,
        )
        .unwrap();
    }
    assert_eq!(
        writer
            .secret_record(&rec.context.id)
            .unwrap()
            .unwrap()
            .attached_at,
        None
    );
    let marker: bool = writer
        .connection()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key='synthetic-operation-marker')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!marker);
}

#[test]
fn attachment_replay_cannot_add_or_remove_secret_ids() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let first = record("synthetic-user", agent_grant());
    let second = record("synthetic-user", project_grant());
    writer.secrets_stage(&first).unwrap();
    writer.secrets_stage(&second).unwrap();
    writer
        .secrets_attach(
            std::slice::from_ref(&first.context.id),
            "synthetic-operation",
            NOW + 1,
        )
        .unwrap();
    assert!(writer
        .secrets_attach(
            &[first.context.id.clone(), second.context.id.clone()],
            "synthetic-operation",
            NOW + 2
        )
        .is_err());
    assert_eq!(
        writer
            .secret_record(&second.context.id)
            .unwrap()
            .unwrap()
            .attached_at,
        None
    );
}

#[test]
fn tombstones_remain_after_blob_retirement_and_full_durability_restores_store_mode() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let rec = record("synthetic-user", agent_grant());
    writer.secrets_stage(&rec).unwrap();
    writer.secrets_revoke(&rec.context.id, NOW + 1).unwrap();
    writer.secrets_flush().unwrap();
    writer.secrets_mark_deleted(&rec.context.id).unwrap();
    assert!(writer.secrets_have_records().unwrap());
    assert!(writer.secret_records().unwrap().is_empty());
    assert!(
        !writer
            .secret_record(&rec.context.id)
            .unwrap()
            .unwrap()
            .blob_present
    );
    let synchronous: i64 = writer
        .connection()
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .unwrap();
    assert_eq!(synchronous, 1);
}

#[test]
fn preflight_rejects_invalid_metadata_and_conflicts_before_blob_write() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let rec = record("synthetic-user", agent_grant());
    writer.secrets_stage_preflight(&rec).unwrap();
    writer.secrets_stage(&rec).unwrap();
    assert!(matches!(
        writer.secrets_stage_preflight(&rec),
        Err(SecretError::Conflict)
    ));
    let mut duplicate = rec.clone();
    duplicate.context.id = format!("sec_{}", uuid::Uuid::new_v4());
    assert!(matches!(
        writer.secrets_stage_preflight(&duplicate),
        Err(SecretError::Conflict)
    ));
    let mut invalid = record("synthetic-user", agent_grant());
    invalid.upload_digest = "invalid".into();
    assert!(matches!(
        writer.secrets_stage_preflight(&invalid),
        Err(SecretError::Invalid)
    ));
}

#[test]
fn corrupt_deadline_metadata_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let changes = [
        "UPDATE secret_records SET expires_at=NULL WHERE id=?1",
        "UPDATE secret_records SET expires_at=expires_at+1 WHERE id=?1",
        "UPDATE secret_records SET attached_at=created_at-1 WHERE id=?1",
        "UPDATE secret_records SET revoked_at=-1 WHERE id=?1",
    ];
    for sql in changes {
        let rec = record("synthetic-user", agent_grant());
        writer.secrets_stage(&rec).unwrap();
        writer
            .secrets_attach(
                std::slice::from_ref(&rec.context.id),
                &uuid::Uuid::new_v4().to_string(),
                NOW + 1,
            )
            .unwrap();
        writer.connection().execute(sql, [&rec.context.id]).unwrap();
        assert!(writer.secret_record(&rec.context.id).is_err());
    }
    let mut forever = record("synthetic-user", project_grant());
    forever.context.ttl_seconds = None;
    writer.secrets_stage(&forever).unwrap();
    writer
        .secrets_attach(
            std::slice::from_ref(&forever.context.id),
            &uuid::Uuid::new_v4().to_string(),
            NOW + 1,
        )
        .unwrap();
    writer
        .connection()
        .execute(
            "UPDATE secret_records SET expires_at=?2 WHERE id=?1",
            rusqlite::params![forever.context.id, NOW + 10],
        )
        .unwrap();
    assert!(writer.secret_record(&forever.context.id).is_err());
}

#[test]
fn attachment_hook_refuses_a_non_durable_outer_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let rec = record("synthetic-user", agent_grant());
    writer.secrets_stage(&rec).unwrap();
    let mut connection = writer.connection();
    let transaction = connection.transaction().unwrap();
    assert!(super::attach_in(
        &transaction,
        std::slice::from_ref(&rec.context.id),
        "synthetic-operation",
        NOW + 1
    )
    .is_err());
}
