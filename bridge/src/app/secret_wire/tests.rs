use super::*;
use crate::harness::HarnessContext;
use crate::secrets::{SecretError, SecretValue};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

struct SyntheticOpener(Arc<AtomicUsize>);
impl UploadOpener for SyntheticOpener {
    fn open(&self, sealed: &str, _: &UploadContext) -> crate::secrets::Result<SecretValue> {
        self.0.fetch_add(1, Ordering::SeqCst);
        if sealed != "synthetic-ciphertext" {
            return Err(SecretError::Invalid);
        }
        SecretValue::new(b"synthetic-test-value-only".to_vec())
    }
}
struct Fixture {
    app: AppState,
    root: tempfile::TempDir,
    params: Value,
    calls: Arc<AtomicUsize>,
}
fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let context =
        HarnessContext::resolved(root.path().join("mcp.sock"), root.path().into()).unwrap();
    let mut app =
        AppState::new_unrooted_configured(root.path().join("worktrees"), "main", true, context)
            .with_task_store(root.path().join("tasks"))
            .unwrap();
    app.projects_dir = root.path().join("projects");
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let project = app.add_project(source, "main".into());
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app
        .with_secret_vault(root.path(), "synthetic-device")
        .unwrap()
        .with_secret_upload_opener(Arc::new(SyntheticOpener(calls.clone())));
    let params = json!({"request_id":"11111111-1111-4111-8111-111111111111","device_id":"synthetic-device","creator":"device_user",
        "scope":{"owner_id":project,"conversation_id":"provisional-agent","generation":"provisional-thread","draft_id":"22222222-2222-4222-8222-222222222222"},
        "grant":{"scope":"project","project_id":project},"name":"SYNTHETIC_TOKEN","ttl_seconds":null,"sealed_payload":"synthetic-ciphertext"});
    Fixture {
        app,
        root,
        params,
        calls,
    }
}
fn create(f: &mut Fixture) -> Result<Value, ApiError> {
    f.app.dispatch_api_with_context(
        "secrets.create",
        &f.params,
        Some(&CallContext::detached("synthetic-owner")),
    )
}
#[test]
fn upload_replay_is_exact_and_timeout_lookup_uses_request_id() {
    let mut f = fixture();
    let first = create(&mut f).unwrap();
    assert_eq!(first["status"], "staged");
    assert!(first["expires_at"].is_null());
    assert_eq!(create(&mut f).unwrap(), first);
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    let record = f
        .app
        .store
        .as_ref()
        .unwrap()
        .writer()
        .secret_by_upload_request(
            "synthetic-device",
            "device_user",
            f.params["request_id"].as_str().unwrap(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(record.context.id, first["secret_id"]);
    f.params["name"] = json!("CHANGED_LABEL");
    assert_eq!(create(&mut f).err().unwrap().code(), "conflict");
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    let bytes = std::fs::read(f.root.path().join("tasks/build.db")).unwrap();
    assert!(!bytes
        .windows(b"synthetic-test-value-only".len())
        .any(|window| window == b"synthetic-test-value-only"));
    assert!(first.get("upload_digest").is_none());
    assert!(first.get("sealed_payload").is_none());
}
#[test]
fn upload_refuses_internal_ended_and_mismatched_principals_before_opening() {
    let mut f = fixture();
    assert_eq!(
        f.app
            .dispatch_api("secrets.create", &f.params)
            .err()
            .unwrap()
            .code(),
        "unavailable"
    );
    let ended = CallContext::detached("ended");
    ended.opening_ended();
    assert!(f
        .app
        .dispatch_api_with_context("secrets.create", &f.params, Some(&ended))
        .is_err());
    f.params["creator"] = json!("other-owner");
    assert_eq!(create(&mut f).err().unwrap().message(), "unauthorized");
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
}
#[test]
fn upload_strict_fields_size_limits_and_cross_project_scope_are_refused() {
    let mut f = fixture();
    let original = f.params.clone();
    f.params["value"] = json!("synthetic-forbidden-outer-plaintext");
    assert_eq!(create(&mut f).err().unwrap().code(), "invalid_params");
    f.params = original.clone();
    f.params["scope"]["extra"] = json!(true);
    assert_eq!(create(&mut f).err().unwrap().code(), "invalid_params");
    f.params = original.clone();
    f.params["sealed_payload"] = json!("x".repeat(65_536));
    assert_eq!(create(&mut f).err().unwrap().code(), "invalid_params");
    f.params = original;
    f.params["grant"]["project_id"] = json!("wrong-project");
    assert_eq!(create(&mut f).err().unwrap().code(), "invalid_params");
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
}
#[test]
fn revoke_replays_tombstone_and_cannot_select_another_project() {
    let mut f = fixture();
    let first = create(&mut f).unwrap();
    let mut params = json!({"secret_id":first["secret_id"],"operation_id":"33333333-3333-4333-8333-333333333333","scope":{"project_id":f.params["grant"]["project_id"]}});
    let caller = CallContext::detached("owner");
    let revoked = f
        .app
        .dispatch_api_with_context("secrets.revoke", &params, Some(&caller))
        .unwrap();
    assert_eq!(revoked["status"], "revoked");
    let replay = f
        .app
        .dispatch_api_with_context("secrets.revoke", &params, Some(&caller))
        .unwrap();
    assert_eq!(revoked["revoked_at"], replay["revoked_at"]);
    params["scope"]["project_id"] = json!("wrong-project");
    assert!(f
        .app
        .dispatch_api_with_context("secrets.revoke", &params, Some(&caller))
        .is_err());
}

#[test]
fn missing_upload_opener_fails_closed_before_creating_any_record() {
    let mut f = fixture();
    f.app.secret_upload_opener = None;
    assert_eq!(create(&mut f).err().unwrap().code(), "unavailable");
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    assert!(f
        .app
        .store
        .as_ref()
        .unwrap()
        .writer()
        .secret_records()
        .unwrap()
        .is_empty());
}

struct RetiringOpener {
    database: std::path::PathBuf,
    project: String,
}
impl UploadOpener for RetiringOpener {
    fn open(&self, _: &str, _: &UploadContext) -> crate::secrets::Result<SecretValue> {
        let connection = rusqlite::Connection::open(&self.database).unwrap();
        connection
            .execute(
                "DELETE FROM device_project_ids WHERE project_id=?1",
                [&self.project],
            )
            .unwrap();
        SecretValue::new(b"synthetic-retired-stage".to_vec())
    }
}
#[test]
fn upload_rechecks_committed_scope_after_encryption_and_before_metadata_acceptance() {
    let mut f = fixture();
    f.app.secret_upload_opener = Some(Arc::new(RetiringOpener {
        database: f.root.path().join("tasks/build.db"),
        project: f.params["grant"]["project_id"].as_str().unwrap().into(),
    }));
    assert_eq!(create(&mut f).err().unwrap().code(), "unavailable");
    assert!(f
        .app
        .store
        .as_ref()
        .unwrap()
        .writer()
        .secret_records()
        .unwrap()
        .is_empty());
    let orphan_count = std::fs::read_dir(f.root.path().join("secrets"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("sec_")
        })
        .count();
    assert_eq!(
        orphan_count, 1,
        "encrypted orphan is inaccessible after authority retirement"
    );
    assert_eq!(f.app.secret_service().unwrap().sweep().unwrap().deleted, 1);
}
