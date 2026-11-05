use super::*;
use crate::secrets::{GrantScope, StageContext, MAX_VALUE_BYTES, STAGED_LIFETIME_SECONDS};
use crate::store::Store;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use tempfile::TempDir;

struct TestClock {
    wall: AtomicI64,
    mono: AtomicU64,
}
impl Clock for TestClock {
    fn wall_seconds(&self) -> i64 {
        self.wall.load(Ordering::SeqCst)
    }
    fn monotonic_seconds(&self) -> u64 {
        self.mono.load(Ordering::SeqCst)
    }
}
fn clock() -> Arc<TestClock> {
    Arc::new(TestClock {
        wall: AtomicI64::new(1000),
        mono: AtomicU64::new(0),
    })
}
fn context(ttl: Option<i64>) -> SecretContext {
    SecretContext {
        id: format!("sec_{}", uuid::Uuid::new_v4()),
        device_id: "synthetic-device".into(),
        creator: "synthetic-user".into(),
        name: "SYNTHETIC_TOKEN".into(),
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
    }
}
fn fixture() -> (TempDir, Store, Arc<TestClock>, SecretService) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).unwrap();
    let clock = clock();
    let service = SecretService::open_with_clock(
        dir.path(),
        "synthetic-device",
        store.writer(),
        clock.clone(),
    )
    .unwrap();
    (dir, store, clock, service)
}
fn stage(service: &SecretService, ttl: Option<i64>) -> SecretRecord {
    service
        .stage(
            context(ttl),
            SecretValue::new(b"synthetic-value-only".to_vec()).unwrap(),
            uuid::Uuid::new_v4().to_string(),
            "a".repeat(64),
        )
        .unwrap()
}
fn attach(service: &SecretService, record: &SecretRecord) {
    service
        .attach(
            std::slice::from_ref(&record.context.id),
            &format!("synthetic-message-{}", record.context.id),
        )
        .unwrap();
}
#[test]
fn value_limits_and_no_plaintext_debug_or_serialization() {
    assert!(SecretValue::new(vec![b'x'; MAX_VALUE_BYTES]).is_ok());
    assert!(SecretValue::new(vec![b'x'; MAX_VALUE_BYTES + 1]).is_err());
    assert!(SecretValue::new(b"synthetic\0data".to_vec()).is_err());
    assert!(SecretValue::new(Vec::new()).is_ok());
}
#[test]
fn ttl_starts_on_attach_and_boundary_is_closed() {
    let (_dir, _store, clock, service) = fixture();
    let record = stage(&service, Some(10));
    assert!(service.read(&record.context.id).is_err());
    clock.wall.store(1100, Ordering::SeqCst);
    clock.mono.store(100, Ordering::SeqCst);
    attach(&service, &record);
    clock.wall.store(1109, Ordering::SeqCst);
    clock.mono.store(109, Ordering::SeqCst);
    assert_eq!(
        service.read(&record.context.id).unwrap().as_bytes(),
        b"synthetic-value-only"
    );
    clock.wall.store(1110, Ordering::SeqCst);
    clock.mono.store(110, Ordering::SeqCst);
    assert!(service.read(&record.context.id).is_err());
}
#[test]
fn null_expiry_survives_rollback_but_ttl_fails_closed_until_recovery() {
    let (_dir, _store, clock, service) = fixture();
    let ttl = stage(&service, Some(100));
    let forever = stage(&service, None);
    attach(&service, &ttl);
    attach(&service, &forever);
    clock.wall.store(1020, Ordering::SeqCst);
    clock.mono.store(20, Ordering::SeqCst);
    assert!(service.read(&ttl.context.id).is_ok());
    clock.wall.store(1010, Ordering::SeqCst);
    clock.mono.store(21, Ordering::SeqCst);
    assert!(matches!(
        service.read(&ttl.context.id),
        Err(SecretError::ClockRollback)
    ));
    assert!(service.read(&forever.context.id).is_ok());
    clock.wall.store(1020, Ordering::SeqCst);
    assert!(service.read(&ttl.context.id).is_ok());
}
#[test]
fn monotonic_deadline_prevents_frozen_wall_time_from_extending_ttl() {
    let (_dir, _store, clock, service) = fixture();
    let record = stage(&service, Some(10));
    attach(&service, &record);
    clock.mono.store(10, Ordering::SeqCst);
    assert!(service.read(&record.context.id).is_err());
}
#[test]
fn restart_preserves_records_absolute_deadlines_and_clock_highwater() {
    let (dir, store, clock, service) = fixture();
    let record = stage(&service, Some(100));
    attach(&service, &record);
    clock.wall.store(1050, Ordering::SeqCst);
    clock.mono.store(50, Ordering::SeqCst);
    assert!(service.read(&record.context.id).is_ok());
    drop(service);
    drop(store);
    let store = Store::new(dir.path().join("tasks")).unwrap();
    clock.wall.store(1040, Ordering::SeqCst);
    clock.mono.store(0, Ordering::SeqCst);
    let service = SecretService::open_with_clock(
        dir.path(),
        "synthetic-device",
        store.writer(),
        clock.clone(),
    )
    .unwrap();
    assert!(matches!(
        service.read(&record.context.id),
        Err(SecretError::ClockRollback)
    ));
    clock.wall.store(1050, Ordering::SeqCst);
    assert!(service.read(&record.context.id).is_ok());
    clock.wall.store(1100, Ordering::SeqCst);
    assert!(service.read(&record.context.id).is_err());
}
#[test]
fn startup_sweeps_orphans_and_staged_expiry_without_touching_live_blobs() {
    let (dir, store, clock, service) = fixture();
    let staged = stage(&service, None);
    let live = stage(&service, None);
    attach(&service, &live);
    let orphan = context(None);
    service
        .inner
        .vault
        .as_ref()
        .unwrap()
        .write(
            &orphan,
            &SecretValue::new(b"synthetic-orphan".to_vec()).unwrap(),
        )
        .unwrap();
    drop(service);
    clock
        .wall
        .store(1000 + STAGED_LIFETIME_SECONDS, Ordering::SeqCst);
    let service =
        SecretService::open_with_clock(dir.path(), "synthetic-device", store.writer(), clock)
            .unwrap();
    assert!(!dir.path().join("secrets").join(&orphan.id).exists());
    assert!(!dir.path().join("secrets").join(&staged.context.id).exists());
    assert!(service.read(&live.context.id).is_ok());
}
#[test]
fn revoke_is_durable_before_failed_unlink_and_sweep_retries() {
    let (dir, store, _clock, service) = fixture();
    let record = stage(&service, None);
    attach(&service, &record);
    let path = dir.path().join("secrets").join(&record.context.id);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let result = service.revoke(&record.context.id).unwrap();
    assert!(result.revoked_at.is_some());
    assert!(store
        .writer()
        .secret_record(&record.context.id)
        .unwrap()
        .unwrap()
        .revoked_at
        .is_some());
    assert!(service.read(&record.context.id).is_err());
    assert!(path.exists());
    std::fs::remove_dir(&path).unwrap();
    service.sweep().unwrap();
    assert!(
        !store
            .writer()
            .secret_record(&record.context.id)
            .unwrap()
            .unwrap()
            .blob_present
    );
}
#[test]
fn missing_key_and_device_identity_change_fail_closed() {
    let (dir, store, clock, service) = fixture();
    let record = stage(&service, None);
    attach(&service, &record);
    drop(service);
    let service = SecretService::open_with_clock(
        dir.path(),
        "different-synthetic-device",
        store.writer(),
        clock.clone(),
    )
    .unwrap();
    assert!(service.read(&record.context.id).is_err());
    drop(service);
    std::fs::remove_file(dir.path().join("secrets/vault.key")).unwrap();
    let recovered =
        SecretService::open_with_clock(dir.path(), "synthetic-device", store.writer(), clock)
            .unwrap();
    assert!(matches!(
        recovered.read(&record.context.id),
        Err(SecretError::Unavailable)
    ));
    assert!(dir.path().join("secrets/vault.key").exists());
}
#[test]
fn four_jobs_are_admitted_without_eviction() {
    let (_dir, _store, _clock, service) = fixture();
    let permits: Vec<_> = (0..MAX_CONCURRENT_JOBS)
        .map(|_| service.inner.jobs.clone().try_acquire_owned().unwrap())
        .collect();
    assert!(matches!(
        service.stage(
            context(None),
            SecretValue::new(b"synthetic".to_vec()).unwrap(),
            uuid::Uuid::new_v4().to_string(),
            "b".repeat(64)
        ),
        Err(SecretError::Limit)
    ));
    drop(permits);
    assert!(stage(&service, None).blob_present);
}
#[tokio::test]
async fn periodic_sweep_needs_no_app_lock_or_client() {
    let (dir, _store, clock, service) = fixture();
    let record = stage(&service, None);
    let handle = service.spawn_sweeper(std::time::Duration::from_millis(5));
    clock
        .wall
        .store(1000 + STAGED_LIFETIME_SECONDS, Ordering::SeqCst);
    clock
        .mono
        .store(STAGED_LIFETIME_SECONDS as u64, Ordering::SeqCst);
    let path = dir.path().join("secrets").join(&record.context.id);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while path.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    handle.stop().await;
}

