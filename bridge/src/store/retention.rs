//! Bounded, restartable conversation pruning. Call outside application guards.
use super::{ReadTransaction, Store, StoreError};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

const BATCH_ROWS: usize = 1_000;

/// Legacy reservations have no creation clock. Give them one migration-wide
/// grace period without rewriting every reservation during store startup.
pub(super) fn ensure_upload_retention_columns(conn: &Connection) -> Result<(), StoreError> {
    let mut columns = conn.prepare("PRAGMA table_info(conversation_attachment_uploads)")?;
    let names = columns
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(columns);
    if !names.iter().any(|name| name == "uploaded_at") {
        conn.execute("ALTER TABLE conversation_attachment_uploads ADD COLUMN uploaded_at TEXT NOT NULL DEFAULT ''", [])?;
    }
    conn.execute("INSERT OR IGNORE INTO meta(key,value) VALUES ('retention_uploads_registered_at', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))", [])?;
    let mut columns = conn.prepare("PRAGMA table_info(conversation_retention)")?;
    let names = columns
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(columns);
    if !names.iter().any(|name| name == "pruned_message_times") {
        conn.execute("ALTER TABLE conversation_retention ADD COLUMN pruned_message_times TEXT NOT NULL DEFAULT '[]'", [])?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub struct RetentionPolicy {
    pub min_messages: usize,
    pub max_messages: usize,
    pub max_age_days: u64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            min_messages: 50,
            max_messages: 100,
            max_age_days: 7,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RetentionBoundary {
    pub thread_id: String,
    pub history_start_sequence: u64,
    pub high_watermark: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConversationRetentionReport {
    pub conversation_id: String,
    pub thread_id: String,
    pub history_start_sequence: u64,
    pub high_watermark: u64,
    pub items_to_prune: u64,
    pub items_bytes_to_prune: u64,
    pub operations_to_prune: u64,
    pub protected: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct RetentionBatch {
    pub conversation_id: String,
    pub items_deleted: usize,
    pub operations_deleted: usize,
    pub boundary: Option<RetentionBoundary>,
    pub has_more: bool,
}

fn cutoff_time(policy: RetentionPolicy, now: &str) -> Result<String, StoreError> {
    if policy.min_messages > policy.max_messages {
        return Err(StoreError::RetentionInvalid(
            "min_messages exceeds max_messages".into(),
        ));
    }
    let now: jiff::Timestamp = now
        .parse()
        .map_err(|error: jiff::Error| StoreError::RetentionInvalid(error.to_string()))?;
    let seconds = policy
        .max_age_days
        .checked_mul(86_400)
        .and_then(|days| i64::try_from(days).ok())
        .ok_or_else(|| {
            StoreError::RetentionInvalid("max_age_days exceeds supported range".into())
        })?;
    now.checked_sub(jiff::SignedDuration::from_secs(seconds))
        .map(|cutoff| cutoff.to_string())
        .map_err(|error| StoreError::RetentionInvalid(error.to_string()))
}

pub(super) fn read_boundary(
    conn: &Connection,
    id: &str,
) -> Result<Option<RetentionBoundary>, StoreError> {
    conn.query_row(
        "SELECT thread_id, history_start_sequence, high_watermark FROM conversation_retention WHERE agent_id = ?1",
        [id], |row| Ok(RetentionBoundary { thread_id: row.get(0)?, history_start_sequence: row.get::<_, i64>(1)?.max(0) as u64, high_watermark: row.get::<_, i64>(2)?.max(0) as u64 })
    ).optional().map_err(StoreError::from)
}

// A pending delivery can still read old context or mutate old message rows.
// Protect the entire conversation until it has settled, including legacy sends.
fn protected(conn: &Connection, id: &str) -> Result<bool, StoreError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM operations WHERE conversation_id = ?1 AND status IN ('queued','claimed','uncertain'))
         OR EXISTS(SELECT 1 FROM operations WHERE status IN ('queued','claimed','uncertain')
             AND json_extract(delivery, '$.agent_id') IN (SELECT ?1 UNION SELECT id FROM agents WHERE json_extract(record, '$.conversation_id') = ?1))
         OR EXISTS(SELECT 1 FROM thread_items WHERE agent_id = ?1 AND message = 1
             AND json_extract(item, '$.data.role') = 'user'
             AND json_extract(item, '$.data.operation_id') IS NULL
             AND json_extract(item, '$.data.seen_at') IS NULL
             AND COALESCE(json_extract(item, '$.data.delivery_status'), 'queued') IN ('queued','submitted'))",
        [id], |row| row.get(0))?)
}

struct RetentionPlan {
    thread_id: String,
    history_start_sequence: u64,
    high_watermark: u64,
    protected: bool,
}

fn plan(
    conn: &Connection,
    id: &str,
    policy: RetentionPolicy,
    cutoff: &str,
) -> Result<RetentionPlan, StoreError> {
    let thread_id: String = conn.query_row(
        "SELECT COALESCE(json_extract(record, '$.thread.id'), '') FROM agents WHERE id = ?1",
        [id],
        |row| row.get(0),
    )?;
    let high_watermark: u64 = conn.query_row(super::THREAD_LAST_SEQUENCE_SQL, [id], |row| {
        row.get::<_, i64>(0)
    })? as u64;
    // Count at most the cap through the timestamp index. Timestamps need not
    // follow sequence order (imports and delayed messages can arrive later).
    let recent: usize = conn.query_row(
        "SELECT COUNT(*) FROM (SELECT 1 FROM thread_items WHERE agent_id = ?1 AND message = 1
         AND julianday(json_extract(item, '$.data.created_at')) >= julianday(?3) LIMIT ?2)",
        rusqlite::params![
            id,
            i64::try_from(policy.max_messages).unwrap_or(i64::MAX),
            cutoff
        ],
        |row| row.get::<_, i64>(0),
    )? as usize;
    let pruned_recent: i64 = conn.query_row(
        "SELECT COUNT(*) FROM conversation_retention r, json_each(r.pruned_message_times) t
         WHERE r.agent_id = ?1 AND CAST(t.value AS REAL) >= julianday(?2)",
        rusqlite::params![id, cutoff],
        |row| row.get(0),
    )?;
    let keep = recent
        .saturating_add(pruned_recent as usize)
        .clamp(policy.min_messages, policy.max_messages);
    let floor: Option<u64> = conn.query_row(
        "SELECT MIN(sequence) FROM (SELECT sequence FROM thread_items WHERE agent_id = ?1 AND message = 1 ORDER BY sequence DESC LIMIT ?2)",
        rusqlite::params![id, i64::try_from(keep).unwrap_or(i64::MAX)], |row| row.get::<_, Option<i64>>(0))?.map(|value| value as u64);
    let floor = floor.unwrap_or(high_watermark.saturating_add(1));
    Ok(RetentionPlan {
        thread_id,
        history_start_sequence: floor,
        high_watermark,
        protected: protected(conn, id)?,
    })
}

