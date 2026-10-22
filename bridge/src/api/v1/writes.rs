//! Typed recovery reads. Runtime execution uses the independent SQL lane.
use super::Handler;
use crate::api::writes::{StatusParams, StatusResult, SubscribeParams, SubscribeResult};
use crate::api::ApiError;
use crate::app::AppState;
use crate::{v1_method, v1_methods};

pub fn methods() -> &'static [(&'static str, Handler)] {
    v1_methods![
        v1_method!("writes.status", status, StatusParams, StatusResult),
        v1_method!(
            "writes.subscribe",
            subscribe,
            SubscribeParams,
            SubscribeResult
        ),
    ]
}

fn status(_app: &mut AppState, _params: StatusParams) -> Result<StatusResult, ApiError> {
    Err(ApiError::unavailable(
        "write recovery requires the authenticated SQL read lane",
    ))
}

fn subscribe(_app: &mut AppState, _params: SubscribeParams) -> Result<SubscribeResult, ApiError> {
    Err(ApiError::unavailable(
        "write subscriptions require the authenticated SQL read lane",
    ))
}