#[test]
fn monotonic_expiry_is_durable_across_restart_with_frozen_utc() {
    let (dir, store, clock, service) = fixture();
    let record = stage(&service, Some(10));
    attach(&service, &record);
    clock.mono.store(10, Ordering::SeqCst);
    assert!(service.read(&record.context.id).is_err());
    drop(service);
    clock.mono.store(0, Ordering::SeqCst);
    let service =
        SecretService::open_with_clock(dir.path(), "synthetic-device", store.writer(), clock)
            .unwrap();
    assert!(service.read(&record.context.id).is_err());
}

#[test]
fn monotonic_stage_expiry_is_durable_across_restart_with_frozen_utc() {
    let (dir, store, clock, service) = fixture();
    let record = stage(&service, None);
    clock
        .mono
        .store(STAGED_LIFETIME_SECONDS as u64, Ordering::SeqCst);
    assert!(service
        .attach(
            std::slice::from_ref(&record.context.id),
            "synthetic-expired-stage"
        )
        .is_err());
    drop(service);
    clock.mono.store(0, Ordering::SeqCst);
    let service =
        SecretService::open_with_clock(dir.path(), "synthetic-device", store.writer(), clock)
            .unwrap();
    assert!(service
        .attach(
            std::slice::from_ref(&record.context.id),
            "synthetic-expired-stage"
        )
        .is_err());
}

