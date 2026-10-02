//! Durable task review metadata. Git content stays in each source repository.

use super::model::ReviewSnapshot;
use crate::tracker::Actor;
use serde::{Deserialize, Serialize};

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
