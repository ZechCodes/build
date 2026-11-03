//! Author verdict outbox. Acceptance and its immutable review batch share the
//! verdict transaction; delivery state changes never rewrite the batch.

use super::{decode, load_header};
use crate::store::{now_rfc3339, Store, StoreError};
use crate::tracker::{Actor, ReviewCommentAnchor, ReviewVerdict, Task, TaskComment};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdictDeliveryState {
    Pending,
    Delivered,
    Failed,
}

impl ReviewVerdictDeliveryState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivered => "delivered",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewVerdictBatchComment {
    pub comment_id: String,
    pub body: String,
    pub anchor: ReviewCommentAnchor,
}

/// Public delivery state keeps anchor identities without repeating comment text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewVerdictBatchAnchor {
    pub comment_id: String,
    pub anchor: ReviewCommentAnchor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewVerdictDeliverySummary {
    pub comment_id: String,
    pub snapshot_id: String,
    pub revision: u64,
    pub verdict: ReviewVerdict,
    pub batch: Vec<ReviewVerdictBatchAnchor>,
    pub state: ReviewVerdictDeliveryState,
    pub recipient_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_comment_id: Option<String>,
    pub attempts: u64,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewVerdictRetryContext {
    pub current_status: crate::reviews::model::PullRequestStatus,
    pub current_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewVerdictDelivery {
    pub comment_id: String,
    pub task_id: String,
    pub task_number: u64,
    pub project_path: String,
    pub workspace_id: String,
    pub snapshot_id: String,
    pub revision: u64,
    pub verdict: ReviewVerdict,
    pub reason: String,
    pub reviewer: Actor,
    pub batch: Vec<ReviewVerdictBatchComment>,
    pub operation_id: String,
    pub state: ReviewVerdictDeliveryState,
    pub recipient_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_comment_id: Option<String>,
    /// A superseded retry reports current context without repeating stale instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_context: Option<ReviewVerdictRetryContext>,
    /// Safe redirects and superseded retries use a fresh operation identity.
    pub attempts: u64,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl Store {
    #[cfg(test)]
    pub(crate) fn review_verdict_total_changes_for_test(&self) -> u64 {
        self.connection().total_changes()
    }

    pub(crate) fn review_verdict_author(
        &self,
        task_id: &str,
    ) -> Result<Option<String>, StoreError> {
        Ok(load_header(&self.connection(), task_id)?
            .and_then(|header| header.pull_request)
            .and_then(|metadata| metadata.author))
    }
    pub fn pending_review_verdict_deliveries(
        &self,
        limit: usize,
    ) -> Result<Vec<ReviewVerdictDelivery>, StoreError> {
        let conn = self.connection();
        // Receipt-less rows have never been accepted by a conversation. Keep
        // their immutable journal acceptance order even if binding touched an
        // older row before a crash. Already accepted rows rotate separately;
        // a stopped author cannot block another review's first acceptance.
        let mut query = conn.prepare("SELECT d.comment_id,d.record FROM review_verdict_deliveries d LEFT JOIN operations o ON o.operation_id=d.operation_id WHERE d.state='pending' ORDER BY (o.operation_id IS NOT NULL),CASE WHEN o.operation_id IS NULL THEN d.rowid END,d.updated_at,d.rowid LIMIT ?1")?;
        let records = query.query_map([limit.min(100) as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        records
            .map(|row| {
                let (id, raw) = row?;
                decode(&raw, "review_verdict_deliveries", &id)
            })
            .collect()
    }

    /// First acceptance is FIFO. Existing receipts are polled in a separate
    /// rotating page, so fairness requires no writes to delivery state.
    pub(crate) fn review_verdict_delivery_page(
        &self,
        limit: usize,
        after: i64,
    ) -> Result<(Vec<ReviewVerdictDelivery>, i64), StoreError> {
        let conn = self.connection();
        let mut fresh = conn.prepare("SELECT d.comment_id,d.record FROM review_verdict_deliveries d LEFT JOIN operations o ON o.operation_id=d.operation_id WHERE d.state='pending' AND o.operation_id IS NULL ORDER BY d.rowid LIMIT ?1")?;
        let mut deliveries = fresh
            .query_map([limit.min(100) as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .map(|row| {
                let (id, raw) = row?;
                decode(&raw, "review_verdict_deliveries", &id)
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        let mut polled = conn.prepare("SELECT d.rowid,d.comment_id,d.record FROM review_verdict_deliveries d JOIN operations o ON o.operation_id=d.operation_id WHERE d.state='pending' ORDER BY (d.rowid <= ?1),d.rowid LIMIT ?2")?;
        let rows = polled.query_map(params![after, limit.min(100) as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut next = after;
        for row in rows {
            let (sequence, id, raw) = row?;
            deliveries.push(decode(&raw, "review_verdict_deliveries", &id)?);
            next = sequence;
        }
        Ok((deliveries, next))
    }

    pub fn load_review_verdict_delivery(
        &self,
        comment_id: &str,
    ) -> Result<Option<ReviewVerdictDelivery>, StoreError> {
        load_delivery(&self.connection(), comment_id)
    }

    pub fn latest_review_verdict_delivery(
        &self,
        task_id: &str,
    ) -> Result<Option<ReviewVerdictDelivery>, StoreError> {
        let conn = self.connection();
        let id: Option<String> = conn.query_row("SELECT comment_id FROM review_verdict_deliveries WHERE task_id=?1 ORDER BY rowid DESC LIMIT 1", [task_id], |row| row.get(0)).optional()?;
        id.map(|id| load_delivery(&conn, &id))
            .transpose()
            .map(Option::flatten)
    }

    /// Record a recipient before accepting a message. The delivery service
    /// checks the operation receipt before rebinding; once accepted, that
    /// receipt remains the authority for the original target.
    pub fn bind_review_verdict_delivery(
        &self,
        comment_id: &str,
        agent_id: &str,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            let mut delivery = load_delivery(tx, comment_id)?
                .ok_or_else(|| invalid("unknown verdict delivery"))?;
            if delivery.state != ReviewVerdictDeliveryState::Pending
                || delivery.recipient_agent_id.as_deref() == Some(agent_id)
            {
                return Ok(());
            }
            delivery.recipient_agent_id = Some(agent_id.into());
            delivery.updated_at = now_rfc3339();
            write_delivery(tx, &delivery)?;
            crate::store::task_summaries::metadata_refresh(tx, &delivery.task_id)
        })
    }

    /// Reserve the human-attention comment identity before its own write, so
    /// replay after a crash can find and settle the same fallback comment.
    pub fn reserve_review_verdict_fallback_comment(
        &self,
        comment_id: &str,
    ) -> Result<String, StoreError> {
        self.in_transaction(|tx| {
            let mut delivery = load_delivery(tx, comment_id)?
                .ok_or_else(|| invalid("unknown verdict delivery"))?;
            if let Some(id) = delivery.fallback_comment_id.as_ref() {
                return Ok(id.clone());
            }
            let id = crate::tracker::new_comment_id();
            delivery.fallback_comment_id = Some(id.clone());
            delivery.updated_at = now_rfc3339();
            write_delivery(tx, &delivery)?;
            Ok(id)
        })
    }

    pub fn finish_review_verdict_delivery(
        &self,
        comment_id: &str,
        recipient: Option<&str>,
    ) -> Result<(), StoreError> {
        self.update_verdict_delivery(comment_id, |delivery| {
            if delivery.state == ReviewVerdictDeliveryState::Pending {
                delivery.state = ReviewVerdictDeliveryState::Delivered;
                delivery.recipient_agent_id = recipient.map(str::to_owned);
                delivery.error = None;
            }
            Ok(())
        })
    }

    pub fn fail_review_verdict_delivery(
        &self,
        comment_id: &str,
        error: &str,
    ) -> Result<(), StoreError> {
        self.update_verdict_delivery(comment_id, |delivery| {
            if delivery.state == ReviewVerdictDeliveryState::Pending {
                delivery.state = ReviewVerdictDeliveryState::Failed;
                delivery.error = Some(error.into());
            }
            Ok(())
        })
    }

    pub fn retry_review_verdict_delivery(
        &self,
        task_id: &str,
        comment_id: &str,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            let mut delivery = load_delivery(tx, comment_id)?
                .ok_or_else(|| invalid("unknown verdict delivery"))?;
            if delivery.task_id != task_id {
                return Err(invalid("verdict delivery belongs to another task"));
            }
            if delivery.state != ReviewVerdictDeliveryState::Failed {
                return Err(invalid("only failed verdict deliveries may be retried"));
            }
            delivery.attempts += 1;
            delivery.retry_context = superseded_retry_context(tx, &delivery)?;
            if delivery.retry_context.is_some() {
                retire_superseded_retry(tx, &delivery.operation_id)?;
                delivery.operation_id = operation_id(comment_id, delivery.attempts);
                delivery.recipient_agent_id = None;
            }
            delivery.state = ReviewVerdictDeliveryState::Pending;
            delivery.error = None;
            delivery.updated_at = now_rfc3339();
            write_delivery(tx, &delivery)?;
            crate::store::task_summaries::metadata_refresh(tx, &delivery.task_id)
        })
    }

    pub(crate) fn current_review_verdict_retry_context(
        &self,
        task: &str,
    ) -> Result<ReviewVerdictRetryContext, StoreError> {
        current_retry_context(&self.connection(), task)
    }

    /// Retire an old target only when its durable operation proves the
    /// message was never handed off. Claimed or uncertain work cannot move.
    pub fn redirect_review_verdict_delivery(
        &self,
        comment_id: &str,
        expected_operation_id: &str,
    ) -> Result<bool, StoreError> {
        self.in_transaction(|tx| {
            let Some(mut delivery) = load_delivery(tx, comment_id)? else { return Ok(false); };
            if delivery.state != ReviewVerdictDeliveryState::Pending || delivery.operation_id != expected_operation_id { return Ok(false); }
            let receipt: Option<(String, Option<String>)> = tx.query_row("SELECT status,execution_error FROM operations WHERE operation_id=?1", [expected_operation_id], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
            match receipt {
                Some((status, _)) if status == "queued" => {
                    let changed = tx.execute("UPDATE operations SET status='delivered',execution_error='Recipient removed before verdict handoff',updated_at=?2 WHERE operation_id=?1 AND status='queued'", params![expected_operation_id,now_rfc3339()])?;
                    if changed == 0 { return Ok(false); }
                }
                Some((status, Some(_))) if status == "delivered" => {}
                _ => return Ok(false),
            }
            delivery.attempts += 1;
            delivery.operation_id = operation_id(comment_id, delivery.attempts);
            delivery.recipient_agent_id = None;
            delivery.updated_at = now_rfc3339();
            write_delivery(tx, &delivery)?;
            crate::store::task_summaries::metadata_refresh(tx, &delivery.task_id)?;
            Ok(true)
        })
    }

    fn update_verdict_delivery(
        &self,
        comment_id: &str,
        update: impl FnOnce(&mut ReviewVerdictDelivery) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            let mut delivery = load_delivery(tx, comment_id)?
                .ok_or_else(|| invalid("unknown verdict delivery"))?;
            update(&mut delivery)?;
            delivery.updated_at = now_rfc3339();
            write_delivery(tx, &delivery)?;
            crate::store::task_summaries::metadata_refresh(tx, &delivery.task_id)
        })
    }
}

pub(crate) fn enqueue_verdict_deliveries(
    tx: &Transaction,
    task: &Task,
    comments: &[TaskComment],
) -> Result<(), StoreError> {
    for comment in comments {
        let Some(opinion) = &comment.opinion else {
            continue;
        };
        if !crate::reviews::lifecycle::is_deciding_reviewer(&comment.author, task.assignee.as_ref())
        {
            continue;
        }
        if load_delivery(tx, &comment.id)?.is_some() {
            continue;
        }
        let Some(header) = load_header(tx, &task.id)? else {
            continue;
        };
        let Some(metadata) = header.pull_request else {
            continue;
        };
        if metadata.latest_published_snapshot_id.as_deref() != Some(&opinion.snapshot_id) {
            continue;
        }
        let revision: i64 = tx.query_row(
            "SELECT number FROM review_snapshots WHERE task_id=?1 AND id=?2",
            params![task.id, opinion.snapshot_id],
            |row| row.get(0),
        )?;
        let delivery = ReviewVerdictDelivery {
            comment_id: comment.id.clone(),
            task_id: task.id.clone(),
            task_number: task.number,
            project_path: task.project_path.clone(),
            workspace_id: header.workspace_id,
            snapshot_id: opinion.snapshot_id.clone(),
            revision: revision as u64,
            verdict: opinion.verdict,
            reason: comment.body.clone(),
            reviewer: comment.author.clone(),
            batch: batch_comments(tx, comment)?,
            operation_id: operation_id(&comment.id, 0),
            state: ReviewVerdictDeliveryState::Pending,
            recipient_agent_id: None,
            fallback_comment_id: None,
            retry_context: None,
            attempts: 0,
            error: None,
            created_at: comment.created_at.clone(),
            updated_at: comment.created_at.clone(),
        };
        write_delivery(tx, &delivery)?;
    }
    Ok(())
}

fn current_retry_context(
    conn: &Connection,
    task: &str,
) -> Result<ReviewVerdictRetryContext, StoreError> {
    let metadata = load_header(conn, task)?
        .and_then(|header| header.pull_request)
        .ok_or_else(|| invalid("retry requires a published PR"))?;
    let snapshot = metadata
        .latest_published_snapshot_id
        .as_deref()
        .ok_or_else(|| invalid("PR has no current revision"))?;
    let revision: i64 = conn.query_row(
        "SELECT number FROM review_snapshots WHERE task_id=?1 AND id=?2",
        params![task, snapshot],
        |row| row.get(0),
    )?;
    Ok(ReviewVerdictRetryContext {
        current_status: metadata.status,
        current_revision: revision as u64,
    })
}

fn superseded_retry_context(
    conn: &Connection,
    delivery: &ReviewVerdictDelivery,
) -> Result<Option<ReviewVerdictRetryContext>, StoreError> {
    use crate::reviews::model::PullRequestStatus;
    let context = current_retry_context(conn, &delivery.task_id)?;
    let newer:bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM review_verdict_deliveries newer WHERE newer.task_id=?1 AND newer.rowid>(SELECT rowid FROM review_verdict_deliveries WHERE comment_id=?2))",params![delivery.task_id,delivery.comment_id],|row|row.get(0))?;
    let original_status = match delivery.verdict {
        ReviewVerdict::Approve => PullRequestStatus::Approved,
        ReviewVerdict::RequestChanges => PullRequestStatus::ChangesRequested,
        ReviewVerdict::Reject => PullRequestStatus::Rejected,
    };
    Ok((newer
        || context.current_revision != delivery.revision
        || context.current_status != original_status)
        .then_some(context))
}

// Uncertain receipts remain audit evidence of an unknown provider outcome.
// They are never requeued here; the outbox points only to the fresh context
// attempt. Marking them Delivered would falsely claim that outcome is known.
fn retire_superseded_retry(tx: &Transaction, operation: &str) -> Result<(), StoreError> {
    let claimed: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM operations WHERE operation_id=?1 AND status='claimed')",
        [operation],
        |row| row.get(0),
    )?;
    if claimed {
        return Err(invalid("a claimed verdict operation cannot be replaced"));
    }
    tx.execute("UPDATE operations SET status='delivered',execution_error='Superseded verdict retried as context only',updated_at=?2 WHERE operation_id=?1 AND status='queued'",params![operation,now_rfc3339()])?;
    Ok(())
}

fn batch_comments(
    tx: &Transaction,
    verdict: &TaskComment,
) -> Result<Vec<ReviewVerdictBatchComment>, StoreError> {
    let snapshot_id = &verdict
        .opinion
        .as_ref()
        .expect("verdict checked")
        .snapshot_id;
    let mut query = tx.prepare("SELECT id,record FROM tracker_comments WHERE task_id=?1 AND (created_at,id) <= (?2,?3) ORDER BY created_at,id")?;
    let records = query.query_map(
        params![verdict.task_id, verdict.created_at, verdict.id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    let mut batch = Vec::new();
    for record in records {
        let (id, raw) = record?;
        let comment: TaskComment = decode(&raw, "tracker_comments", &id)?;
        if comment.author != verdict.author {
            continue;
        }
        if id != verdict.id
            && comment
                .opinion
                .as_ref()
                .is_some_and(|opinion| &opinion.snapshot_id == snapshot_id)
        {
            batch.clear();
            continue;
        }
        if let Some(anchor) = comment
            .anchor
            .filter(|anchor| &anchor.snapshot_id == snapshot_id)
        {
            batch.push(ReviewVerdictBatchComment {
                comment_id: id,
                body: comment.body,
                anchor: *anchor,
            });
        }
    }
    Ok(batch)
}

pub(super) fn load_deliveries(
    conn: &Connection,
    task_id: &str,
) -> Result<Vec<ReviewVerdictDeliverySummary>, StoreError> {
    let mut query = conn.prepare("SELECT comment_id,record FROM (SELECT rowid delivery_sequence,comment_id,record FROM review_verdict_deliveries WHERE task_id=?1 ORDER BY rowid DESC LIMIT 100) ORDER BY delivery_sequence")?;
    let rows = query.query_map([task_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.map(|row| {
        let (id, raw) = row?;
        decode::<ReviewVerdictDeliverySummary>(&raw, "review_verdict_deliveries", &id)
    })
    .collect()
}

fn load_delivery(
    conn: &Connection,
    comment_id: &str,
) -> Result<Option<ReviewVerdictDelivery>, StoreError> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT record FROM review_verdict_deliveries WHERE comment_id=?1",
            [comment_id],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| decode(&raw, "review_verdict_deliveries", comment_id))
        .transpose()
}

fn write_delivery(tx: &Transaction, delivery: &ReviewVerdictDelivery) -> Result<(), StoreError> {
    tx.execute("INSERT INTO review_verdict_deliveries(comment_id,operation_id,task_id,state,created_at,updated_at,record) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(comment_id) DO UPDATE SET operation_id=excluded.operation_id,state=excluded.state,updated_at=excluded.updated_at,record=excluded.record", params![delivery.comment_id,delivery.operation_id,delivery.task_id,delivery.state.as_str(),delivery.created_at,delivery.updated_at,serde_json::to_string(delivery).expect("verdict delivery serializes")])?;
    Ok(())
}

fn operation_id(comment_id: &str, attempt: u64) -> String {
    format!("review-verdict:{comment_id}:{attempt}")
}

fn invalid(reason: &str) -> StoreError {
    StoreError::ReviewPullRequestInvalid(reason.into())
}

#[cfg(test)]
mod tests;
