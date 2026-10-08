use super::*;
use crate::reviews::actions::{MergeSelection, PushSelection, ReviewActParams, SourceSelection};
use std::collections::BTreeMap;

pub(super) fn actions(
    review: &Review,
    intent: &ReviewMergeIntent,
    job: &MergeJob,
) -> Result<Vec<crate::reviews::actions::plan::PreparedSource>, String> {
    let mut selections = Vec::new();
    let publication_only = publication_only(review, intent);
    for source in &intent.request.sources {
        let previous = successful_merge(review, intent, &source.directory_id);
        let merge = (previous.is_none() && !publication_only).then(|| MergeSelection {
            branch: branch(&source.base_branch_ref).unwrap_or_default().into(),
        });
        let push = source
            .push
            .as_ref()
            .filter(|_| previous.is_some() || !publication_only)
            .filter(|push| !pushed(review, intent, &source.directory_id, push))
            .map(|push| PushSelection {
                remote: push.remote.clone(),
                branch: push.branch.clone(),
                merge_action_id: previous.map(|action| action.id.clone()),
            });
        if merge.is_some() || push.is_some() {
            selections.push(SourceSelection {
                directory_id: source.directory_id.clone(),
                merge,
                push,
            });
        }
    }
    if selections.is_empty() {
        return Ok(Vec::new());
    }
    crate::reviews::actions::plan::prepare(
        review,
        &crate::reviews::actions::ActionRequest {
            params: ReviewActParams {
                task_id: review.task_id.clone(),
                expected_version: review.version,
                snapshot_id: intent.request.snapshot_id.clone(),
                sources: selections,
            },
            actor: intent.request.actor.clone(),
            sources: job.sources.clone(),
        },
    )
}

pub(super) fn validate_sources(review: &Review, job: &MergeJob) -> Result<(), String> {
    for binding in &review.bindings {
        let source = job
            .sources
            .iter()
            .find(|source| source.directory.id == binding.directory_id)
            .ok_or("source unavailable; restore its configured identity before merging")?;
        if source.directory.source_id != binding.source_id
            || source
                .source_path
                .canonicalize()
                .map_err(|error| error.to_string())?
                != binding.source_repository
        {
            return Err("review source identity or placement changed".into());
        }
        if let Some(error) = &source.error {
            return Err(error.clone());
        }
        crate::reviews::receivers::validate_binding_receiver(binding)?;
    }
    Ok(())
}

pub(super) fn targets(
    review: &Review,
    intent: &ReviewMergeIntent,
) -> BTreeMap<(String, String), String> {
    let mut targets = BTreeMap::new();
    for id in &intent.action_ids {
        if let Some(action) = review.actions.iter().find(|action| &action.id == id) {
            if let Some(source) = intent
                .request
                .sources
                .iter()
                .find(|source| source.directory_id == action.directory_id)
            {
                for step in &action.steps {
                    if step.kind == StepKind::Merge && step.status == StepStatus::Succeeded {
                        if let Some(head) = &step.result_head {
                            targets.insert(
                                (source.repository_id.clone(), source.base_branch_ref.clone()),
                                head.clone(),
                            );
                        }
                    }
                }
            }
        }
    }
    targets
}

pub(super) fn branch(reference: &str) -> Result<&str, String> {
    reference
        .strip_prefix("refs/heads/")
        .filter(|_| git2::Reference::is_valid_name(reference))
        .ok_or_else(|| "PR merge requires a configured local base branch".into())
}
