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

/// One generation and its marker share the caller's canonical transaction.
pub(super) fn mark_dirty(
    conn: &rusqlite::Connection,
    kind: &str,
    key: &str,
    now: i64,
) -> Result<i64, StoreError> {
    let generation: i64 = conn.query_row(
        "UPDATE meta SET value=CAST(CAST(value AS INTEGER)+1 AS TEXT)
         WHERE key='device_dirty_generation' AND CAST(value AS INTEGER)<9223372036854775807
         RETURNING CAST(value AS INTEGER)",
        [],
        |row| row.get(0),
    )?;
    conn.execute(
        "INSERT INTO sync_dirty(kind,key,generation,marked_at_ms) VALUES(?1,?2,?3,?4)
         ON CONFLICT(kind,key) DO UPDATE SET generation=?3,marked_at_ms=?4",
        params![kind, key, generation, now],
    )?;
    Ok(generation)
}

impl StoreWriter {
    /// Completion is advisory: contention leaves the finite lease to expire.
    /// Neither the writer mutex nor an external SQLite writer may delay a read.
    pub fn release_device_lease_best_effort(&self, id: &str, opening: &str, epoch: &str) {
        let Ok(connection) = self.connection.try_lock() else {
            return;
        };
        if connection.busy_timeout(std::time::Duration::ZERO).is_err() {
            return;
        }
        let _ = connection.execute(
            "DELETE FROM sync_leases WHERE id=?1 AND opening=?2 AND epoch=?3",
            params![id, opening, epoch],
        );
        let _ = connection.busy_timeout(std::time::Duration::from_secs(5));
    }

    /// Persist before acknowledging a file or memory authority change.
    pub fn mark_sync_dirty(&self, kind: &str, key: &str, now: i64) -> Result<i64, SyncError> {
        validate_identity(kind, key)?;
        self.transaction(|tx| mark_dirty(tx, kind, key, now))
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
                Some(mut record) => {
                    bound_display(&mut record);
                    batch.upsert(entity, record, &[Stream::device()]);
                }
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
        let mut connection = match self.connection.try_lock() {
            Ok(connection) => connection,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                return Err(SyncError::Unavailable(
                    "device snapshot admission busy".into(),
                ))
            }
        };
        #[cfg(test)]
        let _hold = crate::timing::phase_samples::phase("device.lease_writer_hold");
        connection.busy_timeout(std::time::Duration::ZERO)?;
        let result = admit_lease(&mut connection, opening, epoch, now);
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        match result {
            Err(SyncError::Database(error))
                if matches!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
                ) =>
            {
                Err(SyncError::Unavailable(
                    "device snapshot admission busy".into(),
                ))
            }
            result => result,
        }
    }
}

