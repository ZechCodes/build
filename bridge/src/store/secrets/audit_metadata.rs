//! User lifecycle facts share the release audit sequence without changing its policy.
use super::{metadata, records};
use crate::secrets::{GrantScope, Result, SecretError, SecretRecord};
use rusqlite::{params, OptionalExtension, Transaction};

pub(crate) fn append_metadata_in(
    tx: &Transaction<'_>,
    record: &SecretRecord,
    outcome: &str,
    now: i64,
) -> Result<()> {
    if !matches!(outcome, "created" | "attached" | "expired" | "revoked") || now < 0 {
        return Err(SecretError::Invalid);
    }
    let mut resolved = record.clone();
    metadata::resolve_binding(tx, &mut resolved, None)?;
    let context = &resolved.context;
    let project=match &context.grant {
        GrantScope::Project {project_id}=>project_id.clone(),
        GrantScope::Agent {..} | GrantScope::Environment {..} =>tx.query_row("SELECT p.project_id FROM device_owner_projects o JOIN device_project_ids p ON p.path=o.project_path WHERE o.owner_id=?1",[&context.stage.owner_id],|row|row.get(0)).optional()?.unwrap_or_else(||"unbound".into()),
    };
    let agent = match &context.grant {
        GrantScope::Agent { agent_id, .. } => agent_id.as_str(),
        GrantScope::Project { .. } | GrantScope::Environment { .. } => {
            context.stage.conversation_id.as_str()
        }
    };
    let request_id = uuid::Uuid::new_v4().to_string();
    let changed=tx.execute("INSERT INTO secret_audit(secret_id,creator,project_id,owner_id,agent_id,conversation_id,generation,session_incarnation,request_id,occurred_at,attempt_count,last_occurred_at,outcome,environment_id) VALUES (?1,?2,?3,?4,?5,?6,?7,'device_user',?8,?9,1,?9,?10,?11)",params![context.id,context.creator,project,context.stage.owner_id,agent,context.stage.conversation_id,context.stage.generation,request_id,now,outcome,super::membership::grant_environment(&context.grant)])?;
    if changed != 1 {
        return Err(SecretError::Unavailable);
    }
    // Require an existing record in the same transaction; no orphan audit IDs.
    records::required(tx, &context.id)?;
    Ok(())
}
