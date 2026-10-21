//! OFFSOURCE TESTFIRST task half. The accepted-record scaffold emits nothing.
//! Transport owner must close/join RPC receivers and MCP servers before check().
use super::fixture::SavedRequest;
use crate::app::accepted_records::{
    AcceptedRecord, AcceptedRecords, AcceptedTaskFields, TaskRecordWitness, TaskStatusDisposition,
};
use crate::app::tests::runtime::write_wait_qualification::rpc::ReplyProof;
use crate::mcp::BridgeAction;
use crate::store::Store;
use crate::tracker::{Actor, Task, TaskEventKind, TaskState, TimelineEntry};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const RPC_IDS: [&str; 2] = ["saved-task/rpc/0", "saved-task/rpc/1"];
pub(super) const MCP_IDS: [&str; 2] = ["saved-task/mcp/0", "saved-task/mcp/1"];

/// Four independent originals; submit together behind one held AppState.
pub(super) fn task_requests(task_id: &str) -> Vec<SavedRequest> {
    RPC_IDS
        .into_iter()
        .map(|original_id| SavedRequest::Rpc {
            original_id: original_id.into(),
            method: "tasks.update",
            params: json!({"task_id":task_id,"status":"in_progress"}),
        })
        .chain(MCP_IDS.into_iter().map(|original_id| SavedRequest::Mcp {
            original_id: original_id.into(),
            action: BridgeAction::TrackerMoveTask {
                task_id: task_id.into(),
                status: "in_progress".into(),
                track: Some(false),
            },
        }))
        .collect()
}

/// Invoke only after SavedFixture::finish has closed and joined every owned
/// holder/socket/server/RPC receiver and its actual cleanup report is checked.
pub(super) fn assert_task_witnesses(
    baseline: Task,
    project: &str,
    coding_agent: &str,
    replies: Vec<Result<ReplyProof, String>>,
    records: &AcceptedRecords,
    store: &Store,
) -> Vec<TaskRecordWitness> {
    let replies = replies
        .into_iter()
        .map(|result| {
            result.expect("every original transport completed; errors are not successful writes")
        })
        .collect();
    TaskControl::new(baseline, project.into(), coding_agent.into()).check(replies, records, store)
}

struct TaskControl {
    baseline: Task,
    project: String,
    coding_agent: String,
}

impl TaskControl {
    /// Seed one fresh Open/backlog card, without assignment, dependencies,
    /// trackers or linked workspaces. Clone Store while holding AppState once;
    /// release AppState before loading this baseline through that Store clone.
    pub(super) fn new(baseline: Task, project: String, coding_agent: String) -> Self {
        assert_eq!(baseline.state, TaskState::Open);
        assert_eq!(baseline.status, "backlog");
        assert!(baseline.assignee.is_none());
        assert!(baseline.trackers.is_empty());
        assert!(baseline.links.workspace_ids.is_empty());
        assert!(baseline.title.len() <= 128 && baseline.body.len() <= 256);
        assert!(baseline.labels.len() <= 2);
        Self {
            baseline,
            project,
            coding_agent,
        }
    }

    /// Called only AFTER all four actual calls and all cleanup/join attempts.
    /// Caller checks native transport cleanup reports before this hard oracle.
    /// No AppState reference is accepted, so these supplemental SQL reads cannot
    /// accidentally extend its critical section.
    pub(super) fn check(
        &self,
        replies: Vec<ReplyProof>,
        records: &AcceptedRecords,
        store: &Store,
    ) -> Vec<TaskRecordWitness> {
        let ids = RPC_IDS.into_iter().chain(MCP_IDS).collect::<BTreeSet<_>>();
        assert_eq!(replies.len(), 4);
        let mut actual = BTreeMap::new();
        for proof in replies {
            assert!(ids.contains(proof.original_id.as_str()));
            let original = proof.original_id.clone();
            assert!(
                actual.insert(original, proof).is_none(),
                "duplicate reply original"
            );
        }
        assert_eq!(actual.len(), ids.len());
        // All four actual positive replies and durable effects are checked
        // before the deliberate missing-observer RED can fire.
        for original in &ids {
            let proof = &actual[*original];
            let rpc = RPC_IDS.contains(original);
            assert_eq!(
                proof.method,
                if rpc {
                    "tasks.update"
                } else {
                    "mcp.control:move_task"
                }
            );
            assert_eq!(proof.reply["ok"], true, "{original}: {}", proof.reply);
            if rpc {
                assert_eq!(proof.reply["id"], *original);
            }
            let task = &proof.reply["result"]["task"];
            assert_eq!(task["id"], self.baseline.id);
            assert_eq!(task["status"], "in_progress");
            assert_eq!(task["state"], "open");
            assert!(
                chrono::DateTime::parse_from_rfc3339(task["updated_at"].as_str().unwrap()).is_ok()
            );
        }
        let final_task = store.load_tracker_task(&self.baseline.id).unwrap().unwrap();
        assert_eq!(final_task.status, "in_progress");
        assert_eq!(final_task.state, TaskState::Open);
        let timeline = store.load_tracker_timeline(&self.baseline.id).unwrap();
        assert_eq!(
            timeline
                .iter()
                .filter(|entry| matches!(entry,
            TimelineEntry::Event(event) if event.kind == TaskEventKind::Moved))
                .count(),
            1
        );
        let mut witnesses = Vec::new();
        for original in ids {
            let proof = &actual[original];
            let rpc = RPC_IDS.contains(&original);
            assert_eq!(
                proof.method,
                if rpc {
                    "tasks.update"
                } else {
                    "mcp.control:move_task"
                }
            );
            assert_eq!(proof.reply["ok"], true, "{original}: {}", proof.reply);
            if rpc {
                assert_eq!(proof.reply["id"], original);
            }
            let captured = records.take(original);
            assert_eq!(
                captured.len(),
                1,
                "{original}: missing/duplicate accepted native record; NOOP scaffold expected RED"
            );
            let AcceptedRecord::Task(witness) = captured.into_iter().next().unwrap() else {
                panic!("{original}: agent witness cannot prove a task write")
            };
            assert_eq!(witness.original_ids, vec![original.to_owned()]);
            assert_eq!(
                witness.native_revision, None,
                "Task has native updated_at, not a monotonic product revision"
            );
            self.assert_accepted(&witness.accepted, &proof.reply["result"]["task"]);
            witnesses.push(witness);
        }
        self.check_status_dispositions(&witnesses, store);
        let final_fields = fields(&final_task);
        assert!(
            witnesses
                .iter()
                .any(|witness| witness.accepted == final_fields),
            "the final durable record must be an actual accepted record, not replacement work"
        );
        witnesses
    }

