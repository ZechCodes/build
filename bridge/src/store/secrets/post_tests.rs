use super::*;
use crate::secrets::{GrantScope, SecretContext, SecretPostBinding, SecretRecord, StageContext};

fn staged(id: u128, draft: bool) -> SecretRecord {
    SecretRecord {
        context: SecretContext {
            id: format!("sec_{}", uuid::Uuid::from_u128(id)),
            device_id: "test-device".into(),
            creator: "device_user".into(),
            name: format!("TEST_{id}"),
            ttl_seconds: Some(60),
            grant: GrantScope::Agent {
                agent_id: "agent".into(),
                conversation_id: "agent".into(),
                generation: "thread".into(),
            },
            stage: StageContext {
                owner_id: "owner".into(),
                conversation_id: "agent".into(),
                generation: "thread".into(),
                draft_id: draft.then(|| "draft".into()),
            },
        },
        created_at: 100,
        attached_at: None,
        expires_at: None,
        revoked_at: None,
        revocation_reason: None,
        revision: 1,
        message_operation_id: None,
        upload_request_id: uuid::Uuid::from_u128(id).to_string(),
        upload_digest: "a".repeat(64),
        blob_present: true,
    }
}
fn binding(ids: Vec<String>) -> SecretPostBinding {
    SecretPostBinding {
        ids,
        device_id: "test-device".into(),
        creator: "device_user".into(),
        project_id: "project".into(),
        owner_id: "owner".into(),
        agent_id: "agent".into(),
        conversation_id: "agent".into(),
        generation: "thread".into(),
        draft_id: None,
        attached_at: 200,
    }
}

#[test]
fn post_secret_claim_is_all_or_none_and_retry_preserves_ttl() {
    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let first = staged(1, false);
    let second = staged(2, false);
    writer.secrets_stage(&first).unwrap();
    writer.secrets_stage(&second).unwrap();
    let mut claim = binding(vec![first.context.id.clone(), second.context.id.clone()]);
    claim.creator = "wrong-user".into();
    assert!(writes::durable(&writer, |tx| attach_post_in(tx, &claim, "operation")).is_err());
    assert!(writer
        .secret_record(&first.context.id)
        .unwrap()
        .unwrap()
        .attached_at
        .is_none());
    claim.creator = "device_user".into();
    let accepted = writes::durable(&writer, |tx| attach_post_in(tx, &claim, "operation")).unwrap();
    claim.attached_at = 400;
    assert_eq!(
        writes::durable(&writer, |tx| attach_post_in(tx, &claim, "operation")).unwrap(),
        accepted
    );
    assert_eq!(accepted[0].attached_at, Some(200));
    assert_eq!(accepted[0].expires_at, Some(260));
}

#[test]
fn post_secret_claim_checks_generation_and_provisional_draft() {
    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let record = staged(3, true);
    writer.secrets_stage(&record).unwrap();
    let mut claim = binding(vec![record.context.id.clone()]);
    claim.generation = "new-thread".into();
    assert!(writes::durable(&writer, |tx| attach_post_in(tx, &claim, "operation")).is_err());
    claim.draft_id = Some("draft".into());
    assert!(writes::durable(&writer, |tx| attach_post_in(tx, &claim, "operation")).is_ok());
    assert_eq!(
        writer
            .secret_record(&record.context.id)
            .unwrap()
            .unwrap()
            .context,
        record.context,
        "AAD remains immutable"
    );
    let connection = writer.connection();
    let generation: String = connection
        .query_row(
            "SELECT generation FROM secret_final_bindings WHERE secret_id=?1",
            [&record.context.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(generation, "new-thread");
}

#[test]
fn failure_after_secret_binding_rolls_back_all_claims() {
    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let record = staged(4, false);
    writer.secrets_stage(&record).unwrap();
    let claim = binding(vec![record.context.id.clone()]);
    let result: crate::secrets::Result<()> = writes::durable(&writer, |tx| {
        attach_post_in(tx, &claim, "operation")?;
        Err(crate::secrets::SecretError::Unavailable)
    });
    assert!(result.is_err());
    assert!(writer
        .secret_record(&record.context.id)
        .unwrap()
        .unwrap()
        .attached_at
        .is_none());
    let connection = writer.connection();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM secret_bindings", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn bound_provisional_agent_grants_retire_with_the_final_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::Store::new(dir.path()).unwrap();
    let writer = store.writer();
    let record = staged(5, true);
    writer.secrets_stage(&record).unwrap();
    let mut claim = binding(vec![record.context.id.clone()]);
    claim.conversation_id = "final-conversation".into();
    claim.draft_id = Some("draft".into());
    writes::durable(&writer, |tx| attach_post_in(tx, &claim, "operation")).unwrap();
    writes::durable(&writer, |tx| {
        retirement::retire_conversation_in(tx, "final-conversation")?;
        Ok(())
    })
    .unwrap();
    assert!(writer
        .secret_record(&record.context.id)
        .unwrap()
        .unwrap()
        .revoked_at
        .is_some());
}
