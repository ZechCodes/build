use super::*;

#[test]
fn unavailable_diagnostic_uses_exact_count_only_format() {
    for (count, expected) in [
        (0, "secret vault unavailable records: 0"),
        (2, "secret vault unavailable records: 2"),
        (1000, "secret vault unavailable records: 1000"),
    ] {
        let mut captured = Vec::new();
        emit_unavailable(count, |message| captured.push(message.to_owned()));
        assert_eq!(captured, vec![expected]);
        for forbidden in [
            "SYNTHETIC_PRIVATE_NAME",
            "sec_synthetic-private-id",
            "synthetic-device",
            "vault.key",
        ] {
            assert!(!captured[0].contains(forbidden));
        }
    }
}

#[test]
fn pending_publication_resumes_after_interruption_before_registration() {
    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::Store::new(dir.path().join("tasks")).unwrap();
    let writer = store.writer();
    writer.secrets_retire_key_lost(1000).unwrap();
    let vault = Vault::open(dir.path(), false).unwrap();
    let key_id = vault.key_id();
    // Model the durable publication intent written by the new protocol. The
    // conditional ALTER lets this regression run against the previous schema.
    let connection = store.writer_for_test();
    let columns = connection
        .prepare("PRAGMA table_info(secret_vault_state)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    if !columns.iter().any(|column| column == "pending_key_id") {
        connection
            .execute_batch("ALTER TABLE secret_vault_state ADD COLUMN pending_key_id TEXT")
            .unwrap();
    }
    connection
        .execute(
            "UPDATE secret_vault_state SET pending_key_id=?1 WHERE singleton=1",
            [&key_id],
        )
        .unwrap();
    drop(connection);
    drop(vault);
    let original = std::fs::read(dir.path().join("secrets/vault.key")).unwrap();
    let resumed = open_vault_with(dir.path(), &writer, 1001, Vault::open).unwrap();
    assert!(
        resumed.is_some(),
        "matching durable pending key must resume registration"
    );
    assert_eq!(writer.secret_vault_key_id().unwrap(), Some(key_id));
    let pending: Option<String> = store
        .writer_for_test()
        .query_row(
            "SELECT pending_key_id FROM secret_vault_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, None);
    assert_eq!(
        std::fs::read(dir.path().join("secrets/vault.key")).unwrap(),
        original
    );
}

fn pending_fixture() -> (tempfile::TempDir, crate::store::Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::Store::new(dir.path().join("tasks")).unwrap();
    store.writer().secrets_retire_key_lost(1000).unwrap();
    (dir, store)
}

#[test]
fn failed_pending_reservation_never_publishes_a_key() {
    let (dir, store) = pending_fixture();
    store.writer_for_test().execute_batch("CREATE TRIGGER synthetic_prepare_failure BEFORE UPDATE OF pending_key_id ON secret_vault_state WHEN NEW.pending_key_id IS NOT NULL BEGIN SELECT RAISE(ABORT,'synthetic reservation failure'); END;").unwrap();
    assert!(
        open_vault_with(dir.path(), &store.writer(), 1000, Vault::open)
            .unwrap()
            .is_none()
    );
    assert!(!dir.path().join("secrets/vault.key").exists());
    assert_eq!(store.writer().secret_vault_pending_key_id().unwrap(), None);
}

#[test]
fn failed_post_rename_directory_fsync_leaves_a_resumable_pending_key() {
    let (dir, store) = pending_fixture();
    let writer = store.writer();
    let candidate = Vault::prepare_after_retirement(dir.path()).unwrap();
    let key_id = candidate.key_id();
    writer.secrets_prepare_vault_key(&key_id).unwrap();
    let mut after_rename = false;
    assert!(candidate
        .publish_key_with_sync(|_| {
            after_rename = dir.path().join("secrets/vault.key").exists();
            Err(std::io::Error::from_raw_os_error(libc::EIO))
        })
        .is_err());
    assert!(after_rename);
    let bytes = std::fs::read(dir.path().join("secrets/vault.key")).unwrap();
    drop(candidate);
    assert_eq!(writer.secret_vault_key_id().unwrap(), None);
    assert_eq!(
        writer.secret_vault_pending_key_id().unwrap(),
        Some(key_id.clone())
    );
    let resumed = open_vault_with(dir.path(), &writer, 1001, Vault::open)
        .unwrap()
        .unwrap();
    assert_eq!(resumed.key_id(), key_id);
    assert_eq!(
        std::fs::read(dir.path().join("secrets/vault.key")).unwrap(),
        bytes
    );
    assert_eq!(writer.secret_vault_pending_key_id().unwrap(), None);
}

#[test]
fn pending_record_without_key_creates_a_new_candidate_after_retirement() {
    let (dir, store) = pending_fixture();
    let writer = store.writer();
    let candidate = Vault::prepare_after_retirement(dir.path()).unwrap();
    let interrupted = candidate.key_id();
    writer.secrets_prepare_vault_key(&interrupted).unwrap();
    drop(candidate);
    assert!(!dir.path().join("secrets/vault.key").exists());
    let resumed = open_vault_with(dir.path(), &writer, 1001, Vault::open)
        .unwrap()
        .unwrap();
    assert_ne!(resumed.key_id(), interrupted);
    assert_eq!(writer.secret_vault_pending_key_id().unwrap(), None);
    assert_eq!(
        writer.secret_vault_key_id().unwrap(),
        Some(resumed.key_id())
    );
}

#[test]
fn pending_foreign_and_corrupt_keys_are_preserved_without_marker_changes() {
    for damage in ["foreign", "short", "substituted"] {
        let (dir, store) = pending_fixture();
        let writer = store.writer();
        let candidate = Vault::prepare_after_retirement(dir.path()).unwrap();
        let pending = candidate.key_id();
        writer.secrets_prepare_vault_key(&pending).unwrap();
        if damage == "foreign" {
            Vault::reinitialize_after_retirement(dir.path()).unwrap();
        } else {
            candidate.publish_key().unwrap();
            let path = dir.path().join("secrets/vault.key");
            let mut bytes = std::fs::read(&path).unwrap();
            if damage == "short" {
                bytes.truncate(1);
            } else {
                bytes[0] ^= 1;
            }
            std::fs::write(path, bytes).unwrap();
        }
        let path = dir.path().join("secrets/vault.key");
        let bytes = std::fs::read(&path).unwrap();
        assert!(open_vault_with(dir.path(), &writer, 1001, Vault::open)
            .unwrap()
            .is_none());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert_eq!(writer.secret_vault_pending_key_id().unwrap(), Some(pending));
        assert_eq!(writer.secret_vault_key_id().unwrap(), None);
    }
}

struct RecoveryClock;
impl crate::secrets::Clock for RecoveryClock {
    fn wall_seconds(&self) -> i64 {
        1000
    }
    fn monotonic_seconds(&self) -> u64 {
        0
    }
}

fn recovery_context() -> crate::secrets::SecretContext {
    use crate::secrets::{GrantScope, SecretContext, StageContext};
    SecretContext {
        id: format!("sec_{}", uuid::Uuid::new_v4()),
        device_id: "synthetic-device".into(),
        creator: "synthetic-user".into(),
        name: "SYNTHETIC_VALUE".into(),
        ttl_seconds: None,
        grant: GrantScope::Project {
            project_id: "synthetic-project".into(),
        },
        stage: StageContext {
            owner_id: "synthetic-project".into(),
            conversation_id: "synthetic-conversation".into(),
            generation: "synthetic-generation".into(),
            draft_id: None,
        },
    }
}

fn recovery_service(
    dir: &std::path::Path,
    store: &crate::store::Store,
) -> crate::secrets::SecretService {
    crate::secrets::SecretService::open_with_clock(
        dir,
        "synthetic-device",
        store.writer(),
        std::sync::Arc::new(RecoveryClock),
    )
    .unwrap()
}

fn recovery_stage(service: &crate::secrets::SecretService) -> crate::secrets::SecretRecord {
    let record = service
        .stage(
            recovery_context(),
            crate::secrets::SecretValue::new(b"synthetic recovery fixture".to_vec()).unwrap(),
            uuid::Uuid::new_v4().to_string(),
            "a".repeat(64),
        )
        .unwrap();
    service
        .attach(
            std::slice::from_ref(&record.context.id),
            &format!("synthetic-op-{}", record.context.id),
        )
        .unwrap();
    record
}

fn registration_failure_fixture(
    with_old_grant: bool,
) -> (
    tempfile::TempDir,
    crate::store::Store,
    Option<crate::secrets::SecretRecord>,
) {
    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::Store::new(dir.path().join("tasks")).unwrap();
    let old = if with_old_grant {
        let service = recovery_service(dir.path(), &store);
        let record = recovery_stage(&service);
        drop(service);
        std::fs::remove_file(dir.path().join("secrets/vault.key")).unwrap();
        Some(record)
    } else {
        None
    };
    store.writer_for_test().execute_batch("CREATE TRIGGER synthetic_register_failure BEFORE UPDATE OF active_key_id ON secret_vault_state WHEN NEW.active_key_id IS NOT NULL BEGIN SELECT RAISE(ABORT,'synthetic registration failure'); END;").unwrap();
    (dir, store, old)
}

fn cold_reopen_after_registration_failure(
    dir: &std::path::Path,
    store: crate::store::Store,
) -> crate::store::Store {
    drop(store);
    let tasks = dir.join("tasks");
    let connection = rusqlite::Connection::open(tasks.join("build.db")).unwrap();
    connection
        .execute_batch("DROP TRIGGER synthetic_register_failure")
        .unwrap();
    drop(connection);
    crate::store::Store::new(tasks).unwrap()
}

fn assert_old_grant_retired(
    store: &crate::store::Store,
    old: Option<&crate::secrets::SecretRecord>,
) {
    if let Some(old) = old {
        assert_eq!(
            store
                .writer()
                .secret_record(&old.context.id)
                .unwrap()
                .unwrap()
                .revocation_reason
                .as_deref(),
            Some("key_lost")
        );
    }
}

#[test]
fn failed_registration_cold_restart_keeps_key_lost_tombstones_and_reuses_published_key() {
    for with_old_grant in [false, true] {
        let (dir, store, old) = registration_failure_fixture(with_old_grant);
        let closed = recovery_service(dir.path(), &store);
        assert!(closed
            .stage(
                recovery_context(),
                crate::secrets::SecretValue::new(b"synthetic refused fixture".to_vec()).unwrap(),
                uuid::Uuid::new_v4().to_string(),
                "a".repeat(64)
            )
            .is_err());
        let bytes = std::fs::read(dir.path().join("secrets/vault.key")).unwrap();
        let pending = store
            .writer()
            .secret_vault_pending_key_id()
            .unwrap()
            .unwrap();
        assert_eq!(store.writer().secret_vault_key_id().unwrap(), None);
        assert_eq!(Vault::open(dir.path(), true).unwrap().key_id(), pending);
        assert_old_grant_retired(&store, old.as_ref());
        drop(closed);
        let store = cold_reopen_after_registration_failure(dir.path(), store);
        let resumed = recovery_service(dir.path(), &store);
        assert_eq!(
            std::fs::read(dir.path().join("secrets/vault.key")).unwrap(),
            bytes
        );
        assert_eq!(store.writer().secret_vault_key_id().unwrap(), Some(pending));
        assert_eq!(store.writer().secret_vault_pending_key_id().unwrap(), None);
        assert_old_grant_retired(&store, old.as_ref());
        if let Some(old) = old {
            assert!(resumed.read(&old.context.id).is_err());
        }
        let fresh = recovery_stage(&resumed);
        assert_eq!(
            resumed.read(&fresh.context.id).unwrap().as_bytes(),
            b"synthetic recovery fixture"
        );
    }
}

#[test]
fn ignored_pending_reservation_does_not_publish_an_unknown_key() {
    let (dir, store) = pending_fixture();
    store.writer_for_test().execute_batch("CREATE TRIGGER synthetic_ignore_prepare BEFORE UPDATE OF pending_key_id ON secret_vault_state WHEN NEW.pending_key_id IS NOT NULL BEGIN SELECT RAISE(IGNORE); END;").unwrap();
    assert!(
        open_vault_with(dir.path(), &store.writer(), 1000, Vault::open)
            .unwrap()
            .is_none()
    );
    assert!(!dir.path().join("secrets/vault.key").exists());
    assert_eq!(store.writer().secret_vault_pending_key_id().unwrap(), None);
    assert_eq!(store.writer().secret_vault_key_id().unwrap(), None);
}
