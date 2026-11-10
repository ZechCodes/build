//! Scoped, bounded metadata reads on either an independent snapshot or writer fallback.
use super::records;
use crate::api::v1::secrets::{
    AuditPage, AuditParams, ListParams, MetadataPage, SecretMetadata, SecretScope,
};
use crate::secrets::{Result, SecretError, StageContext};
use crate::store::{ReadTransaction, StoreWriter};
use base64::Engine;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

impl ReadTransaction<'_> {
    pub fn secret_metadata_page(
        &self,
        device: &str,
        creator: &str,
        params: &ListParams,
    ) -> Result<MetadataPage> {
        list(self.connection(), device, creator, params)
    }
    pub fn secret_metadata_audit(
        &self,
        device: &str,
        creator: &str,
        params: &AuditParams,
    ) -> Result<AuditPage> {
        audit(self.connection(), device, creator, params)
    }
}
impl StoreWriter {
    pub fn secret_metadata_resolve(
        &self,
        record: &crate::secrets::SecretRecord,
    ) -> Result<crate::secrets::SecretRecord> {
        let mut record = record.clone();
        resolve_binding(&self.connection(), &mut record, None)?;
        Ok(record)
    }
    pub fn secret_metadata_page(
        &self,
        device: &str,
        creator: &str,
        params: &ListParams,
    ) -> Result<MetadataPage> {
        list(&self.connection(), device, creator, params)
    }
    pub fn secret_metadata_audit(
        &self,
        device: &str,
        creator: &str,
        params: &AuditParams,
    ) -> Result<AuditPage> {
        audit(&self.connection(), device, creator, params)
    }
}

pub(super) fn authorize(connection: &Connection, scope: &SecretScope) -> Result<()> {
    scope.validate()?;
    if !super::membership::clean(connection)? {
        return Err(SecretError::Unavailable);
    }
    if let Some(environment) = &scope.environment_id {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM secret_environments WHERE environment_id=?1)",
            [environment],
            |r| r.get(0),
        )?;
        return if exists {
            Ok(())
        } else {
            Err(SecretError::Unavailable)
        };
    }
    authorize_project(connection, scope)
}

fn authorize_project(connection: &Connection, scope: &SecretScope) -> Result<()> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM device_project_ids WHERE project_id=?1)",
        [&scope.project_id],
        |row| row.get(0),
    )?;
    if !exists || !super::membership::current_project(connection, &scope.project_id)? {
        return Err(SecretError::Unavailable);
    }
    let Some(stage) = &scope.stage else {
        return Ok(());
    };
    let project: Option<String> = connection.query_row("SELECT p.project_id FROM device_owner_projects o JOIN device_project_ids p ON p.path=o.project_path WHERE o.owner_id=?1", [&stage.owner_id], |row| row.get(0)).optional()?;
    if project.as_deref() != Some(scope.project_id.as_str()) && stage.owner_id != scope.project_id {
        return Err(SecretError::Unavailable);
    }
    if stage.draft_id.is_some() {
        return Ok(());
    }
    let json: Option<String> = connection
        .query_row(
            "SELECT a.record FROM agents a JOIN device_owner_projects o ON o.owner_id=a.owner_id JOIN device_project_ids p ON p.path=o.project_path WHERE a.id=?1 AND p.project_id=?2",
            params![stage.conversation_id, scope.project_id],
            |row| row.get(0),
        )
        .optional()?;
    let agent: crate::agent::Agent = serde_json::from_str(&json.ok_or(SecretError::Unavailable)?)
        .map_err(|_| SecretError::Unavailable)?;
    if agent.conversation_id() != stage.conversation_id
        || agent.thread.id != stage.generation
        || agent.state == crate::agent::AgentLifecycle::Ended
    {
        return Err(SecretError::Unavailable);
    }
    Ok(())
}

