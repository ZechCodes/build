use super::{
    is_json_record, now_rfc3339, write_run, write_task, PersistedPlan, PersistedRun, Store,
    StoreError,
};
use crate::agent::Agent;
use crate::operation::OperationReceipt;
use crate::operation::OperationStatus;
use crate::thread::{MessageDeliveryStatus, ThreadItem};
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use std::path::Path;
use std::path::PathBuf;

impl Store {
    /// Copy the database to `destination` as a consistent snapshot.
    ///
    /// The state dir holds a live `build.db` plus its `-wal` and `-shm`
    /// sidecars, and a file-at-a-time copy of that triple while the daemon is
    /// running takes a torn database — the JSON records it replaced could each
    /// be copied on their own, and this cannot. `VACUUM INTO` writes one
    /// self-contained file from a single consistent read, so a backup taken
    /// mid-write is a database rather than a puzzle.
    ///
    /// Refuses to overwrite: a backup that silently replaced the previous one
    /// is a backup that can be lost twice.
    pub fn backup_to(&self, destination: &Path) -> Result<(), StoreError> {
        if destination.exists() {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{} already exists", destination.display()),
            )));
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // A bound parameter is not accepted here (VACUUM INTO takes a literal),
        // so the path is quoted the way SQLite quotes a string literal.
        let quoted = destination.to_string_lossy().replace('\'', "''");
        self.connection()
            .execute_batch(&format!("VACUUM INTO '{quoted}'"))?;
        Ok(())
    }
    /// Accept one reviewer post: its new transcript rows and immutable
    /// operation receipt land in the same transaction. A retry that presents
    /// the same operation and resolved request returns the original receipt
    /// without touching either table.
    pub fn accept_thread_post(
        &self,
        conversation_owner_id: &str,
        agents: &[Agent],
        receipt: &OperationReceipt,
    ) -> Result<OperationReceipt, StoreError> {
        self.in_transaction(|tx| {
            if let Some(existing) = existing_operation_or_conflict(tx, receipt)? {
                return Ok(existing);
            }
            Store::write_agents(tx, conversation_owner_id, agents)?;
            insert_operation(tx, receipt)?;
            Ok(receipt.clone())
        })
    }
    /// An immutable receipt by its client-generated id.
    pub fn operation(&self, operation_id: &str) -> Result<Option<OperationReceipt>, StoreError> {
        read_operation(&self.connection(), operation_id)
    }
    /// Mark one authorized operation's exact historical messages seen without
    /// loading the rest of the conversation. The agent skeleton and selected
    /// item rows advance in one transaction, including when the messages sit
    /// below the bounded resident tail after restart.
    pub fn acknowledge_operation_messages(
        &self,
        conversation_id: &str,
        operation_id: &str,
        start_sequence: u64,
        end_sequence: u64,
        now: &str,
    ) -> Result<u64, StoreError> {
        self.acknowledge_operation_messages_with_working(
            conversation_id,
            Some(operation_id),
            start_sequence,
            end_sequence,
            now,
            true,
        )
    }
    pub fn acknowledge_native_operation_messages(
        &self,
        conversation_id: &str,
        operation_id: &str,
        start_sequence: u64,
        end_sequence: u64,
        now: &str,
    ) -> Result<u64, StoreError> {
        self.acknowledge_operation_messages_with_working(
            conversation_id,
            Some(operation_id),
            start_sequence,
            end_sequence,
            now,
            false,
        )
    }
    pub fn acknowledge_native_legacy_messages(
        &self,
        conversation_id: &str,
        start_sequence: u64,
        end_sequence: u64,
        now: &str,
    ) -> Result<u64, StoreError> {
        self.acknowledge_operation_messages_with_working(
            conversation_id,
            None,
            start_sequence,
            end_sequence,
            now,
            false,
        )
    }
    fn acknowledge_operation_messages_with_working(
        &self,
        conversation_id: &str,
        operation_id: Option<&str>,
        start_sequence: u64,
        end_sequence: u64,
        now: &str,
        mark_working: bool,
    ) -> Result<u64, StoreError> {
        self.in_transaction(|tx| {
            let raw_agent: String = tx.query_row(
                "SELECT record FROM agents WHERE id = ?1",
                [conversation_id],
                |row| row.get(0),
            )?;
            let mut agent: Agent =
                serde_json::from_str(&raw_agent).map_err(|source| StoreError::Corrupt {
                    path: PathBuf::from(format!("agents/{conversation_id}")),
                    source,
                })?;
            let mut statement = tx.prepare(
                "SELECT item FROM thread_items WHERE agent_id = ?1 \
                 AND sequence BETWEEN ?2 AND ?3 ORDER BY sequence",
            )?;
            let raw_items = statement
                .query_map(
                    rusqlite::params![
                        conversation_id,
                        i64::try_from(start_sequence).unwrap_or(i64::MAX),
                        i64::try_from(end_sequence).unwrap_or(i64::MAX)
                    ],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            agent.thread.items = raw_items
                .into_iter()
                .map(|raw| {
                    serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                        path: PathBuf::from(format!("thread_items/{conversation_id}")),
                        source,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            match (operation_id, mark_working) {
                (Some(operation_id), true) => {
                    agent.thread.read_operation_messages(
                        operation_id,
                        start_sequence,
                        end_sequence,
                        now,
                    );
                    agent.thread.note_operation_read(now);
                }
                (Some(operation_id), false) => {
                    agent.thread.read_native_operation_messages(
                        operation_id,
                        start_sequence,
                        end_sequence,
                        now,
                    );
                }
                (None, false) => {
                    agent
                        .thread
                        .read_native_legacy_messages(start_sequence, end_sequence, now);
                }
                (None, true) => unreachable!("legacy acknowledgement never starts work"),
            }
            let acknowledged_sequence = agent
                .thread
                .items
                .iter()
                .map(ThreadItem::latest_sequence)
                .max()
                .unwrap_or_else(|| agent.thread.last_sequence());
            for item in &agent.thread.items {
                tx.execute(
                    "UPDATE thread_items SET item = ?3, updated_sequence = ?4 \
                     WHERE agent_id = ?1 AND sequence = ?2",
                    rusqlite::params![
                        conversation_id,
                        i64::try_from(item.sequence()).unwrap_or(i64::MAX),
                        serde_json::to_string(item).expect("a thread item always serializes"),
                        i64::try_from(item.latest_sequence()).unwrap_or(i64::MAX),
                    ],
                )?;
            }
            agent.thread.items.clear();
            tx.execute(
                "UPDATE agents SET record = ?2 WHERE id = ?1",
                rusqlite::params![
                    conversation_id,
                    serde_json::to_string(&agent).expect("an agent always serializes")
                ],
            )?;
            Ok(acknowledged_sequence)
        })
    }
    /// Persist one delivery-state transition across a bounded operation span,
    /// including messages older than the resident in-memory tail.
    pub fn set_operation_delivery_status(
        &self,
        conversation_id: &str,
        operation_id: &str,
        start_sequence: u64,
        end_sequence: u64,
        status: MessageDeliveryStatus,
    ) -> Result<u64, StoreError> {
        self.set_delivery_status(
            conversation_id,
            Some(operation_id),
            start_sequence,
            end_sequence,
            status,
        )
    }
    /// Persist a delivery transition for unmanaged reviewer messages only.
    pub fn set_legacy_delivery_status(
        &self,
        conversation_id: &str,
        start_sequence: u64,
        end_sequence: u64,
        status: MessageDeliveryStatus,
    ) -> Result<u64, StoreError> {
        self.set_delivery_status(conversation_id, None, start_sequence, end_sequence, status)
    }
    fn set_delivery_status(
        &self,
        conversation_id: &str,
        operation_id: Option<&str>,
        start_sequence: u64,
        end_sequence: u64,
        status: MessageDeliveryStatus,
    ) -> Result<u64, StoreError> {
        self.in_transaction(|tx| {
            let raw_agent: String = tx.query_row(
                "SELECT record FROM agents WHERE id = ?1",
                [conversation_id],
                |row| row.get(0),
            )?;
            let mut agent: Agent =
                serde_json::from_str(&raw_agent).map_err(|source| StoreError::Corrupt {
                    path: PathBuf::from(format!("agents/{conversation_id}")),
                    source,
                })?;
            let mut statement = tx.prepare(
                "SELECT item FROM thread_items WHERE agent_id = ?1 \
                 AND sequence BETWEEN ?2 AND ?3 ORDER BY sequence",
            )?;
            agent.thread.items = statement
                .query_map(
                    rusqlite::params![
                        conversation_id,
                        i64::try_from(start_sequence).unwrap_or(i64::MAX),
                        i64::try_from(end_sequence).unwrap_or(i64::MAX)
                    ],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(|raw| {
                    serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                        path: PathBuf::from(format!("thread_items/{conversation_id}")),
                        source,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            match operation_id {
                Some(operation_id) => agent.thread.set_operation_delivery_status(
                    operation_id,
                    start_sequence,
                    end_sequence,
                    status,
                ),
                None => {
                    agent
                        .thread
                        .set_legacy_delivery_status(start_sequence, end_sequence, status)
                }
            }
            let resulting_sequence = agent.thread.last_sequence();
            for item in &agent.thread.items {
                tx.execute(
                    "UPDATE thread_items SET item = ?3, updated_sequence = ?4 \
                     WHERE agent_id = ?1 AND sequence = ?2",
                    rusqlite::params![
                        conversation_id,
                        i64::try_from(item.sequence()).unwrap_or(i64::MAX),
                        serde_json::to_string(item).expect("a thread item always serializes"),
                        i64::try_from(item.latest_sequence()).unwrap_or(i64::MAX),
                    ],
                )?;
            }
            agent.thread.items.clear();
            tx.execute(
                "UPDATE agents SET record = ?2 WHERE id = ?1",
                rusqlite::params![
                    conversation_id,
                    serde_json::to_string(&agent).expect("an agent always serializes")
                ],
            )?;
            Ok(resulting_sequence)
        })
    }
    /// Boot recovery for provider delivery intents. A queued intent never left
    /// this database and is safe to replay. A durable claim might have crossed
    /// the provider boundary before the process died, so it becomes uncertain
    /// and is intentionally not made replayable again.
    pub fn recover_operations(&self) -> Result<Vec<OperationReceipt>, StoreError> {
        let (operations, interrupted, submitted_legacy) = self.in_transaction(|tx| {
            let now = now_rfc3339();
            let interrupted =
                read_operations_with_statuses(tx, &["claimed", "delivered", "uncertain"])?;
            let submitted_legacy = read_submitted_legacy_messages(tx)?;
            tx.execute(
                "UPDATE operations SET status = 'uncertain', \
                 execution_error = 'provider handoff outcome unknown after restart', \
                 updated_at = ?1 \
                 WHERE status = 'claimed'",
                [&now],
            )?;
            let mut operations = read_recoverable_operations(tx)?;
            for receipt in &mut operations {
                let bounded = receipt
                    .delivery
                    .as_ref()
                    .and_then(|delivery| delivery.payload.as_ref())
                    .is_some();
                if receipt.status == OperationStatus::Queued && !bounded {
                    tx.execute(
                        "UPDATE operations SET status = 'uncertain', \
                         execution_error = 'legacy delivery has no bounded operation payload', \
                         updated_at = ?2 WHERE operation_id = ?1 AND status = 'queued'",
                        rusqlite::params![receipt.operation_id, now],
                    )?;
                    receipt.status = OperationStatus::Uncertain;
                    receipt.execution_error =
                        Some("legacy delivery has no bounded operation payload".to_string());
                }
            }
            Ok((operations, interrupted, submitted_legacy))
        })?;
        for receipt in interrupted {
            if receipt.status == OperationStatus::Delivered && receipt.execution_error.is_some() {
                continue;
            }
            let Some(payload) = receipt
                .delivery
                .as_ref()
                .and_then(|delivery| delivery.payload.as_ref())
            else {
                continue;
            };
            self.set_operation_delivery_status(
                &receipt.conversation_id,
                &receipt.operation_id,
                payload.start_sequence,
                payload.end_sequence,
                MessageDeliveryStatus::Uncertain,
            )?;
        }
        for (conversation_id, sequence) in submitted_legacy {
            self.set_legacy_delivery_status(
                &conversation_id,
                sequence,
                sequence,
                MessageDeliveryStatus::Uncertain,
            )?;
        }
        Ok(operations)
    }
    /// Advance an intent only from the state its caller observed. The compare
    /// in SQL keeps two delivery workers from claiming the same operation.
    pub fn transition_operation(
        &self,
        operation_id: &str,
        expected: OperationStatus,
        next: OperationStatus,
        execution_error: Option<&str>,
    ) -> Result<bool, StoreError> {
        self.in_transaction(|tx| {
            let changed = tx.execute(
                "UPDATE operations SET status = ?3, execution_error = ?4, updated_at = ?5 \
                 WHERE operation_id = ?1 AND status = ?2",
                rusqlite::params![
                    operation_id,
                    expected.as_str(),
                    next.as_str(),
                    execution_error,
                    now_rfc3339()
                ],
            )?;
            Ok(changed == 1)
        })
    }
    /// [`save_task_plan`](Self::save_task_plan) plus the operation receipt in
    /// one commit. Used by `thread.post`, where acknowledging the operation
    /// without its message (or vice versa) would make a retry unsafe.
    pub fn save_task_plan_accepting_operation(
        &self,
        record: &PersistedPlan,
        receipt: &OperationReceipt,
    ) -> Result<OperationReceipt, StoreError> {
        self.in_transaction(|tx| {
            if let Some(existing) = existing_operation_or_conflict(tx, receipt)? {
                return Ok(existing);
            }
            write_task(tx, record)?;
            Store::write_agents(tx, &record.id, &record.agents)?;
            insert_operation(tx, receipt)?;
            Ok(receipt.clone())
        })
    }
    /// Run-owned counterpart of
    /// [`save_task_plan_accepting_operation`](Self::save_task_plan_accepting_operation).
    pub fn save_run_accepting_operation(
        &self,
        record: &PersistedRun,
        receipt: &OperationReceipt,
    ) -> Result<OperationReceipt, StoreError> {
        self.in_transaction(|tx| {
            if let Some(existing) = existing_operation_or_conflict(tx, receipt)? {
                return Ok(existing);
            }
            write_run(tx, record)?;
            Store::write_agents(tx, &record.id, &record.agents)?;
            insert_operation(tx, receipt)?;
            Ok(receipt.clone())
        })
    }
    /// Refuse to start when the imported JSON has been written to since the
    /// import.
    ///
    /// The rollback hazard runs in one direction and is silent in both halves:
    /// an older bridge run against this directory reads the JSON tree, serves
    /// state frozen at the import, and writes its own changes back there — and
    /// then a newer bridge, coming forward again, reads only the database and
    /// never sees any of it. Neither half says anything.
    ///
    /// So the newer one checks. If a JSON record is newer than the note the
    /// import left, something wrote to a store Build stopped reading, and the
    /// honest answer is to stop and say which file rather than to quietly
    /// discard whichever copy is younger.
    pub fn refuse_a_rolled_back_store(&self) -> Result<(), StoreError> {
        let note = self.dir.join(Store::SUPERSEDED_NOTE);
        let Ok(imported_at) = std::fs::metadata(&note).and_then(|meta| meta.modified()) else {
            // No note: nothing was ever imported here, so there is no older
            // store to have been rolled back to.
            return Ok(());
        };
        let mut newer = Vec::new();
        let mut look = |path: PathBuf| {
            if let Ok(modified) = std::fs::metadata(&path).and_then(|meta| meta.modified()) {
                if modified > imported_at {
                    newer.push(path);
                }
            }
        };
        if let Ok(entries) = std::fs::read_dir(self.dir.join(Store::PLANS_DIR)) {
            for entry in entries.flatten() {
                look(entry.path().join("record.json"));
            }
        }
        for dir in ["runs", "captures", "archived-worktrees"] {
            if let Ok(entries) = std::fs::read_dir(self.dir.join(dir)) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if is_json_record(&path) {
                        look(path);
                    }
                }
            }
        }
        look(self.dir.join("attention").join("map.json"));
        match newer.first() {
            None => Ok(()),
            Some(path) => Err(StoreError::RolledBack {
                path: path.clone(),
                count: newer.len(),
            }),
        }
    }
}

/// One page off an already-prepared [`THREAD_PAGE_SQL`] — shared by the load,
/// which pages every conversation of an owner off one statement, and by
/// [`Store::thread_page`], which prepares its own. Turns the seek's
/// newest-first read into the order the conversation happened in.
pub(super) const OPERATION_COLUMNS: &str = "operation_id, method, entity_id, agent_id, \
    conversation_id, choice_revision, request_hash, posted_sequence, status, \
    delivery, execution_error, message_start_sequence, requested_by";

pub(super) fn ensure_operation_receipt_columns(conn: &Connection) -> Result<(), StoreError> {
    let mut columns = conn.prepare("PRAGMA table_info(operations)")?;
    let names = columns
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(columns);
    if !names.iter().any(|name| name == "execution_error") {
        conn.execute("ALTER TABLE operations ADD COLUMN execution_error TEXT", [])?;
    }
    if !names.iter().any(|name| name == "message_start_sequence") {
        conn.execute(
            "ALTER TABLE operations ADD COLUMN message_start_sequence INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    // Every operation written before an agent could ask for one is the human's,
    // which is what a null here reads as.
    if !names.iter().any(|name| name == "requested_by") {
        conn.execute("ALTER TABLE operations ADD COLUMN requested_by TEXT", [])?;
    }
    Ok(())
}

pub(super) fn insert_operation(
    tx: &rusqlite::Transaction,
    receipt: &OperationReceipt,
) -> Result<(), StoreError> {
    let now = now_rfc3339();
    let delivery = receipt
        .delivery
        .as_ref()
        .map(|intent| serde_json::to_string(intent).expect("a delivery intent always serializes"));
    let requested_by = receipt
        .requested_by
        .as_ref()
        .map(|requester| serde_json::to_string(requester).expect("a requester always serializes"));
    tx.execute(
        "INSERT INTO operations \
         (operation_id, method, entity_id, agent_id, conversation_id, choice_revision, \
          request_hash, posted_sequence, message_start_sequence, status, execution_error, delivery, \
          requested_by, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14)",
        rusqlite::params![
            receipt.operation_id,
            receipt.method,
            receipt.entity_id,
            receipt.agent_id,
            receipt.conversation_id,
            i64::try_from(receipt.choice_revision).unwrap_or(i64::MAX),
            receipt.request_hash,
            i64::try_from(receipt.posted_sequence).unwrap_or(i64::MAX),
            i64::try_from(receipt.message_start_sequence).unwrap_or(i64::MAX),
            receipt.status.as_str(),
            receipt.execution_error,
            delivery,
            requested_by,
            now,
        ],
    )?;
    Ok(())
}

pub(super) fn existing_operation_or_conflict(
    conn: &Connection,
    receipt: &OperationReceipt,
) -> Result<Option<OperationReceipt>, StoreError> {
    let Some(existing) = read_operation(conn, &receipt.operation_id)? else {
        return Ok(None);
    };
    if same_operation(&existing, receipt) {
        Ok(Some(existing))
    } else {
        Err(StoreError::OperationConflict {
            operation_id: receipt.operation_id.clone(),
        })
    }
}

pub(super) fn read_operation(
    conn: &Connection,
    operation_id: &str,
) -> Result<Option<OperationReceipt>, StoreError> {
    let sql = format!("SELECT {OPERATION_COLUMNS} FROM operations WHERE operation_id = ?1");
    conn.query_row(&sql, [operation_id], decode_operation_row)
        .optional()
        .map_err(StoreError::from)
}

pub(super) fn read_recoverable_operations(
    conn: &Connection,
) -> Result<Vec<OperationReceipt>, StoreError> {
    let sql = format!(
        "SELECT {OPERATION_COLUMNS} FROM operations \
         WHERE status IN ('queued', 'uncertain') ORDER BY created_at, operation_id"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map([], decode_operation_row)?;
    rows.collect::<Result<_, _>>().map_err(StoreError::from)
}

fn read_operations_with_statuses(
    conn: &Connection,
    statuses: &[&str],
) -> Result<Vec<OperationReceipt>, StoreError> {
    let placeholders = (1..=statuses.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT {OPERATION_COLUMNS} FROM operations \
         WHERE status IN ({placeholders}) \
           AND EXISTS (SELECT 1 FROM agents WHERE agents.id = operations.conversation_id) \
         ORDER BY created_at, operation_id"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(
        rusqlite::params_from_iter(statuses.iter()),
        decode_operation_row,
    )?;
    rows.collect::<Result<_, _>>().map_err(StoreError::from)
}

fn read_submitted_legacy_messages(conn: &Connection) -> Result<Vec<(String, u64)>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT agent_id, sequence FROM thread_items \
         WHERE json_extract(item, '$.type') = 'message' \
           AND json_extract(item, '$.data.role') = 'user' \
           AND json_extract(item, '$.data.operation_id') IS NULL \
           AND json_extract(item, '$.data.delivery_status') = 'submitted' \
           AND EXISTS (SELECT 1 FROM agents WHERE agents.id = thread_items.agent_id) \
         ORDER BY agent_id, sequence",
    )?;
    let rows = statement.query_map([], |row| {
        let conversation_id: String = row.get(0)?;
        let sequence = row.get::<_, i64>(1)?.max(0) as u64;
        Ok((conversation_id, sequence))
    })?;
    rows.collect::<Result<_, _>>().map_err(StoreError::from)
}

pub(super) fn decode_operation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OperationReceipt> {
    let operation_id: String = row.get(0)?;
    let status: String = row.get(8)?;
    let status = match status.as_str() {
        "queued" => OperationStatus::Queued,
        "claimed" => OperationStatus::Claimed,
        "delivered" => OperationStatus::Delivered,
        "uncertain" => OperationStatus::Uncertain,
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                8,
                rusqlite::types::Type::Text,
                format!("invalid operation status {status:?}").into(),
            ))
        }
    };
    let delivery = row
        .get::<_, Option<String>>(9)?
        .map(|raw| {
            serde_json::from_str(&raw).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    9,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .transpose()?;
    Ok(OperationReceipt {
        operation_id,
        method: row.get(1)?,
        entity_id: row.get(2)?,
        agent_id: row.get(3)?,
        conversation_id: row.get(4)?,
        choice_revision: row.get::<_, i64>(5)?.max(0) as u64,
        request_hash: row.get(6)?,
        posted_sequence: row.get::<_, i64>(7)?.max(0) as u64,
        message_start_sequence: {
            let start = row.get::<_, i64>(11)?.max(0) as u64;
            if start == 0 {
                row.get::<_, i64>(7)?.max(0) as u64
            } else {
                start
            }
        },
        status,
        execution_error: row.get(10)?,
        delivery,
        requested_by: row
            .get::<_, Option<String>>(12)?
            .map(|raw| {
                serde_json::from_str(&raw).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        12,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            })
            .transpose()?,
    })
}

pub(super) fn same_operation(left: &OperationReceipt, right: &OperationReceipt) -> bool {
    left.operation_id == right.operation_id
        && left.method == right.method
        && left.entity_id == right.entity_id
        && left.agent_id == right.agent_id
        && left.conversation_id == right.conversation_id
        && left.choice_revision == right.choice_revision
        && left.request_hash == right.request_hash
}
