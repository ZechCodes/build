//! Saved workspace reviews, independently usable by agents (wire 3.6.0).

use super::{answer, Answer};
use crate::api::ApiError;
use crate::app::AppState;
use crate::reviews::read::{ReviewReadRequest, ReviewReadResult};
use crate::reviews::records::Review;
use crate::tracker::Actor;
use crate::{v1_method, v1_methods};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub fn methods() -> &'static [(&'static str, super::Handler)] {
    v1_methods![
        v1_method!(
            "tasks.review.snapshot",
            snapshot,
            ReviewSnapshotParams,
            ReviewResult
        ),
        v1_method!("tasks.review.get", get, ReviewGetParams, ReviewResult),
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
