use super::*;
use crate::reviews::model::ReviewDirectory;
use std::collections::HashSet;

pub(super) struct PreparedSource {
    pub source: ActionSource,
    pub action: ReviewAction,
    pub head: Option<String>,
    pub merged: bool,
}

pub(super) fn prepare(
    review: &Review,
    request: &ActionRequest,
) -> Result<Vec<PreparedSource>, String> {
    let snapshot = review
        .snapshots
        .iter()
        .find(|snapshot| snapshot.id == request.params.snapshot_id)
        .ok_or_else(|| format!("unknown snapshot_id: {}", request.params.snapshot_id))?;
    if request.params.sources.is_empty() || request.params.sources.len() > 100 {
        return Err("sources must contain 1 to 100 directory selections".into());
    }
    let mut seen = HashSet::new();
    let mut prepared = Vec::new();
    for selection in &request.params.sources {
        if !seen.insert(&selection.directory_id) {
            return Err(format!(
                "duplicate directory_id: {}",
                selection.directory_id
            ));
        }
        let directory = snapshot
            .directories
            .iter()
            .find(|directory| directory.id == selection.directory_id)
            .ok_or_else(|| format!("unknown directory_id: {}", selection.directory_id))?;
        if selection.merge.is_some() || selection.push.is_some() {
            prepared.push(prepare_source(review, request, directory, selection));
        }
    }
    Ok(prepared)
}

fn prepare_source(
    review: &Review,
    request: &ActionRequest,
    directory: &ReviewDirectory,
    selection: &SourceSelection,
) -> PreparedSource {
    let mut source = request
        .sources
        .iter()
        .find(|source| source.directory.id == directory.id)
        .cloned()
        .unwrap_or_else(|| ActionSource {
            directory: directory.clone(),
            source_path: directory.source_path.clone(),
            error: Some("Source unavailable".into()),
        });
    source.directory = directory.clone();
    let mut head = directory.head.clone();
    let mut merged = false;
    let retry_id = selection
        .push
        .as_ref()
        .and_then(|push| push.merge_action_id.as_deref())
        .filter(|_| selection.merge.is_none());
    if let Some(id) = retry_id {
        match merge_tip(review, request, directory, id) {
            Ok(tip) => {
                head = Some(tip);
                merged = true;
            }
            Err(error) => source.error = Some(error),
        }
    }
    let mut steps = Vec::new();
    if let Some(merge) = &selection.merge {
        head = directory.head.clone();
        merged = false;
        steps.push(step(StepKind::Merge, &merge.branch, None));
    }
    if let Some(push) = &selection.push {
        let mut push_step = step(StepKind::Push, &push.branch, Some(push.remote.clone()));
        push_step.merge_action_id = retry_id.map(str::to_owned);
        steps.push(push_step);
    }
    PreparedSource {
        action: ReviewAction {
            id: uuid::Uuid::new_v4().to_string(),
            snapshot_id: request.params.snapshot_id.clone(),
            directory_id: directory.id.clone(),
            source_name: directory.name.clone(),
            source_path: source.source_path.clone(),
            actor: request.actor.clone(),
            started_at: now_rfc3339(),
            finished_at: None,
            status: ActionStatus::Running,
            steps,
        },
        source,
        head,
        merged,
    }
}

fn merge_tip(
    review: &Review,
    request: &ActionRequest,
    directory: &ReviewDirectory,
    id: &str,
) -> Result<String, String> {
    review
        .actions
        .iter()
        .find(|action| {
            action.id == id
                && action.snapshot_id == request.params.snapshot_id
                && action.directory_id == directory.id
        })
        .and_then(|action| {
            action
                .steps
                .iter()
                .find(|step| step.kind == StepKind::Merge && step.status == StepStatus::Succeeded)
        })
        .and_then(|step| step.result_head.clone())
        .ok_or_else(|| format!("No successful merge recorded for this source in action {id}."))
}

fn step(kind: StepKind, branch: &str, remote: Option<String>) -> ActionStep {
    ActionStep {
        kind,
        branch: branch.into(),
        remote,
        merge_action_id: None,
        status: StepStatus::Pending,
        input_head: None,
        result_head: None,
        error: None,
        warning: None,
    }
}