    fn assert_accepted(&self, fields: &AcceptedTaskFields, reply: &Value) {
        assert_eq!(fields.id, self.baseline.id);
        assert_eq!(fields.project_path, self.baseline.project_path);
        assert_eq!(fields.number, self.baseline.number);
        assert_eq!(fields.state, TaskState::Open);
        assert_eq!(fields.status, "in_progress");
        assert_eq!(fields.title, self.baseline.title);
        assert_eq!(fields.body, self.baseline.body);
        assert_eq!(fields.priority, self.baseline.priority);
        assert_eq!(fields.labels, self.baseline.labels);
        assert!(chrono::DateTime::parse_from_rfc3339(&fields.updated_at).is_ok());
        assert_eq!(reply["project_id"], self.project);
        assert_eq!(reply["id"], fields.id);
        assert_eq!(reply["number"], fields.number);
        assert_eq!(reply["state"], json!(fields.state));
        assert_eq!(reply["status"], fields.status);
        assert_eq!(reply["title"], fields.title);
        assert_eq!(reply["body"], fields.body);
        assert_eq!(reply["priority"], json!(fields.priority));
        assert_eq!(reply["labels"], json!(fields.labels));
        assert_eq!(
            reply["updated_at"], fields.updated_at,
            "native saved timestamp belongs to this original, not a later latest-record read"
        );
    }

    fn check_status_dispositions(&self, witnesses: &[TaskRecordWitness], store: &Store) {
        let timeline = store.load_tracker_timeline(&self.baseline.id).unwrap();
        let moved = timeline
            .iter()
            .filter_map(|entry| match entry {
                TimelineEntry::Event(event) if event.kind == TaskEventKind::Moved => Some(event),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            moved.len(),
            1,
            "one real transition; repeated statuses create no invented events"
        );
        let mut changed = 0;
        let mut noops = 0;
        for witness in witnesses {
            match &witness.status_disposition {
                TaskStatusDisposition::StatusNoop => noops += 1,
                TaskStatusDisposition::StatusChanged { event_id, from, to } => {
                    changed += 1;
                    assert_eq!(from, "backlog");
                    assert_eq!(to, "in_progress");
                    let event = moved[0];
                    assert_eq!(&event.id, event_id);
                    assert_eq!(event.task_id, self.baseline.id);
                    assert_eq!(event.payload, json!({"from":"backlog","to":"in_progress"}));
                    let expected_actor = if RPC_IDS.contains(&witness.original_ids[0].as_str()) {
                        Actor::User
                    } else {
                        Actor::Agent {
                            agent_id: self.coding_agent.clone(),
                        }
                    };
                    assert_eq!(event.actor, expected_actor);
                    assert_eq!(event.at, witness.accepted.updated_at);
                }
            }
        }
        assert_eq!(
            (changed, noops),
            (1, 3),
            "same-target concurrent calls guarantee real no-ops without caller ordering"
        );
    }
}

/// Pure bounded test projection. This is never an observer callback or a
/// successful witness insertion; it serves only baseline/final comparisons.
fn fields(task: &Task) -> AcceptedTaskFields {
    AcceptedTaskFields {
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
    }
}
