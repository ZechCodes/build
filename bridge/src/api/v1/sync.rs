//! Durable scoped recovery reads, including finite leased device snapshots.

use super::Handler;
use crate::api::ApiError;
use crate::app::AppState;
use crate::store::sync::{ResyncReason, Sequence};
use crate::{v1_method, v1_methods};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub fn methods() -> &'static [(&'static str, Handler)] {
    v1_methods![
        v1_method!("sync.head", unavailable_head, HeadParams, HeadResult),
        v1_method!(
            "sync.snapshot",
            unavailable_snapshot,
            SnapshotParams,
            SnapshotResult
        ),
        v1_method!(
            "sync.changes",
            unavailable_changes,
            ChangesParams,
            ChangesResult
        ),
    ]
}

/// The closed set of supported scopes. Capability names gate client use.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum Scope {
    #[serde(rename = "device")]
    Device,
    #[serde(rename = "project_tasks")]
    ProjectTasks {
        project_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filters: Option<Box<ProjectTaskFilters>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        order: Option<crate::store::TaskQueryOrder>,
    },
    #[serde(rename = "task")]
    Task {
        #[serde(deserialize_with = "crate::renamed_ids::current")]
        task_id: String,
    },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectTaskFilters {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub states: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statuses: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignees: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watched: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unfinished: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undelivered: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention_rule: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_review_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_since: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_assignee: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HeadParams {
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotParams {
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChangesParams {
    pub scope: Scope,
    pub cursor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HeadResult {
    Ok {
        scope: Scope,
        incarnation: String,
        head: Sequence,
        retention_floor: Sequence,
        bridge_epoch: String,
    },
    ResyncRequired {
        scope: Scope,
        incarnation: String,
        head: Sequence,
        bridge_epoch: String,
        reason: ResyncReason,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotConsistency {
    LiveKeyset,
    SingleRead,
    Leased,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Record {
    pub kind: String,
    pub key: String,
    pub revision: Sequence,
    pub row: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<Selection>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Selection {
    pub pending_present: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SnapshotLease {
    pub id: String,
    pub expires_at_ms: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SnapshotResult {
    Ok {
        scope: Scope,
        incarnation: String,
        watermark: Sequence,
        cursor: String,
        bridge_epoch: String,
        consistency: SnapshotConsistency,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lease: Option<SnapshotLease>,
        records: Vec<Record>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_page: Option<String>,
    },
    ResyncRequired {
        scope: Scope,
        incarnation: String,
        head: Sequence,
        bridge_epoch: String,
        reason: ResyncReason,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Upsert,
    Delete,
    Invalidate,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Change {
    pub seq: Sequence,
    pub kind: String,
    pub key: String,
    pub operation: Operation,
    pub revision: Sequence,
    pub member: bool,
    pub row: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<Selection>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ChangesResult {
    Ok {
        scope: Scope,
        incarnation: String,
        head: Sequence,
        cursor: String,
        bridge_epoch: String,
        changes: Vec<Change>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        continuation: Option<String>,
    },
    ResyncRequired {
        scope: Scope,
        incarnation: String,
        head: Sequence,
        bridge_epoch: String,
        reason: ResyncReason,
    },
}

// Production calls are classified as independent SQL reads before mutable
// dispatch. These handlers keep the typed registry closed if a caller bypasses
// that execution boundary.
fn unavailable_head(_: &mut AppState, _: HeadParams) -> Result<HeadResult, ApiError> {
    Err(ApiError::unavailable(
        "sync reads require the independent read lane",
    ))
}
fn unavailable_snapshot(_: &mut AppState, _: SnapshotParams) -> Result<SnapshotResult, ApiError> {
    Err(ApiError::unavailable(
        "sync reads require the independent read lane",
    ))
}
fn unavailable_changes(_: &mut AppState, _: ChangesParams) -> Result<ChangesResult, ApiError> {
    Err(ApiError::unavailable(
        "sync reads require the independent read lane",
    ))
}
