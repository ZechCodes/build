//! Join S2.1's durable grant retirement to S2.2's real delete RPC.
use super::environment_deletion::call;
use super::project_agent::rooted;
use super::tracker::tracked;
use super::*;
use crate::secrets::{GrantScope, SecretContext, SecretRecord, StageContext};

struct Fixture {
    root: tempfile::TempDir,
    _repo: tempfile::TempDir,
    app: AppState,
    attached: SecretRecord,
    config: PathBuf,
}
fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let (repo, mut app, project) = tracked(root.path());
    let config = root.path().join("config.json");
    app.config_path = Some(config.clone());
    let now = crate::agent::now_ms() as i64 / 1000;
    let record = SecretRecord {
        context: SecretContext {
            id: format!("sec_{}", uuid::Uuid::new_v4()),
            device_id: "synthetic-device".into(),
            creator: "synthetic-user".into(),
            name: "SYNTHETIC_ENVIRONMENT_TOKEN".into(),
            ttl_seconds: None,
            grant: GrantScope::Environment {
                environment_id: "env-1".into(),
            },
            stage: StageContext {
                owner_id: project.clone(),
                conversation_id: "synthetic-conversation".into(),
                generation: "synthetic-generation".into(),
                draft_id: None,
            },
        },
        created_at: now,
        attached_at: None,
        expires_at: None,
        revoked_at: None,
        revocation_reason: None,
        revision: 1,
        message_operation_id: None,
        upload_request_id: uuid::Uuid::new_v4().to_string(),
        upload_digest: "ab".repeat(32),
        blob_present: true,
    };
    let store = app.store.as_ref().unwrap().clone();
    store.writer().secrets_stage(&record).unwrap();
    let attached = store
        .writer()
        .secrets_attach(
            std::slice::from_ref(&record.context.id),
            "synthetic-environment-send",
            now,
        )
        .unwrap()
        .remove(0);
    call(
        &mut app,
        "project.delete",
        json!({"project_id":project,"confirm":true}),
    );
    // Removing the last member retires project grants, not environment grants.
    assert_eq!(
        store.writer().secret_record(&attached.context.id).unwrap(),
        Some(attached.clone())
    );
    assert!(app.projects.environment("env-1").is_some());
    Fixture {
        root,
        _repo: repo,
        app,
        attached,
        config,
    }
}
fn revoked_audit(app: &AppState, id: &str) -> i64 {
    app.store.as_ref().unwrap().writer_for_test().query_row(
        "SELECT COUNT(*) FROM secret_audit WHERE secret_id=?1 AND outcome='revoked' AND environment_id='env-1'", [id], |r|r.get(0)).unwrap()
}

#[test]
fn environment_delete_rpc_retires_environment_secret_grant() {
    let Fixture {
        root,
        _repo,
        mut app,
        attached,
        config,
    } = fixture();
    call(
        &mut app,
        "environment.delete",
        json!({"environment_id":"env-1"}),
    );
    assert!(app.projects.environment("env-1").is_none());
    let retired = app
        .store
        .as_ref()
        .unwrap()
        .writer()
        .secret_record(&attached.context.id)
        .unwrap()
        .unwrap();
    assert!(
        retired.revoked_at.is_some(),
        "RPC removed config without retiring its environment grant"
    );
    assert_eq!(retired.revocation_reason.as_deref(), Some("revoked"));
    assert_eq!(revoked_audit(&app, &attached.context.id), 1);
    call(
        &mut app,
        "environment.delete",
        json!({"environment_id":"env-1"}),
    );
    assert_eq!(revoked_audit(&app, &attached.context.id), 1);
    drop(app);
    let app = rooted(root.path())
        .with_config(config)
        .unwrap()
        .with_task_store(root.path().join("store"))
        .unwrap();
    assert!(app.projects.environment("env-1").is_none());
    assert_eq!(
        app.store
            .as_ref()
            .unwrap()
            .writer()
            .secret_record(&attached.context.id)
            .unwrap(),
        Some(retired)
    );
}

#[test]
fn environment_delete_rpc_retire_failure_preserves_config_and_grants_for_retry() {
    let Fixture {
        root: _root,
        _repo,
        mut app,
        attached,
        config,
    } = fixture();
    let bytes = std::fs::read(&config).unwrap();
    let store = app.store.as_ref().unwrap().clone();
    store.writer_for_test().execute_batch("CREATE TEMP TRIGGER fail_environment_retire BEFORE UPDATE OF revoked_at ON secret_records WHEN OLD.grant_scope='environment' AND OLD.grant_environment_id='env-1' AND NEW.revoked_at IS NOT NULL BEGIN SELECT RAISE(ABORT,'injected environment retire failure'); END;").unwrap();
    let refused = app.handle(req("environment.delete", json!({"environment_id":"env-1"})));
    assert_eq!(
        refused["ok"], false,
        "retire failure must refuse config removal: {refused}"
    );
    assert_eq!(std::fs::read(config).unwrap(), bytes);
    assert!(app.projects.environment("env-1").is_some());
    assert_eq!(
        store.writer().secret_record(&attached.context.id).unwrap(),
        Some(attached.clone())
    );
    assert_eq!(revoked_audit(&app, &attached.context.id), 0);
    store
        .writer_for_test()
        .execute_batch("DROP TRIGGER fail_environment_retire")
        .unwrap();
    call(
        &mut app,
        "environment.delete",
        json!({"environment_id":"env-1"}),
    );
    assert!(app.projects.environment("env-1").is_none());
    assert!(store
        .writer()
        .secret_record(&attached.context.id)
        .unwrap()
        .unwrap()
        .revoked_at
        .is_some());
}

#[test]
fn project_delete_reconciles_environment_secret_membership() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let sibling = app.add_environment_sibling_for_test(&project);
    let store = app.store.as_ref().unwrap().clone();
    let count = |id: &str| -> i64 {
        store
            .writer_for_test()
            .query_row(
                "SELECT COUNT(*) FROM secret_project_membership WHERE project_id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(count(&project), 1);
    assert_eq!(count(&sibling), 1);
    call(
        &mut app,
        "project.delete",
        json!({"project_id":project,"confirm":true}),
    );
    assert_eq!(
        count(&project),
        0,
        "deleted project retained environment-secret membership"
    );
    assert_eq!(count(&sibling), 1);
    assert!(store.writer().secret_membership_clean().unwrap());
}
