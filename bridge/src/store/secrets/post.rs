//! Atomic user-owned claims; immutable encrypted upload contexts are preserved.
use super::{attach_in, records};
use crate::secrets::{
    GrantScope, Result, SecretError, SecretPostBinding, SecretRecord, STAGED_LIFETIME_SECONDS,
};
use crate::store::StoreWriter;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::collections::HashSet;

impl StoreWriter {
    pub fn secrets_post_preflight(&self, binding: &SecretPostBinding) -> Result<Vec<SecretRecord>> {
        preflight(&self.connection(), binding)
    }
}

fn preflight(connection: &Connection, binding: &SecretPostBinding) -> Result<Vec<SecretRecord>> {
    if binding.ids.is_empty()
        || binding.ids.len() > 10
        || binding.ids.iter().collect::<HashSet<_>>().len() != binding.ids.len()
    {
        return Err(SecretError::Invalid);
    }
    let mut names = HashSet::new();
    let mut result = Vec::with_capacity(binding.ids.len());
    for id in &binding.ids {
        let mut record = records::required(connection, id)?;
        validate_context(&record, binding)?;
        if !names.insert(record.context.name.clone()) {
            return Err(SecretError::Invalid);
        }
        if record.attached_at.is_some()
            || record.revoked_at.is_some()
            || !record.blob_present
            || binding.attached_at < record.created_at
            || binding.attached_at.saturating_sub(record.created_at) >= STAGED_LIFETIME_SECONDS
        {
            return Err(SecretError::Unavailable);
        }
        record.attached_at = Some(binding.attached_at);
        record.expires_at = record
            .context
            .ttl_seconds
            .map(|ttl| {
                binding
                    .attached_at
                    .checked_add(ttl)
                    .ok_or(SecretError::Invalid)
            })
            .transpose()?;
        record.revision += 1;
        result.push(record);
    }
    Ok(result)
}

fn validate_context(record: &SecretRecord, binding: &SecretPostBinding) -> Result<()> {
    let context = &record.context;
    if context.device_id != binding.device_id
        || context.creator != binding.creator
        || context.stage.owner_id != binding.owner_id
    {
        return Err(SecretError::Unavailable);
    }
    match (&context.stage.draft_id, &binding.draft_id) {
        (Some(staged), Some(sent)) if staged == sent => {}
        (None, None)
            if context.stage.conversation_id == binding.conversation_id
                && context.stage.generation == binding.generation => {}
        _ => return Err(SecretError::Unavailable),
    }
    let allowed = match &context.grant {
        GrantScope::Project { project_id } => project_id == &binding.project_id,
        GrantScope::Agent {
            agent_id,
            conversation_id,
            generation,
        } => {
            context.stage.draft_id.is_some()
                || (agent_id == &binding.agent_id
                    && conversation_id == &binding.conversation_id
                    && generation == &binding.generation)
        }
    };
    if !allowed {
        return Err(SecretError::Unavailable);
    }
    Ok(())
}

/// Must run in the same FULL transaction as the message and operation receipt.
pub fn attach_post_in(
    tx: &Transaction<'_>,
    binding: &SecretPostBinding,
    operation: &str,
) -> Result<Vec<SecretRecord>> {
    let existing: Option<String> = tx
        .query_row(
            "SELECT secret_ids FROM secret_bindings WHERE operation_id=?1",
            [operation],
            |row| row.get(0),
        )
        .optional()?;
    if existing.is_some() {
        // attach_in validates the exact set; a replay never restarts TTL.
        return attach_in(tx, &binding.ids, operation, binding.attached_at);
    }
    preflight(tx, binding)?;
    let records = attach_in(tx, &binding.ids, operation, binding.attached_at)?;
    for record in &records {
        tx.execute("INSERT INTO secret_final_bindings(secret_id,operation_id,project_id,owner_id,agent_id,conversation_id,generation) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![record.context.id,operation,binding.project_id,binding.owner_id,binding.agent_id,binding.conversation_id,binding.generation])?;
        super::append_metadata_in(tx, record, "attached", binding.attached_at)?;
    }
    Ok(records)
}

pub(super) fn final_scope_allows(
    connection: &Connection,
    caller: &crate::secrets::SecretCaller,
    record: &SecretRecord,
) -> Result<bool> {
    if record.context.stage.draft_id.is_none()
        || matches!(record.context.grant, GrantScope::Project { .. })
    {
        return Ok(super::scope_allows(caller, record));
    }
    if caller.device_id != record.context.device_id {
        return Ok(false);
    }
    let binding: Option<(String,String,String,String)> = connection.query_row("SELECT project_id,agent_id,conversation_id,generation FROM secret_final_bindings WHERE secret_id=?1",[&record.context.id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
    Ok(
        binding.is_some_and(|(project, agent, conversation, generation)| {
            project == caller.project_id
                && agent == caller.agent_id
                && conversation == caller.conversation_id
                && generation == caller.generation
        }),
    )
}
