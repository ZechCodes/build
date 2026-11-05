use crate::operation::OperationReceipt;
use crate::secrets::SecretPostBinding;
use crate::store::StoreError;
use rusqlite::{Connection, OptionalExtension, Transaction};

pub(super) fn claim_secrets_in(
    tx: &Transaction<'_>,
    claim: Option<&SecretPostBinding>,
    receipt: &OperationReceipt,
) -> Result<(), StoreError> {
    let Some(claim) = claim else {
        return Ok(());
    };
    if receipt.requested_by.is_some()
        || receipt.method != crate::operation::THREAD_POST_METHOD
        || receipt.entity_id != claim.owner_id
        || receipt.agent_id != claim.agent_id
        || receipt.conversation_id != claim.conversation_id
        || !current_target(tx, claim)?
    {
        return Err(StoreError::Io(std::io::Error::other("secret_unavailable")));
    }
    let records = crate::store::secrets::attach_post_in(tx, claim, &receipt.operation_id)
        .map_err(|error| StoreError::Io(std::io::Error::other(error.to_string())))?;
    let expected = records
        .iter()
        .map(crate::thread::MessageSecret::from_record)
        .collect::<crate::secrets::Result<Vec<_>>>()
        .map_err(|error| StoreError::Io(std::io::Error::other(error.to_string())))?;
    let delivered = receipt
        .delivery
        .as_ref()
        .and_then(|delivery| delivery.payload.as_ref())
        .and_then(|payload| payload.messages.last());
    if delivered.is_none_or(|message| message.secrets != expected) {
        return Err(StoreError::Io(std::io::Error::other(
            "secret metadata mismatch",
        )));
    }
    Ok(())
}

fn current_target(connection: &Connection, claim: &SecretPostBinding) -> Result<bool, StoreError> {
    let saved: Option<(String, String)> = connection
        .query_row(
            "SELECT owner_id,record FROM agents WHERE id=?1",
            [&claim.agent_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((owner, raw)) = saved else {
        return Ok(false);
    };
    let agent: crate::agent::Agent = serde_json::from_str(&raw)
        .map_err(|_| StoreError::Io(std::io::Error::other("secret_unavailable")))?;
    if owner != claim.owner_id
        || agent.conversation_id() != claim.conversation_id
        || agent.state == crate::agent::AgentLifecycle::Ended
    {
        return Ok(false);
    }
    let canonical: Option<String> = connection
        .query_row(
            "SELECT record FROM agents WHERE id=?1",
            [&claim.conversation_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(raw) = canonical else {
        return Ok(false);
    };
    let canonical: crate::agent::Agent = serde_json::from_str(&raw)
        .map_err(|_| StoreError::Io(std::io::Error::other("secret_unavailable")))?;
    let project: Option<String> = connection.query_row("SELECT p.project_id FROM device_owner_projects o JOIN device_project_ids p ON p.path=o.project_path WHERE o.owner_id=?1",[&claim.owner_id],|row|row.get(0)).optional()?;
    Ok(canonical.thread.id == claim.generation
        && project.as_deref() == Some(claim.project_id.as_str()))
}