fn report(
    conn: &Connection,
    id: &str,
    policy: RetentionPolicy,
    cutoff: &str,
) -> Result<ConversationRetentionReport, StoreError> {
    let plan = plan(conn, id, policy, cutoff)?;
    let (items_to_prune, items_bytes_to_prune): (i64,i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(length(CAST(item AS BLOB))),0) FROM thread_items WHERE agent_id = ?1 AND sequence < ?2",
        rusqlite::params![id, plan.history_start_sequence as i64], |row| Ok((row.get(0)?,row.get(1)?)))?;
    Ok(ConversationRetentionReport {
        conversation_id: id.into(),
        thread_id: plan.thread_id,
        history_start_sequence: plan.history_start_sequence,
        high_watermark: plan.high_watermark,
        items_to_prune: items_to_prune as u64,
        items_bytes_to_prune: items_bytes_to_prune as u64,
        operations_to_prune: 0,
        protected: plan.protected,
    })
}

fn has_old_items(conn: &Connection, id: &str, floor: u64) -> Result<bool, StoreError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM thread_items WHERE agent_id = ?1 AND sequence < ?2)",
        rusqlite::params![id, floor as i64],
        |row| row.get(0),
    )?)
}

/// Preserve only the timestamp evidence needed to count a future window. The
/// selected item span is exactly the deletion batch, never all old history.
fn batch_message_times(
    conn: &Connection,
    id: &str,
    floor: u64,
    limit: usize,
    policy: RetentionPolicy,
    cutoff: &str,
) -> Result<String, StoreError> {
    let mut prior = conn.prepare("SELECT CAST(t.value AS REAL) FROM conversation_retention r, json_each(r.pruned_message_times) t
        WHERE r.agent_id = ?1 AND CAST(t.value AS REAL) >= julianday(?2)")?;
    let mut times = prior
        .query_map(rusqlite::params![id, cutoff], |row| row.get::<_, f64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut selected = conn.prepare("SELECT julianday(json_extract(item, '$.data.created_at')) FROM
        (SELECT message, item FROM thread_items WHERE agent_id = ?1 AND sequence < ?2 ORDER BY sequence LIMIT ?3)
        WHERE message = 1 AND julianday(json_extract(item, '$.data.created_at')) >= julianday(?4)")?;
    for timestamp in selected.query_map(
        rusqlite::params![id, floor as i64, limit as i64, cutoff],
        |row| row.get::<_, f64>(0),
    )? {
        times.push(timestamp?);
    }
    times.sort_unstable_by(|left, right| right.total_cmp(left));
    times.truncate(policy.max_messages);
    Ok(serde_json::to_string(&times).expect("SQLite Julian days are finite numbers"))
}

impl ReadTransaction<'_> {
    pub fn thread_retention_boundary(
        &self,
        id: &str,
    ) -> Result<Option<RetentionBoundary>, StoreError> {
        read_boundary(self.connection(), id)
    }

    pub fn retention_report(
        &self,
        policy: RetentionPolicy,
        now: &str,
    ) -> Result<Vec<ConversationRetentionReport>, StoreError> {
        let cutoff = cutoff_time(policy, now)?;
        let mut statement = self
            .connection()
            .prepare("SELECT id FROM agents ORDER BY id")?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids.iter()
            .map(|id| report(self.connection(), id, policy, &cutoff))
            .collect()
    }
}

impl Store {
    pub fn thread_retention_boundary(
        &self,
        id: &str,
    ) -> Result<Option<RetentionBoundary>, StoreError> {
        read_boundary(&self.connection(), id)
    }

    /// Release expired draft reservations; the exact-reference file sweep is
    /// separate. A pending delivery protects its entire conversation.
    pub fn cleanup_conversation_attachment_uploads(
        &self,
        max_age_days: u64,
        now: &str,
        limit: usize,
    ) -> Result<usize, StoreError> {
        let cutoff = cutoff_time(
            RetentionPolicy {
                max_age_days,
                ..RetentionPolicy::default()
            },
            now,
        )?;
        self.in_transaction(|tx| {
            let mut statement = tx.prepare("SELECT rowid FROM conversation_attachment_uploads
                WHERE julianday(CASE WHEN uploaded_at = '' THEN (SELECT value FROM meta WHERE key = 'retention_uploads_registered_at') ELSE uploaded_at END) < julianday(?1)
                AND NOT EXISTS (SELECT 1 FROM operations WHERE conversation_id = conversation_attachment_uploads.conversation_id AND status IN ('queued','claimed','uncertain'))
                AND NOT EXISTS (SELECT 1 FROM operations WHERE status IN ('queued','claimed','uncertain')
                    AND json_extract(delivery, '$.agent_id') IN (SELECT conversation_attachment_uploads.conversation_id UNION SELECT id FROM agents WHERE json_extract(record, '$.conversation_id') = conversation_attachment_uploads.conversation_id))
                AND NOT EXISTS (SELECT 1 FROM thread_items WHERE agent_id = conversation_attachment_uploads.conversation_id AND message = 1
                    AND json_extract(item, '$.data.role') = 'user' AND json_extract(item, '$.data.operation_id') IS NULL
                    AND json_extract(item, '$.data.seen_at') IS NULL
                    AND COALESCE(json_extract(item, '$.data.delivery_status'), 'queued') IN ('queued','submitted'))
                ORDER BY rowid LIMIT ?2")?;
            let candidates = statement.query_map(rusqlite::params![cutoff, limit.min(BATCH_ROWS) as i64], |row| row.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
            drop(statement);
            let mut removed = 0;
            for rowid in candidates {
                removed += tx.execute("DELETE FROM conversation_attachment_uploads WHERE rowid = ?1", [rowid])?;
            }
            Ok(removed)
        })
    }

    pub fn retention_report_conversation(
        &self,
        id: &str,
        policy: RetentionPolicy,
        now: &str,
    ) -> Result<ConversationRetentionReport, StoreError> {
        let cutoff = cutoff_time(policy, now)?;
        report(&self.connection(), id, policy, &cutoff)
    }

    /// Bounded keyset discovery for the background service.
    pub fn retention_conversations_after(
        &self,
        after_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, StoreError> {
        let conn = self.connection();
        let mut statement =
            conn.prepare("SELECT id FROM agents WHERE id > ?1 ORDER BY id LIMIT ?2")?;
        let rows = statement.query_map(
            rusqlite::params![after_id.unwrap_or(""), limit.min(BATCH_ROWS) as i64],
            |row| row.get(0),
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// This read-only report never advances a boundary or changes history.
    pub fn retention_report(
        &self,
        policy: RetentionPolicy,
        now: &str,
    ) -> Result<Vec<ConversationRetentionReport>, StoreError> {
        ReadTransaction::new(&self.connection()).retention_report(policy, now)
    }

    /// Each call rechecks delivery protection inside the write transaction.
    /// No durable job cursor is needed: committed deletes are the resume point.
    pub fn prune_conversation_batch(
        &self,
        id: &str,
        policy: RetentionPolicy,
        now: &str,
    ) -> Result<RetentionBatch, StoreError> {
        let cutoff = cutoff_time(policy, now)?;
        self.in_transaction(|tx| {
            let plan = plan(tx, id, policy, &cutoff)?;
            if plan.protected {
                return Ok(RetentionBatch {conversation_id: id.into(), items_deleted: 0, operations_deleted: 0, boundary: read_boundary(tx, id)?, has_more: false});
            }
            let floor = plan.history_start_sequence;
            let backups_deleted = if has_old_items(tx,id,floor)? {
                tx.execute("DELETE FROM agent_migration_backups WHERE rowid IN (SELECT rowid FROM agent_migration_backups WHERE agent_id = ?1 LIMIT ?2)", rusqlite::params![id, BATCH_ROWS as i64])?
            } else { 0 };
            let operations_deleted = 0;
            let batch_limit = BATCH_ROWS - backups_deleted;
            let message_times = batch_message_times(tx,id,floor,batch_limit,policy,&cutoff)?;
            let items_deleted = tx.execute(
                "DELETE FROM thread_items WHERE agent_id = ?1 AND sequence IN (SELECT sequence FROM thread_items WHERE agent_id = ?1 AND sequence < ?2 ORDER BY sequence LIMIT ?3)",
                rusqlite::params![id, floor as i64, batch_limit as i64])?;
            if items_deleted > 0 {
                let actual_floor: u64 = tx.query_row("SELECT COALESCE(MIN(sequence), ?2) FROM thread_items WHERE agent_id = ?1", rusqlite::params![id, floor as i64], |row| row.get::<_, i64>(0))? as u64;
                tx.execute("INSERT INTO conversation_retention(agent_id,thread_id,history_start_sequence,high_watermark,pruned_message_times) VALUES (?1,?2,?3,?4,?5)
                    ON CONFLICT(agent_id) DO UPDATE SET thread_id = excluded.thread_id, history_start_sequence = MAX(history_start_sequence, excluded.history_start_sequence), high_watermark = MAX(high_watermark, excluded.high_watermark), pruned_message_times = excluded.pruned_message_times",
                    rusqlite::params![id, plan.thread_id, actual_floor as i64, plan.high_watermark as i64, message_times])?;
            }
            Ok(RetentionBatch {conversation_id: id.into(), items_deleted, operations_deleted, boundary: read_boundary(tx, id)?, has_more: has_old_items(tx,id,floor)?})
        })
    }
}

#[cfg(test)]
mod tests;
