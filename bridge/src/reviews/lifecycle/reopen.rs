//! Restore only a recoverable closed PR; Git publication shares sync recovery.
use crate::reviews::model::{
    PullRequestStatus, ReviewBranchBinding, ReviewMode, ReviewPublicationState,
};
use crate::reviews::publication::{self, ReceivedCommit};
use crate::reviews::records::Review;
use crate::reviews::sync::reconcile::{capture, recovery::CaptureJournal};
use crate::store::Store;
use crate::tracker::Actor;
use crate::workspace::{Workspace, WorkspaceSource, WorkspaceStatus, MANIFEST_FILE};

pub struct ReopenRequest {
    pub task_id: String,
    pub expected_version: u64,
    pub actor: Actor,
    pub workspace: Workspace,
    pub sources: Vec<WorkspaceSource>,
}

/// The adapter holds a workspace mutation lease and checks live reclaim and
/// placement state. Reopening does not change the workspace's removal lock.
pub trait ReopenHooks {
    fn check_workspace(&self, workspace: &Workspace) -> Result<(), String>;
}

pub fn reopen(
    store: &Store,
    request: &ReopenRequest,
    hooks: &dyn ReopenHooks,
) -> Result<Review, String> {
    let review = store
        .load_review_sync_state(&request.task_id)
        .map_err(|error| error.to_string())?
        .ok_or("PR review missing")?;
    validate_review(&review, request)?;
    validate_workspace(&review, request)?;
    hooks.check_workspace(&request.workspace)?;
    let mut journal = CaptureJournal::acquire(&review.task_id, &review.bindings)?;
    journal.recover(store, &review.bindings)?;
    let snapshot_id = uuid::Uuid::new_v4().to_string();
    journal.begin(store, &snapshot_id)?;
    let result = publish(store, request, hooks, &review, &snapshot_id);
    let cleanup = journal.finish(store, &review.bindings);
    if let Err(error) = cleanup {
        eprintln!("review reopen: retain candidate {snapshot_id} for recovery: {error}");
    }
    result?;
    store
        .load_review(&request.task_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "PR review missing after reopen".into())
}

fn publish(
    store: &Store,
    request: &ReopenRequest,
    hooks: &dyn ReopenHooks,
    review: &Review,
    snapshot_id: &str,
) -> Result<Review, String> {
    let tips: Vec<ReceivedCommit> = review
        .bindings
        .iter()
        .map(publication::observe_received)
        .collect::<Result<_, _>>()?;
    let mut snapshot = capture(review, snapshot_id, &tips)?;
    snapshot.author = request.actor.clone();
    // This explicit publication resets review context even with the same vector.
    snapshot.publication = None;
    let mut bindings = review.bindings.clone();
    for (binding, tip) in bindings.iter_mut().zip(tips) {
        binding.last_received_head = Some(tip.head);
        binding.publication = ReviewPublicationState::Published;
        binding.recovery = None;
    }
    publication::with_received_snapshot_locked(&bindings, &review.task_id, snapshot_id, || {
        validate_workspace(review, request)?;
        hooks.check_workspace(&request.workspace)?;
        store
            .reopen_review_received_snapshot(
                &request.task_id,
                request.expected_version,
                snapshot,
                &bindings,
                &request.actor,
            )
            .map_err(|error| error.to_string())
    })
}

fn validate_review(review: &Review, request: &ReopenRequest) -> Result<(), String> {
    if review.mode != ReviewMode::PullRequest
        || review
            .pull_request
            .as_ref()
            .is_none_or(|pr| pr.status != PullRequestStatus::Closed)
    {
        return Err("only Closed unmerged PRs can reopen; open a new PR after merge".into());
    }
    if review.version != request.expected_version {
        return Err(format!(
            "conflict: review version changed: expected {}, found {}",
            request.expected_version, review.version
        ));
    }
    if review.workspace_id != request.workspace.id || review.bindings.is_empty() {
        return Err(
            "restore the original review workspace and Git bindings before reopening".into(),
        );
    }
    Ok(())
}

fn validate_workspace(review: &Review, request: &ReopenRequest) -> Result<(), String> {
    let saved: Workspace = std::fs::read(request.workspace.root.join(MANIFEST_FILE))
        .map_err(|_| "restore the original review workspace before reopening".to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))?;
    if saved.id != review.workspace_id
        || saved.project_id != request.workspace.project_id
        || saved.root != request.workspace.root
        || saved.status != WorkspaceStatus::Ready
        || !saved.managed
    {
        return Err("restore the original managed review workspace before reopening".into());
    }
    for binding in &review.bindings {
        validate_binding(binding, &saved, &request.sources)?;
    }
    Ok(())
}

fn validate_binding(
    binding: &ReviewBranchBinding,
    workspace: &Workspace,
    sources: &[WorkspaceSource],
) -> Result<(), String> {
    let directory = workspace
        .directories
        .iter()
        .find(|dir| dir.id == binding.directory_id)
        .ok_or("restore the original review directory before reopening")?;
    let source = sources
        .iter()
        .find(|source| source.id == binding.source_id)
        .ok_or("review source is no longer configured; restore it before reopening")?;
    if directory.source_id != binding.source_id
        || directory.path != binding.working_repository
        || directory.source_path != binding.source_repository
        || !directory.is_git
        || source
            .path
            .canonicalize()
            .map_err(|error| error.to_string())?
            != binding
                .source_repository
                .canonicalize()
                .map_err(|error| error.to_string())?
    {
        return Err("review source or workspace identity changed; restore the original bindings before reopening".into());
    }
    publication::registered_received_head(binding)?;
    crate::reviews::receivers::validate_binding_receiver(binding)?;
    let working = git2::Repository::open(&binding.working_repository)
        .map_err(|_| "restore the original review checkout before reopening")?;
    working
        .find_reference(&binding.dedicated_branch_ref)
        .and_then(|reference| reference.peel_to_commit())
        .map_err(|_| "restore the original review branch before reopening")?;
    let head = working.head().map_err(|error| error.to_string())?;
    if head.name() != Some(binding.dedicated_branch_ref.as_str()) {
        return Err("restore the dedicated review branch checkout before reopening".into());
    }
    publication::validate_bound_remote(binding)
}
