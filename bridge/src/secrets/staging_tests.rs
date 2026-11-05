use super::*;
use crate::secrets::{GrantScope, SecretValue, StageContext};
use crate::store::Store;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;

const VALUE: &[u8] = b"synthetic-staging-value-never-history";

#[derive(Default)]
struct SyntheticOpener(AtomicUsize);
impl UploadOpener for SyntheticOpener {
    fn open(&self, _: &str, _: &UploadContext) -> Result<SecretValue> {
        self.0.fetch_add(1, Ordering::SeqCst);
        SecretValue::new(VALUE.to_vec())
    }
}
struct UnavailableOpener;
impl UploadOpener for UnavailableOpener {
    fn open(&self, _: &str, _: &UploadContext) -> Result<SecretValue> {
        Err(SecretError::Unavailable)
    }
}

fn context() -> UploadContext {
    UploadContext {
        device_id: "synthetic-device".into(),
        creator: "synthetic-user".into(),
        request_id: "557ab895-85e0-4c94-82ad-f16fcb0dd11c".into(),
        name: "SYNTHETIC_TOKEN".into(),
        ttl_seconds: Some(3600),
        grant: GrantScope::Project {
            project_id: "synthetic-project".into(),
        },
        stage: StageContext {
            owner_id: "synthetic-owner".into(),
            conversation_id: "synthetic-conversation".into(),
            generation: "synthetic-generation".into(),
            draft_id: None,
        },
    }
}
fn fixture() -> (TempDir, Store, SecretService, SyntheticOpener) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().join("tasks")).unwrap();
    let service = SecretService::open(root.path(), "synthetic-device", store.writer()).unwrap();
    (root, store, service, SyntheticOpener::default())
}
fn blobs(root: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(root.join("secrets"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("sec_")
        })
        .collect()
}
fn database(root: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(root.join("tasks/build.db")).unwrap()
}
fn record_count(connection: &rusqlite::Connection) -> i64 {
    connection
        .query_row("SELECT COUNT(*) FROM secret_records", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn acknowledged_or_reply_lost_upload_replays_exact_record_after_cold_restart() {
    let (root, store, service, opener) = fixture();
    let expected = context();
    let accepted = service
        .stage_upload(&expected, "synthetic-sealed-bytes", &opener)
        .unwrap();
    let blob_before =
        std::fs::read(root.path().join("secrets").join(&accepted.context.id)).unwrap();
    let replay = service
        .stage_upload(&expected, "synthetic-sealed-bytes", &UnavailableOpener)
        .unwrap();
    assert_eq!(replay, accepted);
    assert_eq!(opener.0.load(Ordering::SeqCst), 1);
    assert_eq!(blobs(root.path()).len(), 1);
    drop(service);
    drop(store);
    let reopened_store = Store::new(root.path().join("tasks")).unwrap();
    let reopened =
        SecretService::open(root.path(), "synthetic-device", reopened_store.writer()).unwrap();
    let after_restart = reopened
        .stage_upload(&expected, "synthetic-sealed-bytes", &UnavailableOpener)
        .unwrap();
    assert_eq!(after_restart, accepted);
    assert_eq!(record_count(&database(root.path())), 1);
    assert_eq!(
        std::fs::read(root.path().join("secrets").join(&accepted.context.id)).unwrap(),
        blob_before
    );
    assert!(accepted.attached_at.is_none());
    assert!(accepted.expires_at.is_none());
}

#[test]
fn changed_sealed_bytes_or_bound_context_conflicts_without_opening_again() {
    let (root, _store, service, opener) = fixture();
    let expected = context();
    let accepted = service
        .stage_upload(&expected, "synthetic-sealed-bytes", &opener)
        .unwrap();
    assert!(matches!(
        service.stage_upload(&expected, "different-sealed-bytes", &opener),
        Err(SecretError::Conflict)
    ));
    let mut changed = expected.clone();
    changed.name = "OTHER_LABEL".into();
    assert!(matches!(
        service.stage_upload(&changed, "synthetic-sealed-bytes", &opener),
        Err(SecretError::Conflict)
    ));
    changed = expected.clone();
    changed.stage.draft_id = Some("synthetic-draft".into());
    assert!(matches!(
        service.stage_upload(&changed, "synthetic-sealed-bytes", &opener),
        Err(SecretError::Conflict)
    ));
    assert_eq!(opener.0.load(Ordering::SeqCst), 1);
    assert_eq!(record_count(&database(root.path())), 1);
    assert_eq!(
        blobs(root.path()),
        [root.path().join("secrets").join(accepted.context.id)]
    );
}

#[test]
fn sql_insert_failure_after_encrypted_publication_is_inaccessible_and_swept_on_restart() {
    let (root, store, service, opener) = fixture();
    let connection = database(root.path());
    connection.execute_batch("CREATE TRIGGER synthetic_refuse_stage BEFORE INSERT ON secret_records BEGIN SELECT RAISE(ABORT,'synthetic stage failure'); END;").unwrap();
    let expected = context();
    assert!(matches!(
        service.stage_upload(&expected, "synthetic-sealed-bytes", &opener),
        Err(SecretError::Database(_))
    ));
    let orphaned = blobs(root.path());
    assert_eq!(orphaned.len(), 1, "publication precedes metadata commit");
    let ciphertext = std::fs::read(&orphaned[0]).unwrap();
    assert!(!ciphertext
        .windows(VALUE.len())
        .any(|window| window == VALUE));
    assert_eq!(record_count(&connection), 0);
    assert!(store
        .writer()
        .secret_by_upload_request(&expected.device_id, &expected.creator, &expected.request_id)
        .unwrap()
        .is_none());
    drop(connection);
    drop(service);
    drop(store);
    let reopened_store = Store::new(root.path().join("tasks")).unwrap();
    let reopened =
        SecretService::open(root.path(), "synthetic-device", reopened_store.writer()).unwrap();
    assert!(blobs(root.path()).is_empty());
    let connection = database(root.path());
    assert_eq!(record_count(&connection), 0);
    connection
        .execute_batch("DROP TRIGGER synthetic_refuse_stage")
        .unwrap();
    let retried = reopened
        .stage_upload(&expected, "synthetic-sealed-bytes", &opener)
        .unwrap();
    assert_ne!(
        orphaned[0].file_name().unwrap(),
        retried.context.id.as_str()
    );
    assert_eq!(record_count(&connection), 1);
    assert_eq!(blobs(root.path()).len(), 1);
}

#[test]
fn unavailable_opener_and_invalid_context_leave_no_blob_or_metadata() {
    let (root, _store, service, opener) = fixture();
    let mut expected = context();
    assert!(matches!(
        service.stage_upload(&expected, "synthetic-sealed-bytes", &UnavailableOpener),
        Err(SecretError::Unavailable)
    ));
    expected.ttl_seconds = Some(0);
    assert!(matches!(
        service.stage_upload(&expected, "synthetic-sealed-bytes", &opener),
        Err(SecretError::Invalid)
    ));
    assert_eq!(opener.0.load(Ordering::SeqCst), 0);
    assert_eq!(record_count(&database(root.path())), 0);
    assert!(blobs(root.path()).is_empty());
}

#[test]
fn upload_job_limit_applies_before_opening_sealed_values() {
    let (root, _store, service, opener) = fixture();
    let permits: Vec<_> = (0..crate::secrets::MAX_CONCURRENT_JOBS)
        .map(|_| service.inner.jobs.clone().try_acquire_owned().unwrap())
        .collect();
    assert!(matches!(
        service.stage_upload(&context(), "synthetic-sealed-bytes", &opener),
        Err(SecretError::Limit)
    ));
    assert_eq!(opener.0.load(Ordering::SeqCst), 0);
    assert_eq!(record_count(&database(root.path())), 0);
    assert!(blobs(root.path()).is_empty());
    drop(permits);
    assert!(service
        .stage_upload(&context(), "synthetic-sealed-bytes", &opener)
        .is_ok());
    assert_eq!(opener.0.load(Ordering::SeqCst), 1);
}
