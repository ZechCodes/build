use super::{append_metadata_in, records};
use crate::secrets::{Result, SecretError, SecretRecord};
use rusqlite::{params, Transaction};

pub(in crate::store) fn retire_conversation_in(
    tx: &Transaction<'_>,
    conversation: &str,
) -> Result<Vec<SecretRecord>> {
    let ids = ids(tx, "SELECT id FROM secret_records WHERE grant_scope='agent' AND (grant_conversation_id=?1 OR id IN (SELECT secret_id FROM secret_final_bindings WHERE conversation_id=?1)) AND revoked_at IS NULL", [conversation])?;
    retire(tx, ids)
}

pub(in crate::store) fn retire_agent_in(
    tx: &Transaction<'_>,
    agent: &str,
) -> Result<Vec<SecretRecord>> {
    let ids = ids(tx, "SELECT id FROM secret_records WHERE grant_scope='agent' AND (grant_agent_id=?1 OR grant_conversation_id=?1 OR id IN (SELECT secret_id FROM secret_final_bindings WHERE agent_id=?1 OR conversation_id=?1)) AND revoked_at IS NULL", [agent])?;
    retire(tx, ids)
}

pub(in crate::store) fn retire_owner_in(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<SecretRecord>> {
    let ids = ids(tx, "SELECT id FROM secret_records WHERE grant_scope='agent' AND (grant_agent_id IN (SELECT id FROM agents WHERE owner_id=?1) OR grant_conversation_id IN (SELECT id FROM agents WHERE owner_id=?1) OR id IN (SELECT secret_id FROM secret_final_bindings WHERE owner_id=?1 OR agent_id IN (SELECT id FROM agents WHERE owner_id=?1) OR conversation_id IN (SELECT id FROM agents WHERE owner_id=?1))) AND revoked_at IS NULL", [owner])?;
    retire(tx, ids)
}

pub(in crate::store) fn retire_project_in(
    tx: &Transaction<'_>,
    project: &str,
) -> Result<Vec<SecretRecord>> {
    let ids = ids(tx, "SELECT id FROM secret_records WHERE grant_scope='project' AND grant_project_id=?1 AND revoked_at IS NULL", [project])?;
    let mut retired = retire(tx, ids)?;
    retired.extend(retire_owner_in(tx, project)?);
    Ok(retired)
}

fn ids<const N: usize>(tx: &Transaction<'_>, sql: &str, values: [&str; N]) -> Result<Vec<String>> {
    let mut query = tx.prepare(sql)?;
    let ids = query
        .query_map(rusqlite::params_from_iter(values), |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

fn retire(tx: &Transaction<'_>, ids: Vec<String>) -> Result<Vec<SecretRecord>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let now: i64 = tx.query_row("SELECT CAST(strftime('%s','now') AS INTEGER)", [], |row| {
        row.get(0)
    })?;
    let mut retired = Vec::with_capacity(ids.len());
    for id in ids {
        let changed = tx.execute("UPDATE secret_records SET revoked_at=?2,revocation_reason='revoked',revision=revision+1 WHERE id=?1 AND revoked_at IS NULL", params![id, now])?;
        if changed != 1 {
            return Err(SecretError::Unavailable);
        }
        let record = records::required(tx, &id)?;
        append_metadata_in(tx, &record, "revoked", now)?;
        retired.push(record);
    }
    Ok(retired)
}
