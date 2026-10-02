//! Review metadata and task completion in SQLite. Git refs are pinned by the
//! review service before `save_review_snapshot` commits their identities here.

use super::{now_rfc3339, Store, StoreError};
use crate::reviews::model::ReviewSnapshot;
use crate::reviews::records::{Review, ReviewCompletion, ReviewState};
use crate::tracker::{Actor, Task, TaskEvent, TaskEventKind, DONE_STATUS};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize)]
struct ReviewHeader {
    task_id: String,
    workspace_id: String,
    version: u64,
    state: ReviewState,
    completion: Option<ReviewCompletion>,
}

impl Store {
    /// The review on a task, if one was created. Its metadata remains after
    /// completion and ordinary task closure.
    pub fn load_review(&self, task_id: &str) -> Result<Option<Review>, StoreError> {
        load_review(&self.connection(), task_id)
    }

    /// Append one already-captured snapshot. The caller minted its unique ID
    /// before pinning Git refs; this transaction alone assigns its display
    /// number and checks the version. A losing caller can clean only its refs.
    pub fn save_review_snapshot(
        &self,
        task_id: &str,
        workspace_id: &str,
        expected_version: u64,
        mut snapshot: ReviewSnapshot,
    ) -> Result<Review, StoreError> {
        self.in_transaction(|tx| {
            require_task(tx, task_id)?;
            let mut header = match load_header(tx, task_id)? {
                Some(header) => {
                    check_version(&header, expected_version)?;
                    if header.workspace_id != workspace_id {
                        return Err(StoreError::ReviewWorkspaceMismatch {
                            task_id: task_id.into(),
                            existing: header.workspace_id,
                            requested: workspace_id.into(),
                        });
                    }
                    header
                }
                None if expected_version == 0 => ReviewHeader {
                    task_id: task_id.into(),
                    workspace_id: workspace_id.into(),
                    version: 0,
                    state: ReviewState::Open,
                    completion: None,
                },
                None => return Err(version_conflict(task_id, expected_version, 0)),
            };
            if snapshot_exists(tx, &snapshot.id)? {
                return Err(StoreError::ReviewSnapshotExists {
                    snapshot_id: snapshot.id,
                });
            }
            snapshot.number = next_snapshot_number(tx, task_id)?;
            header.version += 1;
            header.state = ReviewState::Open;
            header.completion = None;
            write_header(tx, &header)?;
            tx.execute(
                "INSERT INTO review_snapshots (id, task_id, number, record) VALUES (?1, ?2, ?3, ?4)",
                params![
                    snapshot.id,
                    task_id,
                    snapshot.number as i64,
                    serde_json::to_string(&snapshot).expect("snapshot serializes")
                ],
            )?;
            Ok(load_review(tx, task_id)?.expect("review was written"))
        })
    }

    /// Finish the review and move its task to Done in one metadata commit.
    /// Task closure stays independent; the task remains open unless it was
    /// already explicitly closed.
    pub fn complete_review(
        &self,
        task_id: &str,
        expected_version: u64,
        actor: &Actor,
        description: &str,
    ) -> Result<Review, StoreError> {
        let description = description.trim();
        if description.is_empty() || description.len() > 2000 {
            return Err(StoreError::ReviewDescriptionInvalid);
        }
        self.in_transaction(|tx| {
            let mut header =
                load_header(tx, task_id)?.ok_or_else(|| StoreError::ReviewNotFound {
                    task_id: task_id.into(),
                })?;
            check_version(&header, expected_version)?;
            if header.state == ReviewState::Completed {
                return Err(StoreError::ReviewCompleted {
                    task_id: task_id.into(),
                });
            }
            let mut task = require_task(tx, task_id)?;
            let now = now_rfc3339();
            let latest = latest_snapshot_id(tx, task_id)?;
            header.version += 1;
            header.state = ReviewState::Completed;
            header.completion = Some(ReviewCompletion {
                actor: actor.clone(),
                description: description.into(),
                completed_at: now.clone(),
            });
            task.status = DONE_STATUS.into();
            task.done_at = Some(now.clone());
            task.updated_at = now.clone();
            let event = TaskEvent::new(
                task_id,
                actor.clone(),
                TaskEventKind::ReviewCompleted,
                json!({
                    "workspace_id": header.workspace_id,
                    "snapshot_id": latest,
                    "description": description,
                }),
                &now,
            );
            write_header(tx, &header)?;
            tx.execute(
                "UPDATE tracker_tasks SET status = ?2, updated_at = ?3, record = ?4 WHERE id = ?1",
                params![
                    task_id,
                    task.status,
                    now,
                    serde_json::to_string(&task).expect("task serializes")
                ],
            )?;
            tx.execute(
                "INSERT INTO tracker_events (id, task_id, at, record) VALUES (?1, ?2, ?3, ?4)",
                params![
                    event.id,
                    task_id,
                    now,
                    serde_json::to_string(&event).expect("event serializes")
                ],
            )?;
            Ok(load_review(tx, task_id)?.expect("review was written"))
        })
    }