#[test]
fn monotonic_expiry_during_rollback_remains_retired_after_clock_recovery() {
    let (dir, store, clock, service) = fixture();
    let record = stage(&service, Some(10));
    attach(&service, &record);
    clock.wall.store(999, Ordering::SeqCst);
    clock.mono.store(10, Ordering::SeqCst);
    assert!(service.read(&record.context.id).is_err());
    drop(service);
    clock.mono.store(0, Ordering::SeqCst);
    clock.wall.store(1000, Ordering::SeqCst);
    let service =
        SecretService::open_with_clock(dir.path(), "synthetic-device", store.writer(), clock)
            .unwrap();
    assert!(service.read(&record.context.id).is_err());
}

#[test]
fn key_loss_recovers_storage_only_after_retiring_old_contexts() {
    let (dir, store, clock, service) = fixture();
    let record = stage(&service, None);
    attach(&service, &record);
    let key = dir.path().join("secrets/vault.key");
    let original = std::fs::read(&key).unwrap();
    drop(service);
    std::fs::remove_dir_all(dir.path().join("secrets")).unwrap();
    let recovered =
        SecretService::open_with_clock(dir.path(), "synthetic-device", store.writer(), clock)
            .unwrap();
    let retired = store
        .writer()
        .secret_record(&record.context.id)
        .unwrap()
        .unwrap();
    assert_eq!(retired.revocation_reason.as_deref(), Some("key_lost"));
    assert!(retired.revoked_at.is_some());
    assert!(!retired.blob_present);
    assert_ne!(std::fs::read(&key).unwrap(), original);
    assert!(recovered.read(&record.context.id).is_err());
    let fresh = stage(&recovered, None);
    attach(&recovered, &fresh);
    assert!(recovered.read(&fresh.context.id).is_ok());
}

