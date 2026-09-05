use serde_json::{json, Value};

use super::protocol::CLIENT_NAME;
use crate::models::{AgentProvider, ModelChoice};

pub(super) const SELECTED_MODEL: &str = "gpt-5.6-sol";
pub(super) const SELECTED_EFFORT: &str = "high";
pub(super) const WORKTREE_ROOT: &str = "/tmp/worktree";
pub(super) const THREAD_ID: &str = "thread-1";
pub(super) const TURN_ID: &str = "turn-1";
pub(super) const CHILD_THREAD_ID: &str = "thread-child";
pub(super) const CHILD_TURN_ID: &str = "turn-child";
pub(super) const EXACT_THREAD_ID: &str = "thread-exact";

pub(super) fn supported_user_agent() -> String {
    format!("{CLIENT_NAME}/0.153.0 (fixture)")
}

pub(super) fn selected_choice() -> ModelChoice {
    ModelChoice {
        provider: AgentProvider::CodexAppServer,
        model: Some(SELECTED_MODEL.to_string()),
        effort: Some(SELECTED_EFFORT.to_string()),
    }
}

pub(super) fn initialize_result(user_agent: &str) -> Value {
    json!({ "userAgent": user_agent })
}

pub(super) fn thread_opened_with(
    thread_id: &str,
    reasoning_effort: Option<&str>,
    overrides: Value,
) -> Value {
    let mut opened = json!({
        "thread": { "id": thread_id },
        "model": SELECTED_MODEL,
        "reasoningEffort": reasoning_effort,
        "cwd": WORKTREE_ROOT,
        "approvalPolicy": "never",
        "sandbox": { "type": "dangerFullAccess" }
    });
    opened
        .as_object_mut()
        .expect("an opened thread is an object")
        .extend(
            overrides
                .as_object()
                .expect("thread-open overrides are an object")
                .clone(),
        );
    opened
}

pub(super) fn thread_opened_at(
    cwd: &str,
    thread_id: &str,
    reasoning_effort: Option<&str>,
) -> Value {
    thread_opened_with(thread_id, reasoning_effort, json!({ "cwd": cwd }))
}

pub(super) fn thread_opened(thread_id: &str, reasoning_effort: Option<&str>) -> Value {
    thread_opened_with(thread_id, reasoning_effort, json!({}))
}

pub(super) fn item_envelope_at(thread_id: &str, turn_id: &str, item: Value) -> Value {
    json!({ "threadId": thread_id, "turnId": turn_id, "item": item })
}

pub(super) fn item_envelope(item: Value) -> Value {
    item_envelope_at(THREAD_ID, TURN_ID, item)
}
