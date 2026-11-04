use super::writes::durable;
use crate::secrets::{Result, SecretError};
use crate::store::StoreWriter;
use rusqlite::{params, Connection, OptionalExtension};

type KeyState = (Option<String>, Option<String>);

impl StoreWriter {
    /// Identifier of the active vault key, never key material. Both markers
    /// are validated together so malformed pending state cannot be ignored.
    pub fn secret_vault_key_id(&self) -> Result<Option<String>> {
        Ok(key_state_in(&self.connection())?.0)
    }

    pub fn secret_vault_pending_key_id(&self) -> Result<Option<String>> {
        Ok(key_state_in(&self.connection())?.1)
    }

    pub fn secrets_unretired_count(&self) -> Result<usize> {
        unretired_in(&self.connection())
    }

    /// A different or previously unknown key cannot acquire existing grants.
    pub fn secrets_set_vault_key_id(&self, key_id: &str) -> Result<()> {
        validate_key_id(key_id)?;
        durable(self, |tx| {
            let (previous, pending) = key_state_in(tx)?;
            if pending.is_some() || (previous.as_deref() != Some(key_id) && unretired_in(tx)? > 0) {
                return Err(SecretError::Conflict);
            }
            tx.execute("INSERT INTO secret_vault_state(singleton,active_key_id,pending_key_id) VALUES (1,?1,NULL) ON CONFLICT(singleton) DO UPDATE SET active_key_id=excluded.active_key_id,pending_key_id=NULL",[key_id])?;
            Ok(())
        })
    }

    /// FULL-sync intent must commit before any candidate key bytes are
    /// published. Only a completed retirement barrier permits reservation.
    pub fn secrets_prepare_vault_key(&self, key_id: &str) -> Result<()> {
        validate_key_id(key_id)?;
        durable(self, |tx| {
            let (active, pending) = key_state_in(tx)?;
            if active.is_some()
                || pending.as_deref().is_some_and(|pending| pending != key_id)
                || unretired_in(tx)? > 0
            {
                return Err(SecretError::Conflict);
            }
            let changed = tx.execute("INSERT INTO secret_vault_state(singleton,active_key_id,pending_key_id) VALUES (1,NULL,?1) ON CONFLICT(singleton) DO UPDATE SET active_key_id=NULL,pending_key_id=excluded.pending_key_id",[key_id])?;
            if changed != 1 {
                return Err(SecretError::Conflict);
            }
            Ok(())
        })
    }

    /// Compare-and-swap a durably published pending identity into active state.
    /// Publication and its file/directory fsync must precede this FULL commit.
    pub fn secrets_finish_vault_key(&self, key_id: &str) -> Result<()> {
        validate_key_id(key_id)?;
        durable(self, |tx| {
            let (active, pending) = key_state_in(tx)?;
            if active.as_deref() == Some(key_id) && pending.is_none() {
                return Ok(());
            }
            if active.is_some() || pending.as_deref() != Some(key_id) || unretired_in(tx)? > 0 {
                return Err(SecretError::Conflict);
            }
            let changed = tx.execute("UPDATE secret_vault_state SET active_key_id=?1,pending_key_id=NULL WHERE singleton=1 AND active_key_id IS NULL AND pending_key_id=?1",[key_id])?;
            if changed != 1 {
                return Err(SecretError::Conflict);
            }
            Ok(())
        })
    }

    /// The retirement barrier must commit before any new key is prepared.
    /// Failures roll back tombstones and both key markers together.
    pub fn secrets_retire_key_lost(&self, now: i64) -> Result<usize> {
        if now < 0 {
            return Err(SecretError::Invalid);
        }
        durable(self, |tx| {
            let retired = tx.execute("UPDATE secret_records SET revoked_at=?1,revocation_reason='key_lost',revision=revision+1 WHERE revoked_at IS NULL",params![now])?;
            tx.execute("INSERT INTO secret_vault_state(singleton,active_key_id,pending_key_id) VALUES (1,NULL,NULL) ON CONFLICT(singleton) DO UPDATE SET active_key_id=NULL,pending_key_id=NULL",[])?;
            Ok(retired)
        })
    }
}

fn key_state_in(connection: &Connection) -> Result<KeyState> {
    let (active, pending): KeyState = connection
        .query_row(
            "SELECT active_key_id,pending_key_id FROM secret_vault_state WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .unwrap_or_default();
    for key_id in [active.as_deref(), pending.as_deref()]
        .into_iter()
        .flatten()
    {
        validate_key_id(key_id).map_err(|_| SecretError::Unavailable)?;
    }
    if active.is_some() && pending.is_some() {
        return Err(SecretError::Unavailable);
    }
    Ok((active, pending))
}

fn unretired_in(connection: &Connection) -> Result<usize> {
    let count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM secret_records WHERE revoked_at IS NULL",
        [],
        |row| row.get(0),
    )?;
    usize::try_from(count).map_err(|_| SecretError::Unavailable)
}

fn validate_key_id(key_id: &str) -> Result<()> {
    if key_id.len() != 32
        || !key_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(SecretError::Invalid);
    }
    Ok(())
}
