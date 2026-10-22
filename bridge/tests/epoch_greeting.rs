use build_bridge::api::writes::ACK_CONFIRM_METHODS;
use build_bridge::app::AppState;
use build_bridge::carrier::SessionSender;
use build_bridge::harness::HarnessContext;
use build_bridge::transport::Frame;
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn greeting_reports_the_current_ack_confirm_method_allowlist() {
    let directory = tempfile::tempdir().unwrap();
    let context = HarnessContext::resolved(
        directory.path().join("test-mcp.sock"),
        directory.path().to_path_buf(),
    )
    .unwrap();
    let app = AppState::new_configured(
        directory.path(),
        directory.path().join("worktrees"),
        "main",
        true,
        context,
    );
    let handler = AppState::handler(Arc::new(Mutex::new(app)));
    let (sender, mut wire, key) = SessionSender::observable("allowlist-greeting");
    let reply = handler.call(
        sender.clone(),
        Frame {
            session_id: "allowlist-greeting".into(),
            message_id: "hello".into(),
            frame_type: "data".into(),
            sender: "client".into(),
            created_at: "test".into(),
            payload: json!({"id": "hello", "method": "session.hello", "params": {}}),
        },
    );
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(sender.push(reply));
    let greeting = SessionSender::decrypt_push(&key, &wire.try_recv().unwrap());
    assert_eq!(greeting["result"]["ack_confirm_methods"], json!([]));
    assert_eq!(
        greeting["result"]["ack_confirm_methods"],
        json!(ACK_CONFIRM_METHODS)
    );
}
