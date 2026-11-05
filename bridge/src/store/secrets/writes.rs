use super::{attach_in, records, retirement};
use crate::secrets::{
    GrantScope, Result, SecretError, SecretRecord, MAX_RETAINED_BLOBS, MAX_STAGED_PER_CREATOR,
};
use crate::store::StoreWriter;
use rusqlite::{params, Connection, Transaction, TransactionBehavior};

impl StoreWriter {
    /// Admission hint before encrypted file I/O. The final insert rechecks all
    /// limits and conflicts in its IMMEDIATE transaction.
    pub fn secrets_stage_preflight(&self, record: &SecretRecord) -> Result<()> {
        validate_stage(record)?;
        let connection = self.connection();
        check_conflict(&connection, record)?;
        check_quota(&connection, record)
    }

    pub fn secrets_stage(&self, record: &SecretRecord) -> Result<()> {
        validate_stage(record)?;
        durable(self, |tx| {
            check_conflict(tx, record)?;
            check_quota(tx, record)?;
            insert(tx, record)?;
            super::append_metadata_in(tx, record, "created", record.created_at)
        })
    }

    /// Recheck committed target authority in the very transaction that publishes
    /// the staged descriptor. Reset/deletion while encryption waits cannot
    /// resurrect a retired stage after its lifecycle transaction has committed.
    pub fn secrets_stage_authorized(
        &self,
        record: &SecretRecord,
        scope: &crate::api::v1::secrets::SecretScope,
    ) -> Result<()> {
        validate_stage(record)?;
        durable(self, |tx| {
            super::metadata::authorize(tx, scope)?;
            check_conflict(tx, record)?;
            check_quota(tx, record)?;
            insert(tx, record)?;
            super::append_metadata_in(tx, record, "created", record.created_at)
        })
    }

    /// Internal convenience for foundation tests. Sends use `attach_in` in the
    /// operation transaction instead of independently committing this wrapper.
    pub fn secrets_attach(
        &self,
        ids: &[String],
        operation_id: &str,
        now: i64,
    ) -> Result<Vec<SecretRecord>> {
        durable(self, |tx| {
            let already: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM secret_bindings WHERE operation_id=?1)",
                [operation_id],
                |row| row.get(0),
            )?;
            let records = attach_in(tx, ids, operation_id, now)?;
            if !already {
                for record in &records {
                    super::append_metadata_in(tx, record, "attached", now)?;
                }
            }
            Ok(records)
        })
    }

    pub fn secrets_revoke(&self, id: &str, now: i64) -> Result<SecretRecord> {
        tombstone(self, id, now, "revoked")
    }

    /// Persist the observed expiry before deleting bytes or returning denial.
    /// The original tombstone remains authoritative on every retry.
    pub fn secrets_expire(&self, id: &str, now: i64) -> Result<SecretRecord> {
        tombstone(self, id, now, "expired")
    }

    pub fn secrets_mark_deleted(&self, id: &str) -> Result<()> {
        crate::secrets::validate_id(id)?;
        durable(self, |tx| {
            tx.execute("UPDATE secret_records SET blob_present=0,revision=revision+1 WHERE id=?1 AND blob_present=1", [id])?;
            Ok(())
        })
    }

    /// FULL commit also flushes earlier lifecycle tombstones before unlink.
    pub fn secret_clock_observe(&self, now: i64) -> Result<bool> {
        if now < 0 {
            return Err(SecretError::Invalid);
        }
        durable(self, |tx| {
            let valid = now >= records::highwater(tx)?;
            observe_in(tx, now)?;
            Ok(valid)
        })
    }

    /// Retire project grants before committing removal of its external config.
    pub fn secrets_retire_project(&self, project_id: &str) -> Result<()> {
        let retired = durable(self, |tx| retirement::retire_project_in(tx, project_id))?;
        self.notify_secret_records(&retired);
        Ok(())
    }

    pub fn secrets_flush(&self) -> Result<()> {
        durable(self, |tx| observe_in(tx, 0))
    }
}

fn tombstone(
    writer: &StoreWriter,
    id: &str,
    now: i64,
    reason: &'static str,
) -> Result<SecretRecord> {
    if now < 0 {
        return Err(SecretError::Invalid);
    }
    durable(writer, |tx| tombstone_in(tx, id, now, reason))
}

pub(super) fn tombstone_in(
    tx: &Transaction<'_>,
    id: &str,
    now: i64,
    reason: &'static str,
) -> Result<SecretRecord> {
    let before = records::required(tx, id)?;
    tx.execute("UPDATE secret_records SET revoked_at=?2,revocation_reason=?3,revision=revision+1 WHERE id=?1 AND revoked_at IS NULL",params![id,now,reason])?;
    let record = records::required(tx, id)?;
    if before.revoked_at.is_none() {
        super::append_metadata_in(tx, &record, reason, now)?;
    }
    Ok(record)
}

