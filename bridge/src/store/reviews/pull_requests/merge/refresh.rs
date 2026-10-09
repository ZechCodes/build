//! Only explicit ref-fenced confirmation may replace unresolved preconditions.
use super::execution::{advance, require_intent};
use super::*;
use crate::reviews::actions::{ActionStatus, ReviewAction, StepStatus};
use crate::reviews::merge::progress::successful_merge;
use crate::reviews::records::Review;
use crate::store::reviews::{check_version, load_review};

impl Store {
    /// The service holds the merge lease plus every caller-confirmed Git ref.
    /// Keep the original identity, actor, successful results and Push obligations.
    pub(crate) fn refresh_review_merge_plan(
        &self,
        intent: &ReviewMergeIntent,
        confirmed: &ReviewMergeRequest,
    ) -> Result<ReviewMergeIntent, StoreError> {
        self.in_transaction(|tx| {
            let mut saved = require_intent(tx, intent)?;
            validate_identity(&saved.request, confirmed)?;
            let header = require_pull_request(tx, &saved.request.task_id)?;
            check_version(&header, confirmed.expected_version)?;
            let metadata = header.pull_request.expect("PR checked");
            if !metadata.status.is_active()
                || metadata.latest_published_snapshot_id.as_deref() != Some(&confirmed.snapshot_id)
            {
                return Err(invalid("refresh requires the latest active PR snapshot"));
            }
            validate_sources(tx, confirmed)?;
            let review = load_review(tx, &saved.request.task_id)?.expect("PR checked");
            refuse_uncertain(&review, &saved)?;
            refresh_unresolved(&review, &mut saved, confirmed);
            saved.request.expected_version = confirmed.expected_version;
            saved.execution_version = Some(confirmed.expected_version);
            advance(tx, saved)
        })
    }
}

fn validate_identity(
    original: &ReviewMergeRequest,
    confirmed: &ReviewMergeRequest,
) -> Result<(), StoreError> {
    let mut candidate = confirmed.clone();
    candidate.expected_version = original.expected_version;
    for source in &mut candidate.sources {
        if let Some(previous) = original
            .sources
            .iter()
            .find(|row| row.directory_id == source.directory_id)
        {
            source.expected_base_head = previous.expected_base_head.clone();
        }
    }
    candidate
        .sources
        .sort_by(|left, right| left.directory_id.cmp(&right.directory_id));
    let mut held = original.clone();
    held.sources
        .sort_by(|left, right| left.directory_id.cmp(&right.directory_id));
    if candidate != held {
        return Err(invalid(
            "refresh cannot change saved merge identity or publication obligations",
        ));
    }
    Ok(())
}

fn refuse_uncertain(review: &Review, intent: &ReviewMergeIntent) -> Result<(), StoreError> {
    let uncertain = review
        .actions
        .iter()
        .filter(|row| intent.action_ids.contains(&row.id))
        .any(uncertain_action);
    if intent.state == ReviewMergeState::Running || uncertain {
        return Err(invalid(
            "interrupted: recover uncertain or running Git work before refreshing its plan",
        ));
    }
    Ok(())
}

fn uncertain_action(row: &ReviewAction) -> bool {
    row.status == ActionStatus::Running
        || row.steps.is_empty()
        || row.steps.iter().enumerate().any(|(index, step)| {
            match step.status {
                StepStatus::Running | StepStatus::Interrupted => true,
                StepStatus::Succeeded => step.result_head.is_none(),
                // A later Pending step after a definite failure never started Git.
                StepStatus::Pending => !row.steps[..index]
                    .iter()
                    .any(|earlier| earlier.status == StepStatus::Failed),
                StepStatus::Failed => false,
            }
        })
}

fn refresh_unresolved(
    review: &Review,
    intent: &mut ReviewMergeIntent,
    confirmed: &ReviewMergeRequest,
) {
    let integrated = intent
        .request
        .sources
        .iter()
        .filter(|source| successful_merge(review, intent, &source.directory_id).is_some())
        .map(|source| source.directory_id.clone())
        .collect::<BTreeSet<_>>();
    for source in &mut intent.request.sources {
        if integrated.contains(&source.directory_id) {
            continue;
        }
        let target = confirmed
            .sources
            .iter()
            .find(|row| row.directory_id == source.directory_id)
            .expect("identity was checked");
        source.expected_base_head = target.expected_base_head.clone();
    }
}
