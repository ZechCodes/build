use super::*;
use crate::app::projects::ProjectSource;
use crate::harness::HarnessContext;
use serde_json::json;
use std::path::PathBuf;

fn app(root: &Path) -> AppState {
    let context =
        HarnessContext::resolved(root.join("synthetic-mcp.sock"), root.to_path_buf()).unwrap();
    let mut app = AppState::new_unrooted_configured(root.join("worktrees"), "main", true, context)
        .with_task_store(root.join("tasks"))
        .unwrap();
    app.projects_dir = root.join("projects");
    app
}

fn protected_app(root: &Path) -> AppState {
    app(root)
        .with_secret_vault(root, "synthetic-device")
        .unwrap()
}

#[test]
fn secrets_startup_degrades_for_registered_source_and_directory_root_overlaps() {
    for kind in ["source", "projects", "worktrees"] {
        let root = tempfile::tempdir().unwrap();
        let mut app = app(root.path());
        match kind {
            "source" => {
                app.add_project(root.path().to_path_buf(), "main".into());
            }
            "projects" => app.projects_dir = root.path().to_path_buf(),
            _ => app.worktrees_root = root.path().to_path_buf(),
        }
        let app = app
            .with_secret_vault(root.path(), "synthetic-device")
            .expect("layout overlap must not stop the normal daemon");
        assert!(app.secret_service().is_none(), "{kind}");
        assert!(!root.path().join("secrets").exists());
    }
}

#[test]
fn degraded_secret_storage_still_publishes_metadata_retirement() {
    use crate::secrets::{GrantScope, SecretContext, SecretRecord, StageContext};
    let root = tempfile::tempdir().unwrap();
    let mut state = app(root.path());
    let record = SecretRecord {
        context: SecretContext {
            id: format!("sec_{}", uuid::Uuid::new_v4()),
            device_id: "synthetic-device".into(),
            creator: "synthetic-user".into(),
            name: "SYNTHETIC_DEGRADED_TOKEN".into(),
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
        },
        created_at: 1_800_000_000,
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
    state
        .store
        .as_ref()
        .unwrap()
        .writer()
        .secrets_stage(&record)
        .unwrap();
    state.projects_dir = root.path().to_path_buf();
    let state = state
        .with_secret_vault(root.path(), "synthetic-device")
        .unwrap();
    assert!(state.secret_service().is_none());
    let (browser, mut rx, key) = crate::carrier::SessionSender::observable("degraded-secret");
    state.changes.subscribe(
        &browser,
        crate::changes::SubscriptionSpec {
            id: "degraded-secret".into(),
            scope: crate::changes::Scope::All,
            kinds: [crate::changes::Kind::SecretsChanged].into_iter().collect(),
            mode: Default::default(),
            priority: Default::default(),
        },
    );
    state
        .store
        .as_ref()
        .unwrap()
        .writer()
        .secrets_retire_project("synthetic-project")
        .unwrap();
    assert_eq!(state.changes.flush(), 1);
    let event = crate::carrier::SessionSender::decrypt_push(&key, &rx.try_recv().unwrap());
    assert_eq!(event["secret_ids"], json!([record.context.id]));
}

#[test]
fn secrets_startup_degrades_for_parked_primary_and_secondary_sources() {
    for secondary in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut app = app(root.path());
        let blocked = root.path().join("secrets/not-created");
        let primary = if secondary {
            root.path().join("safe-missing")
        } else {
            blocked.clone()
        };
        app.projects.park(json!({
            "id":"proj-7", "path":primary,
            "sources":[{"id":"source-1","path":primary},{"id":"source-2","path":blocked}]
        }));
        let app = app
            .with_secret_vault(root.path(), "synthetic-device")
            .expect("parked source overlap must not stop the normal daemon");
        assert!(app.secret_service().is_none());
        assert!(!root.path().join("secrets").exists());
    }
}

