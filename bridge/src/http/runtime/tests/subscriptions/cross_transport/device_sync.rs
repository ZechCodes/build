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
    // This transport contract needs an uncontended writer. AppState::shared
    // also starts unrelated maintenance/delivery writers; admission correctly
    // refuses those instead of waiting. The handler still performs startup
    // projection and read-context initialization for this isolated app.
    let state = std::sync::Arc::new(std::sync::Mutex::new(app));
    let fixture = Fixture::new(AppState::handler(state));
    let client = fixture.http_client("device-sync-family").await;
    for (body, valid) in [
        (r#"{"scope":{"kind":"device"},"limit":100}"#, true),
        (r#"{"scope":{"kind":"unknown_device"}}"#, false),
    ] {
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
        let (_, answer) = json_response(response).await;
        flush(output).await;
        assert_eq!(answer["ok"], valid, "{answer}");
        if valid {
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
            assert_eq!(answer["error_code"], "invalid_params", "{answer}");
        }
    }
}
