//! Validated, canonical selectors for bounded projection reads.
use crate::store::{TaskQueryFilter, TaskQueryOrder};
use serde_json::{json, Value};
pub(super) struct QuerySelections {
    pub order: TaskQueryOrder,
    pub filter: TaskQueryFilter,
}
impl QuerySelections {
    pub fn parse(params: &Value) -> Result<Self, String> {
        let order = params
            .get("order")
            .map(|value| {
                serde_json::from_value(value.clone())
                    .map_err(|_| "order must be number, updated, priority or anchor".to_string())
            })
            .transpose()?
            .unwrap_or_default();
        let mut selected = serde_json::Map::new();
        for name in [
            "watched",
            "unfinished",
            "blocked",
            "held",
            "undelivered",
            "attention_rule",
            "linked_workspace_id",
            "tracker_agent_id",
            "exclude_review_mode",
            "done_since",
            "exclude_assignee",
        ] {
            if let Some(value) = params.get(name) {
                selected.insert(name.into(), value.clone());
            }
        }
        let filter: TaskQueryFilter = serde_json::from_value(Value::Object(selected))
            .map_err(|error| format!("invalid task query selector: {error}"))?;
        for (value, allowed, name) in [
            (
                filter.attention_rule.as_deref(),
                &["asked", "legacy"][..],
                "attention_rule",
            ),
            (
                filter.exclude_review_mode.as_deref(),
                &["pull_request", "snapshot"][..],
                "exclude_review_mode",
            ),
        ] {
            if value.is_some_and(|value| !allowed.contains(&value)) {
                return Err(format!("{name} must be {}", allowed.join(" or ")));
            }
        }
        for value in [&filter.linked_workspace_id, &filter.tracker_agent_id]
            .into_iter()
            .flatten()
        {
            if value.is_empty() || value.len() > 128 {
                return Err("membership id must contain 1 to 128 bytes".into());
            }
        }
        validate_exclusion(filter.exclude_assignee.as_ref())?;
        Ok(Self { order, filter })
    }
    pub fn canonical(&self) -> Value {
        json!([self.order, self.filter])
    }
    pub fn matches_row(&self, row: &Value) -> bool {
        self.filter.done_since.is_none() || row["status"] == "done"
    }
    pub fn provenance(&self) -> Value {
        json!({"linked_workspace_id":self.filter.linked_workspace_id,"tracker_agent_id":self.filter.tracker_agent_id})
    }
}
pub(super) fn ensure_supported_markers(
    source: &impl super::read_source::TaskReadSource,
    project: &str,
    project_id: &str,
    operation: &str,
) -> Result<(), String> {
    use super::StoredAnswer;
    if source.task_query_markers_unsupported(project).stored()? {
        return Err(format!(
            "task_query_marker_unsupported:{}",
            json!({"reason":"task_query_marker_unsupported","scope":"project_markers","project_id":project_id,"domain":"opaque-marker-v1","max_bytes":128,"operation":operation})
        ));
    }
    Ok(())
}
fn validate_exclusion(value: Option<&Value>) -> Result<(), String> {
    if let Some(value) = value {
        let expected = match value["kind"].as_str() {
            Some("agent") => &["kind", "agent_id"][..],
            Some("user" | "project_agent") => &["kind"][..],
            _ => return Err("exclude_assignee must be an exact assignee object".into()),
        };
        if value.as_object().is_none_or(|object| {
            object.len() != expected.len()
                || object.keys().any(|key| !expected.contains(&key.as_str()))
        }) || (expected.len() == 2
            && value["agent_id"]
                .as_str()
                .is_none_or(|id| id.is_empty() || id.len() > 128))
        {
            return Err("exclude_assignee must be an exact valid assignee object".into());
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_query_values_and_preserves_exact_ids() {
        for params in [
            json!({"order":"search"}),
            json!({"watched":"true"}),
            json!({"attention_rule":"other"}),
            json!({"linked_workspace_id":""}),
            json!({"exclude_assignee":{"kind":"unknown"}}),
        ] {
            assert!(QuerySelections::parse(&params).is_err());
        }
        let selected=QuerySelections::parse(&json!({"tracker_agent_id":"AgentA","exclude_assignee":{"kind":"agent","agent_id":"AgentA"}})).unwrap();
        assert_eq!(selected.provenance()["tracker_agent_id"], "AgentA");
        assert_eq!(
            selected.filter.exclude_assignee,
            Some(json!({"kind":"agent","agent_id":"AgentA"}))
        );
        let lower = QuerySelections::parse(
            &json!({"exclude_assignee":{"kind":"agent","agent_id":"agenta"}}),
        )
        .unwrap();
        assert_ne!(
            selected.filter.exclude_assignee,
            lower.filter.exclude_assignee
        );
    }
}
