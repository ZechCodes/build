//! Merge intent/action linkage is committed before Git can start.

use super::*;
use crate::reviews::actions::ReviewAction;
use crate::reviews::merge::progress::successful_merge;
use crate::reviews::records::Review;
use crate::store::reviews::{actions, check_version, lifecycle, load_review};

impl Store {
    /// Defer comparison-base churn caused by our own partial integration.
    /// A different received head still publishes immediately.
    pub fn review_merge_holds_snapshot(&self, task_id: &str) -> Result<bool, StoreError> {
        self.connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM review_merge_intents AS merge_intent
             JOIN reviews ON reviews.task_id = merge_intent.task_id
             WHERE merge_intent.task_id = ?1 AND merge_intent.state != 'succeeded'
             AND json_extract(merge_intent.record, '$.request.snapshot_id') = json_extract(reviews.record, '$.pull_request.latest_published_snapshot_id')
             AND (merge_intent.state = 'running' OR EXISTS(
                SELECT 1 FROM review_actions AS action, json_each(action.record, '$.steps') AS step
                WHERE action.task_id = merge_intent.task_id
                AND action.id IN (SELECT value FROM json_each(merge_intent.record, '$.action_ids'))
                AND json_extract(step.value, '$.kind') = 'merge'
                AND json_extract(step.value, '$.status') = 'succeeded')))",
            [task_id], |row| row.get(0),
        ).map_err(StoreError::from)
    }

    pub fn review_merge_is_running(&self, task_id: &str) -> Result<bool, StoreError> {
        self.connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM review_merge_intents WHERE task_id = ?1 AND state = 'running')",
            [task_id], |row| row.get(0),
        ).map_err(StoreError::from)
    }

    /// No snapshot/history reads in the reclaim check. Retain unsettled
    /// requested publication even after local merge completed the PR.
    pub fn workspace_review_publication_pending(
        &self,
        workspace_id: &str,
    ) -> Result<bool, StoreError> {
        let conn = self.connection();
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM review_merge_intents AS merge_intent
             JOIN reviews ON reviews.task_id = merge_intent.task_id
             WHERE reviews.workspace_id = ?1 AND
             (merge_intent.state = 'running' OR
              EXISTS(SELECT 1 FROM json_each(merge_intent.record, '$.request.sources') AS source
               WHERE json_extract(source.value, '$.push') IS NOT NULL AND EXISTS(
                SELECT 1 FROM review_actions AS merged_action,
                json_each(merged_action.record, '$.steps') AS merge_step
                WHERE merged_action.task_id = merge_intent.task_id
                AND merged_action.id IN (SELECT value FROM json_each(merge_intent.record, '$.action_ids'))
                AND json_extract(merged_action.record, '$.directory_id') = json_extract(source.value, '$.directory_id')
                AND json_extract(merge_step.value, '$.kind') = 'merge'
                AND json_extract(merge_step.value, '$.status') = 'succeeded'
               ) AND NOT EXISTS(
                SELECT 1 FROM review_actions AS action,
                json_each(action.record, '$.steps') AS step
                WHERE action.task_id = merge_intent.task_id
                AND action.id IN (SELECT value FROM json_each(merge_intent.record, '$.action_ids'))
                AND json_extract(action.record, '$.directory_id') = json_extract(source.value, '$.directory_id')
                AND json_extract(step.value, '$.kind') = 'push'
                AND json_extract(step.value, '$.status') = 'succeeded'
                AND json_extract(step.value, '$.remote') = json_extract(source.value, '$.push.remote')
                AND json_extract(step.value, '$.branch') = json_extract(source.value, '$.push.branch')
              ))))",
            [workspace_id], |row| row.get(0),
        ).map_err(StoreError::from)
    }

    pub(crate) fn start_review_merge_actions(
        &self,
        intent: &ReviewMergeIntent,
        expected_review_version: u64,
        rows: &[ReviewAction],
    ) -> Result<ReviewMergeIntent, StoreError> {
        self.in_transaction(|tx| {
            let mut saved = require_intent(tx, intent)?;
            let other_running: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM review_merge_intents WHERE task_id = ?1 AND state = 'running' AND request_id != ?2)",
                params![saved.request.task_id, saved.request_id], |row| row.get(0),
            )?;
            if other_running { return Err(invalid("a merge is already running for this PR")); }
            let review = actions::start_actions(tx, &saved.request.task_id, expected_review_version, rows)?;
            saved.execution_version = Some(review.version);
            saved.action_ids.extend(rows.iter().map(|row| row.id.clone()));
            saved.state = ReviewMergeState::Running;
            saved.error = None;
            advance(tx, saved)
        })
    }

    pub(crate) fn save_review_merge_action(
        &self,
        intent: &ReviewMergeIntent,
        action: &ReviewAction,
    ) -> Result<ReviewMergeIntent, StoreError> {
        self.in_transaction(|tx| {
            let mut saved = require_intent(tx, intent)?;
            if !saved.action_ids.contains(&action.id) || saved.state != ReviewMergeState::Running {
                return Err(invalid("action is not owned by this running PR merge"));
            }
            actions::save_action(tx, &saved.request.task_id, action)?;
            let version = saved
                .execution_version
                .ok_or_else(|| invalid("merge has no admitted execution version"))?;
            saved.execution_version = Some(version + 1);
            advance(tx, saved)
        })
    }

    /// Caller holds real receiving and target ref locks across this write.
    /// Neither arbitrary actions nor a successful intent alone proves a merge.
    pub(crate) fn finalize_review_merge(
        &self,
        intent: &ReviewMergeIntent,
    ) -> Result<Review, StoreError> {
        self.in_transaction(|tx| {
            let mut saved = require_intent(tx, intent)?;
            let header = require_pull_request(tx, &saved.request.task_id)?;
            check_version(
                &header,
                saved
                    .execution_version
                    .ok_or_else(|| invalid("merge has no execution version"))?,
            )?;
            let metadata = header.pull_request.as_ref().expect("PR metadata checked");
            if metadata.latest_published_snapshot_id.as_deref() != Some(&saved.request.snapshot_id)
            {
                return Err(invalid("stale: PR snapshot changed during merge"));
            }
            let review = load_review(tx, &saved.request.task_id)?.expect("PR checked");
            for source in &saved.request.sources {
                if successful_merge(&review, &saved, &source.directory_id).is_none() {
                    return Err(invalid(
                        "not every included source has a recorded successful merge",
                    ));
                }
            }
            if metadata.status == PullRequestStatus::Merged {
                return Ok(review);
            }
            if !metadata.status.is_active() {
                return Err(invalid("closed PR cannot be finalized as merged"));
            }
            let (review, _) = lifecycle::transition_in_tx(
                tx,
                &saved.request.task_id,
                PullRequestStatus::Merged,
                &saved.request.actor,
                "Merged all included heads into their configured bases",
            )?;
            saved.execution_version = Some(review.version);
            advance(tx, saved)?;
            Ok(review)
        })
    }
}

fn require_intent(
    tx: &Transaction,
    intent: &ReviewMergeIntent,
) -> Result<ReviewMergeIntent, StoreError> {
    let saved = load_intent(tx, &intent.project_path, &intent.request_id)?
        .ok_or_else(|| invalid("unknown PR merge intent"))?;
    check_operation_version(&intent.request_id, intent.version, saved.version)?;
    if saved.request != intent.request {
        return Err(invalid("merge intent identity changed"));
    }
    Ok(saved)
}

fn advance(
    tx: &Transaction,
    mut saved: ReviewMergeIntent,
) -> Result<ReviewMergeIntent, StoreError> {
    saved.version += 1;
    saved.updated_at = now_rfc3339();
    write_intent(tx, &saved)?;
    Ok(saved)
}
