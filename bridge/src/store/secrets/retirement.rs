use rusqlite::Transaction;

pub(in crate::store) fn retire_conversation_in(
    tx: &Transaction<'_>,
    conversation: &str,
) -> rusqlite::Result<()> {
    tx.execute("UPDATE secret_records SET revoked_at=CAST(strftime('%s','now') AS INTEGER),revocation_reason='revoked',revision=revision+1 WHERE grant_scope='agent' AND (grant_conversation_id=?1 OR id IN (SELECT secret_id FROM secret_final_bindings WHERE conversation_id=?1)) AND revoked_at IS NULL",[conversation])?;
    Ok(())
}

pub(in crate::store) fn retire_agent_in(tx: &Transaction<'_>, agent: &str) -> rusqlite::Result<()> {
    tx.execute("UPDATE secret_records SET revoked_at=CAST(strftime('%s','now') AS INTEGER),revocation_reason='revoked',revision=revision+1 WHERE grant_scope='agent' AND (grant_agent_id=?1 OR grant_conversation_id=?1 OR id IN (SELECT secret_id FROM secret_final_bindings WHERE agent_id=?1 OR conversation_id=?1)) AND revoked_at IS NULL",[agent])?;
    Ok(())
}

pub(in crate::store) fn retire_owner_in(tx: &Transaction<'_>, owner: &str) -> rusqlite::Result<()> {
    tx.execute("UPDATE secret_records SET revoked_at=CAST(strftime('%s','now') AS INTEGER),revocation_reason='revoked',revision=revision+1 WHERE grant_scope='agent' AND (grant_agent_id IN (SELECT id FROM agents WHERE owner_id=?1) OR grant_conversation_id IN (SELECT id FROM agents WHERE owner_id=?1) OR id IN (SELECT secret_id FROM secret_final_bindings WHERE owner_id=?1 OR agent_id IN (SELECT id FROM agents WHERE owner_id=?1) OR conversation_id IN (SELECT id FROM agents WHERE owner_id=?1))) AND revoked_at IS NULL",[owner])?;
    Ok(())
}

pub(in crate::store) fn retire_project_in(
    tx: &Transaction<'_>,
    project: &str,
) -> rusqlite::Result<()> {
    tx.execute("UPDATE secret_records SET revoked_at=CAST(strftime('%s','now') AS INTEGER),revocation_reason='revoked',revision=revision+1 WHERE grant_scope='project' AND grant_project_id=?1 AND revoked_at IS NULL",[project])?;
    retire_owner_in(tx, project)
}
