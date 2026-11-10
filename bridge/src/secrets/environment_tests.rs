use super::*;
use crate::api::v1::secrets::{AuditParams, ListParams, SecretScope};
use crate::secrets::SecretPostBinding;
use serde_json::json;

fn grant() -> GrantScope {
    serde_json::from_value(json!({"scope":"environment","environment_id":"env-a"})).unwrap()
}
fn membership(f: &Fixture, project: &str, environment: &str, archived: bool) {
    f.store
        .writer_for_test()
        .execute(
            "INSERT OR IGNORE INTO secret_environments VALUES (?1)",
            [environment],
        )
        .unwrap();
    f.store.writer_for_test().execute("INSERT INTO secret_project_membership(project_id,environment_id,archived) VALUES (?1,?2,?3) ON CONFLICT(project_id) DO UPDATE SET environment_id=?2,archived=?3",rusqlite::params![project,environment,archived]).unwrap();
}
fn setup() -> (Fixture, SecretCaller) {
    let f = Fixture::new();
    let sibling = f.add_member("sibling-agent", "sibling-owner", "sibling-project", false);
    membership(&f, &f.caller.project_id, "env-a", false);
    membership(&f, &sibling.project_id, "env-a", false);
    (f, sibling)
}
fn query(project: &str) -> ListParams {
    ListParams {
        include_environment: true,
        scope: serde_json::from_value(json!({"project_id":project})).unwrap(),
        secret_ids: vec![],
        request_ids: vec![],
        after: None,
        limit: 100,
    }
}
fn list(f: &Fixture, project: &str) -> Vec<crate::api::v1::secrets::SecretMetadata> {
    f.store
        .writer()
        .secret_metadata_page(&f.caller.device_id, "synthetic-user", &query(project))
        .unwrap()
        .secrets
}
fn read(f: &Fixture, caller: &SecretCaller, id: &str) -> Result<SecretValue> {
    f.service.release(caller, id, "synthetic-request", || true)
}
#[test]
fn environment_scoped_secret_readable_across_projects() {
    let (f, sibling) = setup();
    let secret = f.secret(grant(), None);
    for caller in [&f.caller, &sibling] {
        assert_eq!(
            read(&f, caller, &secret.context.id).unwrap().as_bytes(),
            b"synthetic-value-only"
        );
        assert_eq!(list(&f, &caller.project_id)[0].secret_id, secret.context.id);
    }
    let outsider = f.add_member("outside-agent", "outside-owner", "outside-project", false);
    membership(&f, &outsider.project_id, "env-b", false);
    assert!(matches!(
        read(&f, &outsider, &secret.context.id),
        Err(SecretError::Unavailable)
    ));
    assert!(list(&f, &outsider.project_id).is_empty());
}
#[test]
fn project_secret_overrides_environment_name() {
    let (f, sibling) = setup();
    let inherited = f.secret(grant(), None);
    let local = f.secret(
        GrantScope::Project {
            project_id: f.caller.project_id.clone(),
        },
        None,
    );
    assert_eq!(
        list(&f, &f.caller.project_id)
            .iter()
            .map(|s| &s.secret_id)
            .collect::<Vec<_>>(),
        vec![&local.context.id]
    );
    assert_eq!(
        list(&f, &sibling.project_id)[0].secret_id,
        inherited.context.id
    );
    assert!(matches!(
        read(&f, &f.caller, &inherited.context.id),
        Err(SecretError::Unavailable)
    ));
    assert!(read(&f, &sibling, &inherited.context.id).is_ok());
    f.service.revoke(&local.context.id).unwrap();
    assert!(read(&f, &f.caller, &inherited.context.id).is_ok());
}
#[test]
fn environment_membership_revalidated_per_read() {
    let (f, sibling) = setup();
    let secret = f.secret(grant(), None);
    assert!(read(&f, &sibling, &secret.context.id).is_ok());
    membership(&f, &sibling.project_id, "env-b", false);
    assert!(matches!(
        read(&f, &sibling, &secret.context.id),
        Err(SecretError::Unavailable)
    ));
    assert!(list(&f, &sibling.project_id).is_empty());
    membership(&f, &sibling.project_id, "env-a", false);
    assert!(read(&f, &sibling, &secret.context.id).is_ok());
    f.store
        .writer()
        .mark_sync_dirty("secrets", "membership", 1001)
        .unwrap();
    assert!(read(&f, &sibling, &secret.context.id).is_err());
    assert!(f
        .store
        .writer()
        .secret_metadata_page(
            &f.caller.device_id,
            "synthetic-user",
            &query(&sibling.project_id)
        )
        .is_err());
}
#[test]
fn archived_project_agent_not_current_for_secrets() {
    let (f, sibling) = setup();
    let secret = f.secret(grant(), None);
    membership(&f, &sibling.project_id, "env-a", true);
    assert!(matches!(
        read(&f, &sibling, &secret.context.id),
        Err(SecretError::ProjectArchived)
    ));
    assert!(f
        .store
        .writer()
        .secret_metadata_page(
            &f.caller.device_id,
            "synthetic-user",
            &query(&sibling.project_id)
        )
        .unwrap()
        .secrets
        .is_empty());
    assert!(read(&f, &f.caller, &secret.context.id).is_ok());
    membership(&f, &sibling.project_id, "env-a", false);
    assert!(read(&f, &sibling, &secret.context.id).is_ok());
}
mod release {
    use super::*;
    #[test]
    fn sibling_project_cannot_read_project_scoped_secret() {
        let (f, sibling) = setup();
        let secret = f.secret(
            GrantScope::Project {
                project_id: f.caller.project_id.clone(),
            },
            None,
        );
        assert!(matches!(
            read(&f, &sibling, &secret.context.id),
            Err(SecretError::Unavailable)
        ));
        assert!(f
            .store
            .writer()
            .secret_audit_page(&secret.context.id, 0, 100)
            .unwrap()
            .is_empty());
    }
}
mod listing {
    use super::*;
    #[test]
    fn sibling_project_cannot_read_project_scoped_secret() {
        let (f, sibling) = setup();
        f.secret(
            GrantScope::Project {
                project_id: f.caller.project_id.clone(),
            },
            None,
        );
        assert!(list(&f, &sibling.project_id).is_empty());
        let scope: SecretScope = serde_json::from_value(json!({"environment_id":"env-a"})).unwrap();
        let mut q = query(&sibling.project_id);
        q.scope = scope;
        assert!(f
            .store
            .writer()
            .secret_metadata_page(&f.caller.device_id, "synthetic-user", &q)
            .unwrap()
            .secrets
            .is_empty());
    }
}
mod audit {
    use super::*;
    #[test]
    fn sibling_project_cannot_read_project_scoped_secret() {
        let (f, sibling) = setup();
        let secret = f.secret(
            GrantScope::Project {
                project_id: f.caller.project_id.clone(),
            },
            None,
        );
        let params = AuditParams {
            scope: query(&sibling.project_id).scope,
            secret_id: secret.context.id,
            after: 0,
            limit: 100,
        };
        assert!(matches!(
            f.store
                .writer()
                .secret_metadata_audit(&f.caller.device_id, "synthetic-user", &params),
            Err(SecretError::Unavailable)
        ));
    }
}
mod attach {
    use super::*;
    #[test]
    fn sibling_project_cannot_read_project_scoped_secret() {
        let (f, sibling) = setup();
        let mut record = f.secret(
            GrantScope::Project {
                project_id: f.caller.project_id.clone(),
            },
            None,
        );
        record.context.id = format!("sec_{}", uuid::Uuid::new_v4());
        record.upload_request_id = uuid::Uuid::new_v4().to_string();
        record.attached_at = None;
        record.expires_at = None;
        record.message_operation_id = None;
        record.revision = 1;
        record.context.device_id = f.caller.device_id.clone();
        record.context.stage = StageContext {
            owner_id: sibling.owner_id.clone(),
            conversation_id: sibling.conversation_id.clone(),
            generation: sibling.generation.clone(),
            draft_id: None,
        };
        f.store.writer().secrets_stage(&record).unwrap();
        let binding = SecretPostBinding {
            ids: vec![record.context.id],
            device_id: f.caller.device_id.clone(),
            creator: "synthetic-user".into(),
            project_id: sibling.project_id,
            owner_id: sibling.owner_id,
            agent_id: sibling.agent_id,
            conversation_id: sibling.conversation_id,
            generation: sibling.generation,
            draft_id: None,
            attached_at: record.created_at + 1,
        };
        assert!(f.store.writer().secrets_post_preflight(&binding).is_err());
    }
}

