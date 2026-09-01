use std::path::PathBuf;

use serde::Serialize;
use serde_json::{json, Value};

use super::adk::{task_status_failed, task_status_is_terminal};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AgentSurfaces {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub workflows: Vec<SurfaceWorkflow>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subagents: Vec<SurfaceAgent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shells: Vec<SurfaceShell>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub checklist: Vec<SurfaceChecklistItem>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SurfaceWorkflow {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub phases: Vec<SurfacePhase>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SurfacePhase {
    pub title: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<SurfaceAgent>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SurfaceAgent {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_tool: Option<SurfaceTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    #[serde(skip)]
    pub spawning_call_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SurfaceTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SurfaceShell {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tail: Vec<String>,
    #[serde(skip)]
    pub output_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SurfaceChecklistItem {
    pub id: String,
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

impl AgentSurfaces {
    pub fn is_empty(&self) -> bool {
        self.workflows.is_empty()
            && self.subagents.is_empty()
            && self.shells.is_empty()
            && self.checklist.is_empty()
    }

    pub fn wire_value(&self, call_sequence_of: &dyn Fn(&str) -> Option<u64>) -> Value {
        let mut written =
            serde_json::to_value(self).expect("a surfaces snapshot serializes to an object");
        let spawning_sequences = self.subagents.iter().map(|subagent| {
            subagent
                .spawning_call_id
                .as_deref()
                .and_then(call_sequence_of)
        });
        if let Some(entries) = written.get_mut("subagents").and_then(Value::as_array_mut) {
            for (entry, spawning_sequence) in entries.iter_mut().zip(spawning_sequences) {
                match (entry.as_object_mut(), spawning_sequence) {
                    (Some(fields), Some(sequence)) => {
                        fields.insert("call_sequence".to_string(), json!(sequence));
                    }
                    _ => continue,
                }
            }
        }
        written
    }
}

#[allow(dead_code)]
fn wire_task_state(status: &str) -> Option<&'static str> {
    match task_status_failed(status) {
        true => Some("failed"),
        false => match task_status_is_terminal(status) {
            true => Some("done"),
            false => None,
        },
    }
}

#[allow(dead_code)]
fn wire_agent_state(token: &str, has_started_at: bool) -> Option<&'static str> {
    match token {
        "start" | "progress" => match has_started_at {
            true => Some("running"),
            false => Some("queued"),
        },
        "done" => Some("done"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_counter_from_the_workflow_fixture() -> SurfaceAgent {
        SurfaceAgent {
            id: "acdd7854c4bce379a".to_string(),
            label: "line-counter".to_string(),
            model: Some("claude-haiku-4-5-20251001".to_string()),
            state: Some("done".to_string()),
            started_at: Some(1_788_290_134_700),
            duration_ms: Some(4_732),
            tokens: Some(11_409),
            tool_calls: Some(1),
            last_tool: Some(SurfaceTool {
                name: "Read".to_string(),
                summary: Some(
                    "/private/tmp/claude-501/-Users-zech--superconductor-worktre…".to_string(),
                ),
            }),
            result: Some("2".to_string()),
            error: None,
            attempt: Some(1),
            spawning_call_id: Some("toolu_01TPFUY53rBEJPmkKu7rJPWV".to_string()),
        }
    }

    fn one_checklist_item() -> SurfaceChecklistItem {
        SurfaceChecklistItem {
            id: "task-1".to_string(),
            subject: "Count the lines".to_string(),
            description: Some("Read README.md and count".to_string()),
            state: Some("in_progress".to_string()),
        }
    }

    fn one_shell() -> SurfaceShell {
        SurfaceShell {
            id: "bash-1".to_string(),
            description: Some("run the suite".to_string()),
            state: Some("running".to_string()),
            exit_code: None,
            tail: vec!["test one ... ok".to_string()],
            output_path: Some(PathBuf::from("/private/tmp/shell-out.log")),
        }
    }

    fn no_call_sequence(_spawning_call_id: &str) -> Option<u64> {
        None
    }

    #[test]
    fn a_snapshot_holding_only_a_checklist_writes_only_that_key() {
        let surfaces = AgentSurfaces {
            checklist: vec![one_checklist_item()],
            ..AgentSurfaces::default()
        };

        let written = surfaces.wire_value(&no_call_sequence);
        let keys: Vec<&str> = written
            .as_object()
            .expect("the snapshot writes an object")
            .keys()
            .map(String::as_str)
            .collect();

        assert_eq!(keys, vec!["checklist"]);
    }

    #[test]
    fn an_all_empty_snapshot_is_empty_and_writes_nothing() {
        let surfaces = AgentSurfaces::default();

        assert!(surfaces.is_empty());
        assert_eq!(
            surfaces.wire_value(&no_call_sequence),
            serde_json::json!({})
        );
    }

    #[test]
    fn a_subagent_carries_the_call_sequence_the_closure_answers() {
        let surfaces = AgentSurfaces {
            subagents: vec![line_counter_from_the_workflow_fixture()],
            ..AgentSurfaces::default()
        };
        let answers_forty_one = |spawning_call_id: &str| {
            assert_eq!(spawning_call_id, "toolu_01TPFUY53rBEJPmkKu7rJPWV");
            Some(41)
        };

        let answered = surfaces.wire_value(&answers_forty_one);
        assert_eq!(answered["subagents"][0]["call_sequence"], 41);

        let unanswered = surfaces.wire_value(&no_call_sequence);
        assert!(
            !unanswered["subagents"][0]
                .as_object()
                .expect("a subagent writes an object")
                .contains_key("call_sequence"),
            "an unanswered spawning call writes no key at all: {unanswered}"
        );
    }

    #[test]
    fn no_snapshot_ever_writes_an_internal_field() {
        let surfaces = AgentSurfaces {
            workflows: vec![SurfaceWorkflow {
                id: "w81x1fmx5".to_string(),
                name: "count-and-summarize".to_string(),
                description: Some("Count README.md lines".to_string()),
                state: Some("running".to_string()),
                phases: vec![SurfacePhase {
                    title: "Read".to_string(),
                    agents: vec![line_counter_from_the_workflow_fixture()],
                }],
            }],
            subagents: vec![line_counter_from_the_workflow_fixture()],
            shells: vec![one_shell()],
            checklist: vec![one_checklist_item()],
        };

        let written = surfaces.wire_value(&no_call_sequence).to_string();

        assert!(!written.contains("spawning_call_id"), "{written}");
        assert!(!written.contains("output_path"), "{written}");
        assert!(!written.contains("shell-out.log"), "{written}");
    }

    #[test]
    fn the_task_state_table_maps_terminal_statuses_and_leaves_the_rest_alone() {
        assert_eq!(wire_task_state("completed"), Some("done"));
        assert_eq!(wire_task_state("failed"), Some("failed"));
        assert_eq!(wire_task_state("error"), Some("failed"));
        assert_eq!(wire_task_state("timed_out"), Some("failed"));
        assert_eq!(wire_task_state("killed"), Some("done"));
        assert_eq!(wire_task_state("stopped"), Some("done"));
        assert_eq!(wire_task_state("cancelled"), Some("done"));
        assert_eq!(wire_task_state("running"), None);
    }

    #[test]
    fn the_agent_state_table_reads_a_start_without_a_start_time_as_queued() {
        assert_eq!(wire_agent_state("start", true), Some("running"));
        assert_eq!(wire_agent_state("start", false), Some("queued"));
        assert_eq!(wire_agent_state("progress", true), Some("running"));
        assert_eq!(wire_agent_state("progress", false), Some("queued"));
        assert_eq!(wire_agent_state("done", false), Some("done"));
        assert_eq!(wire_agent_state("done", true), Some("done"));
        assert_eq!(wire_agent_state("thinking", true), None);
        assert_eq!(wire_agent_state("thinking", false), None);
    }
}
