//! Select the router's digest without copying the board's other content.
use super::json::JsonRef;
use super::SharedJson;
use serde_json::{json, Value};

pub(super) fn digest(board: &SharedJson) -> Value {
    #[cfg(test)]
    let _phase = crate::timing::phase_samples::phase("snapshot.work_digest");
    let work = JsonRef::Shared(board)
        .field("items")
        .rows()
        .filter(|row| row.field("kind").string() != Some("capture"))
        .map(row_digest)
        .collect::<Vec<_>>();
    work_reply(work)
}

pub(super) fn work_reply(work: Vec<Value>) -> Value {
    let mut reply = serde_json::Map::new();
    reply.insert("work".into(), Value::Array(work));
    Value::Object(reply)
}

fn row_digest(row: JsonRef<'_>) -> Value {
    let mut answer = serde_json::Map::new();
    for key in [
        "kind",
        "project_id",
        "project",
        "branch",
        "task_id",
        "title",
        "state",
        "working",
    ] {
        answer.insert(key.into(), row.field(key).value());
    }
    let id = ["run_id", "task_id", "worktree_id"]
        .into_iter()
        .find_map(|key| row.field(key).string());
    answer.insert("entity_id".into(), json!(id));
    Value::Object(answer)
}
