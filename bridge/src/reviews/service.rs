//! The review operation shared by RPC and MCP. Git runs off the app lock;
//! SQLite accepts the captured identities only while the version still matches.

use super::capture::{capture, cleanup_pins};
use super::records::Review;
use crate::store::Store;
use crate::tracker::Actor;
use crate::workspace::Workspace;
use std::collections::BTreeMap;

pub struct SnapshotRequest {
    pub task_id: String,
    pub workspace: Workspace,
    pub expected_version: u64,
    pub base_overrides: BTreeMap<String, String>,
    pub author: Actor,
}

pub fn snapshot(store: &Store, request: &SnapshotRequest) -> Result<Review, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let captured = capture(
        &request.task_id,
        &id,
        &request.workspace,
        &request.base_overrides,
        &request.author,
    )?;
    let saved = store.save_review_snapshot(
        &request.task_id,
        &request.workspace.id,
        request.expected_version,
        captured.clone(),
    );
    match saved {
        Ok((review, replaced)) => {
            for snapshot in replaced {
                if let Err(error) = cleanup_pins(&request.task_id, &snapshot) {
                    eprintln!("release replaced review snapshot {}: {error}", snapshot.id);
                }
            }
            Ok(review)
        }
        Err(error) => {
            if let Err(cleanup) = cleanup_pins(&request.task_id, &captured) {
                eprintln!("release refused review snapshot {}: {cleanup}", captured.id);
            }
            Err(error.to_string())
        }
    }
}

/// Explicit task-history deletion, unlike removing a workspace or a project
/// from the registry. Call off the app lock; closing a task keeps its refs.
#[cfg(test)]
pub fn delete_project_history(store: &Store, project_path: &str) -> Result<(), String> {
    let pins = store
        .delete_tracker_tasks_with_checkpoint(project_path, plan_history_pins, None)
        .map_err(|error| error.to_string())?;
    super::history_pins::release(&pins)
}

pub(crate) fn delete_project_history_checkpoint(
    store: &Store,
    project_path: &str,
    deletion: Option<&crate::store::ProjectDeletion>,
) -> Result<(), String> {
    store
        .delete_tracker_tasks_with_checkpoint(project_path, plan_history_pins, deletion)
        .map(|_| ())
        .map_err(|error| error.to_string())
}
fn plan_history_pins(
    review: &Review,
) -> Result<super::history_pins::HistoryPins, crate::store::StoreError> {
    super::history_pins::plan(review).map_err(crate::store::StoreError::ReviewPinCleanup)
}

#[cfg(test)]
mod tests;
