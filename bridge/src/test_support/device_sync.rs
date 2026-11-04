//! Retry policy for device snapshot admission in wire tests only.
use serde_json::Value;
use std::time::Duration;

pub(crate) const ADMISSION_ATTEMPTS: usize = 32;

/// A busy writer is an expected retryable outcome. Every other error is a
/// contract failure and must fail immediately, including other unavailability.
pub(crate) async fn admission_ready(answer: &Value, attempt: usize) -> Result<bool, String> {
    if answer["ok"] == true {
        return Ok(true);
    }
    if answer["error_code"] != "unavailable"
        || answer["retryable"] != true
        || answer["error"] != "device snapshot admission busy"
    {
        return Err(format!("unexpected snapshot response: {answer}"));
    }
    if attempt >= ADMISSION_ATTEMPTS {
        return Err(format!(
            "snapshot admission still busy after {attempt} attempts: {answer}"
        ));
    }
    tokio::time::sleep(Duration::from_millis(5)).await;
    Ok(false)
}

#[tokio::test]
async fn device_sync_wire_retry_policy_rejects_other_errors_and_bounds_busy() {
    use serde_json::json;
    for answer in [
        json!({"error_code":"unavailable","retryable":true,"error":"sync_reconciling"}),
        json!({"error_code":"unavailable","retryable":true,"error":"device lease quota exceeded"}),
        json!({"error_code":"invalid_params","retryable":true,"error":"device snapshot admission busy"}),
        json!({"error_code":"unavailable","retryable":false,"error":"device snapshot admission busy"}),
        json!({"error_code":"unavailable","error":"device snapshot admission busy"}),
    ] {
        assert!(admission_ready(&answer, 1).await.is_err(), "{answer}");
    }
    let busy = json!({"error_code":"unavailable","retryable":true,"error":"device snapshot admission busy"});
    assert!(admission_ready(&busy, ADMISSION_ATTEMPTS).await.is_err());
    assert!(admission_ready(&json!({"ok":true}), 1).await.unwrap());
}