#[test]
fn secrets_project_create_and_multisource_admission_refuse_vault_before_deferring() {
    let root = tempfile::tempdir().unwrap();
    let mut app = protected_app(root.path());
    let result = app
        .project_create(&json!({"name":"synthetic-source","parent":root.path().join("secrets")}));
    assert!(result.is_err());
    assert!(app.deferred_work.is_none());
    assert!(!root.path().join("secrets/synthetic-source").exists());
    let result =
        app.project_create(&json!({"name":"synthetic-project","sources":[{"path":root.path()}]}));
    assert!(result.is_err());
    assert!(app.deferred_work.is_none());
}

#[test]
fn secrets_source_add_and_move_refuse_vault_and_preserve_registered_sources() {
    let root = tempfile::tempdir().unwrap();
    let primary = root.path().join("synthetic-primary");
    let secondary = root.path().join("synthetic-secondary");
    for path in [&primary, &secondary] {
        std::fs::create_dir(path).unwrap();
    }
    let mut app = protected_app(root.path());
    let id = app.add_project(primary.clone(), "main".into());
    let sources: Vec<_> = [&primary, &secondary]
        .into_iter()
        .enumerate()
        .map(|(index, path)| {
            ProjectSource::added(
                format!("source-{}", index + 1),
                format!("synthetic-{index}"),
                format!("synthetic-{index}"),
                path.clone(),
                false,
                "main".into(),
            )
        })
        .collect();
    app.projects.set_sources(&id, sources);
    let before: Vec<PathBuf> = app
        .projects
        .get(&id)
        .unwrap()
        .sources
        .iter()
        .map(|source| source.path.clone())
        .collect();
    assert!(app
        .project_add_source(&json!({"project_id":id,"path":root.path()}))
        .is_err());
    assert!(app
        .project_update_source(
            &json!({"project_id":id,"source_id":"source-2","path":root.path().join("secrets")})
        )
        .is_err());
    assert!(app.deferred_work.is_none());
    let after: Vec<PathBuf> = app
        .projects
        .get(&id)
        .unwrap()
        .sources
        .iter()
        .map(|source| source.path.clone())
        .collect();
    assert_eq!(before, after);
}

#[test]
fn secrets_safe_project_admission_remains_available() {
    let root = tempfile::tempdir().unwrap();
    let mut app = protected_app(root.path());
    let safe = root.path().join("synthetic-safe");
    std::fs::create_dir(&safe).unwrap();
    assert!(app
        .project_create(&json!({"name":"synthetic-project","sources":[{"path":safe}]}))
        .is_ok());
    assert!(app.deferred_work.is_some());
}

