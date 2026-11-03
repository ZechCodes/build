//! New revisions journal their reviewer hand-off in the publication transaction.
use super::*;
use crate::store::now_rfc3339;
use crate::store::tracker::{append_activity, write_tracker_task};
use crate::tracker::{Actor, Assignee, TaskComment};

pub(crate) struct RevisionDispatch {
    pub task_id: String,
    pub snapshot_id: String,
    pub previous_snapshot_id: String,
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
        let mut statement = conn.prepare("SELECT task_id,snapshot_id,previous_snapshot_id,number FROM review_revision_dispatches WHERE state='pending' ORDER BY updated_at,snapshot_id LIMIT 32")?;
        let rows = statement.query_map([], |row| {
            Ok(RevisionDispatch {
                task_id: row.get(0)?,
                snapshot_id: row.get(1)?,
                previous_snapshot_id: row.get(2)?,
                number: row.get::<_, i64>(3)? as u64,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub(crate) fn settle_review_revision_dispatch(
        &self,
        snapshot_id: &str,
        error: Option<&str>,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            tx.execute("UPDATE review_revision_dispatches SET state=?2,error=?3,updated_at=?4 WHERE snapshot_id=?1 AND state='pending'", params![snapshot_id,if error.is_some() { "pending" } else { "delivered" },error,now_rfc3339()])?;
            Ok(())
        })
    }
}
