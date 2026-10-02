use crate::reviews::model::ReviewDirectory;
use crate::tracker::Actor;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ReviewActParams {
    pub task_id: String,
    pub expected_version: u64,
    pub snapshot_id: String,
    pub sources: Vec<SourceSelection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSelection {
    pub directory_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<MergeSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<PushSelection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MergeSelection {
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PushSelection {
    pub remote: String,
    pub branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_action_id: Option<String>,
}

pub struct ActionRequest {
    pub params: ReviewActParams,
    pub actor: Actor,
    pub sources: Vec<ActionSource>,
}

/// Identities resolved by the app from its configured project sources. Clients
/// name a saved directory, never a repository path.
#[derive(Debug, Clone)]
pub struct ActionSource {
    pub directory: ReviewDirectory,
    pub source_path: PathBuf,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    Running,
    Succeeded,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Merge,
    Push,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ReviewAction {
    pub id: String,
    pub snapshot_id: String,
    pub directory_id: String,
    pub source_name: String,
    pub source_path: PathBuf,
    pub actor: Actor,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    pub status: ActionStatus,
    pub steps: Vec<ActionStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ActionStep {
    pub kind: StepKind,
    pub branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// A push-only retry keeps the original successful merge's identity, so
    /// another failed attempt can still retry that same tip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_action_id: Option<String>,
    pub status: StepStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ReviewDestination {
    pub snapshot_id: String,
    pub directory_id: String,
    pub source_path: PathBuf,
    pub branches: Vec<String>,
    pub remotes: Vec<ReviewRemote>,
    pub live_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ReviewRemote {
    pub name: String,
    pub branches: Vec<String>,
}