#[test]
fn retirement_failure_keeps_missing_key_and_all_secret_access_unavailable() {
    let (dir, store, clock, service) = fixture();
    let record = stage(&service, None);
    attach(&service, &record);
    drop(service);
    let key = dir.path().join("secrets/vault.key");
    std::fs::remove_file(&key).unwrap();
    store.writer_for_test().execute_batch("CREATE TRIGGER synthetic_refuse_key_loss BEFORE UPDATE OF revoked_at ON secret_records BEGIN SELECT RAISE(ABORT,'synthetic retirement failure'); END;").unwrap();
    let original = store.writer().secret_record(&record.context.id).unwrap();
    let unavailable = SecretService::open_with_clock(
        dir.path(),
        "synthetic-device",
        store.writer(),
        clock.clone(),
    )
    .unwrap();
    assert!(unavailable.read(&record.context.id).is_err());
    assert!(!key.exists());
    assert_eq!(
        store.writer().secret_record(&record.context.id).unwrap(),
        original
    );
    drop(unavailable);
    store
        .writer_for_test()
        .execute_batch("DROP TRIGGER synthetic_refuse_key_loss")
        .unwrap();
    let recovered =
        SecretService::open_with_clock(dir.path(), "synthetic-device", store.writer(), clock)
            .unwrap();
    assert_eq!(
        store
            .writer()
            .secret_record(&record.context.id)
            .unwrap()
            .unwrap()
            .revocation_reason
            .as_deref(),
        Some("key_lost")
    );
    assert!(recovered.read(&record.context.id).is_err());
    let fresh = stage(&recovered, None);
    attach(&recovered, &fresh);
    assert!(recovered.read(&fresh.context.id).is_ok());
}

#[derive(PartialEq, Eq)]
struct RecoverySnapshot {
    records: Vec<SecretRecord>,
    expected_key_id: Option<String>,
    key: Vec<u8>,
    blobs: Vec<(String, Vec<u8>, u32)>,
    directory_mode: u32,
    key_mode: u32,
}

impl std::fmt::Debug for RecoverySnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RecoverySnapshot")
            .field("records_count", &self.records.len())
            .field("blob_count", &self.blobs.len())
            .field("key_length", &self.key.len())
            .finish_non_exhaustive()
    }
}

fn recovery_snapshot(dir: &std::path::Path, store: &Store) -> RecoverySnapshot {
    use std::os::unix::fs::PermissionsExt;
    let records = store.writer().secret_records().unwrap();
    let blobs = records
        .iter()
        .map(|record| {
            let path = dir.join("secrets").join(&record.context.id);
            (
                record.context.id.clone(),
                std::fs::read(&path).unwrap(),
                std::fs::metadata(path).unwrap().permissions().mode(),
            )
        })
        .collect();
    RecoverySnapshot {
        records,
        expected_key_id: store
            .writer_for_test()
            .query_row(
                "SELECT active_key_id FROM secret_vault_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap(),
        key: std::fs::read(dir.join("secrets/vault.key")).unwrap(),
        blobs,
        directory_mode: std::fs::metadata(dir.join("secrets"))
            .unwrap()
            .permissions()
            .mode(),
        key_mode: std::fs::metadata(dir.join("secrets/vault.key"))
            .unwrap()
            .permissions()
            .mode(),
    }
}

#[test]
fn corrupt_present_key_closes_only_secret_access_and_preserves_recoverable_grants() {
    for same_length in [false, true] {
        let (dir, store, clock, service) = fixture();
        let record = stage(&service, None);
        attach(&service, &record);
        drop(service);
        let key = dir.path().join("secrets/vault.key");
        let original = std::fs::read(&key).unwrap();
        let mut corrupt = original.clone();
        if same_length {
            corrupt[0] ^= 1;
        } else {
            corrupt.truncate(1);
        }
        std::fs::write(&key, corrupt).unwrap();
        let before = recovery_snapshot(dir.path(), &store);
        let unavailable = SecretService::open_with_clock(
            dir.path(),
            "synthetic-device",
            store.writer(),
            clock.clone(),
        )
        .unwrap();
        assert!(unavailable.inner.vault.is_none());
        assert!(unavailable.read(&record.context.id).is_err());
        assert_eq!(recovery_snapshot(dir.path(), &store), before);
        drop(unavailable);
        std::fs::write(&key, original).unwrap();
        let restored =
            SecretService::open_with_clock(dir.path(), "synthetic-device", store.writer(), clock)
                .unwrap();
        assert_eq!(
            restored.read(&record.context.id).unwrap().as_bytes(),
            b"synthetic-value-only"
        );
        assert!(store
            .writer()
            .secret_record(&record.context.id)
            .unwrap()
            .unwrap()
            .revoked_at
            .is_none());
    }
}

#[test]
fn present_key_mode_directory_mode_and_hardlink_drift_leave_vault_and_rows_untouched() {
    use std::os::unix::fs::PermissionsExt;
    for damage in ["key_mode", "directory_mode", "hardlink"] {
        let (dir, store, clock, service) = fixture();
        let record = stage(&service, None);
        attach(&service, &record);
        drop(service);
        let key = dir.path().join("secrets/vault.key");
        let directory = dir.path().join("secrets");
        match damage {
            "key_mode" => {
                std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap()
            }
            "directory_mode" => {
                std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755))
                    .unwrap()
            }
            _ => std::fs::hard_link(&key, dir.path().join("synthetic-linked-key")).unwrap(),
        }
        let before = recovery_snapshot(dir.path(), &store);
        let unavailable = SecretService::open_with_clock(
            dir.path(),
            "synthetic-device",
            store.writer(),
            clock.clone(),
        )
        .unwrap();
        assert!(unavailable.inner.vault.is_none());
        assert_eq!(recovery_snapshot(dir.path(), &store), before);
        drop(unavailable);
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        if damage == "hardlink" {
            std::fs::remove_file(dir.path().join("synthetic-linked-key")).unwrap();
        }
        let restored =
            SecretService::open_with_clock(dir.path(), "synthetic-device", store.writer(), clock)
                .unwrap();
        assert!(restored.read(&record.context.id).is_ok());
    }
}

