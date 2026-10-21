//! Test-only accepted record evidence. This first stage deliberately emits none.
//!
//! Native topic/name records have no product revision. The task timestamp is
//! retained as-is and is not promoted into a monotonic revision.
use super::AppState;
use crate::tracker::{TaskPriority, TaskState};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct AgentRecordWitness {
    pub original_ids: Vec<String>,
    pub owner: String,
    pub agent: String,
    pub topic: Option<String>,
    pub name: Option<String>,
    pub name_asked: bool,
    pub native_revision: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct AcceptedTaskFields {
    pub id: String,
    pub project_path: String,
    pub number: u64,
    pub state: TaskState,
    pub status: String,
    pub title: String,
    pub body: String,
    pub priority: TaskPriority,
    pub labels: Vec<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) enum TaskStatusDisposition {
    StatusChanged {
        event_id: String,
        from: String,
        to: String,
    },
    StatusNoop,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TaskRecordWitness {
    pub original_ids: Vec<String>,
    pub accepted: AcceptedTaskFields,
    pub status_disposition: TaskStatusDisposition,
    pub native_revision: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) enum AcceptedRecord {
    Agent(AgentRecordWitness),
    Task(TaskRecordWitness),
}

#[derive(Default)]
pub(crate) struct AcceptedRecords {
    by_original: Mutex<BTreeMap<String, Vec<AcceptedRecord>>>,
}

impl AcceptedRecords {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Consumers must reject both missing and duplicate records. This is not
    /// an event sequence or a product version, and it invents no evidence.
    pub(crate) fn take(&self, original_id: &str) -> Vec<AcceptedRecord> {
        self.by_original
            .lock()
            .unwrap()
            .remove(original_id)
            .unwrap_or_default()
    }
}

impl AppState {
    /// Test-first compile scaffold. Save/install observation is intentionally
    /// absent until the real socket and failed-save controls produce RED.
    pub(crate) fn test_capture_accepted_records(&mut self, _records: Arc<AcceptedRecords>) {}
}
