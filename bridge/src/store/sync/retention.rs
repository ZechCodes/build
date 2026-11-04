//! Indexed logical floors and bounded physical cleanup, off request paths.
use super::journal::MAX_ROUTES;
use crate::store::{StoreError, StoreWriter};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

pub const MAX_JOURNAL_ENTRIES: i64 = 100_000;
pub const MAX_JOURNAL_AGE_MS: i64 = 7 * 24 * 60 * 60 * 1000;
pub const MAX_JOURNAL_BYTES: i64 = 64 * 1024 * 1024;
/// Includes stream-floor updates, route/journal/revision deletes and counters.
pub const GC_ROWS: usize = 128;

#[derive(Clone, Copy)]
pub(super) struct Policy {
    pub max_entries: i64,
    pub max_bytes: i64,
    pub max_age_ms: i64,
}
impl Policy {
    pub const DEFAULT: Self = Self {
        max_entries: MAX_JOURNAL_ENTRIES,
        max_bytes: MAX_JOURNAL_BYTES,
        max_age_ms: MAX_JOURNAL_AGE_MS,
    };
}

pub(super) fn logical_floor(
    conn: &Connection,
    now: i64,
    policy: Policy,
) -> Result<i64, StoreError> {
    let (head, persisted, bytes): (i64, i64, i64) = conn
        .prepare_cached("SELECT head,floor,cumulative_bytes FROM sync_state WHERE id=1")?
        .query_row([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    // Append timestamps and byte prefixes are monotonic. These reverse seeks
    // touch one index winner, never MAX(seq) across every expired row.
    let age: i64 = conn.prepare_cached("SELECT COALESCE((SELECT seq FROM sync_journal WHERE at_ms<?1 ORDER BY at_ms DESC,seq DESC LIMIT 1),0)")?.query_row([now.saturating_sub(policy.max_age_ms)],|r|r.get(0))?;
    let byte_floor = byte_floor(conn, bytes.saturating_sub(policy.max_bytes))?;
    Ok(persisted
        .max(head.saturating_sub(policy.max_entries))
        .max(age)
        .max(byte_floor))
}

fn byte_floor(conn: &Connection, threshold: i64) -> Result<i64, StoreError> {
    let removed: Option<i64> = conn
        .prepare_cached(
            "SELECT cumulative_bytes-record_bytes FROM sync_journal ORDER BY seq LIMIT 1",
        )?
        .query_row([], |r| r.get(0))
        .optional()?;
    if threshold <= removed.unwrap_or(0) {
        return Ok(0);
    }
    // Round UP to discard the entire entry containing the byte cutoff. If
    // GC already removed that prefix, do not discard the first survivor again.
    Ok(conn.prepare_cached("SELECT COALESCE((SELECT seq FROM sync_journal WHERE cumulative_bytes>=?1 ORDER BY cumulative_bytes,seq LIMIT 1),0)")?.query_row([threshold],|r|r.get(0))?)
}

#[derive(Debug, Default)]
pub struct GcProgress {
    pub removed_journal: usize,
    pub removed_routes: usize,
    pub removed_revisions: usize,
    pub removed_leases: usize,
    pub removed_projection_versions: usize,
    pub removed_projection_rows: usize,
    pub removed_projection_keys: usize,
    pub removed_counter_rows: usize,
    pub changed_rows: u64,
    pub pending: bool,
}

impl StoreWriter {
    pub fn sync_gc_batch(&self, now: i64) -> Result<GcProgress, StoreError> {
        self.transaction(|tx| gc(tx, now))
    }
}

pub(super) const MAX_RETAINED_VERSION_BYTES: i64 = 64 * 1024 * 1024;

pub(super) fn retain_version(
    tx: &Transaction<'_>,
    bytes: i64,
    to_seq: i64,
) -> Result<bool, StoreError> {
    let retained: i64 = tx
        .prepare_cached("SELECT retained_version_bytes FROM sync_state WHERE id=1")?
        .query_row([], |row| row.get(0))?;
    if retained > MAX_RETAINED_VERSION_BYTES.saturating_sub(bytes) {
        tx.prepare_cached("UPDATE sync_state SET projection_pressure_floor=MAX(projection_pressure_floor,?1) WHERE id=1")?.execute([to_seq])?;
        return Ok(false);
    }
    tx.prepare_cached(
        "UPDATE sync_state SET retained_version_bytes=retained_version_bytes+?1 WHERE id=1",
    )?
    .execute([bytes])?;
    Ok(true)
}

fn remaining(tx: &Transaction<'_>, before: u64) -> usize {
    (GC_ROWS - 2).saturating_sub((tx.total_changes() - before) as usize)
}

fn gc(tx: &Transaction<'_>, now: i64) -> Result<GcProgress, StoreError> {
    let before = tx.total_changes();
    let floor = logical_floor(tx, now, Policy::DEFAULT)?;
    let mut progress = GcProgress::default();
    let bytes = gc_journal(tx, floor, &mut progress)?;
    progress.removed_revisions=tx.prepare_cached("DELETE FROM sync_revisions WHERE (entity_kind,entity_key) IN (SELECT entity_kind,entity_key FROM sync_revisions WHERE deleted=1 AND seq<=?1 ORDER BY seq LIMIT ?2)")?.execute(params![floor,remaining(tx,before) as i64])?;
    progress.removed_leases=tx.prepare_cached("DELETE FROM sync_leases WHERE id IN (SELECT id FROM sync_leases WHERE expires_at_ms<=?1 ORDER BY expires_at_ms,id LIMIT ?2)")?.execute(params![now,remaining(tx,before) as i64])?;
    let projection_floor:i64=tx.prepare_cached("SELECT MAX(projection_pressure_floor,COALESCE((SELECT seq FROM sync_leases ORDER BY seq LIMIT 1),head)) FROM sync_state WHERE id=1")?.query_row([],|row|row.get(0))?;
    let retained_bytes = gc_versions(tx, projection_floor, remaining(tx, before), &mut progress)?;
    progress.removed_counter_rows = super::device_thread::gc(tx, remaining(tx, before))?;
    let tombstones_pending = gc_tombstones(
        tx,
        floor,
        remaining(tx, before) / 2,
        progress.removed_projection_versions > 0,
        &mut progress,
    )?;
    tx.prepare_cached("UPDATE sync_state SET floor=MAX(floor,?1),total_bytes=total_bytes-?2,retained_version_bytes=MAX(0,retained_version_bytes-?3) WHERE id=1 AND (floor<?1 OR ?2<>0 OR ?3<>0)")?.execute(params![floor,bytes,retained_bytes])?;
    progress.changed_rows = tx.total_changes() - before;
    progress.pending = tombstones_pending
        || tx
            .prepare_cached(
                "SELECT EXISTS(SELECT 1 FROM sync_journal WHERE seq<=?1)
         OR EXISTS(SELECT 1 FROM sync_revisions WHERE deleted=1 AND seq<=?1)
         OR EXISTS(SELECT 1 FROM sync_leases WHERE expires_at_ms<=?2)
         OR EXISTS(SELECT 1 FROM sync_projection_versions WHERE to_seq<=?3)
         OR EXISTS(SELECT 1 FROM device_attention_gc)",
            )?
            .query_row(params![floor, now, projection_floor], |row| {
                row.get::<_, bool>(0)
            })?;
    Ok(progress)
}

fn gc_journal(
    tx: &Transaction<'_>,
    floor: i64,
    progress: &mut GcProgress,
) -> Result<i64, StoreError> {
    let limit = (GC_ROWS - 2) / (2 * MAX_ROUTES + 1);
    let rows = tx
        .prepare_cached(
            "SELECT seq,record_bytes FROM sync_journal WHERE seq<=?1 ORDER BY seq LIMIT ?2",
        )?
        .query_map(params![floor, limit as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut bytes = 0;
    for (seq, size) in rows {
        let routes = tx
            .prepare_cached("SELECT stream_kind,stream_key FROM sync_routes WHERE seq=?1 LIMIT ?2")?
            .query_map(params![seq, (MAX_ROUTES + 1) as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if routes.len() > MAX_ROUTES {
            return Err(super::invalid("journal routes exceed GC budget"));
        }
        for (kind, key) in routes {
            tx.prepare_cached("UPDATE sync_stream_state SET floor=?3 WHERE stream_kind=?1 AND stream_key=?2 AND floor<?3")?.execute(params![kind,key,seq])?;
            progress.removed_routes += tx
                .prepare_cached(
                    "DELETE FROM sync_routes WHERE stream_kind=?1 AND stream_key=?2 AND seq=?3",
                )?
                .execute(params![kind, key, seq])?;
        }
        progress.removed_journal += tx
            .prepare_cached("DELETE FROM sync_journal WHERE seq=?1")?
            .execute([seq])?;
        bytes += size;
    }
    Ok(bytes)
}

fn gc_versions(
    tx: &Transaction<'_>,
    floor: i64,
    limit: usize,
    progress: &mut GcProgress,
) -> Result<i64, StoreError> {
    let rows=tx.prepare_cached("SELECT kind,key,from_seq,record_bytes FROM sync_projection_versions WHERE to_seq<=?1 ORDER BY to_seq,kind,key,from_seq LIMIT ?2")?
        .query_map(params![floor,limit as i64],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,i64>(2)?,row.get::<_,i64>(3)?)))?.collect::<Result<Vec<_>,_>>()?;
    let mut bytes = 0;
    for (kind, key, from, size) in rows {
        progress.removed_projection_versions += tx
            .prepare_cached(
                "DELETE FROM sync_projection_versions WHERE kind=?1 AND key=?2 AND from_seq=?3",
            )?
            .execute(params![kind, key, from])?;
        bytes += size;
    }
    Ok(bytes)
}

fn gc_tombstones(
    tx: &Transaction<'_>,
    floor: i64,
    limit: usize,
    rescan: bool,
    progress: &mut GcProgress,
) -> Result<bool, StoreError> {
    let (seq, kind, key): (i64, String, String) = tx.prepare_cached("SELECT gc_projection_seq,gc_projection_kind,gc_projection_key FROM sync_state WHERE id=1")?.query_row(
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let after = tombstones_after(tx, floor, seq, &kind, &key)?;
    if !after && rescan {
        tx.prepare_cached("UPDATE sync_state SET gc_projection_seq=0,gc_projection_kind='',gc_projection_key='' WHERE id=1")?.execute([])?;
        return tombstones_after(tx, floor, 0, "", "");
    }
    if limit == 0 {
        return Ok(after);
    }
    let rows=tx.prepare_cached("SELECT from_seq,kind,key FROM sync_projection WHERE deleted=1 AND (from_seq,kind,key)>(?1,?2,?3) AND from_seq<=?4 ORDER BY from_seq,kind,key LIMIT ?5")?
        .query_map(params![seq,kind,key,floor,limit as i64],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
    for (_, kind, key) in &rows {
        let protected: bool = tx
            .prepare_cached(
                "SELECT EXISTS(SELECT 1 FROM sync_projection_versions WHERE kind=?1 AND key=?2)",
            )?
            .query_row(params![kind, key], |row| row.get(0))?;
        if !protected {
            progress.removed_projection_rows += tx
                .prepare_cached("DELETE FROM sync_projection WHERE kind=?1 AND key=?2")?
                .execute(params![kind, key])?;
            progress.removed_projection_keys += tx
                .prepare_cached("DELETE FROM sync_projection_keys WHERE kind=?1 AND key=?2")?
                .execute(params![kind, key])?;
        }
    }
    let (seq, kind, key) = rows
        .last()
        .map(|(seq, kind, key)| (*seq, kind.as_str(), key.as_str()))
        .unwrap_or((0, "", ""));
    tx.prepare_cached("UPDATE sync_state SET gc_projection_seq=?1,gc_projection_kind=?2,gc_projection_key=?3 WHERE id=1 AND (gc_projection_seq<>?1 OR gc_projection_kind<>?2 OR gc_projection_key<>?3)")?.execute(params![seq,kind,key])?;
    Ok(!rows.is_empty() && tombstones_after(tx, floor, seq, kind, key)?)
}
fn tombstones_after(
    tx: &Transaction<'_>,
    floor: i64,
    seq: i64,
    kind: &str,
    key: &str,
) -> Result<bool, StoreError> {
    Ok(tx.prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_projection WHERE deleted=1 AND (from_seq,kind,key)>(?1,?2,?3) AND from_seq<=?4)")?.query_row(params![seq,kind,key,floor],|row|row.get(0))?)
}

#[cfg(test)]
#[path = "retention_tests.rs"]
mod tests;
