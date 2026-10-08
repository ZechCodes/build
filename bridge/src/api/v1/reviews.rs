//! Saved workspace reviews, independently usable by agents (wire 3.6.0).

use super::{answer, Answer};
use crate::api::ApiError;
use crate::app::AppState;
use crate::reviews::actions::ReviewActParams;
use crate::reviews::read::{ReviewReadRequest, ReviewReadResult};
use crate::reviews::records::Review;
use crate::tracker::Actor;
use crate::{v1_method, v1_methods};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(crate) mod errors;
mod validation;

pub fn methods() -> &'static [(&'static str, super::Handler)] {
    v1_methods![
        v1_method!("tasks.review.open", open, ReviewOpenParams, ReviewOpenResult),
        v1_method!("tasks.review.push", push, ReviewPushParams, ReviewPushResult),
        v1_method!("tasks.review.update", update, ReviewUpdateParams, ReviewResult),
        v1_method!("tasks.review.merge", merge, ReviewMergeParams, ReviewResult),
        v1_method!("tasks.review.close", close, ReviewCloseParams, ReviewResult),
        v1_method!("tasks.review.reopen", reopen, ReviewVersionParams, ReviewResult),
        v1_method!("tasks.review.refresh", refresh, ReviewVersionParams, ReviewResult),
        v1_method!(
            "tasks.review.snapshot",
            snapshot,
            ReviewSnapshotParams,
            ReviewResult
        ),
        v1_method!("tasks.review.get", get, ReviewGetParams, ReviewResult),
        v1_method!("tasks.review.act", act, ReviewActParams, ReviewResult),
        v1_method!(
            "tasks.review.diff",
            diff,
            ReviewDiffParams,
            ReviewReadResult
        ),
        v1_method!(
            "tasks.review.complete",
            complete,
            ReviewCompleteParams,
            ReviewResult
        ),
    ]
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReviewSnapshotParams {
    pub task_id: String,
    pub workspace_id: String,
    /// Zero creates the review; later calls use the version returned by get.
    pub expected_version: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub base_overrides: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReviewGetParams {
    pub task_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReviewDiffParams {
    pub task_id: String,
    pub snapshot_id: String,
    pub directory_id: String,
    #[serde(flatten)]
    pub read: ReviewReadRequest,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReviewCompleteParams {
    pub task_id: String,
    pub expected_version: u64,
    pub description: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReviewResult {
    pub review: Option<Review>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sync: Vec<crate::reviews::model::ReviewSyncObservation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merge_intents: Vec<crate::reviews::model::ReviewMergeIntent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewBaseSelection {
    pub directory_id: String,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReviewReviewer {
    User,
    ProjectAgent,
    Agent { agent_id: String },
}

impl ReviewReviewer {
    pub fn assignee(&self) -> crate::tracker::Assignee {
        match self {
            Self::User => crate::tracker::Assignee::User,
            Self::ProjectAgent => crate::tracker::Assignee::ProjectAgent,
            Self::Agent { agent_id } => crate::tracker::Assignee::Agent { agent_id: agent_id.clone() },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewOpenParams {
    pub workspace_id: String,
    pub request_id: String,
    pub title: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<ReviewReviewer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bases: Vec<ReviewBaseSelection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_git_directory_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewPushSource {
    pub directory_id: String,
    pub expected_head: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_received_head: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force_with_lease: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewPushParams {
    pub task_id: String,
    pub expected_version: u64,
    pub sources: Vec<ReviewPushSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewUpdateParams {
    pub task_id: String,
    pub expected_version: u64,
    pub bases: Vec<ReviewBaseSelection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewMergePushSelection {
    pub remote: String,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewMergeSelection {
    pub directory_id: String,
    pub expected_base_head: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<ReviewMergePushSelection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewMergeParams {
    pub task_id: String,
    pub expected_version: u64,
    pub snapshot_id: String,
    pub sources: Vec<ReviewMergeSelection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewCloseParams {
    pub task_id: String,
    pub expected_version: u64,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewVersionParams {
    pub task_id: String,
    pub expected_version: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReviewOpenResult {
    pub task: crate::tracker::Task,
    pub review: Review,
    pub opening_state: crate::reviews::model::ReviewOpeningState,
    pub reviewer_dispatch: crate::reviews::opening::ReviewerDispatch,
    pub push_instructions: Vec<ReviewPushInstruction>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReviewPushInstruction {
    pub directory_id: String,
    pub remote: String,
    pub branch: String,
    pub refspec: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReviewPushResult {
    #[serde(flatten)]
    pub state: ReviewResult,
    pub sources: Vec<crate::reviews::publication::explicit::PushOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
}

fn open(app: &mut AppState, params: ReviewOpenParams) -> Result<Answer<ReviewOpenResult>, ApiError> {
    answer(app.review_open(params, Actor::User))
}

fn push(app: &mut AppState, params: ReviewPushParams) -> Result<Answer<ReviewPushResult>, ApiError> {
    answer(app.review_push(params, Actor::User))
}

fn update(app: &mut AppState, params: ReviewUpdateParams) -> Result<Answer<ReviewResult>, ApiError> {
    answer(app.review_update(params, Actor::User))
}

fn merge(app: &mut AppState, params: ReviewMergeParams) -> Result<Answer<ReviewResult>, ApiError> {
    answer(app.review_merge(params, Actor::User))
}

fn close(app: &mut AppState, params: ReviewCloseParams) -> Result<Answer<ReviewResult>, ApiError> {
    answer(app.review_close(params, Actor::User))
}

fn reopen(app: &mut AppState, params: ReviewVersionParams) -> Result<Answer<ReviewResult>, ApiError> {
    answer(app.review_reopen(params, Actor::User))
}

fn refresh(app: &mut AppState, params: ReviewVersionParams) -> Result<Answer<ReviewResult>, ApiError> {
    answer(app.review_refresh(params, Actor::User))
}

fn snapshot(
    app: &mut AppState,
    params: ReviewSnapshotParams,
) -> Result<Answer<ReviewResult>, ApiError> {
    answer(app.review_snapshot(params, Actor::User))
}

fn get(app: &mut AppState, params: ReviewGetParams) -> Result<Answer<ReviewResult>, ApiError> {
    answer(app.review_get(&params.task_id))
}

fn diff(
    app: &mut AppState,
    params: ReviewDiffParams,
) -> Result<Answer<ReviewReadResult>, ApiError> {
    answer(app.review_diff(params))
}

fn complete(
    app: &mut AppState,
    params: ReviewCompleteParams,
) -> Result<Answer<ReviewResult>, ApiError> {
    answer(app.review_complete(params, Actor::User))
}

fn act(app: &mut AppState, params: ReviewActParams) -> Result<Answer<ReviewResult>, ApiError> {
    answer(app.review_act(params, Actor::User))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pull_request_verbs_are_registered_and_refuse_undeclared_parameters() {
        for verb in ["open", "push", "update", "merge", "close", "reopen", "refresh"] {
            let name = format!("tasks.review.{verb}");
            let handler = methods().iter().find(|(method, _)| *method == name)
                .unwrap_or_else(|| panic!("missing {name}"));
            assert!(handler.1.parse_params(&json!({"author": {"kind":"user"}, "path":"/tmp/repo"})).is_err());
        }
    }
}
