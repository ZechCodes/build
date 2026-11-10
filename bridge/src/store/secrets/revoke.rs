//! Durable, context-bound receipts for user revocations.
use super::{metadata, records, writes};
use crate::api::v1::secrets::{ListParams, RevokeParams, SecretMetadata};
use crate::secrets::{Result, SecretError, SecretRecord};
use crate::store::StoreWriter;
use rusqlite::{params, OptionalExtension, Transaction};

const MAX_RECEIPTS: i64 = 4096;
const RECEIPT_RETENTION_SECONDS: i64 = 90 * 24 * 60 * 60;

/// Receipts may be forgotten; the authoritative revocation tombstone is never
/// removed here. The count cap independently bounds even a flood of new IDs.
pub(super) fn prune_in(tx: &Transaction<'_>, now: i64) -> Result<()> {
    tx.execute("DELETE FROM secret_revocations WHERE accepted_at<?1 AND (NOT EXISTS(SELECT 1 FROM secret_records s WHERE s.id=secret_revocations.secret_id) OR EXISTS(SELECT 1 FROM secret_records s WHERE s.id=secret_revocations.secret_id AND s.revoked_at IS NOT NULL))", [now.saturating_sub(RECEIPT_RETENTION_SECONDS)])?;
    Ok(())
}

fn cap_in(tx: &Transaction<'_>) -> Result<()> {
    tx.execute("DELETE FROM secret_revocations WHERE sequence IN (SELECT sequence FROM secret_revocations ORDER BY sequence LIMIT MAX(0,(SELECT COUNT(*) FROM secret_revocations)-?1))", [MAX_RECEIPTS])?;
    Ok(())
}

impl StoreWriter {
    pub(crate) fn secrets_revoke_operation(
        &self,
        device: &str,
        creator: &str,
        asked: &RevokeParams,
        observe: impl FnOnce(&Transaction<'_>, &SecretRecord) -> Result<(i64, bool)>,
    ) -> Result<(SecretMetadata, Option<SecretRecord>)> {
        if uuid::Uuid::parse_str(&asked.operation_id).is_err() {
            return Err(SecretError::Invalid);
        }
        let request = serde_json::to_string(&(&asked.scope, &asked.secret_id))
            .map_err(|_| SecretError::Invalid)?;
        writes::durable(self, |tx| {
            metadata::authorize(tx, &asked.scope)?;
            let query = ListParams {
                include_environment: true,
                scope: asked.scope.clone(),
                secret_ids: vec![asked.secret_id.clone()],
                request_ids: vec![],
                after: None,
                limit: 1,
            };
            if metadata::list(tx, device, creator, &query)?
                .secrets
                .is_empty()
            {
                return Err(SecretError::Unavailable);
            }
            let before = records::required(tx, &asked.secret_id)?;
            let transitioned = before.revoked_at.is_none();
            let (now, expired) = observe(tx, &before)?;
            prune_in(tx, now)?;
            let prior: Option<(String, String)> = tx.query_row(
                "SELECT request,response FROM secret_revocations WHERE device_id=?1 AND creator=?2 AND operation_id=?3",
                params![device, creator, asked.operation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            if let Some((accepted, response)) = prior {
                if accepted != request {
                    return Err(SecretError::Conflict);
                }
                let response =
                    serde_json::from_str(&response).map_err(|_| SecretError::Unavailable)?;
                return Ok((response, None));
            }
            let record = writes::tombstone_in(
                tx,
                &asked.secret_id,
                now,
                if expired { "expired" } else { "revoked" },
            )?;
            let mut projected = record.clone();
            metadata::resolve_binding(tx, &mut projected, asked.scope.stage.as_ref())?;
            let response = SecretMetadata::from_record(&projected, now);
            let json = serde_json::to_string(&response).map_err(|_| SecretError::Unavailable)?;
            tx.execute("INSERT INTO secret_revocations(device_id,creator,operation_id,request,response,secret_id,accepted_at) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![device,creator,asked.operation_id,request,json,asked.secret_id,now])?;
            cap_in(tx)?;
            Ok((response, transitioned.then_some(record)))
        })
    }
}