fn now() -> i64 {
    crate::store::sync::now_ms() / 1000
}
fn stage_values(stage: Option<&StageContext>) -> (&str, &str, &str, Option<&str>) {
    stage.map_or(("", "", "", None), |stage| {
        (
            &stage.owner_id,
            &stage.conversation_id,
            &stage.generation,
            stage.draft_id.as_deref(),
        )
    })
}
fn filters(params: &ListParams) -> Result<(String, String)> {
    if !(1..=100).contains(&params.limit)
        || params.secret_ids.len() > 100
        || params.request_ids.len() > 100
    {
        return Err(SecretError::Invalid);
    }
    for id in &params.secret_ids {
        crate::secrets::validate_id(id)?;
    }
    for id in &params.request_ids {
        uuid::Uuid::parse_str(id).map_err(|_| SecretError::Invalid)?;
    }
    Ok((
        serde_json::to_string(&params.secret_ids).map_err(|_| SecretError::Invalid)?,
        serde_json::to_string(&params.request_ids).map_err(|_| SecretError::Invalid)?,
    ))
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    query: String,
    id: String,
}
fn query_hash(params: &ListParams) -> Result<String> {
    let json = serde_json::to_vec(&(&params.scope, &params.secret_ids, &params.request_ids))
        .map_err(|_| SecretError::Invalid)?;
    Ok(format!("{:x}", Sha256::digest(json)))
}
fn position(params: &ListParams) -> Result<String> {
    let Some(after) = &params.after else {
        return Ok(String::new());
    };
    if after.len() > 1024 {
        return Err(SecretError::Invalid);
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(after)
        .map_err(|_| SecretError::Invalid)?;
    let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| SecretError::Invalid)?;
    if cursor.query != query_hash(params)? {
        return Err(SecretError::Invalid);
    }
    crate::secrets::validate_id(&cursor.id)?;
    Ok(cursor.id)
}
fn cursor(params: &ListParams, id: String) -> Result<String> {
    let bytes = serde_json::to_vec(&Cursor {
        query: query_hash(params)?,
        id,
    })
    .map_err(|_| SecretError::Invalid)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}
