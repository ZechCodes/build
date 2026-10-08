//! Recoverable, internal-only PR creation. Call off the app mutex.

mod delivery;
pub mod git;
mod journal;
mod preparation;
mod recovery;

use delivery::{deliver_reviewer, initialize_dispatch};
pub use delivery::{retry_reviewer_dispatch, reviewer_dispatch};
pub use preparation::preview_branches;
use preparation::{prepare, validate_request};
pub use recovery::{cancel, interrupt_unfinished};

use super::model::{
    ReviewOpening, ReviewOpeningRequest, ReviewOpeningState, ReviewPreparationState,
    ReviewPublicationState, ReviewSnapshot,
};
use super::records::Review;
use super::{publication, receivers};
use crate::store::Store;
use crate::tracker::{Task, TaskEvent, TaskEventKind};
use crate::workspace::Workspace;
use std::path::PathBuf;

pub struct OpenReviewRequest {
    pub project_path: String,
    pub request_id: String,
    pub receiver_root: PathBuf,
    pub workspace: Workspace,
    pub request: ReviewOpeningRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpeningStep {
    Reserved,
    Planned,
    ReceiverCreated,
    BranchCreated,
    RemoteConfigured,
    RefReceived,
    SnapshotPinned,
    WorkspaceRecorded,
    Published,
}

/// The adapter holds its workspace mutation lease for the entire call. Its
/// checks consult live reclaim reservations and placement under the app lock;
/// Git and file work run off that lock. Dispatch must deduplicate operation_id.
pub trait OpeningHooks {
    fn check_workspace(&self, workspace: &Workspace) -> Result<(), String>;
    fn workspace_changed(&self, workspace: &Workspace) -> Result<(), String>;
    fn dispatch_reviewer(&self, task: &Task, operation_id: &str) -> Result<(), String>;
    fn checkpoint(&self, _step: OpeningStep) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedReview {
    pub task: Task,
    pub review: Review,
    pub reviewer_dispatch: ReviewerDispatch,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ReviewerDispatch {
    NotRequested,
    Pending,
    Delivered,
    Failed { error: String },
}

pub fn open(
    store: &Store,
    request: &OpenReviewRequest,
    hooks: &dyn OpeningHooks,
) -> Result<OpenedReview, String> {
    let _lock = journal::lock(request)?;
    if let Some(existing) = store
        .load_review_opening(&request.project_path, &request.request_id)
        .map_err(|error| error.to_string())?
    {
        if existing.request != request.request {
            return Err("conflict: request_id already names a different review opening".into());
        }
        if existing.state == ReviewOpeningState::Published {
            return result(store, request, &existing.task.id);
        }
    }
    hooks.check_workspace(&request.workspace)?;
    validate_request(request)?;
    let mut opening = store
        .reserve_review_opening(
            &request.project_path,
            &request.request_id,
            request.request.clone(),
        )
        .map_err(|error| error.to_string())?;
    if opening.state == ReviewOpeningState::Cancelled {
        return Err("conflict: review opening was cancelled; use a new request_id".into());
    }
    let prepared = prepare(store, request, hooks, &mut opening);
    match prepared {
        Ok(review) => {
            hooks.checkpoint(OpeningStep::Published)?;
            deliver_reviewer(store, request, hooks, &review.task_id, false)?;
            result(store, request, &review.task_id)
        }
        Err(error) => {
            record_failure(store, &mut opening, &error);
            Err(error)
        }
    }
}

fn save(store: &Store, opening: &mut ReviewOpening) -> Result<(), String> {
    *opening = store
        .save_review_opening(opening, opening.version)
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn record_failure(store: &Store, opening: &mut ReviewOpening, error: &str) {
    // Publication may have committed before a lost response. Its task must
    // never be relabelled as a failed preparation or unwound.
    if store
        .load_review_opening(&opening.project_path, &opening.request_id)
        .ok()
        .flatten()
        .is_some_and(|saved| saved.state == ReviewOpeningState::Published)
    {
        return;
    }
    opening.state = ReviewOpeningState::Failed;
    opening.error = Some(error.into());
    for binding in &mut opening.bindings {
        binding.recovery = Some(error.into());
    }
    if let Err(saved) = save(store, opening) {
        eprintln!(
            "record review opening failure {}: {saved}",
            opening.request_id
        );
    }
}

fn result(
    store: &Store,
    request: &OpenReviewRequest,
    task_id: &str,
) -> Result<OpenedReview, String> {
    let task = store
        .load_tracker_task(task_id)
        .map_err(|error| error.to_string())?
        .ok_or("published review task is unavailable")?;
    let review = store
        .load_review(task_id)
        .map_err(|error| error.to_string())?
        .ok_or("published review is unavailable")?;
    Ok(OpenedReview {
        task,
        review,
        reviewer_dispatch: journal::dispatch(&request.receiver_root, task_id)?,
    })
}

#[cfg(test)]
mod tests;
