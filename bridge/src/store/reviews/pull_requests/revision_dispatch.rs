//! New revisions journal their reviewer hand-off in the publication transaction.
use super::*;
use crate::store::now_rfc3339;
use crate::store::tracker::{append_activity, write_tracker_task};
use crate::tracker::{Actor, Assignee, TaskComment};

pub(crate) struct RevisionDispatch {
    pub task_id: String,
    pub snapshot_id: String,
    pub number: u64,
}

pub(super) fn enqueue(
    tx: &Transaction,
    task_id: &str,
    snapshot: &ReviewSnapshot,
    previous_snapshot_id: &str,
) -> Result<(), StoreError> {
    let mut task = require_task(tx, task_id)?;
    match task.assignee {
        Some(Assignee::User) => {
            let now = now_rfc3339();
            let body = crate::templates::fill(
                crate::templates::MESSAGE_REVIEW_REVISION_USER,
                &[
                    ("number", &snapshot.number.to_string()),
                    ("previous_number", &(snapshot.number - 1).to_string()),
                ],
            );
            let comment = TaskComment {
                review_notice: Some(crate::tracker::ReviewNoticeKind::RevisionReady),
                id: crate::tracker::new_comment_id(),
                task_id: task.id.clone(),
                author: Actor::Build,
                body,
                anchor: None,
                reply_to: None,
                opinion: None,
                mentions_user: true,
                notifies_user: false,
                refs: Vec::new(),
                attachments: Vec::new(),
                created_at: now.clone(),
                author_context: None,
            };
            task.set_watched(true);
            task.dismissed_through = None;
            task.updated_at = now;
            write_tracker_task(tx, &task)?;
            append_activity(tx, &[comment], &[])
        }
        Some(Assignee::Agent { .. } | Assignee::ProjectAgent) => {
            tx.execute("INSERT INTO review_revision_dispatches(task_id,snapshot_id,previous_snapshot_id,number) VALUES (?1,?2,?3,?4)", params![task_id,snapshot.id,previous_snapshot_id,snapshot.number as i64])?;
            Ok(())
        }
        None => Ok(()),
    }
}

impl Store {
    pub(crate) fn pending_review_revision_dispatches(
        &self,
    ) -> Result<Vec<RevisionDispatch>, StoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare("SELECT task_id,snapshot_id,number FROM review_revision_dispatches WHERE state='pending' ORDER BY updated_at,snapshot_id LIMIT 32")?;
        let rows = statement.query_map([], |row| {
            Ok(RevisionDispatch {
                task_id: row.get(0)?,
                snapshot_id: row.get(1)?,
                number: row.get::<_, i64>(2)? as u64,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// A comparison baseline belongs to the recipient who actually received it.
    /// Coalesced/failed attempts never advance it; a new reviewer gets full range.
    pub(crate) fn review_revision_baseline(
        &self,
        task_id: &str,
        recipient_agent_id: &str,
        number: u64,
    ) -> Result<Option<(String, u64)>, StoreError> {
        let conn = self.connection();
        let delivered = conn.query_row(
            "SELECT snapshot_id,number FROM review_revision_dispatches WHERE task_id=?1 AND recipient_agent_id=?2 AND state='delivered' AND number<?3 ORDER BY number DESC LIMIT 1",
            params![task_id,recipient_agent_id,number as i64],
            |row| Ok((row.get(0)?, row.get::<_,i64>(1)? as u64)),
        ).optional()?;
        if delivered.is_some() {
            return Ok(delivered);
        }
        let opening = ReadTransaction::new(&conn).operation(&format!("review-open-{task_id}"))?;
        if !opening.is_some_and(|operation| operation.delivery_target().1 == recipient_agent_id) {
            return Ok(None);
        }
        conn.query_row(
            "SELECT id,number FROM review_snapshots WHERE task_id=?1 ORDER BY number LIMIT 1",
            [task_id],
            |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as u64)),
        )
        .optional()
        .map_err(StoreError::from)
    }

    pub(crate) fn settle_review_revision_dispatch(
        &self,
        snapshot_id: &str,
        recipient_agent_id: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), StoreError> {
        let state = match (recipient_agent_id, error) {
            (_, Some(_)) => "failed",
            (Some(_), None) => "delivered",
            (None, None) => "superseded",
        };
        self.in_transaction(|tx| {
            tx.execute("UPDATE review_revision_dispatches SET state=?2,recipient_agent_id=?3,error=?4,updated_at=?5 WHERE snapshot_id=?1 AND state='pending'", params![snapshot_id,state,recipient_agent_id,error,now_rfc3339()])?;
            Ok(())
        })
    }

    #[cfg(test)]
    pub(crate) fn review_revision_dispatch_result(
        &self,
        snapshot_id: &str,
    ) -> Result<(String, Option<String>), StoreError> {
        self.connection()
            .query_row(
                "SELECT state,error FROM review_revision_dispatches WHERE snapshot_id=?1",
                [snapshot_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(StoreError::from)
    }
}
