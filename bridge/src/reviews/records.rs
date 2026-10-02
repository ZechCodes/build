//! Durable task review metadata. Git content stays in each source repository.

use super::model::ReviewSnapshot;
use crate::tracker::Actor;
use serde::{Deserialize, Serialize};

/// The maximum size of a review's action description after trimming.
pub const MAX_REVIEW_DESCRIPTION_BYTES: usize = 2_000;

/// Keep MCP and storage at the same UTF-8 byte boundary.
pub fn review_description(description: &str) -> Option<&str> {
    let description = description.trim();
    (!description.is_empty() && description.len() <= MAX_REVIEW_DESCRIPTION_BYTES)
        .then_some(description)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Open,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewCompletion {
    pub actor: Actor,
    pub description: String,
    pub completed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub task_id: String,
    pub workspace_id: String,
    pub version: u64,
    pub state: ReviewState,
    pub snapshots: Vec<ReviewSnapshot>,
    pub completion: Option<ReviewCompletion>,
}
