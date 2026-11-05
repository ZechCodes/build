use super::{records, writes};
use crate::secrets::{Result, SecretError, SecretRecord, STAGED_LIFETIME_SECONDS};
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::HashSet;

/// Claim every reference with its message operation. Propagate any error from
/// the surrounding transaction: no message can commit without its bindings.
/// The caller must configure FULL durability before starting that transaction.
pub fn attach_in(
    tx: &Transaction<'_>,
    ids: &[String],
    operation_id: &str,
    now: i64,
) -> Result<Vec<SecretRecord>> {
    validate_claim(ids, operation_id, now)?;
    let synchronous: i64 = tx.pragma_query_value(None, "synchronous", |row| row.get(0))?;
    if synchronous < 2 {
        return Err(SecretError::Invalid);
    }
    let mut canonical_ids = ids.to_vec();
    canonical_ids.sort();
    let identity = serde_json::to_string(&canonical_ids).map_err(|_| SecretError::Invalid)?;
    let existing: Option<String> = tx
        .query_row(
            "SELECT secret_ids FROM secret_bindings WHERE operation_id=?1",
            [operation_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(existing) = existing {
        if existing != identity {
            return Err(SecretError::Conflict);
        }
        return ids.iter().map(|id| records::required(tx, id)).collect();
    }
    if now < records::highwater(tx)? {
        return Err(SecretError::ClockRollback);
    }
    let records = ids
        .iter()
        .map(|id| records::required(tx, id))
        .collect::<Result<Vec<_>>>()?;
    for record in &records {
        validate_attach(record, now)?;
    }
    for record in &records {
        let expiry = expiry(record, now)?;
        tx.execute("UPDATE secret_records SET attached_at=?2,expires_at=?3,message_operation_id=?4,revision=revision+1 WHERE id=?1",params![record.context.id,now,expiry,operation_id])?;
    }
    tx.execute(
        "INSERT INTO secret_bindings(operation_id,secret_ids,attached_at) VALUES (?1,?2,?3)",
        params![operation_id, identity, now],
    )?;
    writes::observe_in(tx, now)?;
    ids.iter().map(|id| records::required(tx, id)).collect()
}

fn validate_claim(ids: &[String], operation: &str, now: i64) -> Result<()> {
    let unique: HashSet<_> = ids.iter().collect();
    if ids.is_empty()
        || ids.len() > 100
        || ids.len() != unique.len()
        || operation.is_empty()
        || operation.len() > 128
        || operation.contains('\0')
        || now < 0
    {
        return Err(SecretError::Invalid);
    }
    for id in ids {
        crate::secrets::validate_id(id)?;
    }
    Ok(())
}

fn validate_attach(record: &SecretRecord, now: i64) -> Result<()> {
    if record.revoked_at.is_some()
        || record.attached_at.is_some()
        || !record.blob_present
        || now < record.created_at
        || now.saturating_sub(record.created_at) >= STAGED_LIFETIME_SECONDS
    {
        return Err(SecretError::Unavailable);
    }
    let _ = expiry(record, now)?;
    Ok(())
}

fn expiry(record: &SecretRecord, now: i64) -> Result<Option<i64>> {
    record
        .context
        .ttl_seconds
        .map(|ttl| now.checked_add(ttl).ok_or(SecretError::Invalid))
        .transpose()
}
