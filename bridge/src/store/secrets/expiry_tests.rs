use super::tests::{agent_grant, record};
use crate::store::Store;

const NOW: i64 = 1_800_000_000;

#[test]
fn expiry_and_explicit_revocation_have_distinct_durable_reasons() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let expired = record("synthetic-user", agent_grant());
    let revoked = record("synthetic-user", agent_grant());
    writer.secrets_stage(&expired).unwrap();
    writer.secrets_stage(&revoked).unwrap();
    writer
        .secrets_attach(
            &[expired.context.id.clone(), revoked.context.id.clone()],
            "synthetic-operation",
            NOW + 1,
        )
        .unwrap();
    writer.connection().execute_batch("CREATE TABLE synthetic_expiry_durability (mode INTEGER NOT NULL); CREATE TRIGGER synthetic_expiry_durability AFTER UPDATE OF revoked_at ON secret_records WHEN NEW.revocation_reason='expired' BEGIN INSERT INTO synthetic_expiry_durability SELECT synchronous FROM pragma_synchronous; END;").unwrap();
    let expired = writer
        .secrets_expire(&expired.context.id, NOW + 61)
        .unwrap();
    let revoked = writer
        .secrets_revoke(&revoked.context.id, NOW + 62)
        .unwrap();
    assert_eq!(expired.revocation_reason.as_deref(), Some("expired"));
    assert_eq!(revoked.revocation_reason.as_deref(), Some("revoked"));
    let mode: i64 = writer
        .connection()
        .query_row("SELECT mode FROM synthetic_expiry_durability", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(mode >= 2);
    let restored: i64 = writer
        .connection()
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .unwrap();
    assert_eq!(restored, 1);
    drop(writer);
    drop(store);
    let store = Store::new(dir.path()).unwrap();
    assert_eq!(
        store.writer().secret_record(&expired.context.id).unwrap(),
        Some(expired)
    );
    assert_eq!(
        store.writer().secret_record(&revoked.context.id).unwrap(),
        Some(revoked)
    );
}

#[test]
fn expiration_preserves_existing_revoked_expired_and_key_lost_tombstones() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let revoked = record("synthetic-user", agent_grant());
    let expired = record("synthetic-user", agent_grant());
    let key_lost = record("synthetic-user", agent_grant());
    for rec in [&revoked, &expired, &key_lost] {
        writer.secrets_stage(rec).unwrap();
    }
    let revoked = writer.secrets_revoke(&revoked.context.id, NOW + 1).unwrap();
    let expired = writer.secrets_expire(&expired.context.id, NOW + 2).unwrap();
    assert_eq!(writer.secrets_retire_key_lost(NOW + 3).unwrap(), 1);
    let key_lost = writer.secret_record(&key_lost.context.id).unwrap().unwrap();
    for original in [&revoked, &expired, &key_lost] {
        assert_eq!(
            writer
                .secrets_expire(&original.context.id, NOW + 100)
                .unwrap(),
            *original
        );
        assert_eq!(
            writer
                .secrets_revoke(&original.context.id, NOW + 101)
                .unwrap(),
            *original
        );
    }
}

#[test]
fn failed_expiration_does_not_change_the_live_record() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let rec = record("synthetic-user", agent_grant());
    writer.secrets_stage(&rec).unwrap();
    writer.connection().execute_batch("CREATE TRIGGER synthetic_expiry_failure BEFORE UPDATE OF revoked_at ON secret_records BEGIN SELECT RAISE(ABORT,'synthetic expiry failure'); END").unwrap();
    assert!(writer.secrets_expire(&rec.context.id, NOW + 1).is_err());
    assert_eq!(writer.secret_record(&rec.context.id).unwrap(), Some(rec));
    let restored: i64 = writer
        .connection()
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .unwrap();
    assert_eq!(restored, 1);
}