fn admit_lease(
    connection: &mut rusqlite::Connection,
    opening: &str,
    epoch: &str,
    now: i64,
) -> Result<DeviceLease, SyncError> {
    let tx = connection.transaction()?;
    ensure_ready(&tx)?;
    let (head,incarnation,retained,floor):(i64,String,i64,i64)=tx.query_row(
        "SELECT s.head,m.value,s.retained_version_bytes,s.projection_pressure_floor FROM sync_state s JOIN meta m ON m.key='sync_incarnation' WHERE s.id=1",[],
        |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))?;
    if retained >= super::retention::MAX_RETAINED_VERSION_BYTES {
        return Err(SyncError::Unavailable(
            "device snapshot retention pressure".into(),
        ));
    }
    // At most sixteen valid leases exist. Reclaiming at most sixteen invalid
    // metadata rows per admission prevents epoch/expiry/pressure churn growth.
    tx.execute("DELETE FROM sync_leases WHERE id IN (SELECT id FROM sync_leases WHERE incarnation<>?1 OR epoch<>?2 OR expires_at_ms<=?3 OR seq<?4 LIMIT 16)",params![incarnation,epoch,now,floor])?;
    let active:i64=tx.query_row("SELECT COUNT(*) FROM (SELECT 1 FROM sync_leases WHERE incarnation=?1 AND epoch=?2 AND expires_at_ms>?3 AND seq>=?4 LIMIT 17)",params![incarnation,epoch,now,floor],|row|row.get(0))?;
    let for_opening:i64=tx.query_row("SELECT COUNT(*) FROM (SELECT 1 FROM sync_leases WHERE incarnation=?1 AND epoch=?2 AND opening=?3 AND expires_at_ms>?4 AND seq>=?5 LIMIT 5)",params![incarnation,epoch,opening,now,floor],|row|row.get(0))?;
    if active >= DEVICE_LEASES_PER_STORE as i64 || for_opening >= DEVICE_LEASES_PER_OPENING as i64 {
        return Err(SyncError::Unavailable("device lease quota exceeded".into()));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let expires_at_ms = now.saturating_add(DEVICE_LEASE_TTL_MS);
    tx.execute("INSERT INTO sync_leases(id,scope_digest,opening,incarnation,epoch,seq,expires_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![id,device_scope().digest(),opening,incarnation,epoch,head,expires_at_ms])?;
    tx.commit()?;
    Ok(DeviceLease {
        id,
        watermark: super::Sequence::new(head)?,
        expires_at_ms,
    })
}

impl ReadTransaction<'_> {
    pub fn sync_device_snapshot(
        &self,
        lease_id: &str,
        after: Option<(i64, &str, &str)>,
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
        after: Option<(i64, &str, &str)>,
        limit: usize,
        epoch: &str,
        now: i64,
        recheck_now: i64,
    ) -> Result<Value, SyncError> {
        self.sync_device_snapshot_inner(lease_id, after, limit, epoch, now, || recheck_now)
    }

    #[cfg(test)]
    pub fn sync_device_snapshot_observed(
        &self,
        lease: &str,
        epoch: &str,
        now: i64,
        recheck: impl FnOnce() -> i64,
    ) -> Result<Value, SyncError> {
        self.sync_device_snapshot_inner(lease, None, 1, epoch, now, recheck)
    }

    fn sync_device_snapshot_inner(
        &self,
        lease_id: &str,
        after: Option<(i64, &str, &str)>,
        limit: usize,
        epoch: &str,
        now: i64,
        recheck_now: impl FnOnce() -> i64,
    ) -> Result<Value, SyncError> {
        ensure_ready(self.connection())?;
        if !(1..=500).contains(&limit) {
            return Err(SyncError::InvalidRequest(
                "snapshot limit must be 1 to 500".into(),
            ));
        }
        let incarnation: String = self.connection().query_row(
            "SELECT value FROM meta WHERE key='sync_incarnation'",
            [],
            |row| row.get(0),
        )?;
        let lease: Option<(String,String,i64,i64)> = self.connection().query_row(
            "SELECT incarnation,epoch,seq,expires_at_ms FROM sync_leases WHERE id=?1 AND scope_digest=?2",
            params![lease_id,device_scope().digest()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
        let Some((lease_incarnation, lease_epoch, watermark, expires)) = lease else {
            return Ok(resync(&incarnation, epoch));
        };
        if lease_incarnation != incarnation || lease_epoch != epoch || expires <= now {
            return Ok(resync(&incarnation, epoch));
        }
        let pressure_floor: i64 = self.connection().query_row(
            "SELECT projection_pressure_floor FROM sync_state WHERE id=1",
            [],
            |row| row.get(0),
        )?;
        if watermark < pressure_floor {
            return Ok(over_budget(&incarnation, epoch));
        }
        let Some((records, next)) = self.device_page(after, limit, watermark)? else {
            return Ok(over_budget(&incarnation, epoch));
        };
        let mut answer = json!({"status":"ok","incarnation":incarnation,"watermark":super::Sequence::new(watermark)?,"bridge_epoch":epoch,"consistency":"leased","lease":{"id":lease_id,"expires_at_ms":expires},"records":records});
        if let Some((seq, kind, key)) = next {
            answer["next"] = json!({"first_seq":super::Sequence::new(seq)?,"kind":kind,"key":key});
        }
        // Includes envelope construction and serialization, after every SQL seek.
        if serde_json::to_vec(&answer)
            .map_err(|_| SyncError::Unavailable("invalid device page".into()))?
            .len()
            > super::MAX_RECOVERY_BYTES
        {
            return Ok(over_budget(&incarnation, epoch));
        }
        if expires <= recheck_now() {
            return Ok(resync(&incarnation, epoch));
        }
        Ok(answer)
    }

    fn device_page(
        &self,
        after: Option<(i64, &str, &str)>,
        limit: usize,
        watermark: i64,
    ) -> Result<Option<DevicePage>, SyncError> {
        let (seq, kind, key) = after.unwrap_or((0, "", ""));
        if seq < 0 || seq > watermark {
            return Err(SyncError::MalformedCursor);
        }
        let cap = (4 * limit).min(2000);
        let keys = self
            .connection()
            .prepare_cached(
                "SELECT first_seq,kind,key FROM sync_projection_keys
             WHERE (first_seq,kind,key)>(?1,?2,?3) AND first_seq<=?4
             ORDER BY first_seq,kind,key LIMIT ?5",
            )?
            .query_map(
                params![seq, kind, key, watermark, (cap + 1) as i64],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let mut records = Vec::new();
        let mut bytes = 0;
        let mut processed = 0;
        let mut last = None;
        for (seq, kind, key) in keys.iter().take(cap) {
            if records.len() == limit {
                break;
            }
            if let Some(record) = self.device_record_at(kind, key, watermark)? {
                let size = serde_json::to_vec(&record)
                    .map_err(|_| SyncError::Unavailable("invalid device record".into()))?
                    .len()
                    + 1;
                if bytes + size > super::MAX_RECOVERY_BYTES - 65536 {
                    break;
                }
                if record["row"].is_null() {
                    return Ok(None);
                }
                bytes += size;
                records.push(record);
            }
            processed += 1;
            last = Some((*seq, kind.clone(), key.clone()));
        }
        if processed == 0 && !keys.is_empty() {
            return Ok(None);
        }
        let next = if processed < keys.len() { last } else { None };
        Ok(Some((records, next)))
    }

    fn device_record_at(
        &self,
        kind: &str,
        key: &str,
        watermark: i64,
    ) -> Result<Option<Value>, SyncError> {
        let Some((revision, deleted, raw)) = projection_at(self, kind, key, watermark)? else {
            return Ok(None);
        };
        if deleted {
            return Ok(None);
        }
        let row = raw
            .map(|raw| {
                serde_json::from_str::<Value>(&raw)
                    .map_err(|_| SyncError::Unavailable("invalid device projection row".into()))
            })
            .transpose()?
            .unwrap_or(Value::Null);
        Ok(Some(
            json!({"kind":kind,"key":key,"revision":super::Sequence::new(revision)?,"row":row}),
        ))
    }

    pub fn sync_device_record(&self, kind: &str, key: &str) -> Result<Value, SyncError> {
        ensure_ready(self.connection())?;
        validate_identity(kind, key)?;
        let (revision, deleted, raw) = projection_at(self, kind, key, i64::MAX)?
            .ok_or_else(|| SyncError::Unavailable("missing device projection row".into()))?;
        let row = if deleted {
            Value::Null
        } else {
            serde_json::from_str(raw.as_deref().ok_or_else(|| {
                SyncError::Unavailable("device record exceeds byte budget".into())
            })?)
            .map_err(|_| SyncError::Unavailable("invalid device record".into()))?
        };
        Ok(
            json!({"kind":kind,"key":key,"revision":super::Sequence::new(revision)?,"row":row,"member":!deleted}),
        )
    }
}

type DeviceKey = (i64, String, String);
type DevicePage = (Vec<Value>, Option<DeviceKey>);

fn projection_at(
    read: &ReadTransaction<'_>,
    kind: &str,
    key: &str,
    watermark: i64,
) -> Result<Option<(i64, bool, Option<String>)>, SyncError> {
    let current = read
        .connection()
        .prepare_cached(
            "SELECT from_seq,deleted,CASE WHEN length(CAST(record AS BLOB))<=65536 THEN record END FROM sync_projection
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
            "SELECT from_seq,deleted,CASE WHEN length(CAST(record AS BLOB))<=65536 THEN record END FROM sync_projection_versions
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
    budget: &mut super::retention::VersionBudget,
) -> Result<(), StoreError> {
    let raw = record.map(Value::to_string);
    let payload_bytes = raw.as_ref().map_or(0, |raw| raw.len());
    if payload_bytes > DEVICE_ROW_BYTES {
        return Err(invalid("device projection row exceeds 64 KiB"));
    }
    let bytes = version_bytes(entity, payload_bytes);
    let previous: Option<(i64, bool, Option<String>, i64)> = tx
        .prepare_cached(
            "SELECT from_seq,deleted,record,record_bytes FROM sync_projection WHERE kind=?1 AND key=?2",
        )?
        .query_row(params![entity.kind, entity.key], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .optional()?;
    if let Some((from, deleted, previous, _)) = previous {
        let previous_bytes = version_bytes(entity, previous.as_ref().map_or(0, String::len));
        if budget.retain(previous_bytes, seq) {
            tx.prepare_cached("INSERT INTO sync_projection_versions(kind,key,from_seq,to_seq,deleted,record,record_bytes)
             VALUES(?1,?2,?3,?4,?5,?6,?7)")?.execute(
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
        }
    } else {
        tx.prepare_cached("INSERT INTO sync_projection_keys(kind,key,first_seq) VALUES(?1,?2,?3)")?
            .execute(params![entity.kind, entity.key, seq])?;
    }
    tx.prepare_cached(
        "INSERT INTO sync_projection(kind,key,from_seq,deleted,record,record_bytes)
         VALUES(?1,?2,?3,?4,?5,?6)
         ON CONFLICT(kind,key) DO UPDATE SET from_seq=excluded.from_seq,deleted=excluded.deleted,
         record=excluded.record,record_bytes=excluded.record_bytes",
    )?
    .execute(params![
        entity.kind,
        entity.key,
        seq,
        record.is_none(),
        raw,
        bytes
    ])?;
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
        Some(mut record) => {
            bound_display(&mut record);
            batch.upsert(entity, record, &[Stream::device()]);
        }
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

/// Scope-local fence: unrelated task streams remain available during registry work.
pub(super) fn ensure_scope_ready(
    conn: &rusqlite::Connection,
    scope: &Scope,
) -> Result<(), SyncError> {
    if scope.stream().kind == super::StreamKind::Device {
        ensure_ready(conn)?;
    }
    Ok(())
}

pub(super) fn ensure_ready(conn: &rusqlite::Connection) -> Result<(), SyncError> {
    // Cleanup and bootstrap checkpoint independently. Keep the same device-
    // local fence through both, including an interrupted startup/reopen.
    let dirty: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_dirty)
         OR EXISTS(SELECT 1 FROM meta WHERE key='device_334_cleanup_position')
         OR COALESCE((SELECT json_extract(value,'$.done') FROM meta WHERE key='device_owner_facts_334_position'),0)<>1
         OR COALESCE((SELECT json_extract(value,'$.phase') FROM meta WHERE key='device_sessions_334_position'),0)<>6",
        [],
        |row| row.get(0),
    )?;
    if dirty {
        return Err(SyncError::Unavailable("sync_reconciling".into()));
    }
    Ok(())
}

fn over_budget(incarnation: &str, epoch: &str) -> Value {
    json!({"status":"resync_required","incarnation":incarnation,"bridge_epoch":epoch,"reason":"over_budget"})
}

fn version_bytes(entity: &Entity, payload: usize) -> i64 {
    (payload + entity.kind.len() + entity.key.len() + 128) as i64
}

fn bound_display(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if matches!(
                    key.as_str(),
                    "name" | "topic" | "title" | "error" | "last_message_summary"
                ) {
                    if let Some(text) = value.as_str() {
                        *value = Value::String(text.chars().take(256).collect());
                    }
                } else {
                    bound_display(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                bound_display(value);
            }
        }
        _ => {}
    }
}
