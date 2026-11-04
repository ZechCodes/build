use super::fixture::*;
use super::*;
use crate::app::AppState;
use crate::harness::HarnessContext;

#[tokio::test]
async fn device_sync_wire_http_uses_the_same_typed_scope_and_lease_contract() {
    let directory = tempfile::tempdir().unwrap();
    let context = HarnessContext::resolved(
        directory.path().join("mcp.sock"),
        directory.path().to_path_buf(),
    )
    .unwrap();
    let app = AppState::new_unrooted_configured(
        directory.path().join("worktrees"),
        "main",
        true,
        context,
    )
    .with_task_store(directory.path().join("store"))
    .unwrap();
    let writer = app.attached_store_for_test().unwrap().writer();
    let now = crate::store::sync::now_ms();
    let generation = writer
        .mark_sync_dirty("wire_fixture", "http-row", now)
        .unwrap();
    writer
        .reconcile_sync_dirty(
            "wire_fixture",
            "http-row",
            generation,
            Some(json!({"name":"HTTP"})),
            now,
        )
        .unwrap();
    // A bare handler isolates this transport contract from maintenance jobs.
    // Admission remains non-blocking; use the same bounded busy retry policy
    // as the carrier tests if a writer does contend.
    let state = std::sync::Arc::new(std::sync::Mutex::new(app));
    let fixture = Fixture::new(AppState::handler(state));
    let client = fixture.http_client("device-sync-family").await;
    for (body, valid) in [
        (r#"{"scope":{"kind":"device"},"limit":100}"#, true),
        (r#"{"scope":{"kind":"unknown_device"}}"#, false),
    ] {
        let mut attempt = 0;
        let (status, answer) = loop {
            attempt += 1;
            let request = request_as(
                &client.family,
                "POST",
                "/v1/sync/snapshot-queries",
                &client.session,
                Some(body),
            );
            let output = request
                .extensions()
                .get::<crate::http::listener::OutputAuthority>()
                .unwrap()
                .clone();
            let response = fixture.app.clone().oneshot(request).await.unwrap();
            let (status, answer) = json_response(response).await;
            flush(output).await;
            if !valid
                || crate::test_support::device_sync::admission_ready(&answer, attempt)
                    .await
                    .expect("HTTP snapshot admission eventually succeeds")
            {
                break (status, answer);
            }
        };
        if valid {
            assert_eq!(status, StatusCode::OK);
            assert_eq!(answer["ok"], true, "{answer}");
            assert_eq!(answer["result"]["scope"]["kind"], "device", "{answer}");
            assert_eq!(answer["result"]["consistency"], "leased", "{answer}");
            assert!(answer["result"]["lease"]["id"].is_string(), "{answer}");
            assert!(
                answer["result"]["records"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["key"] == "http-row"),
                "{answer}"
            );
        } else {
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(answer["error_code"], "invalid_params", "{answer}");
            assert!(
                answer.get("result").is_none(),
                "unknown scopes must not return device data"
            );
        }
    }
}
