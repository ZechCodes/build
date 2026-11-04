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
    for corrupt in [false, true] {
        let (dir, store, clock, service) = fixture();
        let record = stage(&service, None);
        attach(&service, &record);
        let key = dir.path().join("secrets/vault.key");
        let original = std::fs::read(&key).unwrap();
        drop(service);
        if corrupt {
            std::fs::write(&key, [42u8; 32]).unwrap();
        } else {
            std::fs::remove_dir_all(dir.path().join("secrets")).unwrap();
        }
        let recovered =
            SecretService::open_with_clock(dir.path(), "synthetic-device", store.writer(), clock)
                .unwrap();
        assert!(matches!(
            recovered.read(&record.context.id),
            Err(SecretError::Unavailable)
        ));
        let retired = store
            .writer()
            .secret_record(&record.context.id)
            .unwrap()
            .unwrap();
        assert_eq!(retired.revocation_reason.as_deref(), Some("key_lost"));
        assert!(retired.revoked_at.is_some());
        assert!(!retired.blob_present);
        assert_ne!(std::fs::read(&key).unwrap(), original);
        let new = stage(&recovered, None);
        attach(&recovered, &new);
        assert_eq!(
            recovered.read(&new.context.id).unwrap().as_bytes(),
            b"synthetic-value-only"
        );
        assert!(matches!(
            recovered.read(&record.context.id),
            Err(SecretError::Unavailable)
        ));
    }
}

#[test]
fn retirement_failure_keeps_bad_key_and_all_secret_access_unavailable() {
    let (dir, store, clock, service) = fixture();
    let record = stage(&service, None);
    attach(&service, &record);
    let key = dir.path().join("secrets/vault.key");
    drop(service);
    std::fs::write(&key, b"synthetic damaged key").unwrap();
    store.writer_for_test().execute_batch("CREATE TRIGGER synthetic_refuse_key_loss BEFORE UPDATE OF revoked_at ON secret_records BEGIN SELECT RAISE(ABORT,'synthetic retirement failure'); END;").unwrap();
    let unavailable = SecretService::open_with_clock(
        dir.path(),
        "synthetic-device",
        store.writer(),
        clock.clone(),
    )
    .unwrap();
    assert!(matches!(
        unavailable.read(&record.context.id),
        Err(SecretError::Unavailable)
    ));
    assert!(matches!(
        unavailable.stage(
            context(None),
            SecretValue::new(b"synthetic".to_vec()).unwrap(),
            uuid::Uuid::new_v4().to_string(),
            "b".repeat(64)
        ),
        Err(SecretError::Unavailable)
    ));
    assert_eq!(std::fs::read(&key).unwrap(), b"synthetic damaged key");
    assert!(store
        .writer()
        .secret_record(&record.context.id)
        .unwrap()
        .unwrap()
        .revoked_at
        .is_none());
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
    assert!(matches!(
        recovered.read(&record.context.id),
        Err(SecretError::Unavailable)
    ));
    let new = stage(&recovered, None);
    attach(&recovered, &new);
    assert!(recovered.read(&new.context.id).is_ok());
}
