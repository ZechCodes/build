use serde_json::{json, Value};

use crate::models::{AgentProvider, ModelChoice};

pub(super) const SUPPORTED_USER_AGENT: &str = "build_bridge/0.153.0 (fixture)";
pub(super) const SELECTED_MODEL: &str = "gpt-5.6-sol";
pub(super) const SELECTED_EFFORT: &str = "high";
pub(super) const WORKTREE_ROOT: &str = "/tmp/worktree";
pub(super) const EXACT_THREAD_ID: &str = "thread-exact";

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

pub(super) fn thread_opened_at(
    cwd: &str,
    thread_id: &str,
    reasoning_effort: Option<&str>,
) -> Value {
    json!({
        "thread": { "id": thread_id },
        "model": SELECTED_MODEL,
        "reasoningEffort": reasoning_effort,
        "cwd": cwd,
        "approvalPolicy": "never",
        "sandbox": { "type": "dangerFullAccess" }
    })
}

pub(super) fn thread_opened(thread_id: &str, reasoning_effort: Option<&str>) -> Value {
    thread_opened_at(WORKTREE_ROOT, thread_id, reasoning_effort)
}

pub(super) const ITEM_THREAD_ID: &str = "thread-1";
pub(super) const ITEM_TURN_ID: &str = "turn-1";

pub(super) fn item_envelope_at(thread_id: &str, turn_id: &str, item: Value) -> Value {
    json!({ "threadId": thread_id, "turnId": turn_id, "item": item })
}

pub(super) fn item_envelope(item: Value) -> Value {
    item_envelope_at(ITEM_THREAD_ID, ITEM_TURN_ID, item)
}
