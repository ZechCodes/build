//! What travels with an assignment besides the task, and the retry that
//! must not deliver it twice.
//!
//! A client that loses the answer to `tasks.assign` asks again. With the
//! same `operation_id` the second request is the first one: the task already
//! records a `dispatched` event carrying that id, and that dispatch is the
//! answer. Nothing is delivered and no agent is made a second time.

use super::dispatch::AssignTarget;
use super::StoredAnswer;
use crate::app::AppState;
use crate::tracker::{Task, TaskEventKind, TimelineEntry};
use serde_json::{json, Value};

/// The note said under a hand-off, and the operation id its delivery is
/// queued under — the client's, when it named one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::app) struct Handoff {
    pub(in crate::app) note: Option<String>,
    pub(in crate::app) operation_id: Option<String>,
}

impl Handoff {
    /// A hand-off with only a note, which is every caller that predates
    /// operation ids.
    pub(in crate::app) fn noted(note: Option<String>) -> Self {
        Self {
            note,
            operation_id: None,
        }
    }
}

impl AppState {
    /// The dispatch `handoff`'s operation id already made on this task, as
    /// `tasks.assign` answers it, when it made one. The same id asked of a
    /// different assignment is refused rather than answered.
    pub(super) fn replayed_dispatch(
        &self,
        project_id: &str,
        task: &Task,
        target: &AssignTarget,
        handoff: &Handoff,
    ) -> Result<Option<Value>, String> {
        let Some(operation_id) = handoff.operation_id.as_deref() else {
            return Ok(None);
        };
        let timeline = self
            .tracker_store()?
            .load_tracker_timeline(&task.id)
            .stored()?;
        let Some(dispatched) = timeline.into_iter().find_map(|entry| match entry {
            TimelineEntry::Event(event)
                if event.kind == TaskEventKind::Dispatched
                    && event.payload["operation_id"] == operation_id =>
            {
                Some(event.payload)
            }
            _ => None,
        }) else {
            return Ok(None);
        };
        if !target.made(&dispatched) {
            return Err(format!(
                "operation_id {operation_id} was already spent on a different assignment of this task"
            ));
        }
        Ok(Some(json!({
            "task": self.task_json_with_live_identities(project_id, task),
            "dispatch": dispatched,
        })))
    }
}

impl AssignTarget {
    /// Whether a recorded dispatch is the one this target asks for: the same
    /// kind, and the same agent where the target names one.
    fn made(&self, dispatched: &Value) -> bool {
        let same_kind = dispatched["kind"] == self.wire_kind();
        match self {
            AssignTarget::Agent { agent_id } => same_kind && dispatched["agent_id"] == *agent_id,
            _ => same_kind,
        }
    }
}
