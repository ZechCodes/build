//! Select the router's digest without copying the board's other content.
use super::SharedJson;
use serde_json::{json, Value};

#[derive(Clone, Copy)]
enum JsonRef<'a> {
    Shared(&'a SharedJson),
    Value(&'a Value),
}

impl<'a> JsonRef<'a> {
    fn field(self, key: &str) -> Self {
        match self {
            Self::Value(value) => Self::Value(&value[key]),
            Self::Shared(shared) => match shared.leaf() {
                Some(value) => Self::Value(&value[key]),
                None => shared
                    .fields()
                    .and_then(|fields| fields.get(key))
                    .map(Self::Shared)
                    .unwrap_or(Self::Value(&Value::Null)),
            },
        }
    }

    fn string(self) -> Option<&'a str> {
        match self {
            Self::Value(value) => value.as_str(),
            Self::Shared(shared) => shared.leaf().and_then(Value::as_str),
        }
    }

    fn value(self) -> Value {
        match self {
            Self::Value(value) => value.clone(),
            Self::Shared(shared) => shared.materialize(),
        }
    }

    fn rows(self) -> Box<dyn Iterator<Item = Self> + 'a> {
        match self {
            Self::Value(value) => Box::new(value.as_array().into_iter().flatten().map(Self::Value)),
            Self::Shared(shared) => match shared.leaf() {
                Some(value) => Self::Value(value).rows(),
                None => Box::new(shared.elements().into_iter().flatten().map(Self::Shared)),
            },
        }
    }
}

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
