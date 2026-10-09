//! Durable PR integration and publication, independent of browser lifetime.
mod execution;
mod fence;
mod plan;
pub(crate) mod progress;
mod recovery;
mod refresh;
pub use recovery::recover;

use super::actions::{ActionSource, ActionStatus, StepKind, StepStatus};
use super::model::{PullRequestStatus, ReviewMergeIntent, ReviewMergeRequest, ReviewMergeState};
use super::records::Review;
use crate::store::Store;
use progress::{publication_settled, pushed, successful_merge};

pub struct MergeJob {
    pub project_path: String,
    pub request_id: String,
    pub request: ReviewMergeRequest,
    /// Trusted current source identities, resolved by the adapter.
    pub sources: Vec<ActionSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeCheckpoint {
    Admitted,
    BeforeGit,
    GitRecorded,
    BeforeFinalization,
    RefsLocked,
    Finalized,
}

pub fn merge(store: &Store, job: &MergeJob, notify: impl Fn()) -> Result<Review, String> {
    merge_with_refresh(store, job, None, notify)
}

pub(crate) fn merge_with_refresh(
    store: &Store,
    job: &MergeJob,
    refresh: Option<&ReviewMergeRequest>,
    notify: impl Fn(),
) -> Result<Review, String> {
    merge_prepared_observed(store, job, refresh, &notify, &|_| Ok(()))
}

#[cfg(test)]
fn merge_observed(
    store: &Store,
    job: &MergeJob,
    notify: &impl Fn(),
    checkpoint: &impl Fn(MergeCheckpoint) -> Result<(), String>,
) -> Result<Review, String> {
    merge_prepared_observed(store, job, None, notify, checkpoint)
}

fn merge_prepared_observed(
    store: &Store,
    job: &MergeJob,
    refresh: Option<&ReviewMergeRequest>,
    notify: &impl Fn(),
    checkpoint: &impl Fn(MergeCheckpoint) -> Result<(), String>,
) -> Result<Review, String> {
    let review = load(store, &job.request.task_id)?;
    let _lease = fence::lease(&review)?;
    let existing = store
        .load_review_merge_intent(&job.project_path, &job.request_id)
        .map_err(|error| error.to_string())?;
    if let Some(intent) = &existing {
        if intent.request != job.request {
            return Err("conflict: request_id names a different PR merge".into());
        }
        if intent.state == ReviewMergeState::Succeeded {
            return load(store, &job.request.task_id);
        }
    } else {
        super::sync::reconcile::reconcile(store, &job.request.task_id)?;
        notify();
    }
    let review = load(store, &job.request.task_id)?;
    plan::validate_sources(&review, job)?;
    let mut intent = match existing {
        Some(intent) => intent,
        None => admit(store, job, &review)?,
    };
    if let Some(request) = refresh {
        intent = refresh::plan(store, &review, &intent, request)?;
        notify();
    }
    let result = run(store, job, &review, &mut intent, notify, checkpoint);
    if let Err(error) = &result {
        // A storage error never starts later Git. Preserve recorded successes,
        // and leave uncertain rows for boot recovery if persistence is unavailable.
        let _ = interrupt(store, &mut intent, error);
        notify();
    }
    result
}

fn admit(store: &Store, job: &MergeJob, review: &Review) -> Result<ReviewMergeIntent, String> {
    let temporary = ReviewMergeIntent {
        project_path: job.project_path.clone(),
        request_id: job.request_id.clone(),
        version: 0,
        request: job.request.clone(),
        state: ReviewMergeState::Running,
        execution_version: None,
        action_ids: Vec::new(),
        created_at: String::new(),
        updated_at: String::new(),
        error: None,
    };
    fence::with_refs(review, &temporary, true, || {
        store
            .reserve_review_merge(&job.project_path, &job.request_id, job.request.clone())
            .map_err(|error| error.to_string())
    })
}

fn run(
    store: &Store,
    job: &MergeJob,
    review: &Review,
    intent: &mut ReviewMergeIntent,
    notify: &impl Fn(),
    checkpoint: &impl Fn(MergeCheckpoint) -> Result<(), String>,
) -> Result<Review, String> {
    ensure_retryable(review, intent)?;
    let publication_only = publication_only(review, intent);
    let mut sources = plan::actions(review, intent, job)?;
    if !sources.is_empty() {
        let rows = sources
            .iter()
            .map(|source| source.action.clone())
            .collect::<Vec<_>>();
        *intent = store
            .start_review_merge_actions(intent, review.version, &rows)
            .map_err(|error| error.to_string())?;
        notify();
        checkpoint(MergeCheckpoint::Admitted)?;
        let mut targets = plan::targets(review, intent);
        for source in &mut sources {
            execution::source(
                store,
                intent,
                source,
                &mut targets,
                publication_only,
                notify,
                checkpoint,
            )?;
        }
    }
    checkpoint(MergeCheckpoint::BeforeFinalization)?;
    if publication_only {
        let current = load(store, &intent.request.task_id)?;
        return settle_saved_publication(store, intent, &current, notify, checkpoint);
    }
    finalize_and_settle(store, intent, notify, checkpoint)
}

fn ensure_retryable(review: &Review, intent: &ReviewMergeIntent) -> Result<(), String> {
    if review.actions.iter().any(|action| {
        intent.action_ids.contains(&action.id) && action.status == ActionStatus::Running
    }) {
        return Err(
            "interrupted: PR merge has uncertain Git work; recover it before retrying".into(),
        );
    }
    if publication_only(review, intent) {
        return Ok(());
    }
    let expected = intent
        .execution_version
        .unwrap_or(intent.request.expected_version);
    if expected != review.version {
        return Err("stale: PR version changed during merge; refresh the plan".into());
    }
    Ok(())
}

fn historical_snapshot(review: &Review, intent: &ReviewMergeIntent) -> bool {
    review
        .pull_request
        .as_ref()
        .and_then(|metadata| metadata.latest_published_snapshot_id.as_deref())
        != Some(&intent.request.snapshot_id)
}

fn publication_only(review: &Review, intent: &ReviewMergeIntent) -> bool {
    historical_snapshot(review, intent)
        || review.pull_request.as_ref().is_some_and(|metadata| {
            metadata.status == PullRequestStatus::Closed
                || (metadata.status == PullRequestStatus::Merged
                    && intent.execution_version != Some(review.version))
        })
}

fn finalize_and_settle(
    store: &Store,
    intent: &mut ReviewMergeIntent,
    notify: &impl Fn(),
    checkpoint: &impl Fn(MergeCheckpoint) -> Result<(), String>,
) -> Result<Review, String> {
    let review = load(store, &intent.request.task_id)?;
    if publication_only(&review, intent) {
        return settle_saved_publication(store, intent, &review, notify, checkpoint);
    }
    let integrated = intent
        .request
        .sources
        .iter()
        .all(|source| successful_merge(&review, intent, &source.directory_id).is_some());
    let mut failure = None;
    if integrated {
        if let Err(error) = fence::with_refs(&review, intent, false, || {
            checkpoint(MergeCheckpoint::RefsLocked)?;
            for source in &intent.request.sources {
                let binding = review
                    .bindings
                    .iter()
                    .find(|binding| binding.directory_id == source.directory_id)
                    .ok_or("missing PR binding")?;
                super::git_actions::verify_integrated(
                    &binding.source_repository,
                    &source.base_branch_ref,
                    &source.head,
                )?;
            }
            store
                .finalize_review_merge(intent)
                .map_err(|error| error.to_string())
        }) {
            failure = Some(error);
        }
        *intent = store
            .load_review_merge_intent(&intent.project_path, &intent.request_id)
            .map_err(|error| error.to_string())?
            .ok_or("merge intent missing")?;
    }
    let current = load(store, &intent.request.task_id)?;
    let merged = current
        .pull_request
        .as_ref()
        .is_some_and(|metadata| metadata.status == PullRequestStatus::Merged);
    intent.state = if integrated && merged && publication_settled(&current, intent) {
        ReviewMergeState::Succeeded
    } else if current.actions.iter().any(|action| {
        intent.action_ids.contains(&action.id) && action.status == ActionStatus::Interrupted
    }) {
        ReviewMergeState::Interrupted
    } else {
        ReviewMergeState::Failed
    };
    intent.error = failure.or_else(|| {
        (intent.state != ReviewMergeState::Succeeded).then(|| {
            if integrated {
                "Requested publication is unsettled; retry the saved result".into()
            } else {
                "Partially merged or failed: not all included heads reached their configured bases"
                    .into()
            }
        })
    });
    *intent = store
        .save_review_merge_intent(intent, intent.version)
        .map_err(|error| error.to_string())?;
    notify();
    checkpoint(MergeCheckpoint::Finalized)?;
    // Source-base movement caused by this merge must not publish another look
    // before finalization. Once settled, reconcile any newer received work.
    let _ = super::sync::reconcile::reconcile(store, &intent.request.task_id);
    notify();
    load(store, &intent.request.task_id)
}

fn settle_saved_publication(
    store: &Store,
    intent: &mut ReviewMergeIntent,
    review: &Review,
    notify: &impl Fn(),
    checkpoint: &impl Fn(MergeCheckpoint) -> Result<(), String>,
) -> Result<Review, String> {
    intent.state = if review.actions.iter().any(|action| {
        intent.action_ids.contains(&action.id) && action.status == ActionStatus::Interrupted
    }) {
        ReviewMergeState::Interrupted
    } else {
        ReviewMergeState::Failed
    };
    intent.error = Some(
        if publication_settled(review, intent) {
            "Saved merge plan retained; recorded successful local results are published"
        } else {
            "Saved merge plan retained; requested publication is still pending"
        }
        .into(),
    );
    *intent = store
        .save_review_merge_intent(intent, intent.version)
        .map_err(|error| error.to_string())?;
    notify();
    checkpoint(MergeCheckpoint::Finalized)?;
    load(store, &intent.request.task_id)
}

fn interrupt(store: &Store, intent: &mut ReviewMergeIntent, error: &str) -> Result<(), String> {
    execution::interrupt_rows(store, intent, error)?;
    intent.state = ReviewMergeState::Interrupted;
    intent.error = Some(crate::source_sync::without_credentials(error));
    *intent = store
        .save_review_merge_intent(intent, intent.version)
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn load(store: &Store, task_id: &str) -> Result<Review, String> {
    store
        .load_review(task_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "PR review missing".into())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod recovery_tests;

#[cfg(test)]
mod refresh_tests;
