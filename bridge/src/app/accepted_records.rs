//! Opt-in evidence captured at successful native save/install boundaries.
//!
//! Native topic/name records have no product revision. The task timestamp is
//! retained as-is and is not promoted into a monotonic revision.
mod saved_effects;
pub(in crate::app) use saved_effects::SeenProbe;

use super::AppState;
use crate::session_summary::UserSession;
use crate::tracker::{Task, TaskEvent, TaskEventKind, TaskPriority, TaskState};
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
pub(crate) struct SeenRecordWitness {
    pub original_ids: Vec<String>,
    pub owner: String,
    pub agent: String,
    pub conversation_owner: String,
    pub conversation_id: String,
    pub thread_id: String,
    pub generation_revision: u64,
    pub actual_reached_sequence: u64,
    pub saved_through_sequence: u64,
    pub published_through_sequence: u64,
    pub native_revision: Option<u64>, // Always None for this record.
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) enum PresencePersistence {
    Saved,
    SaveFailed,
    NoStore,
    UnchangedNoSave,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PresenceRecordWitness {
    pub original_ids: Vec<String>,
    pub local: UserSession,
    pub published: UserSession,
    pub persistence: PresencePersistence,
    // Exact preceding authoritative durable coverage or exact successful
    // native payload at THIS execution. Never a later SQL summary.
    pub durable_at_execution: Option<UserSession>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) enum AcceptedRecord {
    Agent(AgentRecordWitness),
    Task(TaskRecordWitness),
    Seen(SeenRecordWitness),
    Presence(PresenceRecordWitness),
}

#[derive(Default)]
pub(crate) struct AcceptedRecords {
    by_original: Mutex<BTreeMap<String, Vec<AcceptedRecord>>>,
    // Outer None means unseeded; Some(None) is an actual absent setup row.
    known_durable_user_session: Mutex<Option<Option<UserSession>>>,
}

impl AcceptedRecords {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Seed exactly once from the actual setup Store load before listener setup.
    pub(crate) fn seed_known_durable_user_session(&self, session: Option<UserSession>) {
        let mut known = self.known_durable_user_session.lock().unwrap();
        assert!(
            known.is_none(),
            "durable user session setup already captured"
        );
        *known = Some(session);
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

    /// Final native-quiet audit only. Preserve unknown and late originals too.
    pub(crate) fn take_all(&self) -> BTreeMap<String, Vec<AcceptedRecord>> {
        // TESTFIRST: the native final-audit control must observe missing late
        // and unknown rows before the final-only drain is implemented.
        BTreeMap::new()
    }

    fn record(&self, record: AcceptedRecord) {
        let originals = match &record {
            AcceptedRecord::Agent(witness) => &witness.original_ids,
            AcceptedRecord::Task(witness) => &witness.original_ids,
            AcceptedRecord::Seen(witness) => &witness.original_ids,
            AcceptedRecord::Presence(witness) => &witness.original_ids,
        };
        let mut captured = self.by_original.lock().unwrap();
        for original in originals {
            // Retain duplicates instead of overwriting: consumers must reject
            // an ambiguous original rather than silently taking its latest row.
            captured
                .entry(original.clone())
                .or_default()
                .push(record.clone());
        }
    }
}

impl AppState {
    pub(crate) fn test_capture_accepted_records(&mut self, records: Arc<AcceptedRecords>) {
        assert!(
            self.accepted_records.is_none(),
            "one accepted-record observer per fixture"
        );
        self.accepted_records = Some(records);
    }

    /// Called only after the requested record save and its installation return
    /// success. Reading the installed roster is memory-only; never reload SQL.
    pub(in crate::app) fn test_record_accepted_agent(&self, owner: &str, agent_id: &str) {
        let Some(records) = &self.accepted_records else {
            return;
        };
        let Some(context) = crate::timing::write_wait::current_context() else {
            return;
        };
        let Some(agent) = self
            .entity_agents(owner)
            .ok()
            .and_then(|roster| roster.by_id(agent_id))
        else {
            return;
        };
        records.record(AcceptedRecord::Agent(AgentRecordWitness {
            original_ids: context.original_ids(),
            owner: owner.to_owned(),
            agent: agent.id.clone(),
            topic: agent.topic.clone(),
            name: agent.name.clone(),
            name_asked: agent.name_asked,
            native_revision: None,
        }));
    }

    /// The mutation already has the exact saved task and native events. A
    /// status no-op can still save updated_at and is retained as such.
    pub(in crate::app) fn test_record_accepted_task(&self, task: &Task, events: &[TaskEvent]) {
        let Some(records) = &self.accepted_records else {
            return;
        };
        let Some(context) = crate::timing::write_wait::current_context() else {
            return;
        };
        let Some(status_disposition) = accepted_status(task, events) else {
            return;
        };
        records.record(AcceptedRecord::Task(TaskRecordWitness {
            original_ids: context.original_ids(),
            accepted: AcceptedTaskFields {
                id: task.id.clone(),
                project_path: task.project_path.clone(),
                number: task.number,
                state: task.state,
                status: task.status.clone(),
                title: task.title.clone(),
                body: task.body.clone(),
                priority: task.priority,
                labels: task.labels.clone(),
                updated_at: task.updated_at.clone(),
            },
            status_disposition,
            native_revision: None,
        }));
    }
}

fn accepted_status(task: &Task, events: &[TaskEvent]) -> Option<TaskStatusDisposition> {
    let mut moved = events
        .iter()
        .filter(|event| event.task_id == task.id && event.kind == TaskEventKind::Moved);
    let Some(event) = moved.next() else {
        return Some(TaskStatusDisposition::StatusNoop);
    };
    if moved.next().is_some() {
        // Do not invent a single disposition for an ambiguous native event set.
        return None;
    }
    Some(TaskStatusDisposition::StatusChanged {
        event_id: event.id.clone(),
        from: event.payload.get("from")?.as_str()?.to_owned(),
        to: event.payload.get("to")?.as_str()?.to_owned(),
    })
}
