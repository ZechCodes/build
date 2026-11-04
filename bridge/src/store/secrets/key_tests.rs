use super::tests::{agent_grant, record};
use crate::secrets::SecretError;
use crate::store::Store;

const NOW: i64 = 1_800_000_000;

#[test]
fn lost_key_retires_unrevoked_grants_and_clears_key_identity_durably() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    writer.secrets_set_vault_key_id(&"1".repeat(32)).unwrap();
    let lost = record("synthetic-user", agent_grant());
    let revoked = record("synthetic-user", agent_grant());
    writer.secrets_stage(&lost).unwrap();
    writer.secrets_stage(&revoked).unwrap();
    let prior_revocation = writer.secrets_revoke(&revoked.context.id, NOW + 1).unwrap();
    assert_eq!(writer.secrets_unretired_count().unwrap(), 1);
    assert_eq!(writer.secrets_retire_key_lost(NOW + 2).unwrap(), 1);
    assert_eq!(writer.secret_vault_key_id().unwrap(), None);
    assert_eq!(writer.secrets_unretired_count().unwrap(), 0);
    drop(writer);
    drop(store);
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let retired = writer.secret_record(&lost.context.id).unwrap().unwrap();
    assert_eq!(retired.revoked_at, Some(NOW + 2));
    assert_eq!(retired.revocation_reason.as_deref(), Some("key_lost"));
    assert_eq!(
        writer.secret_record(&revoked.context.id).unwrap(),
        Some(prior_revocation)
    );
    assert_eq!(writer.secrets_retire_key_lost(NOW + 3).unwrap(), 0);
    assert_eq!(
        writer.secret_record(&lost.context.id).unwrap(),
        Some(retired)
    );
    writer.secrets_set_vault_key_id(&"2".repeat(32)).unwrap();
    assert_eq!(writer.secret_vault_key_id().unwrap(), Some("2".repeat(32)));
}

#[test]
fn changing_or_adopting_key_identity_refuses_unretired_grants() {
    for register_first in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let writer = store.writer();
        if register_first {
            writer.secrets_set_vault_key_id(&"1".repeat(32)).unwrap();
        }
        writer
            .secrets_stage(&record("synthetic-user", agent_grant()))
            .unwrap();
        assert!(matches!(
            writer.secrets_set_vault_key_id(&"2".repeat(32)),
            Err(SecretError::Conflict)
        ));
        if register_first {
            writer.secrets_set_vault_key_id(&"1".repeat(32)).unwrap();
            assert_eq!(writer.secret_vault_key_id().unwrap(), Some("1".repeat(32)));
        } else {
            assert_eq!(writer.secret_vault_key_id().unwrap(), None);
        }
    }
}

#[test]
fn failed_retirement_rolls_back_tombstones_and_expected_key_identity_together() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    writer.secrets_set_vault_key_id(&"1".repeat(32)).unwrap();
    let rec = record("synthetic-user", agent_grant());
    writer.secrets_stage(&rec).unwrap();
    for trigger in [
        "CREATE TRIGGER synthetic_failure BEFORE UPDATE ON secret_records BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;",
        "CREATE TRIGGER synthetic_failure BEFORE UPDATE OF active_key_id ON secret_vault_state BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;",
    ] {
        writer.connection().execute_batch(trigger).unwrap();
        assert!(writer.secrets_retire_key_lost(NOW+1).is_err());
        assert_eq!(writer.secret_vault_key_id().unwrap(),Some("1".repeat(32)));
        assert_eq!(writer.secrets_unretired_count().unwrap(),1);
        assert_eq!(writer.secret_record(&rec.context.id).unwrap(),Some(rec.clone()));
        writer.connection().execute_batch("DROP TRIGGER synthetic_failure").unwrap();
    }
}

#[test]
fn existing_schema_24_gets_reason_column_and_key_state_without_losing_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let rec = record("synthetic-user", agent_grant());
    store.writer().secrets_stage(&rec).unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(dir.path().join("build.db")).unwrap();
    connection.execute_batch("ALTER TABLE secret_records DROP COLUMN revocation_reason; DROP TABLE secret_vault_state").unwrap();
    drop(connection);
    let store = Store::new(dir.path()).unwrap();
    assert_eq!(
        store.writer().secret_record(&rec.context.id).unwrap(),
        Some(rec)
    );
    assert_eq!(store.writer().secret_vault_key_id().unwrap(), None);
}

#[test]
fn malformed_key_id_and_inconsistent_revocation_reason_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    for invalid in ["", "short", "not-a-hex-id-of-exactly-32-bytes!!"] {
        assert!(writer.secrets_set_vault_key_id(invalid).is_err());
    }
    let rec = record("synthetic-user", agent_grant());
    writer.secrets_stage(&rec).unwrap();
    writer
        .connection()
        .execute(
            "UPDATE secret_records SET revocation_reason='key_lost' WHERE id=?1",
            [&rec.context.id],
        )
        .unwrap();
    assert!(writer.secret_record(&rec.context.id).is_err());
    writer
        .connection()
        .execute_batch("PRAGMA ignore_check_constraints=ON")
        .unwrap();
    writer
        .connection()
        .execute(
            "UPDATE secret_records SET revoked_at=?2,revocation_reason='unknown' WHERE id=?1",
            rusqlite::params![rec.context.id, NOW + 1],
        )
        .unwrap();
    assert!(writer.secret_record(&rec.context.id).is_err());
}
