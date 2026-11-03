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

pub(crate) struct RevisionFailureNotice {
    pub task_id: String,
    pub number: u64,
    pub snapshot_id: String,
    pub comment_id: String,
    pub reviewer: String,
    pub error: String,
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
            let reviewer = match &task.assignee {
                Some(Assignee::Agent { agent_id }) => format!("@agent:{agent_id}"),
                _ => "the project coordinator".into(),
            };
            tx.execute("INSERT INTO review_revision_dispatches(task_id,snapshot_id,previous_snapshot_id,number,reviewer) VALUES (?1,?2,?3,?4,?5)", params![task_id,snapshot.id,previous_snapshot_id,snapshot.number as i64,reviewer])?;
            Ok(())
        }
        None => Ok(()),
    }
}

impl Store {
    #[cfg(test)]
    pub(crate) fn pending_review_revision_dispatches(
        &self,
    ) -> Result<Vec<RevisionDispatch>, StoreError> {
        self.pending_review_revision_dispatches_at(crate::agent::now_ms() as i64)
    }

    pub(crate) fn pending_review_revision_dispatches_at(
        &self,
        now_ms: i64,
    ) -> Result<Vec<RevisionDispatch>, StoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare("SELECT task_id,snapshot_id,number FROM review_revision_dispatches WHERE state='pending' AND retry_after_ms<=?1 ORDER BY retry_after_ms,updated_at,snapshot_id LIMIT 32")?;
        let rows = statement.query_map([now_ms], |row| {
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

    #[cfg(test)]
    pub(crate) fn settle_review_revision_dispatch(
        &self,
        snapshot_id: &str,
        recipient: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), StoreError> {
        self.settle_review_revision_dispatch_at(
            snapshot_id,
            recipient,
            error,
            false,
            crate::agent::now_ms() as i64,
        )
    }

    pub(crate) fn settle_review_revision_dispatch_at(
        &self,
        snapshot_id: &str,
        recipient: Option<&str>,
        error: Option<&str>,
        transient: bool,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            let current: Option<(String,i64)> = tx.query_row("SELECT state,attempts FROM review_revision_dispatches WHERE snapshot_id=?1",[snapshot_id], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
            let Some((_, attempts)) = current.filter(|(state,_)| state == "pending") else { return Ok(()); };
            let attempts = attempts + i64::from(recipient.is_some() || error.is_some());
            let retry = error.is_some() && transient && attempts < 7;
            let state = if retry { "pending" } else if error.is_some() { "failed" } else if recipient.is_some() { "delivered" } else { "superseded" };
            let delay = 30_000_i64.saturating_mul(1_i64 << attempts.saturating_sub(1).clamp(0, 5));
            let retry_after = if retry { now_ms.saturating_add(delay.min(600_000)) } else { 0 };
            let comment_id = (state == "failed").then(crate::tracker::new_comment_id);
            tx.execute("UPDATE review_revision_dispatches SET state=?2,recipient_agent_id=COALESCE(?3,recipient_agent_id),error=?4,updated_at=?5,attempts=?6,retry_after_ms=?7,failure_comment_id=COALESCE(failure_comment_id,?8) WHERE snapshot_id=?1 AND state='pending'", params![snapshot_id,state,recipient,error,now_rfc3339(),attempts,retry_after,comment_id])?;
            Ok(())
        })
    }

    pub(crate) fn failed_review_revision_notices(
        &self,
    ) -> Result<Vec<RevisionFailureNotice>, StoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare("SELECT task_id,number,snapshot_id,failure_comment_id,COALESCE('@agent:' || recipient_agent_id,reviewer),error FROM review_revision_dispatches WHERE state='failed' AND failure_notice_posted=0 AND failure_comment_id IS NOT NULL ORDER BY updated_at,snapshot_id LIMIT 32")?;
        let rows = statement.query_map([], |row| {
            Ok(RevisionFailureNotice {
                task_id: row.get(0)?,
                number: row.get::<_, i64>(1)? as u64,
                snapshot_id: row.get(2)?,
                comment_id: row.get(3)?,
                reviewer: row.get(4)?,
                error: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub(crate) fn finish_review_revision_notice(
        &self,
        snapshot_id: &str,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            tx.execute("UPDATE review_revision_dispatches SET failure_notice_posted=1 WHERE snapshot_id=?1 AND failure_notice_posted=0 AND state='failed'",[snapshot_id])?;
            Ok(())
        })
    }

    #[cfg(test)]
    pub(crate) fn review_revision_dispatch_schedule(
        &self,
        snapshot_id: &str,
    ) -> Result<(i64, i64, Option<String>), StoreError> {
        self.connection().query_row("SELECT attempts,retry_after_ms,failure_comment_id FROM review_revision_dispatches WHERE snapshot_id=?1",[snapshot_id],|row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(StoreError::from)
    }

    #[cfg(test)]
    pub(crate) fn revision_dispatch_sql_changes(&self) -> u64 {
        self.connection().total_changes()
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
