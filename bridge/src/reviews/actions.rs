//! Selected Git steps, with durable per-source progress and no automatic retry.
mod model;
pub use model::*;

mod plan;
use super::{git_actions, records::Review};
use crate::source_sync::{SyncLock, SERVICE_FETCH_DEADLINE};
use crate::store::{now_rfc3339, Store};
use plan::{prepare, PreparedSource};

/// Work is accepted and recorded before invoking Git. Every selected source
/// gets its own result; a failure in one never rolls another repository back.
pub fn act(store: &Store, request: &ActionRequest, notify: impl Fn()) -> Result<Review, String> {
    let review = store
        .load_review(&request.params.task_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("unknown review for task_id: {}", request.params.task_id))?;
    let mut sources = prepare(&review, request)?;
    let rows = sources
        .iter()
        .map(|source| source.action.clone())
        .collect::<Vec<_>>();
    store
        .start_review_actions(
            &request.params.task_id,
            request.params.expected_version,
            &rows,
        )
        .map_err(|error| error.to_string())?;
    notify();
    for index in 0..sources.len() {
        if let Err(error) = run_source(store, &request.params.task_id, &mut sources[index], &notify)
        {
            return Err(stop_after_write_failure(
                store,
                &request.params.task_id,
                &mut sources[index..],
                &error,
                &notify,
            ));
        }
    }
    let review = store
        .load_review(&request.params.task_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("unknown review for task_id: {}", request.params.task_id))?;
    Ok(with_destinations(review, &request.sources))
}

/// If progress cannot be saved, no later Git starts. Release just this call's
/// accepted rows when storage recovers, keeping any Git outcome already known.
/// A lasting store failure leaves running rows for the ordinary boot recovery.
fn stop_after_write_failure(
    store: &Store,
    task: &str,
    sources: &mut [PreparedSource],
    error: &str,
    notify: &impl Fn(),
) -> String {
    let mut message = error.to_string();
    for source in sources {
        let action = &mut source.action;
        if action.status == ActionStatus::Running {
            action.status = ActionStatus::Interrupted;
            action.finished_at = Some(now_rfc3339());
            for step in &mut action.steps {
                if matches!(step.status, StepStatus::Running | StepStatus::Pending) {
                    step.status = StepStatus::Interrupted;
                    step.error = Some(
                        "Progress could not be saved. Check and retry, or mark complete.".into(),
                    );
                }
            }
        }
        if let Err(stopped) = save(store, task, action, notify) {
            message.push_str(&format!(
                "; could not record stopped action {}: {stopped}",
                action.id
            ));
        }
    }
    message
}

fn run_source(
    store: &Store,
    task: &str,
    source: &mut PreparedSource,
    notify: &impl Fn(),
) -> Result<(), String> {
    let lock = SyncLock::acquire(&source.source.source_path, SERVICE_FETCH_DEADLINE);
    if lock.is_none() {
        source.source.error =
            Some("The source is busy with another Git action. Try again when it finishes.".into());
    }
    for index in 0..source.action.steps.len() {
        source.action.steps[index].status = StepStatus::Running;
        source.action.steps[index].input_head = source.head.clone();
        save(store, task, &source.action, notify)?;
        let outcome = execute(source, index);
        record_outcome(source, index, outcome);
        let failed = source.action.steps[index].status == StepStatus::Failed;
        if failed || index + 1 == source.action.steps.len() {
            source.action.status = if failed {
                ActionStatus::Failed
            } else {
                ActionStatus::Succeeded
            };
            source.action.finished_at = Some(now_rfc3339());
        }
        save(store, task, &source.action, notify)?;
        if failed {
            break;
        }
    }
    drop(lock);
    Ok(())
}

fn execute(source: &PreparedSource, index: usize) -> Result<git_actions::GitStepOutcome, String> {
    if let Some(error) = &source.source.error {
        return Err(error.clone());
    }
    let directory = &source.source.directory;
    let path = &source.source.source_path;
    let step = &source.action.steps[index];
    match step.kind {
        StepKind::Merge => git_actions::merge(directory, path, &step.branch),
        StepKind::Push => git_actions::push(
            directory,
            path,
            source
                .head
                .as_deref()
                .ok_or("The review source has no saved Git head.")?,
            source.merged,
            step.remote.as_deref().unwrap_or_default(),
            &step.branch,
        ),
    }
}

fn record_outcome(
    source: &mut PreparedSource,
    index: usize,
    outcome: Result<git_actions::GitStepOutcome, String>,
) {
    let step = &mut source.action.steps[index];
    match outcome {
        Ok(outcome) => {
            source.head = Some(outcome.head.clone());
            source.merged |= step.kind == StepKind::Merge;
            step.result_head = Some(outcome.head);
            step.warning = outcome.warning;
            step.status = StepStatus::Succeeded;
        }
        Err(error) => {
            step.error = Some(crate::source_sync::without_credentials(&error));
            step.status = StepStatus::Failed;
        }
    }
}

fn save(
    store: &Store,
    task: &str,
    action: &ReviewAction,
    notify: &impl Fn(),
) -> Result<(), String> {
    store
        .save_review_action(task, action)
        .map_err(|error| error.to_string())?;
    notify();
    Ok(())
}

/// Destination reads run off the app mutex. Live facts are returned alongside
/// the saved commit identities; they never rewrite those identities.
/// Only the latest snapshot needs live destinations, regardless of history size.
pub fn with_destinations(mut review: Review, sources: &[ActionSource]) -> Review {
    review.destinations = review
        .snapshots
        .last()
        .into_iter()
        .flat_map(|snapshot| {
            snapshot.directories.iter().map(|directory| {
                let source = sources
                    .iter()
                    .find(|source| source.directory.id == directory.id);
                let source_path = source
                    .map(|source| source.source_path.clone())
                    .unwrap_or_else(|| directory.source_path.clone());
                let mut destination = ReviewDestination {
                    snapshot_id: snapshot.id.clone(),
                    directory_id: directory.id.clone(),
                    source_path: source_path.clone(),
                    branches: vec![],
                    remotes: vec![],
                    live_head: None,
                    error: None,
                };
                let result = source
                    .ok_or_else(|| "Source unavailable".to_string())
                    .and_then(|source| {
                        if let Some(error) = &source.error {
                            return Err(error.clone());
                        }
                        git_actions::destinations(directory, &source_path)
                    });
                match result {
                    Ok(found) => {
                        destination.branches = found.branches;
                        destination.remotes = found.remotes;
                        destination.live_head = found.live_head;
                    }
                    Err(error) => {
                        destination.error = Some(crate::source_sync::without_credentials(&error))
                    }
                }
                destination
            })
        })
        .collect();
    review
}

#[cfg(test)]
mod tests;
