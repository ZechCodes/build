//! Review metadata and task completion in SQLite. Git refs are pinned by the
//! review service before `save_review_snapshot` commits their identities here.

use super::tracker::{append_activity, write_tracker_task};
use super::{now_rfc3339, Store, StoreError};
use crate::reviews::model::ReviewSnapshot;
use crate::reviews::records::{Review, ReviewCompletion, ReviewState};
use crate::tracker::{Actor, Task, TaskComment, TaskEvent, TaskEventKind, DONE_STATUS};
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

struct CompletionWrite<'a> {
    task: &'a Task,
    comments: &'a [TaskComment],
    events: &'a [TaskEvent],
    actor: &'a Actor,
    expected_version: Option<u64>,
    description: &'a str,
    now: &'a str,
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
    /// Return replaced snapshots so the service releases their pins after commit.
    pub fn save_review_snapshot(
        &self,
        task_id: &str,
        workspace_id: &str,
        expected_version: u64,
        mut snapshot: ReviewSnapshot,
    ) -> Result<(Review, Vec<ReviewSnapshot>), StoreError> {
        self.in_transaction(|tx| {
            require_task(tx, task_id)?;
            let mut header = match load_header(tx, task_id)? {
                Some(header) => {
                    check_version(&header, expected_version)?;
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
            let replaced = if header.workspace_id != workspace_id {
                let previous = load_review(tx, task_id)?.expect("review header exists");
                tx.execute("DELETE FROM review_snapshots WHERE task_id = ?1", [task_id])?;
                header.workspace_id = workspace_id.into();
                previous.snapshots
            } else {
                Vec::new()
            };
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
            Ok((load_review(tx, task_id)?.expect("review was written"), replaced))
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
        self.complete_review_with_events(task_id, expected_version, actor, description)
            .map(|(review, _)| review)
    }

    /// Return the activity written in the completion transaction so the app
    /// can deliver the same post-write notices as an ordinary task move.
    pub fn complete_review_with_events(
        &self,
        task_id: &str,
        expected_version: u64,
        actor: &Actor,
        description: &str,
    ) -> Result<(Review, Vec<TaskEvent>), StoreError> {
        let description = validate_description(description)?;
        self.in_transaction(|tx| {
            let task = require_task(tx, task_id)?;
            let now = now_rfc3339();
            complete_review_in_tx(
                tx,
                CompletionWrite {
                    task: &task,
                    comments: &[],
                    events: &[],
                    actor,
                    expected_version: Some(expected_version),
                    description,
                    now: &now,
                },
            )
        })
    }

    /// A task move to Done completes a review if it is open at the moment of
    /// the write. The check and all task activity share one transaction, so a
    /// snapshot reopening the review cannot slip between them.
    pub fn complete_review_with_task_activity(
        &self,
        task: &Task,
        comments: &[TaskComment],
        events: &[TaskEvent],
        actor: &Actor,
        now: &str,
    ) -> Result<Option<TaskEvent>, StoreError> {
        self.in_transaction(|tx| {
            if load_header(tx, &task.id)?.is_some_and(|review| review.state == ReviewState::Open) {
                return complete_review_in_tx(
                    tx,
                    CompletionWrite {
                        task,
                        comments,
                        events,
                        actor,
                        expected_version: None,
                        description: "Marked done",
                        now,
                    },
                )
                .map(|(_, mut events)| events.pop());
            }
            write_tracker_task(tx, task)?;
            append_activity(tx, comments, events)?;
            Ok(None)
        })
    }

    /// Histories belonging to tasks of one project, for an explicit future
    /// history-deletion flow to release their Git refs before deleting rows.
    pub fn load_reviews_of_project(&self, project_path: &str) -> Result<Vec<Review>, StoreError> {
        load_reviews_of_project(&self.connection(), project_path)
    }
}

pub(super) fn load_reviews_of_project(
    conn: &Connection,
    project_path: &str,
) -> Result<Vec<Review>, StoreError> {
    let mut statement = conn.prepare(
            "SELECT reviews.task_id FROM reviews JOIN tracker_tasks ON tracker_tasks.id = reviews.task_id \
             WHERE tracker_tasks.project_key = ?1 ORDER BY tracker_tasks.number",
        )?;
    let ids = statement
        .query_map([project_path], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    ids.iter()
        .map(|id| {
            load_review(conn, id)?.ok_or_else(|| StoreError::ReviewNotFound {
                task_id: id.clone(),
            })
        })
        .collect()
}

fn complete_review_in_tx(
    tx: &Transaction,
    write: CompletionWrite<'_>,
) -> Result<(Review, Vec<TaskEvent>), StoreError> {
    let task_id = &write.task.id;
    require_task(tx, task_id)?;
    let mut header = load_header(tx, task_id)?.ok_or_else(|| StoreError::ReviewNotFound {
        task_id: task_id.clone(),
    })?;
    if let Some(expected) = write.expected_version {
        check_version(&header, expected)?;
    }
    if header.state == ReviewState::Completed {
        return Err(StoreError::ReviewCompleted {
            task_id: task_id.clone(),
        });
    }
    let latest = latest_snapshot_id(tx, task_id)?;
    header.version += 1;
    header.state = ReviewState::Completed;
    header.completion = Some(ReviewCompletion {
        actor: write.actor.clone(),
        description: write.description.into(),
        completed_at: write.now.into(),
    });
    let mut task = write.task.clone();
    let moved = task.status != DONE_STATUS;
    let from = task.status.clone();
    task.status = DONE_STATUS.into();
    if moved {
        task.done_at = Some(write.now.into());
    }
    task.updated_at = write.now.into();
    let moved_event = moved.then(|| {
        TaskEvent::new(
            task_id,
            write.actor.clone(),
            TaskEventKind::Moved,
            json!({ "from": from, "to": DONE_STATUS }),
            write.now,
        )
    });
    let event = TaskEvent::new(
        task_id,
        write.actor.clone(),
        TaskEventKind::ReviewCompleted,
        json!({
            "workspace_id": header.workspace_id,
            "snapshot_id": latest,
            "description": write.description,
        }),
        write.now,
    );
    write_header(tx, &header)?;
    write_tracker_task(tx, &task)?;
    append_activity(tx, write.comments, write.events)?;
    let mut events = Vec::with_capacity(2);
    if let Some(moved_event) = moved_event {
        events.push(moved_event);
    }
    events.push(event);
    append_activity(tx, &[], &events)?;
    Ok((
        load_review(tx, task_id)?.expect("review was written"),
        events,
    ))
}

fn validate_description(description: &str) -> Result<&str, StoreError> {
    crate::reviews::records::review_description(description)
        .ok_or(StoreError::ReviewDescriptionInvalid)
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
         ON CONFLICT(task_id) DO UPDATE SET workspace_id = ?2, version = ?3, record = ?4",
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
