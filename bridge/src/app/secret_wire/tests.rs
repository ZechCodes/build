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

#[test]
fn revoke_operation_replays_exact_response_and_refuses_retargeting() {
    let mut f = fixture();
    let first = create(&mut f).unwrap();
    let mut params = json!({"secret_id":first["secret_id"],"operation_id":"33333333-3333-4333-8333-333333333333","scope":{"project_id":f.params["grant"]["project_id"]}});
    let caller = CallContext::detached("owner");
    let accepted = f
        .app
        .dispatch_api_with_context("secrets.revoke", &params, Some(&caller))
        .unwrap();
    assert_eq!(
        f.app
            .dispatch_api_with_context("secrets.revoke", &params, Some(&caller))
            .unwrap(),
        accepted
    );
    let reopened = rusqlite::Connection::open(f.root.path().join("tasks/build.db")).unwrap();
    let persisted: String = reopened
        .query_row(
            "SELECT response FROM secret_revocations WHERE operation_id=?1",
            [params["operation_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(&persisted).unwrap(), accepted);
    let mut changed_scope = params.clone();
    changed_scope["scope"]["stage"] = f.params["scope"].clone();
    assert_eq!(
        f.app
            .dispatch_api_with_context("secrets.revoke", &changed_scope, Some(&caller))
            .err()
            .unwrap()
            .code(),
        "conflict"
    );
    f.params["request_id"] = json!("44444444-4444-4444-8444-444444444444");
    params["secret_id"] = create(&mut f).unwrap()["secret_id"].clone();
    assert_eq!(
        f.app
            .dispatch_api_with_context("secrets.revoke", &params, Some(&caller))
            .err()
            .unwrap()
            .code(),
        "conflict"
    );
}

#[test]
fn revoke_reports_expired_even_before_the_sweeper_tombstones_it() {
    let mut f = fixture();
    let first = create(&mut f).unwrap();
    let database = rusqlite::Connection::open(f.root.path().join("tasks/build.db")).unwrap();
    database
        .execute(
            "UPDATE secret_records SET created_at=0 WHERE id=?1",
            [first["secret_id"].as_str().unwrap()],
        )
        .unwrap();
    let params = json!({"secret_id":first["secret_id"],"operation_id":"33333333-3333-4333-8333-333333333333","scope":{"project_id":f.params["grant"]["project_id"]}});
    let result = f
        .app
        .dispatch_api_with_context(
            "secrets.revoke",
            &params,
            Some(&CallContext::detached("owner")),
        )
        .unwrap();
    assert_eq!(result["status"], "expired");
    let reason: String = database
        .query_row(
            "SELECT revocation_reason FROM secret_records WHERE id=?1",
            [first["secret_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reason, "expired");
    let mut fresh_operation = params.clone();
    fresh_operation["operation_id"] = json!("55555555-5555-4555-8555-555555555555");
    let persisted_expiry = f
        .app
        .dispatch_api_with_context(
            "secrets.revoke",
            &fresh_operation,
            Some(&CallContext::detached("owner")),
        )
        .unwrap();
    assert_eq!(persisted_expiry["status"], "expired");
    assert_eq!(persisted_expiry["revoked_at"], result["revoked_at"]);
}

#[test]
fn revoke_receipt_failure_rolls_back_the_tombstone_and_audit() {
    let mut f = fixture();
    let first = create(&mut f).unwrap();
    let database = rusqlite::Connection::open(f.root.path().join("tasks/build.db")).unwrap();
    database.execute_batch("CREATE TRIGGER fail_revoke_receipt BEFORE INSERT ON secret_revocations BEGIN SELECT RAISE(ABORT,'synthetic receipt failure'); END").unwrap();
    let params = json!({"secret_id":first["secret_id"],"operation_id":"33333333-3333-4333-8333-333333333333","scope":{"project_id":f.params["grant"]["project_id"]}});
    let caller = CallContext::detached("owner");
    assert!(f
        .app
        .dispatch_api_with_context("secrets.revoke", &params, Some(&caller))
        .is_err());
    let revoked: Option<i64> = database
        .query_row(
            "SELECT revoked_at FROM secret_records WHERE id=?1",
            [first["secret_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(revoked.is_none());
    let audit: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM secret_audit WHERE outcome='revoked'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(audit, 0);
    database
        .execute_batch("DROP TRIGGER fail_revoke_receipt")
        .unwrap();
    assert_eq!(
        f.app
            .dispatch_api_with_context("secrets.revoke", &params, Some(&caller))
            .unwrap()["status"],
        "revoked"
    );
}

#[test]
fn revoke_receipts_are_bounded_without_blocking_revocation_and_pruned_on_sweep() {
    let mut f = fixture();
    let first = create(&mut f).unwrap();
    let mut params = json!({"secret_id":first["secret_id"],"operation_id":"33333333-3333-4333-8333-333333333333","scope":{"project_id":f.params["grant"]["project_id"]}});
    let caller = CallContext::detached("owner");
    let accepted = f
        .app
        .dispatch_api_with_context("secrets.revoke", &params, Some(&caller))
        .unwrap();
    let database = rusqlite::Connection::open(f.root.path().join("tasks/build.db")).unwrap();
    database.execute("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<4096) INSERT INTO secret_revocations(device_id,creator,operation_id,request,response,secret_id,accepted_at) SELECT 'synthetic-device','device_user',printf('00000000-0000-4000-8000-%012d',i),'{}',?1,?2,?3 FROM n",rusqlite::params![serde_json::to_string(&accepted).unwrap(),first["secret_id"].as_str().unwrap(),time::OffsetDateTime::now_utc().unix_timestamp()]).unwrap();
    f.params["request_id"] = json!("44444444-4444-4444-8444-444444444444");
    params["secret_id"] = create(&mut f).unwrap()["secret_id"].clone();
    params["operation_id"] = json!("55555555-5555-4555-8555-555555555555");
    assert_eq!(
        f.app
            .dispatch_api_with_context("secrets.revoke", &params, Some(&caller))
            .unwrap()["status"],
        "revoked"
    );
    let count: i64 = database
        .query_row("SELECT COUNT(*) FROM secret_revocations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 4096);
    let oldest: bool = database.query_row("SELECT EXISTS(SELECT 1 FROM secret_revocations WHERE operation_id='33333333-3333-4333-8333-333333333333')",[],|row|row.get(0)).unwrap();
    assert!(!oldest);
    let tombstone: bool = database
        .query_row(
            "SELECT revoked_at IS NOT NULL FROM secret_records WHERE id=?1",
            [first["secret_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(tombstone, "receipt eviction never reopens a revoked secret");
    database
        .execute("UPDATE secret_revocations SET accepted_at=0", [])
        .unwrap();
    f.app.secret_service().unwrap().sweep().unwrap();
    assert_eq!(
        database
            .query_row("SELECT COUNT(*) FROM secret_revocations", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        f.app
            .dispatch_api_with_context("secrets.revoke", &params, Some(&caller))
            .unwrap()["status"],
        "revoked"
    );
}

#[test]
fn environment_secret_upload_validates_current_stage_project_in_writer() {
    let mut f = fixture();
    let project = f.params["scope"]["owner_id"].as_str().unwrap().to_string();
    let env = f.app.projects.get(&project).unwrap().environment_id.clone();
    f.params["grant"] = json!({"scope":"environment","environment_id":env});
    let accepted = create(&mut f).unwrap();
    assert_eq!(accepted["grant"]["scope"], "environment");
    assert_eq!(accepted["status"], "staged");
    let mut context:UploadContext=serde_json::from_value(json!({
        "device_id":f.params["device_id"],"creator":f.params["creator"],"request_id":uuid::Uuid::new_v4().to_string(),
        "name":"FOREIGN","ttl_seconds":null,"grant":{"scope":"environment","environment_id":"foreign-environment"},"stage":f.params["scope"]
    })).unwrap();
    let authority: SecretScope =
        serde_json::from_value(json!({"project_id":project,"stage":f.params["scope"]})).unwrap();
    let service = f.app.secret_service().unwrap();
    assert!(service
        .stage_upload_authorized(
            &context,
            "synthetic-ciphertext",
            &SyntheticOpener(f.calls.clone()),
            &authority
        )
        .is_err());
    context.grant =
        serde_json::from_value(json!({"scope":"environment","environment_id":env})).unwrap();
    f.app
        .store
        .as_ref()
        .unwrap()
        .writer_for_test()
        .execute(
            "UPDATE secret_project_membership SET archived=1 WHERE project_id=?1",
            [&project],
        )
        .unwrap();
    assert!(service
        .stage_upload_authorized(
            &context,
            "synthetic-ciphertext",
            &SyntheticOpener(f.calls.clone()),
            &authority
        )
        .is_err());
    assert_eq!(
        f.app
            .store
            .as_ref()
            .unwrap()
            .writer_for_test()
            .query_row("SELECT COUNT(*) FROM secret_records", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn environment_secret_upload_rechecks_membership_after_opening() {
    struct MovingOpener {
        writer: crate::store::StoreWriter,
        project: String,
    }
    impl UploadOpener for MovingOpener {
        fn open(&self, _: &str, _: &UploadContext) -> crate::secrets::Result<SecretValue> {
            self.writer.secrets_reconcile_config(&json!({"environments":[{"id":"foreign-env"}],"projects":[{"id":self.project,"environment_id":"foreign-env","archived_at":null}]}))?;
            SecretValue::new(b"synthetic-test-value-only".to_vec())
        }
    }
    let f = fixture();
    let project = f.params["scope"]["owner_id"].as_str().unwrap();
    let environment = &f.app.projects.get(project).unwrap().environment_id;
    let context:UploadContext=serde_json::from_value(json!({"device_id":f.params["device_id"],"creator":f.params["creator"],"request_id":f.params["request_id"],"name":f.params["name"],"ttl_seconds":null,"grant":{"scope":"environment","environment_id":environment},"stage":f.params["scope"]})).unwrap();
    let authority: SecretScope =
        serde_json::from_value(json!({"project_id":project,"stage":f.params["scope"]})).unwrap();
    let opener = MovingOpener {
        writer: f.app.store.as_ref().unwrap().writer(),
        project: project.into(),
    };
    assert!(f
        .app
        .secret_service()
        .unwrap()
        .stage_upload_authorized(&context, "synthetic-ciphertext", &opener, &authority)
        .is_err());
    assert!(!opener.writer.secrets_have_records().unwrap());
}
