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
    let (head, persisted, bytes): (i64, i64, i64) = conn.query_row(
        "SELECT head,floor,cumulative_bytes FROM sync_state WHERE id=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    // Append timestamps and byte prefixes are monotonic. These reverse seeks
    // touch one index winner, never MAX(seq) across every expired row.
    let age: i64 = conn.query_row("SELECT COALESCE((SELECT seq FROM sync_journal WHERE at_ms<?1 ORDER BY at_ms DESC,seq DESC LIMIT 1),0)",[now.saturating_sub(policy.max_age_ms)],|r|r.get(0))?;
    let byte_floor = byte_floor(conn, bytes.saturating_sub(policy.max_bytes))?;
    Ok(persisted
        .max(head.saturating_sub(policy.max_entries))
        .max(age)
        .max(byte_floor))
}

fn byte_floor(conn: &Connection, threshold: i64) -> Result<i64, StoreError> {
    let removed: Option<i64> = conn
        .query_row(
            "SELECT cumulative_bytes-record_bytes FROM sync_journal ORDER BY seq LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if threshold <= removed.unwrap_or(0) {
        return Ok(0);
    }
    // Round UP to discard the entire entry containing the byte cutoff. If
    // GC already removed that prefix, do not discard the first survivor again.
    Ok(conn.query_row("SELECT COALESCE((SELECT seq FROM sync_journal WHERE cumulative_bytes>=?1 ORDER BY cumulative_bytes,seq LIMIT 1),0)",[threshold],|r|r.get(0))?)
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
    pub changed_rows: u64,
    pub pending: bool,
}

impl StoreWriter {
    pub fn sync_gc_batch(&self, now: i64) -> Result<GcProgress, StoreError> {
        self.transaction(|tx| gc(tx, now))
    }
}

fn gc(tx: &Transaction<'_>, now: i64) -> Result<GcProgress, StoreError> {
    let before = tx.total_changes();
    let floor = logical_floor(tx, now, Policy::DEFAULT)?;
    let limit = (GC_ROWS - 1) / (2 * MAX_ROUTES + 1);
    let rows = tx
        .prepare("SELECT seq,record_bytes FROM sync_journal WHERE seq<=?1 ORDER BY seq LIMIT ?2")?
        .query_map(params![floor, limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut progress = GcProgress::default();
    let mut bytes = 0_i64;
    for (seq, record_bytes) in rows {
        let routes = tx
            .prepare("SELECT stream_kind,stream_key FROM sync_routes WHERE seq=?1 LIMIT ?2")?
            .query_map(params![seq, (MAX_ROUTES + 1) as i64], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if routes.len() > MAX_ROUTES {
            return Err(super::invalid("journal routes exceed GC budget"));
        }
        for (kind, key) in routes {
            tx.execute("UPDATE sync_stream_state SET floor=?3 WHERE stream_kind=?1 AND stream_key=?2 AND floor<?3",params![kind,key,seq])?;
            progress.removed_routes += tx.execute(
                "DELETE FROM sync_routes WHERE stream_kind=?1 AND stream_key=?2 AND seq=?3",
                params![kind, key, seq],
            )?;
        }
        progress.removed_journal += tx.execute("DELETE FROM sync_journal WHERE seq=?1", [seq])?;
        bytes += record_bytes;
    }
    let remaining = || (GC_ROWS - 1).saturating_sub((tx.total_changes() - before) as usize);
    progress.removed_revisions = tx.execute("DELETE FROM sync_revisions WHERE (entity_kind,entity_key) IN (SELECT entity_kind,entity_key FROM sync_revisions WHERE deleted=1 AND seq<=?1 ORDER BY seq LIMIT ?2)",params![floor,remaining() as i64])?;
    progress.removed_leases = tx.execute(
        "DELETE FROM sync_leases WHERE id IN
         (SELECT id FROM sync_leases WHERE expires_at_ms<=?1 ORDER BY expires_at_ms,id LIMIT ?2)",
        params![now, remaining() as i64],
    )?;
    let oldest_lease: Option<i64> = tx.query_row(
        "SELECT MIN(seq) FROM sync_leases WHERE expires_at_ms>?1",
        [now],
        |row| row.get(0),
    )?;
    let projection_floor = oldest_lease.map_or(floor, |lease| floor.min(lease));
    progress.removed_projection_versions = tx.execute(
        "DELETE FROM sync_projection_versions WHERE (kind,key,from_seq) IN
         (SELECT kind,key,from_seq FROM sync_projection_versions WHERE to_seq<=?1
          ORDER BY to_seq,kind,key,from_seq LIMIT ?2)",
        params![projection_floor, remaining() as i64],
    )?;
    let tombstone_limit = remaining() / 2;
    let tombstones = tx
        .prepare(
            "SELECT kind,key FROM sync_projection p WHERE deleted=1 AND from_seq<=?1
             AND NOT EXISTS(SELECT 1 FROM sync_projection_versions v
                 WHERE v.kind=p.kind AND v.key=p.key)
             ORDER BY from_seq,kind,key LIMIT ?2",
        )?
        .query_map(params![floor, tombstone_limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (kind, key) in tombstones {
        progress.removed_projection_rows += tx.execute(
            "DELETE FROM sync_projection WHERE kind=?1 AND key=?2",
            params![kind, key],
        )?;
        progress.removed_projection_keys += tx.execute(
            "DELETE FROM sync_projection_keys WHERE kind=?1 AND key=?2",
            params![kind, key],
        )?;
    }
    tx.execute("UPDATE sync_state SET floor=?1,total_bytes=total_bytes-?2 WHERE id=1 AND (floor<>?1 OR ?2<>0)",params![floor,bytes])?;
    progress.changed_rows = tx.total_changes() - before;
    progress.pending = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_journal WHERE seq<=?1)
         OR EXISTS(SELECT 1 FROM sync_revisions WHERE deleted=1 AND seq<=?1)
         OR EXISTS(SELECT 1 FROM sync_leases WHERE expires_at_ms<=?2)
         OR EXISTS(SELECT 1 FROM sync_projection_versions WHERE to_seq<=?3)
         OR EXISTS(SELECT 1 FROM sync_projection p WHERE deleted=1 AND from_seq<=?1
             AND NOT EXISTS(SELECT 1 FROM sync_projection_versions v
                 WHERE v.kind=p.kind AND v.key=p.key))",
        params![floor, now, projection_floor],
        |r| r.get(0),
    )?;
    Ok(progress)
}
