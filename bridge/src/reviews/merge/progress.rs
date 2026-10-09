use crate::reviews::actions::{ActionStatus, ActionStep, ReviewAction, StepKind, StepStatus};
use crate::reviews::model::{ReviewMergeIntent, ReviewMergePush, ReviewMergeSource};
use crate::reviews::records::Review;

pub(crate) fn successful_merge<'a>(
    review: &'a Review,
    intent: &ReviewMergeIntent,
    directory: &str,
) -> Option<&'a ReviewAction> {
    let source = intent
        .request
        .sources
        .iter()
        .find(|source| source.directory_id == directory)?;
    review.actions.iter().rev().find(|action| {
        intent.action_ids.contains(&action.id)
            && action.directory_id == directory
            && action.snapshot_id == intent.request.snapshot_id
            && action
                .steps
                .iter()
                .any(|step| successful_merge_step(source, step))
    })
}

fn successful_merge_step(source: &ReviewMergeSource, step: &ActionStep) -> bool {
    step.kind == StepKind::Merge
        && step.status == StepStatus::Succeeded
        && step.result_head.is_some()
        && step.input_head.as_deref() == Some(&source.head)
        && Some(step.branch.as_str()) == source.base_branch_ref.strip_prefix("refs/heads/")
}

pub(crate) fn pushed(
    review: &Review,
    intent: &ReviewMergeIntent,
    directory: &str,
    push: &ReviewMergePush,
) -> bool {
    review.actions.iter().any(|action| {
        intent.action_ids.contains(&action.id)
            && action.directory_id == directory
            && action.snapshot_id == intent.request.snapshot_id
            && action.steps.iter().any(|step| {
                step.kind == StepKind::Push
                    && step.status == StepStatus::Succeeded
                    && step.remote.as_deref() == Some(&push.remote)
                    && step.branch == push.branch
            })
    })
}

pub(crate) fn publication_settled(review: &Review, intent: &ReviewMergeIntent) -> bool {
    intent
        .request
        .sources
        .iter()
        .filter(|source| successful_merge(review, intent, &source.directory_id).is_some())
        .all(|source| {
            source
                .push
                .as_ref()
                .is_none_or(|push| pushed(review, intent, &source.directory_id, push))
        })
}

pub(crate) fn uncertain_action(row: &ReviewAction) -> bool {
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

pub(crate) fn unresolved_actions(review: &Review, intent: &ReviewMergeIntent) -> bool {
    intent.action_ids.iter().enumerate().any(|(index, id)| {
        let Some(action) = review.actions.iter().find(|action| &action.id == id) else {
            return true;
        };
        let mut observed = action.clone();
        for step in &mut observed.steps {
            if settled_interrupted_push(
                review,
                intent,
                &intent.action_ids[index + 1..],
                action,
                step,
            ) {
                // Evaluate known publication without rewriting interrupted history
                // or giving following Pending steps a Failed predecessor.
                step.status = StepStatus::Succeeded;
                step.result_head = step.input_head.clone();
            }
        }
        uncertain_action(&observed)
    })
}

fn settled_interrupted_push(
    review: &Review,
    intent: &ReviewMergeIntent,
    later_ids: &[String],
    action: &ReviewAction,
    step: &ActionStep,
) -> bool {
    if step.kind != StepKind::Push || step.status != StepStatus::Interrupted {
        return false;
    }
    let Some(head) = step.input_head.as_deref() else {
        return false;
    };
    let Some(origin) = merge_link(action, step) else {
        return false;
    };
    if !known_merge_origin(review, intent, action, step, origin, head) {
        return false;
    }
    later_ids
        .iter()
        .filter_map(|id| review.actions.iter().find(|row| &row.id == id))
        .any(|later| {
            same_source(action, later)
                && later
                    .steps
                    .iter()
                    .any(|candidate| matching_push_result(step, later, candidate, origin))
        })
}

fn known_merge_origin(
    review: &Review,
    intent: &ReviewMergeIntent,
    action: &ReviewAction,
    push_step: &ActionStep,
    origin: &str,
    head: &str,
) -> bool {
    let Some(source) = intent
        .request
        .sources
        .iter()
        .find(|source| source.directory_id == action.directory_id)
    else {
        return false;
    };
    let destination = source.push.as_ref().is_some_and(|push| {
        push_step.remote.as_deref() == Some(&push.remote) && push_step.branch == push.branch
    });
    if !destination
        || action.snapshot_id != intent.request.snapshot_id
        || !intent.action_ids.iter().any(|id| id == origin)
    {
        return false;
    }
    review
        .actions
        .iter()
        .find(|row| row.id == origin && same_source(action, row))
        .is_some_and(|merge| {
            merge.steps.iter().any(|step| {
                successful_merge_step(source, step) && step.result_head.as_deref() == Some(head)
            })
        })
}

fn matching_push_result(
    previous: &ActionStep,
    action: &ReviewAction,
    step: &ActionStep,
    origin: &str,
) -> bool {
    step.kind == StepKind::Push
        && step.status == StepStatus::Succeeded
        && step.input_head == previous.input_head
        && step.result_head == previous.input_head
        && step.branch == previous.branch
        && step.remote == previous.remote
        && merge_link(action, step) == Some(origin)
}

fn merge_link<'a>(action: &'a ReviewAction, step: &'a ActionStep) -> Option<&'a str> {
    step.merge_action_id.as_deref().or_else(|| {
        action
            .steps
            .iter()
            .any(|merge| {
                merge.kind == StepKind::Merge
                    && merge.status == StepStatus::Succeeded
                    && merge.result_head.is_some()
                    && merge.result_head == step.input_head
            })
            .then_some(action.id.as_str())
    })
}

fn same_source(left: &ReviewAction, right: &ReviewAction) -> bool {
    left.directory_id == right.directory_id
        && left.source_path == right.source_path
        && left.snapshot_id == right.snapshot_id
}

/// A later durable push can settle an older selected merge tip when Git proves
/// that tip is in the published history. This reads objects off the app lock.
pub(super) fn published_tip(
    review: &Review,
    source_path: &std::path::Path,
    remote: &str,
    branch: &str,
    head: &str,
) -> Result<Option<crate::reviews::git_actions::GitStepOutcome>, String> {
    let repository = git2::Repository::open(source_path).map_err(|error| error.to_string())?;
    let common = repository
        .commondir()
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let selected = git2::Oid::from_str(head).map_err(|error| error.to_string())?;
    repository
        .find_commit(selected)
        .map_err(|error| error.to_string())?;
    for action in review.actions.iter().rev() {
        if crate::reviews::receivers::canonical_common_git_dir(&action.source_path)
            .ok()
            .as_ref()
            != Some(&common)
        {
            continue;
        }
        for step in &action.steps {
            if !successful_push(step, remote, branch) {
                continue;
            }
            let Some(published) = step
                .result_head
                .as_deref()
                .and_then(|tip| git2::Oid::from_str(tip).ok())
            else {
                continue;
            };
            if published == selected
                || repository
                    .graph_descendant_of(published, selected)
                    .unwrap_or(false)
            {
                return Ok(Some(crate::reviews::git_actions::GitStepOutcome {
                    head: head.into(),
                    warning: Some(format!(
                        "Already published in saved action {} at {published}",
                        action.id
                    )),
                }));
            }
        }
    }
    Ok(None)
}

fn successful_push(step: &crate::reviews::actions::ActionStep, remote: &str, branch: &str) -> bool {
    step.kind == StepKind::Push
        && step.status == StepStatus::Succeeded
        && step.remote.as_deref() == Some(remote)
        && step.branch == branch
}

#[cfg(test)]
mod tests;
