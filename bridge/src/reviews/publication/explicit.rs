//! Explicit publication of bound PR branches and comparison targets.

use crate::reviews::model::ReviewBranchBinding;
use crate::reviews::records::Review;
use crate::store::Store;
use crate::tracker::Actor;
use crate::workspace::{DirectoryStatus, Workspace, WorkspaceStatus, MANIFEST_FILE};
use std::collections::BTreeSet;

mod bases;
mod push;
use serde::{Deserialize, Serialize};

pub struct PushRequest {
    pub task_id: String,
    pub expected_version: u64,
    pub actor: Actor,
    pub sources: Vec<PushSource>,
}

pub struct PushSource {
    pub directory_id: String,
    pub expected_head: String,
    pub expected_received_head: Option<String>,
    pub force_with_lease: bool,
}

#[derive(Debug, Serialize)]
pub struct PushResult {
    pub review: Review,
    pub sources: Vec<PushOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PushOutcome {
    pub directory_id: String,
    pub status: PushStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PushStatus {
    Published,
    Unchanged,
    Failed,
    Interrupted,
}

pub struct BaseRequest {
    pub task_id: String,
    pub expected_version: u64,
    pub actor: Actor,
    pub bases: Vec<BaseSelection>,
}

pub struct BaseSelection {
    pub directory_id: String,
    pub branch: String,
}

pub fn push(store: &Store, request: &PushRequest) -> Result<PushResult, String> {
    push_checked(store, request, &|| Ok(()))
}

pub(crate) fn push_checked(
    store: &Store,
    request: &PushRequest,
    check: &dyn Fn() -> Result<(), String>,
) -> Result<PushResult, String> {
    push::push(store, request, check)
}

pub fn update_bases(store: &Store, request: &BaseRequest) -> Result<Review, String> {
    update_bases_checked(store, request, &|| Ok(()))
}

pub(crate) fn update_bases_checked(
    store: &Store,
    request: &BaseRequest,
    check: &dyn Fn() -> Result<(), String>,
) -> Result<Review, String> {
    bases::update(store, request, check)
}

pub(crate) fn check_workspace(
    expected: &Workspace,
    bindings: &[ReviewBranchBinding],
) -> Result<(), String> {
    let bytes = std::fs::read(expected.root.join(MANIFEST_FILE))
        .map_err(|error| format!("unavailable: original review workspace manifest: {error}"))?;
    let current: Workspace = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if current.id != expected.id
        || current.project_id != expected.project_id
        || current.root != expected.root
        || !current.managed
        || current.status != WorkspaceStatus::Ready
        || current.archived_at.is_some()
    {
        return Err("conflict: original review workspace changed mid-operation".into());
    }
    for binding in bindings {
        if !current.directories.iter().any(|directory| {
            directory.id == binding.directory_id
                && directory.source_id == binding.source_id
                && directory.path == binding.working_repository
                && directory.source_path == binding.source_repository
                && directory.is_git
                && directory.status == DirectoryStatus::Ready
        }) {
            return Err("conflict: bound review directory changed mid-operation".into());
        }
    }
    Ok(())
}

fn active(store: &Store, task_id: &str, expected_version: u64) -> Result<Review, String> {
    let review = store
        .load_review_sync_state(task_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "invalid review params: operation requires a PR-mode review".to_owned())?;
    if review.version != expected_version {
        return Err(format!(
            "stale_version: review {task_id} expected version {expected_version}, found {}",
            review.version
        ));
    }
    if review
        .pull_request
        .as_ref()
        .is_none_or(|pr| !pr.status.is_active())
    {
        return Err("conflict: publication requires an active PR".into());
    }
    if store
        .review_merge_is_running(task_id)
        .map_err(|error| error.to_string())?
    {
        return Err("busy: PR merge is running".into());
    }
    Ok(review)
}

fn selections<'a>(review: &Review, ids: impl Iterator<Item = &'a str>) -> Result<(), String> {
    let ids: Vec<_> = ids.collect();
    if ids.is_empty() || ids.len() > 100 || ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
        return Err("invalid review params: select 1 to 100 unique Git directories".into());
    }
    if ids.iter().any(|id| {
        !review
            .bindings
            .iter()
            .any(|binding| binding.directory_id == *id)
    }) {
        return Err(
            "invalid review params: selection must name a fixed Git review directory".into(),
        );
    }
    Ok(())
}

fn load_full(store: &Store, task_id: &str) -> Result<Review, String> {
    store
        .load_review(task_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "unavailable: PR review is missing".into())
}

#[cfg(test)]
mod tests;
