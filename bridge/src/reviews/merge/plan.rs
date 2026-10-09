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
    prefer_confirmed_targets(review, intent, &mut targets);
    targets
}

fn prefer_confirmed_targets(
    review: &Review,
    intent: &ReviewMergeIntent,
    targets: &mut BTreeMap<(String, String), String>,
) {
    for source in &intent.request.sources {
        if successful_merge(review, intent, &source.directory_id).is_some() {
            continue;
        }
        let binding = review
            .bindings
            .iter()
            .find(|row| row.directory_id == source.directory_id);
        if current_target(binding, &source.base_branch_ref).as_deref()
            == Some(&source.expected_base_head)
            && confirmed_includes_successes(review, intent, source, binding)
        {
            // Refreshed unresolved sources can share a base already advanced by
            // an earlier success. Git still fences this exact confirmed head.
            targets.insert(
                (source.repository_id.clone(), source.base_branch_ref.clone()),
                source.expected_base_head.clone(),
            );
        }
    }
}

fn confirmed_includes_successes(
    review: &Review,
    intent: &ReviewMergeIntent,
    source: &crate::reviews::model::ReviewMergeSource,
    binding: Option<&crate::reviews::model::ReviewBranchBinding>,
) -> bool {
    let Some(binding) = binding else {
        return false;
    };
    let Ok(repository) = git2::Repository::open(&binding.source_repository) else {
        return false;
    };
    let Ok(target) = git2::Oid::from_str(&source.expected_base_head) else {
        return false;
    };
    intent
        .request
        .sources
        .iter()
        .filter(|previous| {
            previous.repository_id == source.repository_id
                && previous.base_branch_ref == source.base_branch_ref
        })
        .all(|previous| contains_recorded_success(&repository, target, review, intent, previous))
}

fn contains_recorded_success(
    repository: &git2::Repository,
    target: git2::Oid,
    review: &Review,
    intent: &ReviewMergeIntent,
    source: &crate::reviews::model::ReviewMergeSource,
) -> bool {
    let Some(action) = successful_merge(review, intent, &source.directory_id) else {
        return true;
    };
    let tip = action
        .steps
        .iter()
        .find(|step| step.kind == StepKind::Merge && step.status == StepStatus::Succeeded)
        .and_then(|step| step.result_head.as_deref())
        .and_then(|head| git2::Oid::from_str(head).ok());
    tip.is_some_and(|tip| {
        target == tip || repository.graph_descendant_of(target, tip).unwrap_or(false)
    })
}

fn current_target(
    binding: Option<&crate::reviews::model::ReviewBranchBinding>,
    reference: &str,
) -> Option<String> {
    let repository = git2::Repository::open(&binding?.source_repository).ok()?;
    repository
        .refname_to_id(reference)
        .ok()
        .map(|head| head.to_string())
}

pub(super) fn branch(reference: &str) -> Result<&str, String> {
    reference
        .strip_prefix("refs/heads/")
        .filter(|_| git2::Reference::is_valid_name(reference))
        .ok_or_else(|| "PR merge requires a configured local base branch".into())
}
