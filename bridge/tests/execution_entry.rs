//! The legacy carrier and shared execution entry use one handler and policy.
use build_bridge::transport::Frame;
use build_bridge::{carrier::SessionSender, execution::FrameHandler, timing::FrameClock};
use serde_json::json;

#[test]
fn shared_entry_preserves_legacy_dispatch_and_metadata_validation() {
    let shared = FrameHandler::new(
        FrameClock::new(),
        |_, frame, _| json!({"id": frame.payload["id"], "ok": true, "result": frame.payload["params"]}),
    );
    let legacy: build_bridge::carrier::FrameHandler = shared.clone();
    let mut frame = Frame {
        session_id: "entry".into(),
        message_id: "m1".into(),
        frame_type: "data".into(),
        sender: "client".into(),
        created_at: String::new(),
        payload: json!({"id": 42, "method": "board.list", "params": {"sentinel": true}}),
    };
    assert_eq!(
        shared.call(SessionSender::detached("entry"), frame.clone()),
        legacy.call(SessionSender::detached("entry"), frame.clone())
    );
    frame.payload["write"] = json!({"operation_id": "invalid"});
    let rejected = shared.call(SessionSender::detached("entry"), frame);
    assert_eq!(rejected["ok"], false);
    assert_eq!(rejected["error_code"], "invalid_params");
}
