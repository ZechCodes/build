//! Bounded storage housekeeping on the existing writer, outside application locks.

use super::{StoreError, StoreWriter};
use rusqlite::{Connection, OptionalExtension, Transaction};
use std::time::Duration;

const OPERATION_BATCH: usize = 1000;
const VACUUM_PAGES: u32 = 32;
const CLEANUP_CURSOR: &str = "delivered_payload_cleanup_cursor";
const CLEANUP_DONE: &str = "delivered_payload_cleanup_complete";
const VACUUM_CHECKPOINT_PENDING: &str = "incremental_vacuum_checkpoint_pending";
const CLEAR_PAYLOADS: &str = "UPDATE operations NOT INDEXED SET delivery =
    CASE WHEN json_valid(delivery) THEN
        CASE WHEN json_type(delivery, '$.owner_id') = 'text'
            AND json_type(delivery, '$.agent_id') = 'text'
            AND (json_extract(delivery, '$.owner_id') != entity_id
                OR json_extract(delivery, '$.agent_id') != agent_id)
            THEN json_set(delivery, '$.payload', NULL)
            ELSE NULL END
        ELSE NULL END,
    message_start_sequence = CASE WHEN message_start_sequence <= 0 THEN
        CASE WHEN json_valid(delivery) THEN
            CASE WHEN json_type(delivery, '$.payload.start_sequence') = 'integer'
                AND json_extract(delivery, '$.payload.start_sequence') > 0
                THEN json_extract(delivery, '$.payload.start_sequence')
                ELSE posted_sequence END
            ELSE posted_sequence END
        ELSE message_start_sequence END
    WHERE rowid > ?1 AND rowid <= ?2 AND status = 'delivered' AND delivery IS NOT NULL";

/// Startup only: changing NONE to INCREMENTAL requires rewriting the database.
/// Do this before reader admission, never from the background worker.
pub(super) fn configure_incremental_vacuum(connection: &Connection) -> Result<(), StoreError> {
    let mode: i64 = connection.pragma_query_value(None, "auto_vacuum", |row| row.get(0))?;
    if mode != 2 {
        connection.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
        let updated: i64 = connection.pragma_query_value(None, "auto_vacuum", |row| row.get(0))?;
        if updated != 2 {
            // VACUUM can commit mode 2 before a crash or failed checkpoint.
            // Keep the retry durable even when the next open skips VACUUM.
            connection.execute(
                "INSERT INTO meta (key, value) VALUES (?1, '1') ON CONFLICT(key) DO NOTHING",
                [VACUUM_CHECKPOINT_PENDING],
            )?;
            eprintln!("store: enabling incremental vacuum before opening readers");
            connection.execute_batch("VACUUM")?;
        }
    }
    let configured: i64 = connection.pragma_query_value(None, "auto_vacuum", |row| row.get(0))?;
    if configured != 2 {
        return Err(StoreError::Unopenable {
            path: connection.path().unwrap_or("build.db").into(),
            cause: "incremental vacuum could not be enabled".into(),
        });
    }
    let checkpoint_pending: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM meta WHERE key = ?1)",
        [VACUUM_CHECKPOINT_PENDING],
        |row| row.get(0),
    )?;
    if checkpoint_pending {
        finish_vacuum_checkpoint(connection)?;
    }
    Ok(())
}

fn finish_vacuum_checkpoint(connection: &Connection) -> Result<(), StoreError> {
    // Only before reader admission: ordinary checkpoints recycle WAL storage,
    // leaving VACUUM's database-sized high-water allocation on disk.
    let result: (i64, i64, i64) =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    if result != (0, 0, 0) {
        return Err(StoreError::Unopenable {
            path: connection.path().unwrap_or("build.db").into(),
            cause: format!(
                "incremental vacuum checkpoint incomplete (busy, log, checkpointed): {result:?}"
            ),
        });
    }
    connection.execute(
        "DELETE FROM meta WHERE key = ?1",
        [VACUUM_CHECKPOINT_PENDING],
    )?;
    Ok(())
}

#[derive(Debug)]
pub struct StorageMaintenanceProgress {
    pub scanned_operations: usize,
    pub cleared_deliveries: usize,
    pub cleanup_complete: bool,
    pub reclaimed_pages: u32,
    pub remaining_free_pages: u32,
}

