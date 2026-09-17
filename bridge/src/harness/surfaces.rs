use std::sync::Arc;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::watch;

mod bounds;
mod checklist;
mod claude_checklist;
mod execution;
mod goal;
mod ledger;
mod observation;
#[cfg(test)]
mod tests;

pub use bounds::{
    CHECKLIST_ITEM_LIMIT, CHECKLIST_TEXT_LIMIT, GOAL_OBJECTIVE_LIMIT, PLAN_EXPLANATION_LIMIT,
    PROVIDER_TOKEN_LIMIT, TERMINAL_EXECUTION_ITEM_LIMIT,
};
pub use checklist::{
    ChecklistCollection, ChecklistProvenance, ChecklistSource, ChecklistState, SurfaceChecklistItem,
};
pub use goal::{GoalState, SurfaceGoal};
pub use ledger::SurfaceLedger;
pub use observation::{
    SurfaceCoverage, SurfaceFreshness, SurfaceObservation, SurfaceObservations, SurfaceSupport,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AgentSurfaces {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<SurfaceGoal>,
    #[serde(skip_serializing_if = "SurfaceObservations::is_empty")]
    pub observations: SurfaceObservations,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub workflows: Vec<SurfaceWorkflow>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subagents: Vec<SurfaceAgent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shells: Vec<SurfaceShell>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub checklist: Vec<SurfaceChecklistItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checklist_provenance: Option<ChecklistProvenance>,
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
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
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
    pub started_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tail: Vec<String>,
    #[serde(skip)]
    pub closed_by_notification: bool,
}

impl AgentSurfaces {
    pub fn is_empty(&self) -> bool {
        self.goal.is_none()
            && self.observations.is_empty()
            && self.workflows.is_empty()
            && self.subagents.is_empty()
            && self.shells.is_empty()
            && self.checklist.is_empty()
            && self.checklist_provenance.is_none()
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

#[derive(Debug, Clone)]
pub struct SurfaceRevision(Arc<watch::Sender<u64>>);

impl Default for SurfaceRevision {
    fn default() -> SurfaceRevision {
        SurfaceRevision(Arc::new(watch::Sender::new(0)))
    }
}

impl SurfaceRevision {
    pub fn bump(&self) {
        self.0.send_modify(|counter| *counter += 1);
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.subscribe()
    }
}
