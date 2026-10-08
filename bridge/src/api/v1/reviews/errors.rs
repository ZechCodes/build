//! Structured refusals survive the deferred Git boundary without new codes.
use crate::api::ApiError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const PREFIX: &str = "review_api_error:";

#[derive(Deserialize, Serialize)]
struct Refusal {
    code: String,
    message: String,
    details: Value,
}

pub(crate) fn encode(code: &str, message: impl Into<String>, details: Value) -> String {
    let refusal = Refusal { code: code.into(), message: message.into(), details };
    format!("{PREFIX}{}", serde_json::to_string(&refusal).expect("refusal serializes"))
}

pub(crate) fn decode(message: &str) -> Option<ApiError> {
    let refusal: Refusal = serde_json::from_str(message.strip_prefix(PREFIX)?).ok()?;
    let message = refusal.message;
    let details = Some(refusal.details);
    Some(match refusal.code.as_str() {
        "invalid_params" => ApiError::InvalidParams { message, details },
        "not_found" => ApiError::NotFound { message, details },
        "conflict" => ApiError::Conflict { message, details },
        "busy" => ApiError::Busy { message, details },
        "stale_version" => ApiError::StaleVersion { message, details },
        "unavailable" => ApiError::Unavailable { message, details },
        _ => ApiError::Internal { message, details },
    })
}

pub(crate) fn service(message: String, details: Value) -> String {
    if decode(&message).is_some() {
        return message;
    }
    let code = service_code(&message);
    encode(code, crate::source_sync::without_credentials(&message), details)
}

fn service_code(message: &str) -> &str {
    if message.starts_with("stale") {
        return "stale_version";
    }
    if message.contains("already running") || message.starts_with("busy:") || message == crate::reclaim::BUSY || message == crate::reclaim::RESERVED {
        return "busy";
    }
    if message.starts_with("unknown ") {
        return "not_found";
    }
    if message.starts_with("invalid review params:") {
        return "invalid_params";
    }
    if message.starts_with("conflict:") || message.starts_with("invalid pull request:") || message.contains("already belongs to active PR") || message.contains("changed") || message.contains("before reopening") || message.starts_with("only Closed") || message.contains("unrelated") || message.contains("base branch") || message.contains("mid-operation") || message.contains("in-progress Git") || message.contains("is already completed") || message.starts_with("review request ") {
        return "conflict";
    }
    if message.starts_with("unavailable:") || message.contains("unavailable") || message.contains("missing") || message.starts_with("restore ") {
        return "unavailable";
    }
    "internal"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn structured_refusals_survive_the_deferred_boundary() {
        let error = ApiError::classify(encode("conflict", "received ref changed", json!({"directory_id":"dir-1","recovery":"Refresh the published review"})));
        assert_eq!(error.code(), "conflict");
        assert_eq!(error.details().unwrap()["directory_id"], "dir-1");
        assert!(!error.retryable());
    }
}