impl StoreWriter {
    /// At most 1,000 indexed rowid entries and 32 reclaimed pages per call.
    /// The durable cursor advances in the same transaction as payload removal.
    /// Callers must run this outside app/domain guards, on a blocking thread.
    pub fn maintain_storage_batch(&self) -> Result<StorageMaintenanceProgress, StoreError> {
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        let (scanned_operations, cleared_deliveries, cleanup_complete) =
            cleanup_batch(&transaction)?;
        transaction.commit()?;
        let before: u32 =
            connection.pragma_query_value(None, "freelist_count", |row| row.get(0))?;
        if before > 0 {
            // Unlike execute_batch/execute, stepping through every result row
            // lets SQLite reclaim the entire requested page budget.
            let mut statement =
                connection.prepare(&format!("PRAGMA incremental_vacuum({VACUUM_PAGES})"))?;
            let mut rows = statement.query([])?;
            while rows.next()?.is_some() {}
        }
        let remaining_free_pages: u32 =
            connection.pragma_query_value(None, "freelist_count", |row| row.get(0))?;
        Ok(StorageMaintenanceProgress {
            scanned_operations,
            cleared_deliveries,
            cleanup_complete,
            reclaimed_pages: before.saturating_sub(remaining_free_pages),
            remaining_free_pages,
        })
    }

    /// Owns only the existing writer handle; no application mutex is captured.
    pub fn spawn_storage_maintenance(&self) -> StorageMaintenanceHandle {
        let writer = self.clone();
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            loop {
                if stopped.try_recv() != Err(tokio::sync::oneshot::error::TryRecvError::Empty) {
                    return;
                }
                let batch_writer = writer.clone();
                let result =
                    tokio::task::spawn_blocking(move || batch_writer.maintain_storage_batch())
                        .await;
                let delay = maintenance_delay(result);
                tokio::select! {
                    _ = &mut stopped => return,
                    _ = tokio::time::sleep(delay) => {}
                }
            }
        });
        StorageMaintenanceHandle {
            stop: Some(stop),
            task: Some(task),
        }
    }
}

fn cleanup_batch(transaction: &Transaction<'_>) -> Result<(usize, usize, bool), StoreError> {
    let complete: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM meta WHERE key = ?1)",
        [CLEANUP_DONE],
        |row| row.get(0),
    )?;
    if complete {
        return Ok((0, 0, true));
    }
    let cursor: i64 = transaction
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key = ?1",
            [CLEANUP_CURSOR],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(i64::MIN);
    let ids = transaction
        .prepare("SELECT rowid FROM operations WHERE rowid > ?1 ORDER BY rowid LIMIT ?2")?
        .query_map(rusqlite::params![cursor, OPERATION_BATCH as i64], |row| {
            row.get::<_, i64>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let cleared = if let Some(last) = ids.last() {
        // Prevent the status index from turning this bounded rowid range
        // into a scan of all delivered history on each batch.
        let changed = transaction.execute(CLEAR_PAYLOADS, rusqlite::params![cursor, last])?;
        transaction.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            rusqlite::params![CLEANUP_CURSOR, last.to_string()],
        )?;
        changed
    } else {
        0
    };
    let complete = ids.len() < OPERATION_BATCH;
    if complete {
        transaction.execute(
            "INSERT INTO meta (key, value) VALUES (?1, '1')",
            [CLEANUP_DONE],
        )?;
    }
    Ok((ids.len(), cleared, complete))
}

fn maintenance_delay(
    result: Result<Result<StorageMaintenanceProgress, StoreError>, tokio::task::JoinError>,
) -> Duration {
    match result {
        Ok(Ok(progress)) if !progress.cleanup_complete || progress.remaining_free_pages > 0 => {
            Duration::from_millis(250)
        }
        Ok(Ok(_)) => Duration::from_secs(60),
        error => {
            eprintln!("store maintenance: {error:?}; retrying in one minute");
            Duration::from_secs(60)
        }
    }
}

