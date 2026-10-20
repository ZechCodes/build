//! The already-rendered collections carried by one board snapshot.
use serde_json::Value;

pub(super) struct BoardSnapshot {
    pub items: Vec<Value>,
    pub projects: Vec<Value>,
    pub runs: Vec<Value>,
    pub external_worktrees: Vec<Value>,
    pub pending: Vec<Value>,
    pub scanning: bool,
    pub workspace_summaries: Value,
    pub usage_limits: Value,
}

impl BoardSnapshot {
    pub(super) fn into_value(self) -> Value {
        Value::Object(
            [
                ("items", Value::Array(self.items)),
                ("projects", Value::Array(self.projects)),
                ("runs", Value::Array(self.runs)),
                ("external_worktrees", Value::Array(self.external_worktrees)),
                ("pending", Value::Array(self.pending)),
                ("scanning", Value::Bool(self.scanning)),
                ("workspace_summaries", self.workspace_summaries),
                ("usage_limits", self.usage_limits),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn board_assembly_moves_every_rendered_collection_without_copying_payloads() {
        let rows = || vec![json!({"payload": "retained nested payload ".repeat(128)})];
        let snapshot = BoardSnapshot {
            items: rows(),
            projects: rows(),
            runs: rows(),
            external_worktrees: rows(),
            pending: rows(),
            scanning: true,
            workspace_summaries: Value::Array(rows()),
            usage_limits: Value::Array(rows()),
        };
        let expected = json!({
            "items": snapshot.items,
            "projects": snapshot.projects,
            "runs": snapshot.runs,
            "external_worktrees": snapshot.external_worktrees,
            "pending": snapshot.pending,
            "scanning": true,
            "workspace_summaries": snapshot.workspace_summaries,
            "usage_limits": snapshot.usage_limits,
        });
        let allocations = [
            (
                "items",
                snapshot.items[0]["payload"].as_str().unwrap().as_ptr(),
            ),
            (
                "projects",
                snapshot.projects[0]["payload"].as_str().unwrap().as_ptr(),
            ),
            (
                "runs",
                snapshot.runs[0]["payload"].as_str().unwrap().as_ptr(),
            ),
            (
                "external_worktrees",
                snapshot.external_worktrees[0]["payload"]
                    .as_str()
                    .unwrap()
                    .as_ptr(),
            ),
            (
                "pending",
                snapshot.pending[0]["payload"].as_str().unwrap().as_ptr(),
            ),
            (
                "workspace_summaries",
                snapshot.workspace_summaries[0]["payload"]
                    .as_str()
                    .unwrap()
                    .as_ptr(),
            ),
            (
                "usage_limits",
                snapshot.usage_limits[0]["payload"]
                    .as_str()
                    .unwrap()
                    .as_ptr(),
            ),
        ];
        let actual = snapshot.into_value();
        assert_eq!(actual, expected);
        for (field, original) in allocations {
            assert_eq!(
                actual[field][0]["payload"].as_str().unwrap().as_ptr(),
                original,
                "board assembly copied the already-rendered {field} collection"
            );
        }
    }
}
