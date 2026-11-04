use super::*;
use crate::secrets::{GrantScope, StageContext, STAGED_LIFETIME_SECONDS};
use crate::store::Store;
use std::sync::atomic::{AtomicI64, Ordering};
use tempfile::TempDir;

struct TestClock(AtomicI64);
impl Clock for TestClock {
    fn wall_seconds(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
    fn monotonic_seconds(&self) -> u64 {
        0
    }
}
fn fixture(
    ttl: Option<i64>,
    attach: bool,
) -> (TempDir, Store, Arc<TestClock>, SecretService, SecretRecord) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).unwrap();
    let clock = Arc::new(TestClock(AtomicI64::new(1000)));
    let service = SecretService::open_with_clock(
        dir.path(),
        "synthetic-device",
        store.writer(),
        clock.clone(),
    )
    .unwrap();
    let context = SecretContext {
        id: format!("sec_{}", uuid::Uuid::new_v4()),
        device_id: "synthetic-device".into(),
        creator: "synthetic-user".into(),
        name: "SYNTHETIC_NAME_MUST_NOT_LOG".into(),
        ttl_seconds: ttl,
        grant: GrantScope::Project {
            project_id: "synthetic-project".into(),
        },
        stage: StageContext {
            owner_id: "synthetic-project".into(),
            conversation_id: "synthetic-conversation".into(),
            generation: "synthetic-generation".into(),
            draft_id: None,
        },
    };
    let record = service
        .stage(
            context,
            SecretValue::new(b"synthetic value fixture".to_vec()).unwrap(),
            uuid::Uuid::new_v4().to_string(),
            "a".repeat(64),
        )
        .unwrap();
    if attach {
        service
            .attach(
                std::slice::from_ref(&record.context.id),
                "synthetic-operation",
            )
            .unwrap();
    }
    (dir, store, clock, service, record)
}
fn bytes(dir: &TempDir, record: &SecretRecord) -> (Vec<u8>, Vec<u8>) {
    (
        std::fs::read(dir.path().join("secrets/vault.key")).unwrap(),
        std::fs::read(dir.path().join("secrets").join(&record.context.id)).unwrap(),
    )
}
#[test]
fn startup_corrupt_metadata_closes_only_secret_access() {
    let (dir, store, clock, service, record) = fixture(Some(10), true);
    let before = bytes(&dir, &record);
    drop(service);
    store
        .writer_for_test()
        .execute(
            "UPDATE secret_records SET expires_at=NULL WHERE id=?1",
            [&record.context.id],
        )
        .unwrap();
    let closed =
        SecretService::open_with_clock(dir.path(), "synthetic-device", store.writer(), clock)
            .unwrap();
    assert!(matches!(
        closed.read(&record.context.id),
        Err(SecretError::Unavailable)
    ));
    assert_eq!(bytes(&dir, &record), before);
    let retired: bool = store
        .writer_for_test()
        .query_row(
            "SELECT revoked_at IS NOT NULL FROM secret_records WHERE id=?1",
            [&record.context.id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!retired);
}
#[test]
fn startup_retirement_database_failure_closes_only_secret_access() {
    let (dir, store, clock, service, record) = fixture(Some(10), true);
    let before = bytes(&dir, &record);
    drop(service);
    clock.0.store(1010, Ordering::SeqCst);
    store.writer_for_test().execute_batch("CREATE TRIGGER synthetic_refuse_expiry BEFORE UPDATE OF revoked_at ON secret_records BEGIN SELECT RAISE(ABORT,'synthetic expiry failure'); END;").unwrap();
    let closed =
        SecretService::open_with_clock(dir.path(), "synthetic-device", store.writer(), clock)
            .unwrap();
    assert!(matches!(
        closed.read(&record.context.id),
        Err(SecretError::Unavailable)
    ));
    assert_eq!(bytes(&dir, &record), before);
    assert!(store
        .writer()
        .secret_record(&record.context.id)
        .unwrap()
        .unwrap()
        .revoked_at
        .is_none());
}
#[test]
fn startup_filesystem_sweep_failure_keeps_the_service_closed_after_permissions_recover() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, store, clock, service, record) = fixture(None, true);
    let before = bytes(&dir, &record);
    drop(service);
    let path = dir.path().join("secrets");
    let closed = SecretService::open_with_clock_and_opener(
        dir.path(),
        "synthetic-device",
        store.writer(),
        clock,
        |root, has_records| {
            let vault = Vault::open(root, has_records)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750))?;
            Ok(vault)
        },
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(
        closed.read(&record.context.id),
        Err(SecretError::Unavailable)
    ));
    assert_eq!(bytes(&dir, &record), before);
}
#[test]
fn ttl_read_and_sweeper_stage_expiry_record_expired_reason() {
    let (_dir, store, clock, service, record) = fixture(Some(10), true);
    clock.0.store(1010, Ordering::SeqCst);
    assert!(matches!(
        service.read(&record.context.id),
        Err(SecretError::Unavailable)
    ));
    assert_eq!(
        store
            .writer()
            .secret_record(&record.context.id)
            .unwrap()
            .unwrap()
            .revocation_reason
            .as_deref(),
        Some("expired")
    );
    for attached in [false, true] {
        let (_dir, store, clock, service, record) =
            fixture(if attached { Some(10) } else { None }, attached);
        clock.0.store(
            1000 + if attached {
                10
            } else {
                STAGED_LIFETIME_SECONDS
            },
            Ordering::SeqCst,
        );
        service.sweep().unwrap();
        assert_eq!(
            store
                .writer()
                .secret_record(&record.context.id)
                .unwrap()
                .unwrap()
                .revocation_reason
                .as_deref(),
            Some("expired")
        );
    }
}
