use crate::tracker::{Actor, Assignee, Task};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<ReviewSnapshotPublication>,
    pub id: String,
    /// Assigned when the metadata write wins its version check.
    pub number: u64,
    pub created_at: String,
    pub author: Actor,
    pub directories: Vec<ReviewDirectory>,
}

/// Publication cause and selected target tips, separate from comparison bases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSnapshotPublication {
    pub reason: ReviewPublicationReason,
    pub directories: Vec<ReviewPublishedDirectory>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewPublicationReason {
    Received,
    BaseChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewPublishedDirectory {
    pub directory_id: String,
    pub target_head: String,
    pub rewritten: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewDirectory {
    pub id: String,
    pub source_id: String,
    pub name: String,
    pub path: PathBuf,
    pub source_path: PathBuf,
    pub is_git: bool,
    pub status: ReviewDirectoryStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub common_git_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<ReviewBase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncommitted_files: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDirectoryStatus {
    Git,
    NotGit,
    NoCommits,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewBase {
    pub kind: ReviewBaseKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub oid: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewBaseKind {
    Override,
    Configured,
    Upstream,
    EmptyTree,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewMode {
    #[default]
    Snapshot,
    PullRequest,
}

impl ReviewMode {
    pub fn is_snapshot(&self) -> bool {
        *self == Self::Snapshot
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestStatus {
    Open,
    ChangesRequested,
    Approved,
    Merged,
    Closed,
}

impl PullRequestStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Open | Self::ChangesRequested | Self::Approved)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestMetadata {
    pub status: PullRequestStatus,
    pub creator: Actor,
    pub originating_workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_published_snapshot_id: Option<String>,
    pub directories: Vec<ReviewMembership>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewMembership {
    pub directory_id: String,
    pub source_id: String,
    pub kind: ReviewMembershipKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewMembershipKind {
    Git,
    Live,
    Excluded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSummary {
    pub task_id: String,
    pub workspace_id: String,
    pub version: u64,
    pub status: PullRequestStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_published_snapshot_id: Option<String>,
}

/// Stable Git placement and recovery progress for one participating directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewBranchBinding {
    pub directory_id: String,
    pub source_id: String,
    pub repository_id: String,
    pub working_repository: PathBuf,
    pub source_repository: PathBuf,
    /// Committed HEAD at opening: restores the original detached HEAD and proves
    /// the owned branch's expected OID before its first push.
    pub initial_head: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_branch_ref: Option<String>,
    pub dedicated_branch_ref: String,
    pub base_branch_ref: String,
    pub receiving_repository: PathBuf,
    pub receiving_ref: String,
    pub remote_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_received_head: Option<String>,
    pub preparation: ReviewPreparationState,
    pub publication: ReviewPublicationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewPreparationState {
    Planned,
    BranchCreated,
    RemoteConfigured,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewPublicationState {
    Pending,
    Published,
    Failed,
    Interrupted,
}

/// Observed Git state has its own revision, independent of review mutations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSyncObservation {
    pub task_id: String,
    pub directory_id: String,
    pub revision: u64,
    pub health: ReviewSyncHealth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison_base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_commits: Option<u64>,
    pub observed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewSyncHealth {
    Pending,
    Current,
    Unavailable,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewOpeningRequest {
    pub workspace_id: String,
    pub title: String,
    pub description: String,
    pub creator: Actor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<Assignee>,
    pub directories: Vec<ReviewMembership>,
    pub base_branches: BTreeMap<String, String>,
}

/// Reserved task identity and all progress needed to resume or cancel opening.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewOpening {
    pub project_path: String,
    pub request_id: String,
    pub version: u64,
    pub request: ReviewOpeningRequest,
    pub task: Task,
    pub state: ReviewOpeningState,
    pub bindings: Vec<ReviewBranchBinding>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewOpeningState {
    Preparing,
    Published,
    Interrupted,
    Failed,
    Cancelled,
}

impl ReviewOpeningState {
    /// Interrupted and failed openings still own recovery until cancellation.
    pub fn is_claiming_workspace(&self) -> bool {
        matches!(self, Self::Preparing | Self::Interrupted | Self::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewMergeRequest {
    pub task_id: String,
    pub expected_version: u64,
    pub snapshot_id: String,
    pub actor: Actor,
    pub sources: Vec<ReviewMergeSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewMergeSource {
    pub directory_id: String,
    pub repository_id: String,
    pub base_branch_ref: String,
    pub head: String,
    pub expected_base_head: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<ReviewMergePush>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewMergePush {
    pub remote: String,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewMergeIntent {
    pub project_path: String,
    pub request_id: String,
    pub version: u64,
    pub request: ReviewMergeRequest,
    pub state: ReviewMergeState,
    pub action_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewMergeState {
    Running,
    Succeeded,
    Failed,
    Interrupted,
}