    /// Histories belonging to tasks of one project, for an explicit future
    /// history-deletion flow to release their Git refs before deleting rows.
    pub fn load_reviews_of_project(&self, project_path: &str) -> Result<Vec<Review>, StoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare(
            "SELECT reviews.task_id FROM reviews JOIN tracker_tasks ON tracker_tasks.id = reviews.task_id \
             WHERE tracker_tasks.project_key = ?1 ORDER BY tracker_tasks.number",
        )?;
        let ids = statement
            .query_map([project_path], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids.iter()
            .map(|id| load_review(&conn, id).map(Option::unwrap))
            .collect()
    }
}

fn load_review(conn: &Connection, task_id: &str) -> Result<Option<Review>, StoreError> {
    let Some(header) = load_header(conn, task_id)? else {
        return Ok(None);
    };
    let mut statement =
        conn.prepare("SELECT id, record FROM review_snapshots WHERE task_id = ?1 ORDER BY number")?;
    let snapshots = statement
        .query_map([task_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .map(|row| {
            let (id, raw) = row?;
            decode(&raw, "review_snapshots", &id)
        })
        .collect::<Result<Vec<ReviewSnapshot>, StoreError>>()?;
    Ok(Some(Review {
        task_id: header.task_id,
        workspace_id: header.workspace_id,
        version: header.version,
        state: header.state,
        snapshots,
        completion: header.completion,
    }))
}

fn load_header(conn: &Connection, task_id: &str) -> Result<Option<ReviewHeader>, StoreError> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT record FROM reviews WHERE task_id = ?1",
            [task_id],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| decode(&raw, "reviews", task_id)).transpose()
}

fn write_header(tx: &Transaction, header: &ReviewHeader) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO reviews (task_id, workspace_id, version, record) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(task_id) DO UPDATE SET version = ?3, record = ?4",
        params![
            header.task_id,
            header.workspace_id,
            header.version as i64,
            serde_json::to_string(header).expect("review header serializes")
        ],
    )?;
    Ok(())
}

fn require_task(tx: &Transaction, task_id: &str) -> Result<Task, StoreError> {
    let raw: Option<String> = tx
        .query_row(
            "SELECT record FROM tracker_tasks WHERE id = ?1",
            [task_id],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| decode(&raw, "tracker_tasks", task_id))
        .transpose()?
        .ok_or_else(|| StoreError::ReviewTaskNotFound {
            task_id: task_id.into(),
        })
}

fn check_version(header: &ReviewHeader, expected: u64) -> Result<(), StoreError> {
    if header.version != expected {
        return Err(version_conflict(&header.task_id, expected, header.version));
    }
    Ok(())
}

fn version_conflict(task_id: &str, expected: u64, found: u64) -> StoreError {
    StoreError::ReviewVersionConflict {
        task_id: task_id.into(),
        expected,
        found,
    }
}

fn snapshot_exists(tx: &Transaction, snapshot_id: &str) -> Result<bool, StoreError> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM review_snapshots WHERE id = ?1)",
        [snapshot_id],
        |row| row.get(0),
    )?)
}

fn next_snapshot_number(tx: &Transaction, task_id: &str) -> Result<u64, StoreError> {
    let number: i64 = tx.query_row(
        "SELECT COALESCE(MAX(number), 0) + 1 FROM review_snapshots WHERE task_id = ?1",
        [task_id],
        |row| row.get(0),
    )?;
    Ok(number as u64)
}

fn latest_snapshot_id(tx: &Transaction, task_id: &str) -> Result<Option<String>, StoreError> {
    Ok(tx
        .query_row(
            "SELECT id FROM review_snapshots WHERE task_id = ?1 ORDER BY number DESC LIMIT 1",
            [task_id],
            |row| row.get(0),
        )
        .optional()?)
}

fn decode<T: serde::de::DeserializeOwned>(
    raw: &str,
    table: &str,
    id: &str,
) -> Result<T, StoreError> {
    serde_json::from_str(raw).map_err(|source| StoreError::Corrupt {
        path: PathBuf::from(format!("{table}/{id}")),
        source,
    })
}