#[test]
fn expected_key_id_mismatch_malformed_or_missing_with_present_key_never_resets() {
    for replacement in [
        Some("f".repeat(32)),
        Some("synthetic-malformed-key-id".into()),
        None,
    ] {
        let (dir, store, clock, service) = fixture();
        let record = stage(&service, None);
        attach(&service, &record);
        drop(service);
        let expected = store.writer().secret_vault_key_id().unwrap();
        store
            .writer_for_test()
            .execute(
                "UPDATE secret_vault_state SET active_key_id=?1",
                [replacement],
            )
            .unwrap();
        let before = recovery_snapshot(dir.path(), &store);
        let unavailable = SecretService::open_with_clock(
            dir.path(),
            "synthetic-device",
            store.writer(),
            clock.clone(),
        )
        .unwrap();
        assert!(unavailable.inner.vault.is_none());
        assert_eq!(recovery_snapshot(dir.path(), &store), before);
        drop(unavailable);
        store
            .writer_for_test()
            .execute("UPDATE secret_vault_state SET active_key_id=?1", [expected])
            .unwrap();
        let restored =
            SecretService::open_with_clock(dir.path(), "synthetic-device", store.writer(), clock)
                .unwrap();
        assert!(restored.read(&record.context.id).is_ok());
    }
}

#[test]
fn injected_owner_eio_and_false_not_found_faults_preserve_every_grant_and_file() {
    for fault in ["owner", "io", "not_found"] {
        let (dir, store, clock, service) = fixture();
        let record = stage(&service, None);
        attach(&service, &record);
        drop(service);
        let before = recovery_snapshot(dir.path(), &store);
        let unavailable = SecretService::open_with_clock_and_opener(
            dir.path(),
            "synthetic-device",
            store.writer(),
            clock.clone(),
            |_, _| match fault {
                "io" => Err(SecretError::Io(std::io::Error::from_raw_os_error(
                    libc::EIO,
                ))),
                "not_found" => Err(SecretError::Io(std::io::ErrorKind::NotFound.into())),
                _ => Err(SecretError::Unavailable),
            },
        )
        .unwrap();
        assert!(unavailable.inner.vault.is_none());
        assert!(unavailable.read(&record.context.id).is_err());
        assert_eq!(recovery_snapshot(dir.path(), &store), before);
        drop(unavailable);
        let restored =
            SecretService::open_with_clock(dir.path(), "synthetic-device", store.writer(), clock)
                .unwrap();
        assert!(restored.read(&record.context.id).is_ok());
    }
}

#[test]
fn present_unregistered_key_with_no_records_is_not_adopted_or_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).unwrap();
    let vault = Vault::open(dir.path(), false).unwrap();
    let key_path = dir.path().join("secrets/vault.key");
    let key = std::fs::read(&key_path).unwrap();
    assert_eq!(store.writer().secret_vault_key_id().unwrap(), None);
    let service =
        SecretService::open_with_clock(dir.path(), "synthetic-device", store.writer(), clock())
            .unwrap();
    assert!(service.inner.vault.is_none());
    assert_eq!(std::fs::read(key_path).unwrap(), key);
    assert_eq!(store.writer().secret_vault_key_id().unwrap(), None);
    assert!(!store.writer().secrets_have_records().unwrap());
    drop(vault);
}
