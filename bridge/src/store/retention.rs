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
         OR EXISTS(SELECT 1 FROM thread_items WHERE agent_id = ?1 AND message = 1
             AND json_extract(item, '$.data.role') = 'user'
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
    let keep = recent.clamp(policy.min_messages, policy.max_messages);
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
            let mut statement = tx.prepare("SELECT rowid, conversation_id FROM conversation_attachment_uploads
                WHERE julianday(CASE WHEN uploaded_at = '' THEN (SELECT value FROM meta WHERE key = 'retention_uploads_registered_at') ELSE uploaded_at END) < julianday(?1)
                AND NOT EXISTS (SELECT 1 FROM operations WHERE conversation_id = conversation_attachment_uploads.conversation_id AND status IN ('queued','claimed','uncertain'))
                AND NOT EXISTS (SELECT 1 FROM thread_items WHERE agent_id = conversation_attachment_uploads.conversation_id AND message = 1
                    AND json_extract(item, '$.data.role') = 'user' AND json_extract(item, '$.data.seen_at') IS NULL
                    AND COALESCE(json_extract(item, '$.data.delivery_status'), 'queued') IN ('queued','submitted'))
                ORDER BY rowid LIMIT ?2")?;
            let candidates = statement.query_map(rusqlite::params![cutoff, limit.min(BATCH_ROWS) as i64], |row| Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
            drop(statement);
            let mut removed = 0;
            for (rowid, id) in candidates {
                if !protected(tx, &id)? {
                    removed += tx.execute("DELETE FROM conversation_attachment_uploads WHERE rowid = ?1", [rowid])?;
                }
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
            let items_deleted = tx.execute(
                "DELETE FROM thread_items WHERE agent_id = ?1 AND sequence IN (SELECT sequence FROM thread_items WHERE agent_id = ?1 AND sequence < ?2 ORDER BY sequence LIMIT ?3)",
                rusqlite::params![id, floor as i64, (BATCH_ROWS - operations_deleted - backups_deleted) as i64])?;
            if items_deleted > 0 {
                let actual_floor: u64 = tx.query_row("SELECT COALESCE(MIN(sequence), ?2) FROM thread_items WHERE agent_id = ?1", rusqlite::params![id, floor as i64], |row| row.get::<_, i64>(0))? as u64;
                tx.execute("INSERT INTO conversation_retention(agent_id,thread_id,history_start_sequence,high_watermark) VALUES (?1,?2,?3,?4)
                    ON CONFLICT(agent_id) DO UPDATE SET thread_id = excluded.thread_id, history_start_sequence = MAX(history_start_sequence, excluded.history_start_sequence), high_watermark = MAX(high_watermark, excluded.high_watermark)",
                    rusqlite::params![id, plan.thread_id, actual_floor as i64, plan.high_watermark as i64])?;
            }
            Ok(RetentionBatch {conversation_id: id.into(), items_deleted, operations_deleted, boundary: read_boundary(tx, id)?, has_more: has_old_items(tx,id,floor)?})
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture(messages: usize) -> (TempDir, Store) {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path()).unwrap();
        store.in_transaction(|tx| {
            tx.execute("INSERT INTO agents VALUES ('a', 'owner', 1, '{\"thread\":{\"id\":\"t\"}}')", [])?;
            for n in 1..=messages {
                let item = serde_json::json!({"type":"message","data":{"created_at":"2026-10-23T12:00:00Z", "role":"assistant"}});
                tx.execute("INSERT INTO thread_items(agent_id,sequence,updated_sequence,message,item) VALUES ('a',?1,?1,1,?2)", rusqlite::params![n as i64, item.to_string()])?;
            }
            Ok(())
        }).unwrap();
        (dir, store)
    }

    #[test]
    fn newest_hundred_and_dry_run() {
        let (_dir, store) = fixture(150);
        let report = store
            .retention_report(RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap();
        assert_eq!(report[0].history_start_sequence, 51);
        assert_eq!(report[0].items_to_prune, 50);
        assert_eq!(store.thread_item_count("a").unwrap(), 150);
        let batch = store
            .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap();
        assert_eq!(batch.items_deleted, 50);
        assert_eq!(store.thread_item_count("a").unwrap(), 100);
    }

    #[test]
    fn explicit_zero_minimum_preserves_counter_after_emptying() {
        let (dir, store) = fixture(5);
        let result = store
            .prune_conversation_batch(
                "a",
                RetentionPolicy {
                    min_messages: 0,
                    ..RetentionPolicy::default()
                },
                "2026-11-24T12:00:00Z",
            )
            .unwrap();
        assert_eq!(result.items_deleted, 5);
        drop(store);
        let store = Store::new(dir.path()).unwrap();
        let boundary = store.thread_retention_boundary("a").unwrap().unwrap();
        assert_eq!(boundary.history_start_sequence, 6);
        assert_eq!(boundary.high_watermark, 5);
        assert_eq!(
            store
                .connection()
                .query_row(super::super::THREAD_LAST_SEQUENCE_SQL, ["a"], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            5
        );
    }
    #[test]
    fn both_limits_and_activity_use_the_oldest_surviving_message() {
        let (_dir, store) = fixture(150);
        store.in_transaction(|tx| {
            tx.execute("UPDATE thread_items SET sequence = -sequence, updated_sequence = -updated_sequence", [])?;
            tx.execute("UPDATE thread_items SET sequence = sequence * -10, updated_sequence = updated_sequence * -10", [])?;
            tx.execute("UPDATE thread_items SET item = json_set(item, '$.data.created_at', '2026-10-01T12:00:00Z') WHERE sequence < 1200", [])?;
            for sequence in [1199,1201,1501] {
                tx.execute("INSERT INTO thread_items(agent_id,sequence,updated_sequence,activity,item) VALUES ('a',?1,?1,1,'{}')", [sequence])?;
            }
            Ok(())
        }).unwrap();
        let batch = store
            .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap();
        assert_eq!(batch.boundary.unwrap().history_start_sequence, 1010);
        assert_eq!(store.thread_item_count("a").unwrap(), 53);
        assert_eq!(
            store
                .connection()
                .query_row(
                    "SELECT COUNT(*) FROM thread_items WHERE agent_id = 'a' AND activity = 1",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            3
        );
    }

    #[test]
    fn exact_age_boundary_is_inclusive() {
        let (_dir, store) = fixture(2);
        store.in_transaction(|tx| {
            tx.execute("UPDATE thread_items SET item = json_set(item, '$.data.created_at', '2026-10-17T12:00:00Z') WHERE sequence = 2", [])?;
            tx.execute("UPDATE thread_items SET item = json_set(item, '$.data.created_at', '2026-10-17T11:59:59Z') WHERE sequence = 1", [])?;
            Ok(())
        }).unwrap();
        let batch = store
            .prune_conversation_batch(
                "a",
                RetentionPolicy {
                    min_messages: 0,
                    ..RetentionPolicy::default()
                },
                "2026-10-24T12:00:00Z",
            )
            .unwrap();
        assert_eq!(batch.items_deleted, 1);
        assert_eq!(batch.boundary.unwrap().history_start_sequence, 2);
    }

    #[test]
    fn operation_and_legacy_pending_deliveries_protect_history() {
        let (_dir, store) = fixture(150);
        for status in ["queued", "claimed", "uncertain"] {
            store.in_transaction(|tx| {
                tx.execute("INSERT OR REPLACE INTO operations(operation_id,method,entity_id,agent_id,conversation_id,choice_revision,request_hash,posted_sequence,message_start_sequence,status,created_at,updated_at) VALUES ('op','thread.post','owner','a','a',0,'hash',1,1,?1,'now','now')", [status])?;
                Ok(())
            }).unwrap();
            let batch = store
                .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
                .unwrap();
            assert_eq!(batch.items_deleted, 0);
        }
        store.in_transaction(|tx| {
            tx.execute("DELETE FROM operations", [])?;
            tx.execute("UPDATE thread_items SET item = json_set(item, '$.data.role', 'user') WHERE sequence = 1", [])?;
            Ok(())
        }).unwrap();
        assert_eq!(
            store
                .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
                .unwrap()
                .items_deleted,
            0
        );
    }

    #[test]
    fn bounded_batches_resume_after_reopen() {
        let (dir, store) = fixture(2500);
        let first = store
            .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap();
        assert_eq!(first.items_deleted, 1000);
        assert!(first.has_more);
        assert_eq!(first.boundary.unwrap().history_start_sequence, 1001);
        drop(store);
        let store = Store::new(dir.path()).unwrap();
        assert_eq!(
            store
                .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
                .unwrap()
                .items_deleted,
            1000
        );
        let last = store
            .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap();
        assert_eq!(last.items_deleted, 400);
        assert!(!last.has_more);
        assert_eq!(store.thread_item_count("a").unwrap(), 100);
    }

    #[test]
    fn stale_resident_saves_cannot_resurrect_pruned_history() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let mut agents = crate::agent::AgentRoster::with_first(
            "owner",
            crate::models::ModelChoice::default(),
            "2026-10-23T12:00:00Z",
        )
        .agents()
        .to_vec();
        for _ in 0..150 {
            agents[0]
                .thread
                .post_agent("hello", None, "2026-10-23T12:00:00Z");
        }
        let id = agents[0].id.clone();
        store
            .in_transaction(|tx| Store::write_agents(tx, "owner", &agents))
            .unwrap();
        store
            .prune_conversation_batch(&id, RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap();
        store
            .in_transaction(|tx| Store::write_agents(tx, "owner", &agents))
            .unwrap();
        assert_eq!(store.thread_item_count(&id).unwrap(), 100);
        store
            .prune_conversation_batch(
                &id,
                RetentionPolicy {
                    min_messages: 0,
                    ..RetentionPolicy::default()
                },
                "2026-11-24T12:00:00Z",
            )
            .unwrap();
        store
            .in_transaction(|tx| Store::write_agents(tx, "owner", &agents))
            .unwrap();
        assert_eq!(store.thread_item_count(&id).unwrap(), 0);
        let loaded = Store::read_agents(
            &store.connection(),
            "owner",
            &crate::models::ModelChoice::default(),
            None,
        )
        .unwrap();
        let mut agent = loaded[0].clone();
        let previous = agent.thread.last_sequence();
        agent.thread.post_agent("new", None, "2026-11-24T12:00:00Z");
        assert!(agent.thread.last_sequence() > previous);
        assert!(previous >= 150);
    }

    #[tokio::test]
    async fn pooled_read_p99_stays_below_100ms_during_pruning() {
        let (_dir, store) = fixture(12_000);
        let mut reader = store.read_pool().acquire().await;
        // Prime the production read connection before measuring contention.
        reader
            .read(|read| read.thread_item_count("a"))
            .unwrap()
            .unwrap();
        let writer = store.clone();
        let start = std::sync::Arc::new(std::sync::Barrier::new(2));
        let writer_start = start.clone();
        let worker = std::thread::spawn(move || {
            writer_start.wait();
            loop {
                if !writer
                    .prune_conversation_batch(
                        "a",
                        RetentionPolicy::default(),
                        "2026-10-24T12:00:00Z",
                    )
                    .unwrap()
                    .has_more
                {
                    break;
                }
            }
        });
        start.wait();
        let mut samples = Vec::new();
        while samples.len() < 200 || !worker.is_finished() {
            let began = std::time::Instant::now();
            reader
                .read(|read| read.thread_item_count("a"))
                .unwrap()
                .unwrap();
            samples.push(began.elapsed());
        }
        worker.join().unwrap();
        samples.sort_unstable();
        let p99 = samples[(samples.len() - 1) * 99 / 100];
        eprintln!(
            "retention pooled read p99={p99:?}; samples={}",
            samples.len()
        );
        assert!(
            p99 < std::time::Duration::from_millis(100),
            "read p99 during pruning: {p99:?}"
        );
    }
    #[test]
    fn settled_operation_retry_and_conflict_evidence_survives_pruning() {
        let (_dir, store) = fixture(150);
        store.in_transaction(|tx| {
            tx.execute("INSERT INTO operations(operation_id,method,entity_id,agent_id,conversation_id,choice_revision,request_hash,posted_sequence,message_start_sequence,status,created_at,updated_at) VALUES ('op','thread.post','owner','a','a',0,'hash',1,1,'delivered','now','now')", [])?;
            Ok(())
        }).unwrap();
        let original = super::super::operations::read_operation(&store.connection(), "op")
            .unwrap()
            .unwrap();
        let batch = store
            .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap();
        assert_eq!(batch.operations_deleted, 0);
        assert_eq!(batch.items_deleted, 50);
        store
            .in_transaction(|tx| {
                let retry =
                    super::super::operations::existing_operation_or_conflict(tx, &original)?
                        .unwrap();
                assert_eq!(retry, original);
                let mut conflict = original.clone();
                conflict.request_hash = "different".into();
                assert!(matches!(
                    super::super::operations::existing_operation_or_conflict(tx, &conflict),
                    Err(StoreError::OperationConflict { .. })
                ));
                Ok(())
            })
            .unwrap();
    }
    #[test]
    fn old_conversations_keep_fifty_and_short_histories_keep_every_message() {
        for (messages, kept) in [(150, 50), (12, 12), (0, 0)] {
            let (_dir, store) = fixture(messages);
            let batch = store
                .prune_conversation_batch("a", RetentionPolicy::default(), "2026-11-24T12:00:00Z")
                .unwrap();
            assert_eq!(store.thread_item_count("a").unwrap(), kept);
            assert_eq!(batch.items_deleted as u64, messages as u64 - kept);
        }
    }

    #[test]
    fn recent_count_between_bounds_selects_that_many_newest_messages() {
        let (_dir, store) = fixture(150);
        store.in_transaction(|tx| {
            tx.execute("UPDATE thread_items SET item = json_set(item, '$.data.created_at', '2026-10-01T12:00:00Z') WHERE sequence <= 70", [])?;
            Ok(())
        }).unwrap();
        let batch = store
            .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap();
        assert_eq!(store.thread_item_count("a").unwrap(), 80);
        assert_eq!(batch.boundary.unwrap().history_start_sequence, 71);
    }

    #[test]
    fn upload_expiry_is_bounded_and_preserves_recent_and_pending_drafts() {
        let (_dir, store) = fixture(1);
        store.in_transaction(|tx| {
            for (id,path,date) in [("a","old","2026-10-01T00:00:00Z"),("a","recent","2026-10-24T00:00:00Z"),("pending","protected","2026-10-01T00:00:00Z")] {
                tx.execute("INSERT INTO conversation_attachment_uploads VALUES (?1,'t',?2,?3)", rusqlite::params![id,path,date])?;
            }
            tx.execute("INSERT INTO operations(operation_id,method,entity_id,agent_id,conversation_id,choice_revision,request_hash,posted_sequence,message_start_sequence,status,created_at,updated_at) VALUES ('op','thread.post','owner','pending','pending',0,'hash',1,1,'queued','now','now')", [])?;
            Ok(())
        }).unwrap();
        assert_eq!(
            store
                .cleanup_conversation_attachment_uploads(7, "2026-10-24T12:00:00Z", 1)
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .cleanup_conversation_attachment_uploads(7, "2026-10-24T12:00:00Z", 1000)
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .connection()
                .query_row(
                    "SELECT COUNT(*) FROM conversation_attachment_uploads",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
    }

    #[test]
    fn message_commit_consumes_its_reservation_but_later_save_preserves_new_draft() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let mut agents = crate::agent::AgentRoster::with_first(
            "owner",
            crate::models::ModelChoice::default(),
            "2026-10-23T12:00:00Z",
        )
        .agents()
        .to_vec();
        let id = agents[0].id.clone();
        let thread_id = agents[0].thread.id.clone();
        let path = ".build/attachments/shared.png";
        store
            .register_conversation_attachment(&id, &thread_id, path)
            .unwrap();
        agents[0]
            .thread
            .post_agent("with attachment", None, "2026-10-23T12:00:00Z");
        if let Some(crate::thread::ThreadItem::Message(message)) = agents[0].thread.items.last_mut()
        {
            message.attachments.push(crate::thread::MessageAttachment {
                name: "shared.png".into(),
                path: path.into(),
                mime: "image/png".into(),
                size: 4,
            });
        }
        store
            .in_transaction(|tx| Store::write_agents(tx, "owner", &agents))
            .unwrap();
        assert_eq!(
            store
                .connection()
                .query_row(
                    "SELECT COUNT(*) FROM conversation_attachment_uploads",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        store
            .register_conversation_attachment(&id, &thread_id, path)
            .unwrap();
        store
            .in_transaction(|tx| Store::write_agents(tx, "owner", &agents))
            .unwrap();
        assert_eq!(
            store
                .connection()
                .query_row(
                    "SELECT COUNT(*) FROM conversation_attachment_uploads",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }
    #[test]
    fn recent_count_includes_timestamps_outside_newest_maximum() {
        let (_dir, store) = fixture(150);
        store.in_transaction(|tx| {
            tx.execute("UPDATE thread_items SET item = json_set(item, '$.data.created_at', '2026-10-01T00:00:00Z') WHERE sequence > 80", [])?;
            Ok(())
        }).unwrap();
        let batch = store
            .prune_conversation_batch("a", RetentionPolicy::default(), "2026-10-24T12:00:00Z")
            .unwrap();
        assert_eq!(store.thread_item_count("a").unwrap(), 80);
        assert_eq!(batch.boundary.unwrap().history_start_sequence, 71);
    }
}