fn attached_synthetic_secret(app: &AppState) -> crate::secrets::SecretRecord {
    use crate::secrets::{GrantScope, SecretContext, SecretValue, StageContext};
    let service = app.secret_service().unwrap();
    let context = SecretContext {
        id: format!("sec_{}", uuid::Uuid::new_v4()),
        device_id: "synthetic-device".into(),
        creator: "synthetic-user".into(),
        name: "SYNTHETIC_RECOVERY_TOKEN".into(),
        ttl_seconds: None,
        grant: GrantScope::Project {
            project_id: "synthetic-project".into(),
        },
        stage: StageContext {
            owner_id: "synthetic-owner".into(),
            conversation_id: "synthetic-conversation".into(),
            generation: "synthetic-generation".into(),
            draft_id: None,
        },
    };
    let record = service
        .stage(
            context,
            SecretValue::new(b"synthetic-recovery-value-only".to_vec()).unwrap(),
            uuid::Uuid::new_v4().to_string(),
            "a".repeat(64),
        )
        .unwrap();
    service
        .attach(
            std::slice::from_ref(&record.context.id),
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap()
        .remove(0)
}

fn assert_secrets_key_loss_recovery(app: &AppState, old_id: &str) -> String {
    use crate::secrets::SecretError;
    let service = app.secret_service().unwrap();
    for _ in 0..2 {
        assert!(matches!(
            service.read(old_id),
            Err(SecretError::Unavailable)
        ));
    }
    let store = app.attached_store_for_test().unwrap();
    let retired = store.writer().secret_record(old_id).unwrap().unwrap();
    assert!(retired.revoked_at.is_some());
    assert_eq!(retired.revocation_reason.as_deref(), Some("key_lost"));
    assert!(!retired.blob_present);
    let fresh = attached_synthetic_secret(app);
    assert_ne!(fresh.context.id, old_id);
    assert_eq!(
        service.read(&fresh.context.id).unwrap().as_bytes(),
        b"synthetic-recovery-value-only"
    );
    assert!(matches!(
        service.read(old_id),
        Err(SecretError::Unavailable)
    ));
    fresh.context.id
}

#[test]
fn secrets_database_only_restore_starts_normal_app_retires_old_ids_and_accepts_new_values() {
    let fixture = tempfile::tempdir().unwrap();
    let original = fixture.path().join("synthetic-original-state");
    std::fs::create_dir(&original).unwrap();
    let source_app = protected_app(&original);
    let old = attached_synthetic_secret(&source_app);
    let original_key = std::fs::read(original.join("secrets/vault.key")).unwrap();
    let restored = fixture.path().join("synthetic-restored-state");
    source_app
        .attached_store_for_test()
        .unwrap()
        .backup_to(&restored.join("tasks/build.db"))
        .unwrap();
    assert!(!restored.join("secrets").exists());

    let restored_app = app(&restored)
        .with_secret_vault(&restored, "synthetic-device")
        .expect("a database-only restore must start the normal daemon");
    let new_id = assert_secrets_key_loss_recovery(&restored_app, &old.context.id);
    let restored_key = std::fs::read(restored.join("secrets/vault.key")).unwrap();
    assert_eq!(restored_key.len(), 32);
    assert_ne!(restored_key, original_key);
    assert_eq!(
        source_app
            .secret_service()
            .unwrap()
            .read(&old.context.id)
            .unwrap()
            .as_bytes(),
        b"synthetic-recovery-value-only"
    );
    assert_eq!(
        std::fs::read(original.join("secrets/vault.key")).unwrap(),
        original_key
    );
    drop(restored_app);
    let restarted = protected_app(&restored);
    assert_eq!(
        restarted
            .secret_service()
            .unwrap()
            .read(&new_id)
            .unwrap()
            .as_bytes(),
        b"synthetic-recovery-value-only"
    );
}

#[test]
fn secrets_corrupt_present_key_preserves_state_and_repair_restores_access() {
    for corrupt in [vec![b'T'; 32], b"synthetic-short-key".to_vec()] {
        assert_corrupt_key_preserved_and_repairable(&corrupt);
    }
}

fn assert_corrupt_key_preserved_and_repairable(corrupt: &[u8]) {
    let root = tempfile::tempdir().unwrap();
    let source_app = protected_app(root.path());
    let old = attached_synthetic_secret(&source_app);
    let key_path = root.path().join("secrets/vault.key");
    let blob_path = root.path().join("secrets").join(&old.context.id);
    let original_key = std::fs::read(&key_path).unwrap();
    let original_blob = std::fs::read(&blob_path).unwrap();
    drop(source_app);
    std::fs::write(&key_path, corrupt).unwrap();

    let unavailable_app = app(root.path())
        .with_secret_vault(root.path(), "synthetic-device")
        .expect("a present corrupt key must not prevent normal daemon startup");
    let service = unavailable_app.secret_service().unwrap();
    assert_secret_reads_and_stage_unavailable(&service, &old.context);
    let store = unavailable_app.attached_store_for_test().unwrap();
    let preserved = store
        .writer()
        .secret_record(&old.context.id)
        .unwrap()
        .unwrap();
    assert_eq!(preserved, old);
    assert!(preserved.revoked_at.is_none());
    assert!(preserved.revocation_reason.is_none());
    assert_eq!(store.writer().secret_records().unwrap(), vec![old.clone()]);
    assert_eq!(std::fs::read(&key_path).unwrap(), corrupt);
    assert_eq!(std::fs::read(&blob_path).unwrap(), original_blob);
    drop(service);
    drop(store);
    drop(unavailable_app);

    assert_original_key_repair(root.path(), &old, &original_key, &original_blob);
}

fn assert_secret_reads_and_stage_unavailable(
    service: &crate::secrets::SecretService,
    context: &crate::secrets::SecretContext,
) {
    use crate::secrets::{SecretError, SecretValue};
    for _ in 0..2 {
        assert!(matches!(
            service.read(&context.id),
            Err(SecretError::Unavailable)
        ));
    }
    let mut context = context.clone();
    context.id = format!("sec_{}", uuid::Uuid::new_v4());
    assert!(matches!(
        service.stage(
            context,
            SecretValue::new(b"synthetic-new-value-only".to_vec()).unwrap(),
            uuid::Uuid::new_v4().to_string(),
            "b".repeat(64),
        ),
        Err(SecretError::Unavailable)
    ));
}

fn assert_original_key_repair(
    root: &Path,
    old: &crate::secrets::SecretRecord,
    original_key: &[u8],
    original_blob: &[u8],
) {
    let key_path = root.join("secrets/vault.key");
    let blob_path = root.join("secrets").join(&old.context.id);
    std::fs::write(&key_path, original_key).unwrap();
    let repaired_app = protected_app(root);
    assert_eq!(
        repaired_app
            .secret_service()
            .unwrap()
            .read(&old.context.id)
            .unwrap()
            .as_bytes(),
        b"synthetic-recovery-value-only"
    );
    let restored = repaired_app
        .attached_store_for_test()
        .unwrap()
        .writer()
        .secret_record(&old.context.id)
        .unwrap()
        .unwrap();
    assert_eq!(&restored, old);
    assert_eq!(std::fs::read(&key_path).unwrap(), original_key);
    assert_eq!(std::fs::read(&blob_path).unwrap(), original_blob);
}

#[test]
fn secrets_degraded_layout_keeps_existing_key_blob_and_records_untouched() {
    for kind in ["source", "ancestor", "projects", "worktrees", "parked"] {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("synthetic-device-state");
        std::fs::create_dir(&root).unwrap();
        let original = protected_app(&root);
        let record = attached_synthetic_secret(&original);
        let key = std::fs::read(root.join("secrets/vault.key")).unwrap();
        let blob = std::fs::read(root.join("secrets").join(&record.context.id)).unwrap();
        drop(original);

        let mut loaded = app(&root);
        match kind {
            "source" => {
                loaded.add_project(root.clone(), "main".into());
            }
            "ancestor" => {
                loaded.add_project(fixture.path().to_path_buf(), "main".into());
            }
            "projects" => loaded.projects_dir = root.clone(),
            "worktrees" => loaded.worktrees_root = root.clone(),
            _ => loaded
                .projects
                .park(json!({"id":"proj-7","path":root.join("secrets/parked-source")})),
        }
        let loaded = loaded
            .with_secret_vault(&root, "synthetic-device")
            .expect("a layout overlap must disable only the secret service");
        assert!(loaded.secret_service().is_none());
        let persisted = loaded
            .attached_store_for_test()
            .unwrap()
            .writer()
            .secret_record(&record.context.id)
            .unwrap()
            .unwrap();
        assert_eq!(persisted, record);
        assert_eq!(std::fs::read(root.join("secrets/vault.key")).unwrap(), key);
        assert_eq!(
            std::fs::read(root.join("secrets").join(&record.context.id)).unwrap(),
            blob
        );
    }
}

#[test]
fn secrets_degraded_startup_keeps_admission_fence_and_allows_disjoint_sources() {
    let root = tempfile::tempdir().unwrap();
    let mut loaded = app(root.path());
    loaded.projects_dir = root.path().to_path_buf();
    let mut loaded = loaded
        .with_secret_vault(root.path(), "synthetic-device")
        .expect("overlap must disable only secret access");
    assert!(loaded.secret_service().is_none());
    assert!(loaded
        .validate_secret_source(&root.path().join("secrets/new-source"))
        .is_err());
    assert!(loaded
        .project_create(&json!({"name":"synthetic-blocked","parent":root.path().join("secrets")}))
        .is_err());
    assert!(loaded.deferred_work.is_none());
    assert!(loaded
        .project_create(&json!({"name":"synthetic-safe","parent":root.path()}))
        .is_ok());
    assert!(loaded.deferred_work.is_some());
    assert!(!root.path().join("secrets").exists());
}

#[test]
fn secrets_open_error_keeps_normal_app_available_and_retains_admission_policy() {
    let root = tempfile::tempdir().unwrap();
    let loaded = app(root.path())
        .with_secret_vault(root.path(), "")
        .expect("a secret-service open error must not stop the normal daemon");
    assert!(loaded.secret_service().is_none());
    assert!(loaded
        .validate_secret_source(&root.path().join("secrets/new-source"))
        .is_err());
    assert!(!root.path().join("secrets").exists());
}

#[test]
fn secret_membership_projection_follows_config_writes_and_startup() {
    let root = tempfile::tempdir().unwrap();
    let mut state = protected_app(root.path());
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let project = state.add_project(source, "main".into());
    let environment = state.projects.get(&project).unwrap().environment_id.clone();
    let config = state.config_value(&state.projects_dir, state.default_harness, state.isolation);
    state.persist_config(&config).unwrap();
    let store = state.store.as_ref().unwrap().clone();
    assert_eq!(
        store
            .writer_for_test()
            .query_row(
                "SELECT environment_id FROM secret_project_membership WHERE project_id=?1",
                [&project],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        environment
    );
    assert!(store
        .writer()
        .device_registry_generation()
        .unwrap()
        .is_some());
    state.project_archive(&project).unwrap();
    let config = state.config_value(&state.projects_dir, state.default_harness, state.isolation);
    state.persist_config(&config).unwrap();
    state.settle_device_lifecycle();
    assert!(store
        .writer_for_test()
        .query_row(
            "SELECT archived FROM secret_project_membership WHERE project_id=?1",
            [&project],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
    // A stale disk projection cannot survive composition from current config.
    store
        .writer_for_test()
        .execute("UPDATE secret_project_membership SET archived=0", [])
        .unwrap();
    let state = state
        .with_secret_vault(root.path(), "synthetic-device")
        .unwrap();
    assert!(state.secret_service().is_some());
    assert!(store
        .writer_for_test()
        .query_row(
            "SELECT archived FROM secret_project_membership WHERE project_id=?1",
            [project],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
}

#[test]
fn secrets_daemon_open_requires_loaded_config_only_for_existing_old_store() {
    for config_state in ["absent", "unreadable", "fresh"] {
        let root = tempfile::tempdir().unwrap();
        let tasks = root.path().join("tasks");
        let config_path = root.path().join("config.json");
        let path = tasks.join("build.db");
        if config_state != "fresh" {
            std::fs::create_dir_all(&tasks).unwrap();
            let c = rusqlite::Connection::open(&path).unwrap();
            c.execute_batch(include_str!(
                "../../../tests/fixtures/secrets-schema-26.sql"
            ))
            .unwrap();
            c.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL); INSERT INTO meta VALUES('schema_version','26'); INSERT INTO secret_vault_state VALUES(1,'retained-key',NULL);INSERT INTO secret_records(id,device_id,creator,name,grant_scope,grant_project_id,stage_owner_id,stage_conversation_id,stage_generation,created_at,revision,upload_request_id,upload_digest,blob_present) VALUES('sec_00000000-0000-4000-8000-000000000001','device','creator','RETAINED','project','project-a','owner','conversation','generation',100,1,'00000000-0000-4000-8000-000000000002','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',1);").unwrap();
        }
        let before = std::fs::read(&path).ok();
        if config_state == "unreadable" {
            // A directory gives a deterministic read failure, even under root.
            std::fs::create_dir(&config_path).unwrap();
        }
        let context =
            HarnessContext::resolved(root.path().join("mcp.sock"), root.path().to_path_buf())
                .unwrap();
        let state =
            AppState::new_unrooted_configured(root.path().join("worktrees"), "main", true, context);
        let configured = state.with_config(&config_path);
        if config_state == "unreadable" {
            let error = configured
                .err()
                .expect("unreadable config must refuse startup");
            assert!(error.to_string().contains("read config"), "{error}");
        } else {
            let opened = configured.unwrap().with_task_store(&tasks);
            if config_state == "fresh" {
                assert!(opened.is_ok());
            } else {
                let error = opened.err().expect("missing config must not migrate");
                assert!(error.contains("authoritative config"), "{error}");
            }
        }
        if let Some(before) = before {
            assert_eq!(std::fs::read(&path).unwrap(), before, "{config_state}");
        }
    }
}
