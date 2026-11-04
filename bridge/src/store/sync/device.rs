//! Device projection copy-on-write, finite leases and durable dirty fencing.
use super::{invalid, Entity, Scope, Stream, SyncError};
use crate::store::{ReadTransaction, StoreError, StoreWriter};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const DEVICE_LEASE_TTL_MS: i64 = 5 * 60 * 1000;
pub const DEVICE_LEASES_PER_OPENING: usize = 4;
const DEVICE_LEASES_PER_STORE: usize = 16;
const DEVICE_ROW_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceLease {
    pub id: String,
    pub watermark: super::Sequence,
    pub expires_at_ms: i64,
}

impl StoreWriter {
    /// Persist before acknowledging a file or memory authority change.
    pub fn mark_sync_dirty(&self, kind: &str, key: &str, now: i64) -> Result<i64, SyncError> {
        validate_identity(kind, key)?;
        self.transaction(|tx| {
            tx.prepare_cached(
                "INSERT INTO sync_dirty(kind,key,generation,marked_at_ms) VALUES(?1,?2,1,?3)
                 ON CONFLICT(kind,key) DO UPDATE SET generation=generation+1,marked_at_ms=?3
                 RETURNING generation",
            )?
            .query_row(params![kind, key, now], |row| row.get(0))
            .map_err(StoreError::from)
        })
        .map_err(SyncError::from)
    }

    /// Projection, journal and marker clear commit together. A newer marker
    /// wins and keeps the scope unavailable for its own reconciliation.
    pub fn reconcile_sync_dirty(
        &self,
        kind: &str,
        key: &str,
        generation: i64,
        record: Option<Value>,
        _now: i64,
    ) -> Result<(), SyncError> {
        validate_identity(kind, key)?;
        if generation <= 0 {
            return Err(SyncError::InvalidRequest("invalid dirty generation".into()));
        }
        self.sync_transaction(|tx, batch| {
            let current: Option<i64> = tx
                .prepare_cached("SELECT generation FROM sync_dirty WHERE kind=?1 AND key=?2")?
                .query_row(params![kind, key], |row| row.get(0))
                .optional()?;
            if current != Some(generation) {
                return Err(invalid("stale sync dirty generation"));
            }
            let entity = Entity::new(kind, key);
            match record {
                Some(record) => batch.upsert(entity, record, &[Stream::device()]),
                None => batch.remove(entity, &[Stream::device()]),
            }
            tx.execute(
                "DELETE FROM sync_dirty WHERE kind=?1 AND key=?2 AND generation=?3",
                params![kind, key, generation],
            )?;
            Ok(())
        })?;
        Ok(())
    }

    /// One short writer transaction captures S and admits only lease metadata.
    pub fn admit_device_lease(
        &self,
        opening: &str,
        epoch: &str,
        now: i64,
    ) -> Result<DeviceLease, SyncError> {
        if opening.is_empty() || opening.len() > 256 || epoch.is_empty() || epoch.len() > 256 {
            return Err(SyncError::InvalidRequest(
                "invalid device lease owner".into(),
            ));
        }
        let mut connection = self.connection();
        let tx = connection.transaction()?;
        let dirty: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_dirty)", [], |row| {
            row.get(0)
        })?;
        if dirty {
            return Err(SyncError::Unavailable("sync_reconciling".into()));
        }
        let active: i64 = tx.query_row(
            "SELECT COUNT(*) FROM sync_leases WHERE expires_at_ms>?1",
            [now],
            |row| row.get(0),
        )?;
        let for_opening: i64 = tx.query_row(
            "SELECT COUNT(*) FROM sync_leases WHERE opening=?1 AND expires_at_ms>?2",
            params![opening, now],
            |row| row.get(0),
        )?;
        if active >= DEVICE_LEASES_PER_STORE as i64
            || for_opening >= DEVICE_LEASES_PER_OPENING as i64
        {
            return Err(SyncError::Unavailable("device lease quota exceeded".into()));
        }
        let (head, incarnation): (i64, String) = tx.query_row(
            "SELECT s.head,m.value FROM sync_state s JOIN meta m ON m.key='sync_incarnation' WHERE s.id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let id = uuid::Uuid::new_v4().to_string();
        let expires_at_ms = now.saturating_add(DEVICE_LEASE_TTL_MS);
        tx.execute(
            "INSERT INTO sync_leases(id,scope_digest,opening,incarnation,epoch,seq,expires_at_ms)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                id,
                device_scope().digest(),
                opening,
                incarnation,
                epoch,
                head,
                expires_at_ms
            ],
        )?;
        tx.commit()?;
        Ok(DeviceLease {
            id,
            watermark: super::Sequence::new(head)?,
            expires_at_ms,
        })
    }
}