pub(super) fn list(
    connection: &Connection,
    device: &str,
    creator: &str,
    asked: &ListParams,
) -> Result<MetadataPage> {
    let (ids, requests) = filters(asked)?;
    let after = position(asked)?;
    authorize(connection, &asked.scope)?;
    let (owner, conversation, generation, draft) = stage_values(asked.scope.stage.as_ref());
    let sql = format!("SELECT {} FROM secret_records WHERE device_id=?1 AND creator=?2 AND id>?3 AND (
        (grant_scope='project' AND grant_project_id=?4)
        OR (grant_scope='environment' AND (
            grant_environment_id=?12 OR (grant_environment_id=(SELECT environment_id FROM secret_project_membership WHERE project_id=?4 AND archived=0)
                AND NOT EXISTS(SELECT 1 FROM secret_records p WHERE p.device_id=secret_records.device_id AND p.name=secret_records.name AND p.grant_scope='project' AND p.grant_project_id=?4 AND p.attached_at IS NOT NULL AND p.revoked_at IS NULL AND p.blob_present=1 AND (p.expires_at IS NULL OR p.expires_at>?13)))))
        OR (grant_scope='agent' AND ?5!='' AND ((stage_owner_id=?5 AND stage_conversation_id=?6 AND stage_generation=?7 AND stage_draft_id IS ?8)
            OR EXISTS(SELECT 1 FROM secret_final_bindings b WHERE b.secret_id=secret_records.id AND b.project_id=?4 AND b.owner_id=?5 AND b.conversation_id=?6 AND b.generation=?7))))
        AND (json_array_length(?9)=0 OR id IN (SELECT value FROM json_each(?9))) AND (json_array_length(?10)=0 OR upload_request_id IN (SELECT value FROM json_each(?10))) ORDER BY id LIMIT ?11", records::COLUMNS);
    let mut statement = connection.prepare(&sql)?;
    let mut records = statement
        .query_map(
            params![
                device,
                creator,
                after,
                asked.scope.project_id,
                owner,
                conversation,
                generation,
                draft,
                ids,
                requests,
                asked.limit as i64 + 1,
                asked.scope.environment_id.as_deref().unwrap_or(""),
                now()
            ],
            records::from_row,
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for record in &mut records {
        records::validate_loaded(record)?;
        resolve_binding(connection, record, asked.scope.stage.as_ref())?;
    }
    let has_more = records.len() > asked.limit;
    records.truncate(asked.limit);
    let next = if has_more {
        records
            .last()
            .map(|record| cursor(asked, record.context.id.clone()))
            .transpose()?
    } else {
        None
    };
    let server_now = now();
    Ok(MetadataPage {
        secrets: records
            .iter()
            .map(|record| SecretMetadata::from_record(record, server_now))
            .collect(),
        server_now,
        next,
    })
}
pub(super) fn resolve_binding(
    connection: &Connection,
    record: &mut crate::secrets::SecretRecord,
    stage: Option<&StageContext>,
) -> Result<()> {
    if stage.is_some_and(|stage| stage.draft_id.is_some()) {
        return Ok(());
    }
    let binding: Option<(String,String,String,String)> = connection.query_row(
        "SELECT owner_id,agent_id,conversation_id,generation FROM secret_final_bindings WHERE secret_id=?1",
        [&record.context.id], |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
    if let Some((owner, agent, conversation, generation)) = binding {
        record.context.stage = StageContext {
            owner_id: owner,
            conversation_id: conversation.clone(),
            generation: generation.clone(),
            draft_id: None,
        };
        if matches!(
            record.context.grant,
            crate::secrets::GrantScope::Agent { .. }
        ) {
            record.context.grant = crate::secrets::GrantScope::Agent {
                agent_id: agent,
                conversation_id: conversation,
                generation,
            };
        }
    }
    Ok(())
}

fn audit(
    connection: &Connection,
    device: &str,
    creator: &str,
    asked: &AuditParams,
) -> Result<AuditPage> {
    if !(1..=100).contains(&asked.limit) || asked.after < 0 {
        return Err(SecretError::Invalid);
    }
    let query = ListParams {
        scope: asked.scope.clone(),
        secret_ids: vec![asked.secret_id.clone()],
        request_ids: vec![],
        after: None,
        limit: 1,
    };
    if list(connection, device, creator, &query)?
        .secrets
        .is_empty()
    {
        return Err(SecretError::Unavailable);
    }
    let mut statement=connection.prepare("SELECT sequence,secret_id,creator,project_id,owner_id,agent_id,conversation_id,generation,session_incarnation,request_id,occurred_at,outcome,attempt_count,last_occurred_at,environment_id FROM secret_audit WHERE secret_id=?1 AND sequence>?2 ORDER BY sequence LIMIT ?3")?;
    let mut events = statement
        .query_map(
            params![asked.secret_id, asked.after, asked.limit as i64 + 1],
            super::audit::audit_row,
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let has_more = events.len() > asked.limit;
    events.truncate(asked.limit);
    let next = if has_more {
        events.last().map(|event| event.sequence)
    } else {
        None
    };
    Ok(AuditPage {
        events,
        server_now: now(),
        next,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::GrantScope;
    use crate::store::{secrets::tests::record, Store};

    fn params() -> ListParams {
        ListParams {
            scope: SecretScope {
                environment_id: None,
                project_id: "project-a".into(),
                stage: None,
            },
            secret_ids: vec![],
            request_ids: vec![],
            after: None,
            limit: 1,
        }
    }
    fn project(store: &Store, id: &str) {
        super::super::seed_current_project_for_test(store, id);

        store
            .writer_for_test()
            .execute(
                "INSERT INTO device_project_ids(path,project_id) VALUES (?1,?2)",
                params![format!("/synthetic/{id}"), id],
            )
            .unwrap();
    }
    #[test]
    fn metadata_scopes_reject_noncanonical_drafts_empty_and_oversize_context() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        project(&store, "project-a");
        let mut asked = params();
        asked.scope.stage = Some(StageContext {
            owner_id: "project-a".into(),
            conversation_id: "provisional".into(),
            generation: "provisional".into(),
            draft_id: Some("not-a-uuid".into()),
        });
        assert!(matches!(
            store
                .writer()
                .secret_metadata_page("device", "device_user", &asked),
            Err(SecretError::Invalid)
        ));
        asked.scope.stage.as_mut().unwrap().draft_id = Some(uuid::Uuid::new_v4().to_string());
        asked.scope.stage.as_mut().unwrap().generation.clear();
        assert!(matches!(
            store
                .writer()
                .secret_metadata_page("device", "device_user", &asked),
            Err(SecretError::Invalid)
        ));
        asked.scope.stage = None;
        asked.scope.project_id = "p".repeat(129);
        assert!(matches!(
            store
                .writer()
                .secret_metadata_page("device", "device_user", &asked),
            Err(SecretError::Invalid)
        ));
    }

    #[test]
    fn timeout_lookup_by_request_and_scoped_paging_return_only_allowlisted_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        project(&store, "project-a");
        let mut first = record(
            "device_user",
            GrantScope::Project {
                project_id: "project-a".into(),
            },
        );
        first.context.id = "sec_00000000-0000-4000-8000-000000000001".into();
        let mut second = record("device_user", first.context.grant.clone());
        second.context.id = "sec_00000000-0000-4000-8000-000000000002".into();
        for value in [&first, &second] {
            store.writer().secrets_stage(value).unwrap();
        }
        let mut asked = params();
        let page = store
            .writer()
            .secret_metadata_page("synthetic-device", "device_user", &asked)
            .unwrap();
        assert_eq!(page.secrets.len(), 1);
        assert_eq!(page.secrets[0].secret_id, first.context.id);
        asked.after = page.next;
        let page = store
            .writer()
            .secret_metadata_page("synthetic-device", "device_user", &asked)
            .unwrap();
        assert_eq!(page.secrets[0].secret_id, second.context.id);
        assert!(page.next.is_none());
        asked.after = None;
        asked.request_ids = vec![first.upload_request_id.clone()];
        let page = store
            .writer()
            .secret_metadata_page("synthetic-device", "device_user", &asked)
            .unwrap();
        assert_eq!(page.secrets[0].secret_id, first.context.id);
        let wire = serde_json::to_value(&page).unwrap();
        for forbidden in ["upload_digest", "blob_present", "sealed_payload", "value"] {
            assert!(!wire.to_string().contains(forbidden));
        }
    }
    #[test]
    fn bound_provisional_metadata_projects_final_recipient_and_generation_is_rechecked() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        project(&store, "project-a");
        let mut agent = crate::agent::Agent::new(
            "synthetic-agent",
            "synthetic-owner",
            crate::models::ModelChoice::default(),
            1,
            "2026-11-05T00:00:00Z",
        );
        store.writer_for_test().execute("INSERT INTO device_owner_projects(owner_id,project_path) VALUES ('synthetic-owner','/synthetic/project-a')",[]).unwrap();
        store
            .writer_for_test()
            .execute(
                "INSERT INTO agents(id,owner_id,ordinal,record) VALUES (?1,?2,1,?3)",
                params![
                    agent.id,
                    agent.owner_id,
                    serde_json::to_string(&agent).unwrap()
                ],
            )
            .unwrap();
        let mut secret = record(
            "device_user",
            crate::secrets::GrantScope::Agent {
                agent_id: "draft-agent".into(),
                conversation_id: "draft-conversation".into(),
                generation: "draft-generation".into(),
            },
        );
        secret.context.stage.draft_id = Some(uuid::Uuid::new_v4().to_string());
        store.writer().secrets_stage(&secret).unwrap();
        store.writer_for_test().execute("INSERT INTO secret_final_bindings(secret_id,operation_id,project_id,owner_id,agent_id,conversation_id,generation) VALUES (?1,'synthetic-operation','project-a',?2,?3,?3,?4)",params![secret.context.id,agent.owner_id,agent.id,agent.thread.id]).unwrap();
        let mut asked = params();
        asked.scope.stage = Some(StageContext {
            owner_id: agent.owner_id.clone(),
            conversation_id: agent.id.clone(),
            generation: agent.thread.id.clone(),
            draft_id: None,
        });
        let page = store
            .writer()
            .secret_metadata_page("synthetic-device", "device_user", &asked)
            .unwrap();
        assert_eq!(page.secrets.len(), 1);
        assert_eq!(page.secrets[0].scope.generation, agent.thread.id);
        assert_eq!(page.secrets[0].scope.draft_id, None);
        let event = store.writer().secret_metadata_resolve(&secret).unwrap();
        assert_eq!(event.context.stage.generation, agent.thread.id);
        agent.thread.id = uuid::Uuid::new_v4().to_string();
        store
            .writer_for_test()
            .execute(
                "UPDATE agents SET record=?2 WHERE id=?1",
                params![agent.id, serde_json::to_string(&agent).unwrap()],
            )
            .unwrap();
        assert!(matches!(
            store
                .writer()
                .secret_metadata_page("synthetic-device", "device_user", &asked),
            Err(SecretError::Unavailable)
        ));
        let project_secret = record(
            "device_user",
            GrantScope::Project {
                project_id: "project-a".into(),
            },
        );
        store.writer().secrets_stage(&project_secret).unwrap();
        store
            .writer_for_test()
            .execute("DELETE FROM agents WHERE id=?1", [agent.id])
            .unwrap();
        let page = store
            .writer()
            .secret_metadata_page("synthetic-device", "device_user", &params())
            .unwrap();
        assert_eq!(page.secrets[0].secret_id, project_secret.context.id);
    }

    #[test]
    fn lifecycle_audit_is_atomic_metadata_only_and_retries_do_not_duplicate_facts() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        project(&store, "project-a");
        let secret = record(
            "device_user",
            GrantScope::Project {
                project_id: "project-a".into(),
            },
        );
        store.writer().secrets_stage(&secret).unwrap();
        for _ in 0..2 {
            store
                .writer()
                .secrets_attach(
                    std::slice::from_ref(&secret.context.id),
                    "synthetic-operation",
                    secret.created_at + 1,
                )
                .unwrap();
        }
        for _ in 0..2 {
            store
                .writer()
                .secrets_revoke(&secret.context.id, secret.created_at + 2)
                .unwrap();
        }
        let asked = AuditParams {
            scope: params().scope,
            secret_id: secret.context.id.clone(),
            after: 0,
            limit: 100,
        };
        let audit = store
            .writer()
            .secret_metadata_audit("synthetic-device", "device_user", &asked)
            .unwrap();
        assert_eq!(
            audit
                .events
                .iter()
                .map(|event| event.outcome.as_str())
                .collect::<Vec<_>>(),
            vec!["created", "attached", "revoked"]
        );
        assert!(audit
            .events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence));
        let wire = serde_json::to_string(&audit).unwrap();
        assert!(!wire.contains("TEST_ONLY_TOKEN"));
        assert!(!wire.contains("upload_digest"));
        assert!(store
            .writer()
            .secret_audit_page(&secret.context.id, 0, 100)
            .unwrap()
            .is_empty());
        assert!(store
            .writer()
            .secret_audit_summary(&secret.context.id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn cursors_are_scoped_and_limits_and_other_creators_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        project(&store, "project-a");
        project(&store, "project-b");
        for _ in 0..2 {
            store
                .writer()
                .secrets_stage(&record(
                    "device_user",
                    GrantScope::Project {
                        project_id: "project-a".into(),
                    },
                ))
                .unwrap();
        }
        let mut asked = params();
        let page = store
            .writer()
            .secret_metadata_page("synthetic-device", "device_user", &asked)
            .unwrap();
        assert!(store
            .writer()
            .secret_metadata_page("synthetic-device", "other-user", &asked)
            .unwrap()
            .secrets
            .is_empty());
        asked.after = page.next;
        asked.scope.project_id = "project-b".into();
        assert!(matches!(
            store
                .writer()
                .secret_metadata_page("synthetic-device", "device_user", &asked),
            Err(SecretError::Invalid)
        ));
        asked = params();
        asked.limit = 101;
        assert!(matches!(
            store
                .writer()
                .secret_metadata_page("synthetic-device", "device_user", &asked),
            Err(SecretError::Invalid)
        ));
    }
}
