//! Persist action facts before and after each Git step. No replay on startup.

use super::{check_version, decode, load_header, load_review, write_header};
use crate::reviews::actions::{ActionStatus, ReviewAction, StepStatus};
use crate::reviews::records::Review;
use crate::store::{now_rfc3339, Store, StoreError};
use rusqlite::{params, Connection, Transaction};

impl Store {
    /// Running and previously interrupted review rows whose Build-owned
    /// temporary checkouts may still need cleanup after a later restart.
    pub fn recoverable_review_actions(&self) -> Result<Vec<ReviewAction>, StoreError> {
        Ok(recoverable_actions(&self.connection())?
            .into_iter()
            .map(|(_, action)| action)
            .collect())
    }

    /// Admit all selected sources in one versioned write. A running row holds
    /// its source across the gap between recording Merge and starting Push.
    pub fn start_review_actions(
        &self,
        task_id: &str,
        expected_version: u64,
        actions: &[ReviewAction],
    ) -> Result<Review, StoreError> {
        self.in_transaction(|tx| {
            let mut header = load_header(tx, task_id)?.ok_or_else(|| StoreError::ReviewNotFound {
                task_id: task_id.into(),
            })?;
            check_version(&header, expected_version)?;
            if actions.is_empty() {
                return Ok(load_review(tx, task_id)?.expect("review exists"));
            }
            let held = load_actions(tx, task_id)?;
            for action in actions {
                if held.iter().any(|row| row.status == ActionStatus::Running
                    && (row.directory_id == action.directory_id || row.source_path == action.source_path)) {
                    return Err(StoreError::ReviewAction(format!(
                        "A Git action is already running for {}.", action.source_name
                    )));
                }
                tx.execute("INSERT INTO review_actions (id, task_id, status, record) VALUES (?1, ?2, 'running', ?3)",
                    params![action.id, task_id, serde_json::to_string(action).expect("action serializes")])?;
            }
            header.version += 1;
            write_header(tx, &header)?;
            Ok(load_review(tx, task_id)?.expect("review exists"))
        })
    }

    /// Save the observed outcome even if somebody completed or replaced the
    /// review during Git. Completion never waits for, or erases, these facts.
    pub fn save_review_action(
        &self,
        task_id: &str,
        action: &ReviewAction,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| save_action(tx, task_id, action))
    }

    /// Called once by daemon recovery, never by an ordinary database reader.
    /// There is intentionally no attempt to infer or replay a Git operation.
    pub fn interrupt_review_actions(&self) -> Result<usize, StoreError> {
        self.in_transaction(|tx| {
            let rows = running_actions(tx)?;
            for (task_id, mut action) in rows.iter().cloned() {
                action.status = ActionStatus::Interrupted;
                action.finished_at = Some(now_rfc3339());
                for step in &mut action.steps {
                    if matches!(step.status, StepStatus::Running | StepStatus::Pending) {
                        step.status = StepStatus::Interrupted;
                        step.error = Some("Interrupted: check and retry, or mark complete".into());
                    }
                }
                save_action(tx, &task_id, &action)?;
            }
            Ok(rows.len())
        })
    }
}

pub(super) fn load_actions(
    conn: &Connection,
    task_id: &str,
) -> Result<Vec<ReviewAction>, StoreError> {
    let mut statement =
        conn.prepare("SELECT id, record FROM review_actions WHERE task_id = ?1 ORDER BY rowid")?;
    let rows = statement.query_map([task_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.map(|row| {
        let (id, raw) = row?;
        decode(&raw, "review_actions", &id)
    })
    .collect()
}

fn running_actions(conn: &Connection) -> Result<Vec<(String, ReviewAction)>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT task_id, id, record FROM review_actions WHERE status = 'running' ORDER BY rowid",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    rows.map(|row| {
        let (task_id, id, raw) = row?;
        Ok((task_id, decode(&raw, "review_actions", &id)?))
    })
    .collect()
}

fn recoverable_actions(conn: &Connection) -> Result<Vec<(String, ReviewAction)>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT task_id, id, record FROM review_actions WHERE status IN ('running', 'interrupted') ORDER BY rowid",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    rows.map(|row| {
        let (task_id, id, raw) = row?;
        Ok((task_id, decode(&raw, "review_actions", &id)?))
    })
    .collect()
}

fn save_action(tx: &Transaction, task_id: &str, action: &ReviewAction) -> Result<(), StoreError> {
    let mut header = load_header(tx, task_id)?.ok_or_else(|| StoreError::ReviewNotFound {
        task_id: task_id.into(),
    })?;
    let status = match action.status {
        ActionStatus::Running => "running",
        ActionStatus::Succeeded => "succeeded",
        ActionStatus::Failed => "failed",
        ActionStatus::Interrupted => "interrupted",
    };
    let updated = tx.execute(
        "UPDATE review_actions SET status = ?3, record = ?4 WHERE id = ?1 AND task_id = ?2",
        params![
            action.id,
            task_id,
            status,
            serde_json::to_string(action).expect("action serializes")
        ],
    )?;
    if updated != 1 {
        return Err(StoreError::ReviewAction(format!(
            "unknown review action: {}",
            action.id
        )));
    }
    header.version += 1;
    write_header(tx, &header)
}

#[cfg(test)]
mod tests;