impl ReadTransaction<'_> {
    pub fn sync_device_snapshot(
        &self,
        lease_id: &str,
        after: Option<(&str, &str)>,
        limit: usize,
        epoch: &str,
        now: i64,
    ) -> Result<Value, SyncError> {
        self.sync_device_snapshot_inner(lease_id, after, limit, epoch, now, || {
            super::now_ms().max(now)
        })
    }

    #[cfg(test)]
    pub fn sync_device_snapshot_with_recheck(
        &self,
        lease_id: &str,
        after: Option<(&str, &str)>,
        limit: usize,
        epoch: &str,
        now: i64,
        recheck_now: i64,
    ) -> Result<Value, SyncError> {
        self.sync_device_snapshot_inner(lease_id, after, limit, epoch, now, || recheck_now)
    }

    fn sync_device_snapshot_inner(
        &self,
        lease_id: &str,
        after: Option<(&str, &str)>,
        limit: usize,
        epoch: &str,
        now: i64,
        recheck_now: impl FnOnce() -> i64,
    ) -> Result<Value, SyncError> {
        if !(1..=500).contains(&limit) {
            return Err(SyncError::InvalidRequest(
                "snapshot limit must be 1 to 500".into(),
            ));
        }
        let lease: Option<(String, String, i64, i64)> = self
            .connection()
            .prepare_cached(
                "SELECT incarnation,epoch,seq,expires_at_ms FROM sync_leases
                 WHERE id=?1 AND scope_digest=?2",
            )?
            .query_row(params![lease_id, device_scope().digest()], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .optional()?;
        let current_incarnation: String = self.connection().query_row(
            "SELECT value FROM meta WHERE key='sync_incarnation'",
            [],
            |row| row.get(0),
        )?;
        let Some((incarnation, lease_epoch, watermark, expires)) = lease else {
            return Ok(resync(&current_incarnation, epoch));
        };
        if incarnation != current_incarnation || lease_epoch != epoch || expires <= now {
            return Ok(resync(&current_incarnation, epoch));
        }
        let (after_kind, after_key) = after.unwrap_or(("", ""));
        let scan = (4 * limit + 1).min(2001);
        let keys = self
            .connection()
            .prepare_cached(
                "SELECT kind,key FROM sync_projection_keys
                 WHERE first_seq<=?1 AND (kind>?2 OR (kind=?2 AND key>?3))
                 ORDER BY kind,key LIMIT ?4",
            )?
            .query_map(
                params![watermark, after_kind, after_key, scan as i64],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let mut records = Vec::new();
        let mut last = None;
        for (kind, key) in keys {
            if records.len() == limit {
                break;
            }
            if let Some((revision, deleted, raw)) = projection_at(self, &kind, &key, watermark)? {
                if !deleted {
                    let row: Value = serde_json::from_str(raw.as_deref().ok_or_else(|| {
                        SyncError::Unavailable("missing device projection row".into())
                    })?)
                    .map_err(|_| SyncError::Unavailable("invalid device projection row".into()))?;
                    records.push(json!({"kind":kind,"key":key,"revision":super::Sequence::new(revision)?,"row":row}));
                }
            }
            last = Some((kind, key));
        }
        // Revalidate the finite lease inside the same read snapshot just before
        // constructing the answer; no transaction survives this request.
        if expires <= recheck_now() || lease_epoch != epoch {
            return Ok(resync(&current_incarnation, epoch));
        }
        let mut answer = json!({"status":"ok","incarnation":incarnation,"watermark":super::Sequence::new(watermark)?,"bridge_epoch":epoch,"consistency":"leased","lease":{"id":lease_id,"expires_at_ms":expires},"records":records});
        if let Some((kind, key)) = last {
            let more: bool = self.connection().query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_projection_keys
                 WHERE first_seq<=?1 AND (kind>?2 OR (kind=?2 AND key>?3)))",
                params![watermark, kind, key],
                |row| row.get(0),
            )?;
            if more {
                answer["next"] = json!({"kind":kind,"key":key});
            }
        }
        Ok(answer)
    }
}

