use crate::tracker::Actor;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSnapshot {
    pub id: String,
    /// Assigned when the metadata write wins its version check.
    pub number: u64,
    pub created_at: String,
    pub author: Actor,
    pub directories: Vec<ReviewDirectory>,
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
