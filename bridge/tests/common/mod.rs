//! A browser as the relay carries it: the two message types a client puts on
//! the relay wire, over the frames `carrier::testing` builds.

use build_bridge::carrier::testing;
use build_bridge::transport::DATA_FRAME_TYPE;
use serde_json::{json, Value};

/// A browser opening a session over the relay.
pub fn session_init_message(
    session_id: &str,
    transport_public_key: &str,
    session_key: &str,
) -> Value {
    json!({
        "type": "session_init",
        "session_id": session_id,
        "session_init": serde_json::to_value(
            testing::session_init(session_id, transport_public_key, session_key),
        ).expect("a session_init serializes"),
    })
}

/// One encrypted request from that browser, in the relay's envelope wrapper.
pub fn request_message(session_key: &str, session_id: &str, payload: Value) -> Value {
    json!({
        "type": "e2ee_envelope",
        "session_id": session_id,
        "envelope": serde_json::to_value(
            testing::client_request(session_key, session_id, DATA_FRAME_TYPE, payload),
        ).expect("an envelope serializes"),
    })
}
