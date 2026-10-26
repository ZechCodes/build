//! One canonical interpretation of scalar and multiple task filters.
use super::edits::{self, AssigneeFilter};
use crate::tracker::{Task, TaskState};
use serde_json::{json, Value};

pub(super) struct TaskSelections {
    pub states: Vec<TaskState>,
    pub statuses: Vec<String>,
    pub labels: Vec<String>,
    assignees: Vec<AssigneeFilter>,
    pub plural: bool,
}

impl TaskSelections {
    pub fn parse(params: &Value) -> Result<Self, String> {
        let states = selection(params, "state", "states", |one| {
            edits::optional_state(&json!({"state":one}))?
                .ok_or_else(|| "state cannot be null".into())
        })?;
        let statuses = selection(params, "status", "statuses", |one| {
            edits::optional_status(&json!({"status":one}), "status")?
                .ok_or_else(|| "status cannot be null".into())
        })?;
        let labels = selection(params, "label", "labels", |one| {
            let params = json!({"label":one});
            let label = crate::app::optional_nonempty_string(&params, "label")?
                .ok_or("label cannot be null")?;
            if label.len() > crate::tracker::MAX_LABEL_BYTES {
                return Err("label exceeds 40 bytes".into());
            }
            Ok(label.to_ascii_lowercase())
        })?;
        let assignees = selection(params, "assignee", "assignees", |one| {
            if one.is_null() {
                return Err("assignee cannot be null in a selection".into());
            }
            validate_assignee(one)?;
            edits::optional_assignee_filter(&json!({"assignee":one}))
        })?;
        Ok(Self {
            states,
            statuses,
            labels,
            assignees,
            plural: ["states", "statuses", "labels", "assignees"]
                .iter()
                .any(|key| params.get(key).is_some()),
        })
    }

    pub fn canonical(&self) -> Value {
        json!([
            canonical(self.states.iter().map(|state| json!(state.as_str()))),
            canonical(self.statuses.iter().map(|status| json!(status))),
            canonical(self.labels.iter().map(|label| json!(label))),
            canonical(self.assignees.iter().map(AssigneeFilter::key)),
        ])
    }

    pub fn matches_task(&self, task: &Task, pending: Option<&Value>) -> bool {
        self.matches_dimensions(task.state.as_str(), &task.status, &task.labels)
            && (self.assignees.is_empty()
                || self
                    .assignees
                    .iter()
                    .any(|filter| filter.matches(task, pending)))
    }

    pub fn matches_row(&self, row: &Value, pending: Option<&Value>) -> bool {
        let labels: Vec<String> = row["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        self.matches_dimensions(
            row["state"].as_str().unwrap_or_default(),
            row["status"].as_str().unwrap_or_default(),
            &labels,
        ) && (self.assignees.is_empty()
            || self.assignees.iter().any(|filter| match filter {
                AssigneeFilter::Unfiltered => true,
                AssigneeFilter::Unassigned => row["assignee"].is_null() && pending.is_none(),
                AssigneeFilter::Assigned => !row["assignee"].is_null() || pending.is_some(),
                AssigneeFilter::Exactly(_) => filter.key() == row["assignee"],
            }))
    }

    fn matches_dimensions(&self, state: &str, status: &str, labels: &[String]) -> bool {
        (self.states.is_empty() || self.states.iter().any(|want| want.as_str() == state))
            && (self.statuses.is_empty() || self.statuses.iter().any(|want| want == status))
            && (self.labels.is_empty()
                || self
                    .labels
                    .iter()
                    .any(|want| labels.iter().any(|label| label.eq_ignore_ascii_case(want))))
    }
}

