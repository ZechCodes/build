//! Same-project dependency graph and durable lifecycle reconciliation (W1).
//! No assignment or delivery authority is inferred from task reads.

mod graph;
mod reads;
mod reconciliation;

use super::{Store, StoreError};
use crate::tracker::{Actor, Task, TaskComment, TaskEvent};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

pub(super) use reconciliation::{note_activity, note_task_write};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyDigest {
    pub task_id: String,
    pub number: u64,
    pub title: String,
    pub state: crate::tracker::TaskState,
    pub status: String,
    pub resolved: bool,
    pub resolved_by: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencySummary {
    pub blocked: bool,
    pub waiting_on: Vec<DependencyDigest>,
    pub blocks_count: u64,
}

#[derive(Debug, Default)]
pub struct DependentsPage {
    pub blocks: Vec<DependencyDigest>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Default)]
pub struct WaitingOnPage {
    pub transitions: Vec<TaskEvent>,
    pub pending: bool,
}

impl Store {
    /// Create plus every edge and event commits as one write. The original
    /// create remains the zero-edge migration/default path.
    pub fn create_tracker_task_waiting_on(
        &self,
        draft: Task,
        events: &[TaskEvent],
        waiting_on: &[String],
    ) -> Result<Task, StoreError> {
        self.in_transaction(|tx| {
            let number = super::tracker::next_task_number(tx, &draft.project_path)?;
            let task = Task { number, ..draft };
            super::tracker::write_tracker_task(tx, &task)?;
            super::tracker::append_activity(tx, &[], events)?;
            graph::edit(
                tx,
                &task,
                waiting_on,
                &[],
                &task.created_by,
                &task.created_at,
            )?;
            Ok(task)
        })
    }

    pub fn edit_waiting_on(
        &self,
        project: &str,
        task_id: &str,
        add: &[String],
        remove: &[String],
        actor: &Actor,
        now: &str,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            let task = require_task(tx, project, task_id)?;
            graph::edit(tx, &task, add, remove, actor, now)
        })
    }

    /// W3 can combine ordinary links/fields and dependency edits without a
    /// partial task or timeline write on graph refusal.
    pub fn save_tracker_task_waiting_on(
        &self,
        task: &Task,
        comments: &[TaskComment],
        events: &[TaskEvent],
        add: &[String],
        remove: &[String],
        actor: &Actor,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            require_task(tx, &task.project_path, &task.id)?;
            super::reviews::lifecycle::save_activity_in_tx(
                tx,
                task,
                comments,
                events,
                actor,
                None,
                &task.updated_at,
            )?;
            let current = require_task(tx, &task.project_path, &task.id)?;
            graph::edit(tx, &current, add, remove, actor, &task.updated_at)
        })
    }

    /// One bounded fan-out page, including its cursor and transition receipts.
    /// Repeating after restart never repeats a transition already committed.
    pub fn reconcile_waiting_on(&self, limit: usize) -> Result<WaitingOnPage, StoreError> {
        self.in_transaction(|tx| reconciliation::page(tx, limit.clamp(1, 100)))
    }

    /// Browser invalidations are separate from the durable transition outbox
    /// W2 will consume. The service calls this only after committing a page.
    pub fn take_waiting_on_changes(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String)>, StoreError> {
        self.in_transaction(|tx| {
            let mut statement = tx.prepare(
                "SELECT t.project_key, t.id FROM tracker_dependency_changes c
                 JOIN tracker_tasks t ON t.id = c.task_id ORDER BY t.id LIMIT ?1",
            )?;
            let changes: Vec<(String, String)> = statement
                .query_map([limit.clamp(1, 100) as i64], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?;
            for (_, id) in &changes {
                tx.execute(
                    "DELETE FROM tracker_dependency_changes WHERE task_id = ?1",
                    [id],
                )?;
            }
            Ok(changes)
        })
    }
}

fn invalid(message: &str) -> StoreError {
    StoreError::WaitingOnInvalid(message.into())
}

fn require_task(conn: &Connection, project: &str, id: &str) -> Result<Task, StoreError> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT record FROM tracker_tasks WHERE id = ?1 AND project_key = ?2",
            rusqlite::params![id, project],
            |r| r.get(0),
        )
        .optional()?;
    super::tracker::decode(
        &raw.ok_or_else(|| invalid("task not found in project"))?,
        "tracker_tasks",
        id,
    )
}

fn changed(conn: &Connection, id: &str) -> Result<(), StoreError> {
    conn.execute(
        "INSERT OR IGNORE INTO tracker_dependency_changes(task_id) VALUES (?1)",
        [id],
    )?;
    Ok(())
}