fn projection_at(
    read: &ReadTransaction<'_>,
    kind: &str,
    key: &str,
    watermark: i64,
) -> Result<Option<(i64, bool, Option<String>)>, SyncError> {
    let current = read
        .connection()
        .prepare_cached(
            "SELECT from_seq,deleted,record FROM sync_projection
             WHERE kind=?1 AND key=?2 AND from_seq<=?3",
        )?
        .query_row(params![kind, key, watermark], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?;
    if current.is_some() {
        return Ok(current);
    }
    Ok(read
        .connection()
        .prepare_cached(
            "SELECT from_seq,deleted,record FROM sync_projection_versions
             WHERE kind=?1 AND key=?2 AND from_seq<=?3 AND to_seq>?3
             ORDER BY from_seq DESC LIMIT 1",
        )?
        .query_row(params![kind, key, watermark], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?)
}

pub(super) fn apply_projection(
    tx: &Transaction<'_>,
    entity: &Entity,
    record: Option<&Value>,
    seq: i64,
) -> Result<(), StoreError> {
    let raw = record.map(Value::to_string);
    let bytes = raw.as_ref().map_or(0, |raw| raw.len());
    if bytes > DEVICE_ROW_BYTES {
        return Err(invalid("device projection row exceeds 64 KiB"));
    }
    let previous: Option<(i64, bool, Option<String>, i64)> = tx
        .prepare_cached(
            "SELECT from_seq,deleted,record,record_bytes FROM sync_projection WHERE kind=?1 AND key=?2",
        )?
        .query_row(params![entity.kind, entity.key], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .optional()?;
    if let Some((from, deleted, previous, previous_bytes)) = previous {
        tx.execute(
            "INSERT INTO sync_projection_versions(kind,key,from_seq,to_seq,deleted,record,record_bytes)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                entity.kind,
                entity.key,
                from,
                seq,
                deleted,
                previous,
                previous_bytes
            ],
        )?;
    } else {
        tx.execute(
            "INSERT INTO sync_projection_keys(kind,key,first_seq) VALUES(?1,?2,?3)",
            params![entity.kind, entity.key, seq],
        )?;
    }
    tx.execute(
        "INSERT INTO sync_projection(kind,key,from_seq,deleted,record,record_bytes)
         VALUES(?1,?2,?3,?4,?5,?6)
         ON CONFLICT(kind,key) DO UPDATE SET from_seq=excluded.from_seq,deleted=excluded.deleted,
         record=excluded.record,record_bytes=excluded.record_bytes",
        params![
            entity.kind,
            entity.key,
            seq,
            record.is_none(),
            raw,
            bytes as i64
        ],
    )?;
    Ok(())
}

/// Stage one optional device row without inventing a tombstone for an entity
/// that has never been a member of the device projection.
pub(in crate::store) fn stage(
    tx: &Transaction<'_>,
    batch: &mut super::PublicationBatch,
    entity: Entity,
    record: Option<Value>,
) -> Result<(), StoreError> {
    match record {
        Some(record) => batch.upsert(entity, record, &[Stream::device()]),
        None => {
            let published: bool = tx
                .prepare_cached(
                    "SELECT EXISTS(SELECT 1 FROM sync_projection
                     WHERE kind=?1 AND key=?2 AND deleted=0)",
                )?
                .query_row(params![entity.kind, entity.key], |row| row.get(0))?;
            if published {
                batch.remove(entity, &[Stream::device()]);
            }
        }
    }
    Ok(())
}

fn device_scope() -> Scope {
    Scope::new(Stream::device(), json!({"kind":"device"}))
}

fn validate_identity(kind: &str, key: &str) -> Result<(), SyncError> {
    if kind.is_empty() || kind.len() > 64 || key.is_empty() || key.len() > 4096 {
        return Err(SyncError::InvalidRequest(
            "invalid device projection identity".into(),
        ));
    }
    Ok(())
}

fn resync(incarnation: &str, epoch: &str) -> Value {
    json!({"status":"resync_required","incarnation":incarnation,"bridge_epoch":epoch,"reason":"lease_expired"})
}