#[test]
fn environment_delete_revokes_environment_scope() {
    let (f, sibling) = setup();
    let shared = f.secret(grant(), None);
    let project = f.secret(
        GrantScope::Project {
            project_id: f.caller.project_id.clone(),
        },
        None,
    );
    f.store
        .writer()
        .secrets_retire_environment("env-a")
        .unwrap();
    f.store
        .writer()
        .secrets_retire_environment("env-a")
        .unwrap();
    assert!(matches!(
        read(&f, &sibling, &shared.context.id),
        Err(SecretError::Revoked)
    ));
    let record = f
        .store
        .writer()
        .secret_record(&shared.context.id)
        .unwrap()
        .unwrap();
    assert!(record.revoked_at.is_some());
    assert!(f.release(&project.context.id).is_ok());
    assert_eq!(f.store.writer_for_test().query_row("SELECT COUNT(*) FROM secret_audit WHERE secret_id=?1 AND outcome='revoked' AND environment_id='env-a'",[&shared.context.id],|r|r.get::<_,i64>(0)).unwrap(),1);
}

#[test]
fn old_secret_list_excludes_environment_without_opt_in_even_after_override_revoke() {
    let (f, _) = setup();
    let local = f.secret(
        GrantScope::Project {
            project_id: f.caller.project_id.clone(),
        },
        None,
    );
    let old_query: ListParams =
        serde_json::from_value(json!({"scope":{"project_id":f.caller.project_id}})).unwrap();
    let page = |q: &ListParams| {
        f.store
            .writer()
            .secret_metadata_page(&f.caller.device_id, "synthetic-user", q)
            .unwrap()
    };
    let old_bytes = serde_json::to_vec(&page(&old_query).secrets).unwrap();
    let inherited = f.secret(grant(), None);
    assert_eq!(
        serde_json::to_vec(&page(&old_query).secrets).unwrap(),
        old_bytes
    );
    f.service.revoke(&local.context.id).unwrap();
    let legacy = page(&old_query);
    let local = f
        .store
        .writer()
        .secret_record(&local.context.id)
        .unwrap()
        .unwrap();
    let expected = crate::api::v1::secrets::SecretMetadata::from_record(&local, legacy.server_now);
    assert_eq!(
        serde_json::to_vec(&legacy.secrets).unwrap(),
        serde_json::to_vec(&vec![expected]).unwrap()
    );
    assert!(legacy.next.is_none());
    let mut staged = old_query.clone();
    staged.scope.stage = Some(local.context.stage.clone());
    assert_eq!(
        serde_json::to_vec(&page(&staged).secrets).unwrap(),
        serde_json::to_vec(&legacy.secrets).unwrap()
    );
    let opted: ListParams = serde_json::from_value(
        json!({"scope":{"project_id":f.caller.project_id},"include_environment":true}),
    )
    .unwrap();
    assert!(page(&opted)
        .secrets
        .iter()
        .any(|s| s.secret_id == inherited.context.id));
    assert!(read(&f, &f.caller, &inherited.context.id).is_ok());
}