/// Dropping cancels future batches; explicit stop gives the current batch a short grace period.
pub struct StorageMaintenanceHandle {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl StorageMaintenanceHandle {
    /// Cancel future batches and allow one second for current work to finish.
    /// A blocking batch may still be waiting on the writer; aborting its async
    /// wrapper detaches that work rather than extending the shutdown budget.
    pub async fn stop(mut self) {
        self.cancel();
        if let Some(mut task) = self.task.take() {
            if tokio::time::timeout(Duration::from_secs(1), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }
    }

    fn cancel(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

impl Drop for StorageMaintenanceHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use rusqlite::Connection;

    fn seed(store: &Store, delivered: usize, pending: usize) {
        let mut connection = store.writer.connection();
        let transaction = connection.transaction().unwrap();
        for i in 0..delivered + pending {
            transaction
                .execute(
                    "INSERT INTO operations (operation_id, method, entity_id, agent_id,
                 conversation_id, choice_revision, request_hash, posted_sequence,
                 message_start_sequence, status, delivery, created_at, updated_at)
                 VALUES (?1, 'thread.post', 'e', 'a', 'c', 0, 'hash', 1, 1,
                         ?2, ?3, 'now', 'now')",
                    rusqlite::params![
                        i.to_string(),
                        if i < delivered {
                            "delivered"
                        } else if i.is_multiple_of(2) {
                            "uncertain"
                        } else {
                            "queued"
                        },
                        serde_json::json!({ "padding": "x".repeat(8192) }).to_string()
                    ],
                )
                .unwrap();
        }
        transaction.commit().unwrap();
    }

    #[test]
    fn cleanup_keeps_forwarded_delivery_target_without_its_payload() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path()).unwrap();
        seed(&store, 1, 0);
        let original = serde_json::json!({
            "owner_id": "forwarded-owner",
            "agent_id": "forwarded-agent",
            "root": "/forwarded/repo",
            "payload": { "start_sequence": 2, "end_sequence": 4, "messages": ["large message"] }
        });
        store.writer.connection().execute(
            "UPDATE operations SET message_start_sequence = 0, posted_sequence = 4, delivery = ?1",
            [original.to_string()],
        ).unwrap();
        let result = store.writer().maintain_storage_batch().unwrap();
        assert_eq!(result.cleared_deliveries, 1);
        assert!(result.cleanup_complete);
        let connection = store.writer.connection();
        let (raw, start): (String, i64) = connection
            .query_row(
                "SELECT delivery, message_start_sequence FROM operations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let stub: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(stub["owner_id"], "forwarded-owner");
        assert_eq!(stub["agent_id"], "forwarded-agent");
        assert_eq!(stub["root"], "/forwarded/repo");
        assert!(stub["payload"].is_null());
        assert_eq!(start, 2);
        drop(connection);
        assert_eq!(
            store
                .writer()
                .maintain_storage_batch()
                .unwrap()
                .cleared_deliveries,
            0
        );
    }

