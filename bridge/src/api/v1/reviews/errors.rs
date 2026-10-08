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

pub(crate) fn encode(code: &str, message: impl Into<String>, mut details: Value) -> String {
    let message = message.into();
    if let Some(details) = details.as_object_mut() {
        details
            .entry("reason")
            .or_insert_with(|| Value::String(reason(&message).unwrap_or(code).into()));
    }
    let refusal = Refusal {
        code: code.into(),
        message,
        details,
    };
    format!(
        "{PREFIX}{}",
        serde_json::to_string(&refusal).expect("refusal serializes")
    )
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

pub(crate) fn service(message: String, mut details: Value) -> String {
    if decode(&message).is_some() {
        return message;
    }
    let code = if let Some((expected, current)) = review_versions(&message) {
        if let Some(details) = details.as_object_mut() {
            details.insert("expected_version".into(), expected.into());
            details.insert("current_version".into(), current.into());
        }
        "stale_version"
    } else {
        service_code(&message)
    };
    encode(
        code,
        crate::source_sync::without_credentials(&message),
        details,
    )
}

// The landed reopen guard and the store's CAS report the same version refusal
// through different strings. Preserve their exact observed versions after any
// deferred precheck without reclassifying unrelated workspace conflicts.
fn review_versions(message: &str) -> Option<(u64, u64)> {
    let versions = message
        .strip_prefix("conflict: review version changed: expected ")
        .or_else(|| {
            message
                .strip_prefix("stale_version: review ")?
                .split_once(" expected version ")
                .map(|(_, versions)| versions)
        })?;
    let (expected, current) = versions.split_once(", found ")?;
    Some((expected.parse().ok()?, current.parse().ok()?))
}

fn service_code(message: &str) -> &str {
    if let Some((_, code, _)) = REFUSALS
        .iter()
        .find(|(needle, _, _)| message.contains(needle))
    {
        return code;
    }
    if message == crate::reclaim::BUSY || message == crate::reclaim::RESERVED {
        return "busy";
    }
    let prefixes = [
        ("stale", "stale_version"),
        ("unknown ", "not_found"),
        ("invalid review params:", "invalid_params"),
        ("conflict:", "conflict"),
        ("invalid pull request:", "conflict"),
        ("busy:", "busy"),
        ("only Closed", "conflict"),
        ("review request ", "conflict"),
        ("restore ", "unavailable"),
    ];
    if let Some((_, code)) = prefixes
        .iter()
        .find(|(prefix, _)| message.starts_with(prefix))
    {
        return code;
    }
    let phrases = [
        ("changed", "conflict"),
        ("unrelated", "conflict"),
        ("base branch", "conflict"),
        ("mid-operation", "conflict"),
        ("before reopening", "conflict"),
        ("is already completed", "conflict"),
        ("unavailable", "unavailable"),
        ("missing", "unavailable"),
    ];
    phrases
        .iter()
        .find(|(phrase, _)| message.contains(phrase))
        .map_or("internal", |(_, code)| code)
}

const REFUSALS: &[(&str, &str, &str)] = &[
    (
        "dedicated review branch already exists",
        "conflict",
        "branch_collision",
    ),
    (
        "dedicated review branch and stable task suffix are already reserved",
        "conflict",
        "branch_collision",
    ),
    ("Git operation in progress", "busy", "git_operation"),
    ("in-progress Git", "busy", "git_operation"),
    ("already running", "busy", "operation_running"),
    ("review base unavailable", "conflict", "missing_base"),
    (
        "already belongs to active PR",
        "conflict",
        "active_workspace_review",
    ),
];

fn reason(message: &str) -> Option<&'static str> {
    REFUSALS
        .iter()
        .find(|(needle, _, _)| message.contains(needle))
        .map(|(_, _, reason)| *reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn structured_refusals_survive_the_deferred_boundary() {
        let error = ApiError::classify(encode(
            "conflict",
            "received ref changed",
            json!({"directory_id":"dir-1","recovery":"Refresh the published review"}),
        ));
        assert_eq!(error.code(), "conflict");
        assert_eq!(error.details().unwrap()["directory_id"], "dir-1");
        assert!(!error.retryable());
    }

    #[test]
    fn target_sync_git_midoperation_refusal_is_busy_with_recovery_details() {
        let error = ApiError::classify(service(
            "A rebase of main is in progress in /sources/api.".into(),
            json!({"directory_id":"dir-api","recovery":"Finish or abort the source Git operation, then retry the saved merge plan."}),
        ));
        assert_eq!(error.code(), "busy");
        assert_eq!(error.details().unwrap()["reason"], "git_operation");
        assert_eq!(error.details().unwrap()["directory_id"], "dir-api");
        assert!(error.retryable());
    }

    #[test]
    fn landed_pr_service_refusals_have_directory_recovery_details_and_stable_codes() {
        for (message, code, reason) in [
            (
                "dedicated review branch already exists for task: task-1",
                "conflict",
                "branch_collision",
            ),
            (
                "review repository has a Git operation in progress",
                "busy",
                "git_operation",
            ),
            (
                "source review base unavailable: reference not found",
                "conflict",
                "missing_base",
            ),
            (
                "workspace workspace-1 already belongs to active PR task-1",
                "conflict",
                "active_workspace_review",
            ),
        ] {
            let error = ApiError::classify(service(
                message.into(),
                json!({"directory_id":"dir-1","recovery":"Resolve the reported state and refresh."}),
            ));
            assert_eq!(error.code(), code, "{message}");
            assert_eq!(error.details().unwrap()["reason"], reason, "{message}");
            assert_eq!(error.details().unwrap()["directory_id"], "dir-1");
        }
    }
}