#[test]
fn project_delete_preserves_environment_scope_for_sibling() {
    let (f, sibling) = setup();
    let shared = f.secret(grant(), None);
    let local = f.secret(
        GrantScope::Project {
            project_id: f.caller.project_id.clone(),
        },
        None,
    );
    f.store
        .writer()
        .secrets_retire_project(&f.caller.project_id)
        .unwrap();
    assert!(f
        .store
        .writer()
        .secret_record(&shared.context.id)
        .unwrap()
        .unwrap()
        .revoked_at
        .is_none());
    assert_eq!(
        read(&f, &sibling, &shared.context.id).unwrap().as_bytes(),
        b"synthetic-value-only"
    );
    assert!(matches!(
        f.release(&local.context.id),
        Err(SecretError::Revoked)
    ));
}

#[test]
fn old_secret_list_nonempty_cursor_preserves_contract_and_refuses_cross_mode() {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let (f, _) = setup();
    for _ in 0..3 {
        f.secret(
            GrantScope::Project {
                project_id: f.caller.project_id.clone(),
            },
            None,
        );
    }
    f.secret(grant(), None);
    let mut old: ListParams =
        serde_json::from_value(json!({"scope":{"project_id":f.caller.project_id},"limit":1}))
            .unwrap();
    let page = |q: &ListParams| {
        f.store
            .writer()
            .secret_metadata_page(&f.caller.device_id, "synthetic-user", q)
    };
    let first = page(&old).unwrap();
    let cursor = first.next.clone().expect("non-empty old cursor");
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&cursor)
        .unwrap();
    let cursor_json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let old_basis = serde_json::to_vec(&(&old.scope, &old.secret_ids, &old.request_ids)).unwrap();
    assert_eq!(
        cursor_json["query"],
        format!("{:x}", Sha256::digest(old_basis))
    );
    old.after = Some(cursor);
    let next = page(&old).unwrap();
    assert_ne!(next.secrets[0].secret_id, first.secrets[0].secret_id);
    assert!(matches!(next.secrets[0].grant, GrantScope::Project { .. }));
    let mut opted = old.clone();
    opted.include_environment = true;
    assert!(
        page(&opted).is_err(),
        "old cursor refused by opted-in query"
    );
    opted.after = None;
    let opted_cursor = page(&opted).unwrap().next.expect("non-empty opt-in cursor");
    old.after = Some(opted_cursor);
    assert!(page(&old).is_err(), "opt-in cursor refused by old query");
}