    #[test]
    fn cleanup_preserves_legacy_spans_before_removing_payloads() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path()).unwrap();
        seed(&store, 3, 0);
        store
            .writer
            .connection()
            .execute(
                "UPDATE operations SET message_start_sequence = 0, posted_sequence = 4,
             delivery = CASE operation_id WHEN '0' THEN '{\"payload\":{\"start_sequence\":2}}'
                WHEN '1' THEN 'invalid old payload' ELSE '{\"payload\":null}' END",
                [],
            )
            .unwrap();
        assert_eq!(
            store
                .writer()
                .maintain_storage_batch()
                .unwrap()
                .cleared_deliveries,
            3
        );
        let connection = store.writer.connection();
        let mut statement = connection.prepare(
            "SELECT message_start_sequence, posted_sequence, delivery FROM operations ORDER BY operation_id"
        ).unwrap();
        let rows: Vec<(i64, i64, Option<String>)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows, vec![(2, 4, None), (4, 4, None), (4, 4, None)]);
    }

    #[test]
    fn cleanup_update_uses_the_bounded_rowid_range() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path()).unwrap();
        let connection = store.writer.connection();
        let plan: String = connection
            .query_row(
                &format!("EXPLAIN QUERY PLAN {CLEAR_PAYLOADS}"),
                [0, 1000],
                |row| row.get(3),
            )
            .unwrap();
        assert!(
            plan.contains("INTEGER PRIMARY KEY (rowid>? AND rowid<?)"),
            "{plan}"
        );
    }

    #[test]
    fn cleanup_is_bounded_resumable_and_retains_unsettled_payloads() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path()).unwrap();
        seed(&store, 1001, 1200);
        let first = store.writer().maintain_storage_batch().unwrap();
        assert_eq!(first.scanned_operations, 1000);
        assert_eq!(first.cleared_deliveries, 1000);
        assert!(!first.cleanup_complete);
        drop(store);
        let store = Store::new(directory.path()).unwrap();
        let second = store.writer().maintain_storage_batch().unwrap();
        assert_eq!(second.scanned_operations, 1000);
        assert_eq!(second.cleared_deliveries, 1);
        let third = store.writer().maintain_storage_batch().unwrap();
        assert_eq!(third.scanned_operations, 201);
        assert!(third.cleanup_complete);
        assert_eq!(store.writer.connection().query_row(
            "SELECT count(*) FROM operations WHERE status!='delivered' AND delivery IS NOT NULL",
            [], |row| row.get::<_, i64>(0)).unwrap(), 1200);
        assert_eq!(
            store
                .writer()
                .maintain_storage_batch()
                .unwrap()
                .scanned_operations,
            0
        );
    }

    #[test]
    fn startup_conversion_truncates_its_database_sized_wal_while_open() {
        let directory = tempfile::tempdir().unwrap();
        let old = Connection::open(directory.path().join("build.db")).unwrap();
        old.execute_batch(
            "CREATE TABLE payload (value BLOB); INSERT INTO payload VALUES (zeroblob(1048576));",
        )
        .unwrap();
        drop(old);
        let store = Store::new(directory.path()).unwrap();
        let wal_bytes = std::fs::metadata(directory.path().join("build.db-wal"))
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        assert!(wal_bytes < 65536, "startup WAL retained {wal_bytes} bytes");
        let pending: i64 = store
            .writer
            .connection()
            .query_row(
                "SELECT count(*) FROM meta WHERE key = ?1",
                [VACUUM_CHECKPOINT_PENDING],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0);
    }

    #[test]
    fn startup_retries_pending_checkpoint_without_repeating_vacuum() {
        let directory = tempfile::tempdir().unwrap();
        let connection = Connection::open(directory.path().join("build.db")).unwrap();
        connection
            .pragma_update(None, "auto_vacuum", "INCREMENTAL")
            .unwrap();
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        connection
            .execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE payload (value BLOB);
             INSERT INTO payload (rowid, value) VALUES (7, zeroblob(1048576));",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO meta VALUES (?1, '1')",
                [VACUUM_CHECKPOINT_PENDING],
            )
            .unwrap();
        configure_incremental_vacuum(&connection).unwrap();
        let wal_bytes = std::fs::metadata(directory.path().join("build.db-wal"))
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        assert!(wal_bytes < 65536, "retry WAL retained {wal_bytes} bytes");
        let rowid: i64 = connection
            .query_row("SELECT rowid FROM payload", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rowid, 7, "a repeated VACUUM would renumber this rowid");
        let pending: i64 = connection
            .query_row(
                "SELECT count(*) FROM meta WHERE key = ?1",
                [VACUUM_CHECKPOINT_PENDING],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0);
    }

    #[test]
    fn busy_startup_checkpoint_preserves_the_retry_marker() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("build.db");
        let connection = Connection::open(&path).unwrap();
        connection
            .pragma_update(None, "auto_vacuum", "INCREMENTAL")
            .unwrap();
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        connection.busy_timeout(Duration::ZERO).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE payload (value BLOB); INSERT INTO payload VALUES (zeroblob(1048576));",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO meta VALUES (?1, '1')",
                [VACUUM_CHECKPOINT_PENDING],
            )
            .unwrap();
        let reader =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        reader.execute_batch("BEGIN").unwrap();
        let _: i64 = reader
            .query_row("SELECT count(*) FROM payload", [], |row| row.get(0))
            .unwrap();
        connection
            .execute("UPDATE payload SET value = zeroblob(524288)", [])
            .unwrap();
        assert!(matches!(
            configure_incremental_vacuum(&connection),
            Err(StoreError::Unopenable { .. })
        ));
        let pending: i64 = connection
            .query_row(
                "SELECT count(*) FROM meta WHERE key = ?1",
                [VACUUM_CHECKPOINT_PENDING],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, 1);
        reader.execute_batch("ROLLBACK").unwrap();
        configure_incremental_vacuum(&connection).unwrap();
        let pending: i64 = connection
            .query_row(
                "SELECT count(*) FROM meta WHERE key = ?1",
                [VACUUM_CHECKPOINT_PENDING],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0);
    }

    #[test]
    fn existing_database_converts_before_readers_and_keeps_its_contents() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("build.db");
        let old = Connection::open(&path).unwrap();
        old.execute_batch(
            "CREATE TABLE preserved (value TEXT); INSERT INTO preserved VALUES ('keep');",
        )
        .unwrap();
        assert_eq!(
            old.query_row("PRAGMA auto_vacuum", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(old);
        for _ in 0..2 {
            let store = Store::new(directory.path()).unwrap();
            let connection = store.writer.connection();
            assert_eq!(
                connection
                    .query_row("PRAGMA auto_vacuum", [], |row| row.get::<_, i64>(0))
                    .unwrap(),
                2
            );
            assert_eq!(
                connection
                    .query_row("SELECT value FROM preserved", [], |row| row
                        .get::<_, String>(0))
                    .unwrap(),
                "keep"
            );
        }
    }

    #[test]
    fn incremental_vacuum_drains_the_statement_but_bounds_reclamation() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path()).unwrap();
        {
            let connection = store.writer.connection();
            connection.execute_batch("CREATE TABLE garbage (value BLOB); INSERT INTO garbage VALUES (zeroblob(1048576)); DELETE FROM garbage;").unwrap();
        }
        let first = store.writer().maintain_storage_batch().unwrap();
        assert_eq!(first.reclaimed_pages, 32);
        assert!(first.remaining_free_pages > 0);
        while store
            .writer()
            .maintain_storage_batch()
            .unwrap()
            .remaining_free_pages
            > 0
        {}
        assert_eq!(
            store
                .writer
                .connection()
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
    }

    #[tokio::test]
    async fn maintenance_worker_can_stop_without_taking_the_application_lock() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path()).unwrap();
        let worker = store.writer().spawn_storage_maintenance();
        tokio::time::timeout(std::time::Duration::from_secs(2), worker.stop())
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn blocked_maintenance_does_not_delay_shutdown_roster_or_children() {
        use crate::resume::{LiveRoster, ResumeRoster, ResumingAgent};

        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path()).unwrap();
        let live = LiveRoster::start(directory.path(), "test");
        live.set_entity(
            "run-1",
            vec![ResumingAgent {
                entity_id: "run-1".into(),
                agent_id: "agent-1".into(),
                conversation_id: "agent-1".into(),
                thread_id: None,
                resume_session_id: Some("session-1".into()),
                was_working: true,
            }],
        );
        // A separate holder keeps the writer unavailable until observation.
        let writer = store.writer();
        let (locked, held) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let _guard = writer.connection();
            let _ = locked.send(());
            let _ = released.recv();
        });
        held.await.unwrap();
        let batch_writer = store.writer();
        let (started, batch_started) = tokio::sync::oneshot::channel();
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = tokio::task::spawn_blocking(move || {
                let _ = started.send(());
                batch_writer.maintain_storage_batch()
            })
            .await;
        });
        batch_started.await.unwrap();
        let worker = StorageMaintenanceHandle {
            stop: Some(stop),
            task: Some(task),
        };
        let mut children_stopped = false;
        let mut children_before_cancel = false;
        let completed = tokio::time::timeout(
            Duration::from_secs(4),
            crate::app::shut_down_with_storage_maintenance(&live, Some(worker), || {
                children_stopped = ResumeRoster::path(directory.path()).exists();
                children_before_cancel = matches!(
                    stopped.try_recv(),
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty)
                );
            }),
        )
        .await
        .is_ok();
        let recorded = ResumeRoster::take(directory.path());
        // Release even on the red path so the blocking pool can finish.
        drop(release);
        holder.join().unwrap();
        assert!(
            children_stopped,
            "writer contention delayed roster/child teardown"
        );
        assert!(
            children_before_cancel,
            "maintenance cancellation preceded child teardown"
        );
        assert_eq!(recorded.unwrap().agents[0].agent_id, "agent-1");
        assert!(completed, "maintenance join exceeded the shutdown budget");
    }
}
