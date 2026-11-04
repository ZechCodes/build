use crate::secrets::{
    validate_id, GrantScope, Result, SecretContext, SecretError, SecretRecord, StageContext,
};
use crate::store::StoreWriter;
use rusqlite::{Connection, OptionalExtension, Row};

pub(super) const COLUMNS: &str = "id,device_id,creator,name,ttl_seconds,grant_scope,grant_agent_id,grant_conversation_id,grant_generation,grant_project_id,stage_owner_id,stage_conversation_id,stage_generation,stage_draft_id,created_at,attached_at,expires_at,revoked_at,revision,message_operation_id,upload_request_id,upload_digest,blob_present,revocation_reason";

impl StoreWriter {
    pub fn secret_record(&self, id: &str) -> Result<Option<SecretRecord>> {
        validate_id(id)?;
        load(&self.connection(), id)
    }

    /// Live and pending-deletion blobs only; retained tombstones are not scanned.
    pub fn secret_records(&self) -> Result<Vec<SecretRecord>> {
        let connection = self.connection();
        let mut query = connection.prepare(&format!(
            "SELECT {COLUMNS} FROM secret_records WHERE blob_present=1 ORDER BY id"
        ))?;
        let records = query
            .query_map([], from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for record in &records {
            validate_loaded(record)?;
        }
        Ok(records)
    }

    /// Includes retained tombstones. Recovery authority instead comes from the
    /// unretired-grant barrier and persisted active key identity.
    pub fn secrets_have_records(&self) -> Result<bool> {
        Ok(self.connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM secret_records)",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn secret_clock_highwater(&self) -> Result<i64> {
        highwater(&self.connection())
    }
}

pub(super) fn highwater(connection: &Connection) -> Result<i64> {
    Ok(connection
        .query_row(
            "SELECT high_water FROM secret_clock WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

pub(super) fn load(connection: &Connection, id: &str) -> Result<Option<SecretRecord>> {
    let record = connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM secret_records WHERE id=?1"),
            [id],
            from_row,
        )
        .optional()?;
    if let Some(record) = &record {
        validate_loaded(record)?;
    }
    Ok(record)
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<SecretRecord> {
    let scope: String = row.get(5)?;
    let grant = match scope.as_str() {
        "agent" => GrantScope::Agent {
            agent_id: row.get(6)?,
            conversation_id: row.get(7)?,
            generation: row.get(8)?,
        },
        "project" => GrantScope::Project {
            project_id: row.get(9)?,
        },
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(SecretRecord {
        context: SecretContext {
            id: row.get(0)?,
            device_id: row.get(1)?,
            creator: row.get(2)?,
            name: row.get(3)?,
            ttl_seconds: row.get(4)?,
            grant,
            stage: StageContext {
                owner_id: row.get(10)?,
                conversation_id: row.get(11)?,
                generation: row.get(12)?,
                draft_id: row.get(13)?,
            },
        },
        created_at: row.get(14)?,
        attached_at: row.get(15)?,
        expires_at: row.get(16)?,
        revoked_at: row.get(17)?,
        revocation_reason: row.get(23)?,
        revision: row.get(18)?,
        message_operation_id: row.get(19)?,
        upload_request_id: row.get(20)?,
        upload_digest: row.get(21)?,
        blob_present: row.get(22)?,
    })
}

pub(super) fn required(connection: &Connection, id: &str) -> Result<SecretRecord> {
    validate_id(id)?;
    load(connection, id)?.ok_or(SecretError::Unavailable)
}

fn validate_loaded(record: &SecretRecord) -> Result<()> {
    record.context.validate()?;
    validate_revocation_reason(record)?;
    if record.created_at < 0 || record.revision < 1 || record.revoked_at.is_some_and(|at| at < 0) {
        return Err(SecretError::Unavailable);
    }
    let Some(attached) = record.attached_at else {
        return if record.expires_at.is_none() && record.message_operation_id.is_none() {
            Ok(())
        } else {
            Err(SecretError::Unavailable)
        };
    };
    let operation_valid = record
        .message_operation_id
        .as_ref()
        .is_some_and(|operation| {
            !operation.is_empty() && operation.len() <= 128 && !operation.contains('\0')
        });
    if attached < record.created_at || !operation_valid {
        return Err(SecretError::Unavailable);
    }
    let expected_expiry = record
        .context
        .ttl_seconds
        .map(|ttl| attached.checked_add(ttl).ok_or(SecretError::Unavailable))
        .transpose()?;
    if record.expires_at != expected_expiry {
        return Err(SecretError::Unavailable);
    }
    Ok(())
}

fn validate_revocation_reason(record: &SecretRecord) -> Result<()> {
    if let Some(reason) = &record.revocation_reason {
        if record.revoked_at.is_none()
            || !matches!(reason.as_str(), "key_lost" | "revoked" | "expired")
        {
            return Err(SecretError::Unavailable);
        }
    }
    Ok(())
}
