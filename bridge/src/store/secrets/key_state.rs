use super::writes::durable;
use crate::secrets::{Result, SecretError};
use crate::store::StoreWriter;
use rusqlite::{params, Connection, OptionalExtension};

impl StoreWriter {
    /// Identifier of the active vault key, never key material.
    pub fn secret_vault_key_id(&self) -> Result<Option<String>> {
        key_id_in(&self.connection())
    }

    pub fn secrets_unretired_count(&self) -> Result<usize> {
        unretired_in(&self.connection())
    }

    /// A different or previously unknown key cannot acquire existing grants.
    pub fn secrets_set_vault_key_id(&self, key_id: &str) -> Result<()> {
        validate_key_id(key_id)?;
        durable(self, |tx| {
            let previous = key_id_in(tx)?;
            if previous.as_deref() != Some(key_id) && unretired_in(tx)? > 0 {
                return Err(SecretError::Conflict);
            }
            tx.execute("INSERT INTO secret_vault_state(singleton,active_key_id) VALUES (1,?1) ON CONFLICT(singleton) DO UPDATE SET active_key_id=excluded.active_key_id",[key_id])?;
            Ok(())
        })
    }

    /// The retirement barrier must commit before any vault key is replaced.
    /// A failed write rolls back both all tombstones and the expected key ID.
    pub fn secrets_retire_key_lost(&self, now: i64) -> Result<usize> {
        if now < 0 {
            return Err(SecretError::Invalid);
        }
        durable(self, |tx| {
            let retired = tx.execute("UPDATE secret_records SET revoked_at=?1,revocation_reason='key_lost',revision=revision+1 WHERE revoked_at IS NULL",params![now])?;
            tx.execute("INSERT INTO secret_vault_state(singleton,active_key_id) VALUES (1,NULL) ON CONFLICT(singleton) DO UPDATE SET active_key_id=NULL",[])?;
            Ok(retired)
        })
    }
}

fn key_id_in(connection: &Connection) -> Result<Option<String>> {
    let key_id: Option<String> = connection
        .query_row(
            "SELECT active_key_id FROM secret_vault_state WHERE singleton=1",
            [],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    if let Some(key_id) = &key_id {
        validate_key_id(key_id).map_err(|_| SecretError::Unavailable)?;
    }
    Ok(key_id)
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