#[test]
fn project_revoke_and_audit_cannot_reach_an_environment_grant() {
    use crate::api::v1::secrets::RevokeParams;
    let (f, sibling) = setup();
    let shared = f.secret(grant(), None);
    let before = f
        .store
        .writer()
        .secret_record(&shared.context.id)
        .unwrap()
        .unwrap();
    let scope = query(&f.caller.project_id).scope;
    let mut audit = AuditParams {
        scope: scope.clone(),
        secret_id: shared.context.id.clone(),
        after: 0,
        limit: 100,
    };
    assert!(matches!(
        f.store
            .writer()
            .secret_metadata_audit(&f.caller.device_id, "synthetic-user", &audit),
        Err(SecretError::Unavailable)
    ));
    let mut revoke = RevokeParams {
        scope,
        secret_id: shared.context.id.clone(),
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    assert!(matches!(
        f.service.revoke_operation("synthetic-user", &revoke),
        Err(SecretError::Unavailable)
    ));
    revoke.secret_id = format!("sec_{}", uuid::Uuid::new_v4());
    assert!(matches!(
        f.service.revoke_operation("synthetic-user", &revoke),
        Err(SecretError::Unavailable)
    ));
    let after = f
        .store
        .writer()
        .secret_record(&shared.context.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(after).unwrap()
    );
    assert!(read(&f, &sibling, &shared.context.id).is_ok());
    audit.scope = serde_json::from_value(json!({"environment_id":"env-a"})).unwrap();
    assert!(f
        .store
        .writer()
        .secret_metadata_audit(&f.caller.device_id, "synthetic-user", &audit)
        .is_ok());
    revoke.scope = audit.scope;
    revoke.secret_id = shared.context.id;
    assert!(f
        .service
        .revoke_operation("synthetic-user", &revoke)
        .is_ok());
}

#[test]
fn legacy_secret_scopes_ignore_registry_and_membership_fences() {
    let (f, sibling) = setup();
    let local = f.secret(
        GrantScope::Project {
            project_id: f.caller.project_id.clone(),
        },
        None,
    );
    let agent = f.secret(f.agent_grant(), None);
    let shared = f.secret(grant(), None);
    f.store
        .writer()
        .mark_sync_dirty("registry", "lifecycle", 1001)
        .unwrap();
    assert!(f.release(&local.context.id).is_ok());
    assert!(f.release(&agent.context.id).is_ok());
    assert!(read(&f, &sibling, &shared.context.id).is_ok());
    assert!(!list(&f, &sibling.project_id).is_empty());
    f.store
        .writer()
        .mark_sync_dirty("secrets", "membership", 1002)
        .unwrap();
    // Environment projection presence is not legacy authority either.
    f.store
        .writer_for_test()
        .execute(
            "DELETE FROM secret_project_membership WHERE project_id=?1",
            [&f.caller.project_id],
        )
        .unwrap();
    assert!(f.release(&local.context.id).is_ok());
    assert!(f.release(&agent.context.id).is_ok());
    let mut old = query(&f.caller.project_id);
    old.include_environment = false;
    assert!(f
        .store
        .writer()
        .secret_metadata_page(&f.caller.device_id, "synthetic-user", &old)
        .is_ok());
    let audit = AuditParams {
        scope: old.scope.clone(),
        secret_id: local.context.id.clone(),
        after: 0,
        limit: 100,
    };
    assert!(f
        .store
        .writer()
        .secret_metadata_audit(&f.caller.device_id, "synthetic-user", &audit)
        .is_ok());
    assert!(matches!(
        f.store.writer().secret_metadata_page(
            &f.caller.device_id,
            "synthetic-user",
            &query(&sibling.project_id)
        ),
        Err(SecretError::Unavailable)
    ));
    assert!(read(&f, &sibling, &shared.context.id).is_err());
    let mut staged = local.clone();
    staged.context.id = format!("sec_{}", uuid::Uuid::new_v4());
    staged.upload_request_id = uuid::Uuid::new_v4().to_string();
    staged.attached_at = None;
    staged.expires_at = None;
    staged.message_operation_id = None;
    staged.revision = 1;
    let mut stage_scope = old.scope.clone();
    stage_scope.stage = Some(staged.context.stage.clone());
    f.store
        .writer()
        .secrets_stage_authorized(&staged, &stage_scope)
        .unwrap();
    let mut stage_list = old.clone();
    stage_list.scope = stage_scope;
    assert!(f
        .store
        .writer()
        .secret_metadata_page(&f.caller.device_id, "synthetic-user", &stage_list)
        .is_ok());
    let revoke = crate::api::v1::secrets::RevokeParams {
        scope: old.scope,
        secret_id: local.context.id,
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    assert!(f
        .service
        .revoke_operation("synthetic-user", &revoke)
        .is_ok());
}

#[test]
fn archived_project_keeps_metadata_and_safety_revoke_but_not_release() {
    let (f, _) = setup();
    let local = f.secret(
        GrantScope::Project {
            project_id: f.caller.project_id.clone(),
        },
        None,
    );
    membership(&f, &f.caller.project_id, "env-a", true);
    let mut q = query(&f.caller.project_id);
    q.include_environment = false;
    assert_eq!(
        f.store
            .writer()
            .secret_metadata_page(&f.caller.device_id, "synthetic-user", &q)
            .unwrap()
            .secrets
            .len(),
        1
    );
    let audit = AuditParams {
        scope: q.scope.clone(),
        secret_id: local.context.id.clone(),
        after: 0,
        limit: 100,
    };
    assert!(f
        .store
        .writer()
        .secret_metadata_audit(&f.caller.device_id, "synthetic-user", &audit)
        .is_ok());
    assert_eq!(
        f.release(&local.context.id).err().unwrap().to_string(),
        "project_archived"
    );
    let revoke = crate::api::v1::secrets::RevokeParams {
        scope: q.scope,
        secret_id: local.context.id,
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    assert!(f
        .service
        .revoke_operation("synthetic-user", &revoke)
        .is_ok());
}
