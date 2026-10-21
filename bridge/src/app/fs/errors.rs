//! Keep existing typed filesystem refusals intact through the deferred drain.
use crate::api::ApiError;
use crate::app::AppState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const PREFIX: &str = "fs_api_error:";
#[derive(Serialize, Deserialize)]
struct Refusal {
    code: String,
    message: String,
    details: Option<Value>,
}

pub(super) fn encode(error: ApiError) -> String {
    let refusal = Refusal {
        code: error.code().into(),
        message: error.message().into(),
        details: error.details().cloned(),
    };
    format!(
        "{PREFIX}{}",
        serde_json::to_string(&refusal).expect("file refusal serializes")
    )
}

impl AppState {
    pub(crate) fn decode_file_error(message: &str) -> Option<ApiError> {
        let Refusal {
            code,
            message,
            details,
        } = serde_json::from_str(message.strip_prefix(PREFIX)?).ok()?;
        Some(match code.as_str() {
            "unknown_method" => ApiError::UnknownMethod { message, details },
            "invalid_params" => ApiError::InvalidParams { message, details },
            "not_found" => ApiError::NotFound { message, details },
            "already_exists" => ApiError::AlreadyExists { message, details },
            "locked" => ApiError::Locked { message, details },
            "conflict" => ApiError::Conflict { message, details },
            "stale_version" => ApiError::StaleVersion { message, details },
            "stale_body" => ApiError::StaleBody { message, details },
            "unavailable" => ApiError::Unavailable { message, details },
            "busy" => ApiError::Busy { message, details },
            "unsupported_version" => ApiError::UnsupportedVersion { message, details },
            _ => ApiError::Internal { message, details },
        })
    }
}