fn selection<T>(
    params: &Value,
    scalar: &str,
    plural: &str,
    parse: impl Fn(&Value) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    if params.get(scalar).is_some() && params.get(plural).is_some() {
        return Err(format!("{scalar} and {plural} cannot be supplied together"));
    }
    if let Some(asked) = params.get(plural) {
        let values = asked
            .as_array()
            .filter(|values| !values.is_empty() && values.len() <= 100)
            .ok_or_else(|| format!("{plural} must contain 1 to 100 selections"))?;
        return values.iter().map(parse).collect();
    }
    match params.get(scalar).filter(|value| !value.is_null()) {
        Some(value) => parse(value).map(|value| vec![value]),
        None => Ok(Vec::new()),
    }
}

fn validate_assignee(value: &Value) -> Result<(), String> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    let expected = match value["kind"].as_str() {
        Some("agent") => &["kind", "agent_id"][..],
        Some("user" | "project_agent") => &["kind"][..],
        _ => return Err("unknown assignee kind".into()),
    };
    if object.keys().any(|key| !expected.contains(&key.as_str())) {
        return Err("unknown field in assignee filter".into());
    }
    if expected.len() == 2
        && !value["agent_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty() && id.len() <= 128)
    {
        return Err("assignee agent_id must contain 1 to 128 bytes".into());
    }
    Ok(())
}

fn canonical(values: impl Iterator<Item = Value>) -> Vec<Value> {
    let mut values: Vec<Value> = values.collect();
    values.sort_by_key(Value::to_string);
    values.dedup();
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selections_are_or_within_and_across_and_canonical() {
        let selected = TaskSelections::parse(&json!({"states":["closed","open","open"],"statuses":["In review","done"],"labels":["Api","api","UI"],"assignees":["none",{"kind":"user"}]})).unwrap();
        let reordered = TaskSelections::parse(&json!({"states":["open","closed"],"statuses":["done","in_review"],"labels":["ui","API"],"assignees":[{"kind":"user"},"none"]})).unwrap();
        assert_eq!(selected.canonical(), reordered.canonical());
        assert!(selected.matches_row(
            &json!({"state":"open","status":"done","labels":["API"],"assignee":null}),
            None
        ));
        assert!(!selected.matches_row(
            &json!({"state":"open","status":"backlog","labels":["API"],"assignee":null}),
            None
        ));
        assert!(!selected.matches_row(
            &json!({"state":"open","status":"done","labels":["other"],"assignee":null}),
            None
        ));
    }

    #[test]
    fn opaque_assignee_identity_case_is_preserved() {
        let upper = TaskSelections::parse(&json!({"assignees":[{"kind":"agent","agent_id":"AgentA"},{"kind":"agent","agent_id":"AgentA"}]})).unwrap();
        let lower =
            TaskSelections::parse(&json!({"assignee":{"kind":"agent","agent_id":"agenta"}}))
                .unwrap();
        assert_ne!(upper.canonical(), lower.canonical());
        let row = json!({"assignee":{"kind":"agent","agent_id":"AgentA"}});
        assert!(upper.matches_row(&row, None));
        assert!(!lower.matches_row(&row, None));
        assert!(!TaskSelections::parse(&json!({"assignee":"none"}))
            .unwrap()
            .matches_row(
                &json!({"assignee":null}),
                Some(&json!({"kind":"new_workspace"}))
            ));
        assert!(TaskSelections::parse(&json!({"assignee":"any"}))
            .unwrap()
            .matches_row(
                &json!({"assignee":null}),
                Some(&json!({"kind":"new_workspace"}))
            ));
    }

    #[test]
    fn selections_reject_empty_ambiguous_and_invalid_inputs() {
        for params in [
            json!({"states":[]}),
            json!({"labels":[null]}),
            json!({"labels":[""]}),
            json!({"states":["done"]}),
            json!({"statuses":["closed"]}),
            json!({"state":"open","states":["closed"]}),
            json!({"assignee":null,"assignees":["none"]}),
            json!({"assignees":[null]}),
        ] {
            assert!(TaskSelections::parse(&params).is_err(), "{params}");
        }
        assert!(TaskSelections::parse(&json!({}))
            .unwrap()
            .matches_row(&json!({}), None));
    }
}
