use super::*;
use crate::reviews::actions::{self, plan::PreparedSource};
use crate::reviews::git_actions::{self, GitActionError};
use crate::source_sync::{SyncLock, SERVICE_FETCH_DEADLINE};
use crate::store::now_rfc3339;
use std::collections::BTreeMap;

type Targets = BTreeMap<(String, String), String>;

pub(super) fn source(
    store: &Store,
    intent: &mut ReviewMergeIntent,
    prepared: &mut PreparedSource,
    targets: &mut Targets,
    publication_only: bool,
    notify: &impl Fn(),
    checkpoint: &impl Fn(MergeCheckpoint) -> Result<(), String>,
) -> Result<(), String> {
    let _lock = SyncLock::acquire(&prepared.source.source_path, SERVICE_FETCH_DEADLINE);
    if _lock.is_none() {
        prepared.source.error =
            Some("Source busy with another Git action; retry when it finishes".into());
    }
    for index in 0..prepared.action.steps.len() {
        current(store, intent, publication_only)?;
        prepared.action.steps[index].status = StepStatus::Running;
        prepared.action.steps[index].input_head = prepared.head.clone();
        save(store, intent, prepared, notify)?;
        checkpoint(MergeCheckpoint::BeforeGit)?;
        current(store, intent, publication_only)?;
        let outcome = execute(store, intent, prepared, index, targets, publication_only);
        actions::record_outcome(prepared, index, outcome);
        let status = prepared.action.steps[index].status;
        let stopped = matches!(status, StepStatus::Failed | StepStatus::Interrupted);
        if stopped || index + 1 == prepared.action.steps.len() {
            prepared.action.status = match status {
                StepStatus::Failed => ActionStatus::Failed,
                StepStatus::Interrupted => ActionStatus::Interrupted,
                _ => ActionStatus::Succeeded,
            };
            prepared.action.finished_at = Some(now_rfc3339());
        }
        save(store, intent, prepared, notify)?;
        if status == StepStatus::Succeeded && prepared.action.steps[index].kind == StepKind::Merge {
            let source = participant(intent, &prepared.action.directory_id)?;
            targets.insert(
                (source.repository_id.clone(), source.base_branch_ref.clone()),
                prepared
                    .head
                    .clone()
                    .ok_or("merge succeeded without a recorded head")?,
            );
        }
        checkpoint(MergeCheckpoint::GitRecorded)?;
        if stopped {
            break;
        }
    }
    Ok(())
}

fn execute(
    store: &Store,
    intent: &ReviewMergeIntent,
    prepared: &PreparedSource,
    index: usize,
    targets: &Targets,
    publication_only: bool,
) -> Result<git_actions::GitStepOutcome, GitActionError> {
    if let Some(error) = &prepared.source.error {
        return Err(GitActionError::Failed(error.clone()));
    }
    let source =
        participant(intent, &prepared.action.directory_id).map_err(GitActionError::Failed)?;
    let step = &prepared.action.steps[index];
    match step.kind {
        StepKind::Merge => {
            if publication_only {
                return Err(GitActionError::Failed(
                    "Saved publication retry may only publish recorded successful tips".into(),
                ));
            }
            let expected = targets
                .get(&(source.repository_id.clone(), source.base_branch_ref.clone()))
                .unwrap_or(&source.expected_base_head);
            git_actions::merge_expected(
                &prepared.source.directory,
                &prepared.source.source_path,
                &source.base_branch_ref,
                expected,
            )
        }
        StepKind::Push => publish(store, intent, prepared, index),
    }
}

fn publish(
    store: &Store,
    intent: &ReviewMergeIntent,
    prepared: &PreparedSource,
    index: usize,
) -> Result<git_actions::GitStepOutcome, GitActionError> {
    let step = &prepared.action.steps[index];
    let head = prepared
        .head
        .as_deref()
        .ok_or_else(|| GitActionError::Failed("missing saved merge tip".into()))?;
    let remote = step.remote.as_deref().unwrap_or_default();
    let review = load(store, &intent.request.task_id).map_err(GitActionError::Failed)?;
    if let Some(published) = progress::published_tip(
        &review,
        &prepared.source.source_path,
        remote,
        &step.branch,
        head,
    )
    .map_err(GitActionError::Failed)?
    {
        return Ok(published);
    }
    git_actions::push_typed(
        &prepared.source.directory,
        &prepared.source.source_path,
        head,
        true,
        remote,
        &step.branch,
    )
}

fn participant<'a>(
    intent: &'a ReviewMergeIntent,
    directory: &str,
) -> Result<&'a super::super::model::ReviewMergeSource, String> {
    intent
        .request
        .sources
        .iter()
        .find(|source| source.directory_id == directory)
        .ok_or_else(|| "action is not a merge participant".into())
}

fn current(
    store: &Store,
    intent: &ReviewMergeIntent,
    publication_only: bool,
) -> Result<(), String> {
    let review = load(store, &intent.request.task_id)?;
    ensure_retryable_running(&review, intent, publication_only)
}

fn ensure_retryable_running(
    review: &Review,
    intent: &ReviewMergeIntent,
    publication_only: bool,
) -> Result<(), String> {
    if Some(review.version) != intent.execution_version
        || (!publication_only && historical_snapshot(review, intent))
        || review
            .pull_request
            .as_ref()
            .is_none_or(|pr| !publication_only && pr.status == PullRequestStatus::Closed)
    {
        return Err("stale: PR changed during admitted merge".into());
    }
    Ok(())
}

fn save(
    store: &Store,
    intent: &mut ReviewMergeIntent,
    source: &PreparedSource,
    notify: &impl Fn(),
) -> Result<(), String> {
    *intent = store
        .save_review_merge_action(intent, &source.action)
        .map_err(|error| error.to_string())?;
    notify();
    Ok(())
}

pub(super) fn interrupt_rows(
    store: &Store,
    intent: &mut ReviewMergeIntent,
    error: &str,
) -> Result<(), String> {
    let review = load(store, &intent.request.task_id)?;
    let ids = intent.action_ids.clone();
    for mut action in review
        .actions
        .into_iter()
        .filter(|action| ids.contains(&action.id) && action.status == ActionStatus::Running)
    {
        action.status = ActionStatus::Interrupted;
        action.finished_at = Some(now_rfc3339());
        for step in &mut action.steps {
            if matches!(step.status, StepStatus::Running | StepStatus::Pending) {
                step.status = StepStatus::Interrupted;
                step.error = Some(crate::source_sync::without_credentials(error));
            }
        }
        *intent = store
            .save_review_merge_action(intent, &action)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