pub(super) fn observe_in(tx: &Transaction<'_>, now: i64) -> Result<()> {
    // Incrementing revision guarantees a WAL write even at an unchanged time.
    tx.execute("INSERT INTO secret_clock(singleton,high_water,revision) VALUES (1,?1,1) ON CONFLICT(singleton) DO UPDATE SET high_water=MAX(high_water,excluded.high_water),revision=revision+1",[now])?;
    Ok(())
}

pub(super) fn durable<T>(
    writer: &StoreWriter,
    work: impl FnOnce(&Transaction<'_>) -> Result<T>,
) -> Result<T> {
    let mut connection = writer.connection();
    let previous: i64 = connection.pragma_query_value(None, "synchronous", |row| row.get(0))?;
    let restore = if previous == 2 {
        None
    } else {
        connection.pragma_update(None, "synchronous", "FULL")?;
        Some(previous)
    };
    let result: Result<T> = (|| {
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        observe_in(&tx, 0)?;
        let result = work(&tx)?;
        tx.commit()?;
        Ok(result)
    })();
    let restored = restore.map_or(Ok(()), |previous| {
        connection.pragma_update(None, "synchronous", previous)
    });
    match result {
        Ok(value) => {
            if let Err(error) = restored {
                eprintln!("secret store: committed durable write but could not restore synchronous pragma: {error}");
            }
            Ok(value)
        }
        Err(error) => {
            if let Err(restore_error) = restored {
                eprintln!("secret store: failed write and could not restore synchronous pragma: {restore_error}");
            }
            Err(error)
        }
    }
}

fn validate_stage(record: &SecretRecord) -> Result<()> {
    record.context.validate()?;
    let canonical_request = uuid::Uuid::parse_str(&record.upload_request_id)
        .map_err(|_| SecretError::Invalid)?
        .hyphenated()
        .to_string();
    let clean = record.created_at >= 0
        && record.revision == 1
        && record.attached_at.is_none()
        && record.expires_at.is_none()
        && record.revoked_at.is_none()
        && record.revocation_reason.is_none()
        && record.message_operation_id.is_none()
        && record.blob_present;
    let valid_digest = record.upload_digest.len() == 64
        && record
            .upload_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit());
    if !clean || !valid_digest || canonical_request != record.upload_request_id {
        return Err(SecretError::Invalid);
    }
    Ok(())
}

fn check_conflict(connection: &Connection, record: &SecretRecord) -> Result<()> {
    let context = &record.context;
    let conflict: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM secret_records WHERE id=?1 OR (device_id=?2 AND creator=?3 AND upload_request_id=?4))",params![context.id,context.device_id,context.creator,record.upload_request_id],|row|row.get(0))?;
    if conflict {
        return Err(SecretError::Conflict);
    }
    Ok(())
}

fn check_quota(tx: &Connection, record: &SecretRecord) -> Result<()> {
    let context = &record.context;
    let retained: i64 = tx.query_row(
        "SELECT COUNT(*) FROM secret_records WHERE device_id=?1 AND blob_present=1",
        [&context.device_id],
        |row| row.get(0),
    )?;
    let staged: i64 = tx.query_row("SELECT COUNT(*) FROM secret_records WHERE device_id=?1 AND creator=?2 AND blob_present=1 AND attached_at IS NULL AND revoked_at IS NULL",params![context.device_id,context.creator],|row|row.get(0))?;
    if retained >= MAX_RETAINED_BLOBS as i64 || staged >= MAX_STAGED_PER_CREATOR as i64 {
        return Err(SecretError::Limit);
    }
    Ok(())
}

fn insert(tx: &Transaction<'_>, record: &SecretRecord) -> Result<()> {
    let context = &record.context;
    let (scope, agent, conversation, generation, project) = match &context.grant {
        GrantScope::Agent {
            agent_id,
            conversation_id,
            generation,
        } => (
            "agent",
            Some(agent_id),
            Some(conversation_id),
            Some(generation),
            None,
        ),
        GrantScope::Project { project_id } => ("project", None, None, None, Some(project_id)),
    };
    tx.execute("INSERT INTO secret_records (id,device_id,creator,name,ttl_seconds,grant_scope,grant_agent_id,grant_conversation_id,grant_generation,grant_project_id,stage_owner_id,stage_conversation_id,stage_generation,stage_draft_id,created_at,revision,upload_request_id,upload_digest,blob_present) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,1,?16,?17,1)",params![context.id,context.device_id,context.creator,context.name,context.ttl_seconds,scope,agent,conversation,generation,project,context.stage.owner_id,context.stage.conversation_id,context.stage.generation,context.stage.draft_id,record.created_at,record.upload_request_id,record.upload_digest])?;
    Ok(())
}
