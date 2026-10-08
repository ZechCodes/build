use crate::reviews::actions::{ReviewAction, StepKind, StepStatus};
use crate::reviews::model::{ReviewMergeIntent, ReviewMergePush};
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
            && action.steps.iter().any(|step| {
                step.kind == StepKind::Merge
                    && step.status == StepStatus::Succeeded
                    && step.result_head.is_some()
                    && step.input_head.as_deref() == Some(&source.head)
                    && Some(step.branch.as_str())
                        == source.base_branch_ref.strip_prefix("refs/heads/")
            })
    })
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
