use crate::api::ApiError;
use crate::app::read_view::ReadView;
use serde_json::{json, Value};

/// The stream verbs predate typed v1 and retain their permissive pagination
/// parameters, legacy refusal codes, and QA-only method visibility.
pub(super) fn read(
    view: &ReadView,
    method: &str,
    params: &Value,
    qa: bool,
) -> Result<Value, ApiError> {
    if !qa || !matches!(method, "stream.events" | "stream.state") {
        return Err(ApiError::unknown_method(method));
    }
    let stream_id = crate::app::require_str(params, "stream_id").map_err(ApiError::from)?;
    let summary = view
        .get_owned("stream.state", &stream_id)
        .ok_or_else(|| ApiError::internal("unknown stream_id"))?;
    if method == "stream.state" {
        return Ok(summary);
    }
    events(view, &stream_id, params, &summary)
}

fn events(
    view: &ReadView,
    stream_id: &str,
    params: &Value,
    summary: &Value,
) -> Result<Value, ApiError> {
    let since = params.get("since").and_then(Value::as_u64).unwrap_or(0);
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(64)
        .clamp(1, 256) as usize;
    let head = summary["head"].as_u64().unwrap_or(0);
    let mut next = since;
    let mut events = Vec::with_capacity(limit);
    // `next < head` also prevents overflow when the caller passes u64::MAX.
    while next < head && events.len() < limit {
        let seq = next + 1;
        let event = view
            .get_owned("stream.events", &format!("{stream_id}/{seq}"))
            .ok_or_else(|| ApiError::internal("stream event snapshot was not initialized"))?;
        events.push(event);
        next = seq;
    }
    Ok(json!({
        "events": events,
        "next": next,
        "head": head,
        "complete": summary["complete"],
    }))
}
