//! A completed turn cannot revive assignment authority retired by a task edit.

use super::project_agent::workspace;
use super::tracker::{filed, tracked};
use super::*;
use crate::tracker::Actor;

struct DispatchedTask {
    _root: tempfile::TempDir,
    _home: tempfile::TempDir,
    app: AppState,
    project_id: String,
    task_id: String,
    entity_id: String,
    agent_id: String,
}

impl DispatchedTask {
    fn new() -> Self {
        Self::prepared(false).0
    }

    fn prepared(with_finished_prerequisite: bool) -> (Self, Option<String>) {
        let root = tempfile::tempdir().unwrap();
        let (home, mut app, project_id) = tracked(root.path());
        let ws = workspace(&mut app, &project_id, "completion-fence");
        let task_id = filed(&mut app, &project_id, "started task")["id"]
            .as_str()
            .unwrap()
            .to_string();
        let prerequisite = with_finished_prerequisite
            .then(|| Self::finished_prerequisite(&mut app, &project_id, &task_id));
        let assigned = app.handle(req(
            "tasks.assign",
            json!({"task_id":task_id,"assignee":{"kind":"new_agent","workspace_id":ws}}),
        ));
        assert_eq!(assigned["ok"], true, "{assigned}");
        let dispatch = &assigned["result"]["dispatch"];
        let dispatched = Self {
            _root: root,
            _home: home,
            app,
            project_id,
            task_id,
            entity_id: dispatch["entity_id"].as_str().unwrap().to_string(),
            agent_id: dispatch["agent_id"].as_str().unwrap().to_string(),
        };
        (dispatched, prerequisite)
    }

    fn finished_prerequisite(app: &mut AppState, project_id: &str, task_id: &str) -> String {
        let prerequisite = filed(app, project_id, "finished prerequisite")["id"]
            .as_str()
            .unwrap()
            .to_string();
        let closed = app.handle(req("tasks.close", json!({"task_id":prerequisite})));
        assert_eq!(closed["ok"], true, "{closed}");
        let project = app.tracker_project_path(project_id).unwrap();
        app.tracker_store()
            .unwrap()
            .edit_waiting_on(
                &project,
                task_id,
                std::slice::from_ref(&prerequisite),
                &[],
                &Actor::User,
                &crate::store::now_rfc3339(),
            )
            .unwrap();
        prerequisite
    }

    fn call(&mut self, method: &str, mut params: Value) {
        params["task_id"] = json!(self.task_id);
        let answered = self.app.handle(req(method, params));
        assert_eq!(answered["ok"], true, "{answered}");
    }

    fn assert_complete_leaves_task(&mut self, status: &str) {
        let before = self
            .app
            .handle(req("tasks.get", json!({"task_id":self.task_id})));
        self.app
            .move_held_task_on_complete(&self.entity_id, &self.agent_id);
        let after = self
            .app
            .handle(req("tasks.get", json!({"task_id":self.task_id})));
        assert_eq!(after["result"]["task"]["status"], status, "{after}");
        assert_eq!(after["result"]["timeline"], before["result"]["timeline"]);
        assert!(!self.app.dispatched_task.contains_key(&self.agent_id));
    }

    fn add_blocker(&mut self) -> String {
        let blocker = filed(&mut self.app, &self.project_id, "new prerequisite")["id"]
            .as_str()
            .unwrap()
            .to_string();
        let project = self.app.tracker_project_path(&self.project_id).unwrap();
        self.app
            .tracker_store()
            .unwrap()
            .edit_waiting_on(
                &project,
                &self.task_id,
                &[blocker],
                &[],
                &Actor::User,
                &crate::store::now_rfc3339(),
            )
            .unwrap();
        project
    }

    fn consume_turn(&mut self) {
        let mut pending = self.app.take_pending_turns();
        let mut consumed = false;
        while let Some((turn, mark)) = pending.next_turn() {
            if turn.agent_id == self.agent_id {
                assert!(turn.operation_id.is_some(), "the assignment has a receipt");
                self.app.consume_task_completion_operation(
                    &turn.owner,
                    &turn.agent_id,
                    turn.operation_id.as_deref().unwrap(),
                );
                consumed = true;
            }
            mark.settle(&mut self.app);
        }
        assert!(consumed, "the dispatched turn was consumed");
    }

    fn operation_id(&self) -> String {
        self.app
            .tracker_store()
            .unwrap()
            .current_delivery_intent(&self.task_id)
            .unwrap()
            .unwrap()
            .operation_id
    }

    fn assert_complete_reviews_task(&mut self) {
        self.app
            .move_held_task_on_complete(&self.entity_id, &self.agent_id);
        let after = self
            .app
            .handle(req("tasks.get", json!({"task_id":self.task_id})));
        assert_eq!(after["result"]["task"]["status"], "in_review", "{after}");
    }

    fn abandon_owner(&mut self) {
        let abandoned = self
            .app
            .handle(req("run.abandon", json!({"run_id":self.entity_id})));
        assert_eq!(abandoned["ok"], true, "{abandoned}");
        let owner = self.app.runs.get(&self.entity_id).unwrap();
        assert!(owner.run.state.is_terminal());
        assert!(owner.agents.by_id(&self.agent_id).is_some());
    }

    fn reset_target(&mut self) -> String {
        let address = self
            .app
            .resolve_conversation_address(&self.entity_id, Some(&self.agent_id))
            .unwrap();
        let old_thread = self.app.conversation_at(&address).unwrap().id.clone();
        let reset = self.app.handle(req(
            "conversation.reset",
            json!({
                "project_id":self.project_id,"entity_id":self.entity_id,"agent_id":self.agent_id,
                "conversation_id":address.conversation_id,"expected_thread_id":old_thread,
            }),
        ));
        assert_eq!(reset["ok"], true, "{reset}");
        let current = self
            .app
            .resolve_conversation_address(&self.entity_id, Some(&self.agent_id))
            .unwrap();
        assert_eq!(current.conversation_id, address.conversation_id);
        let new_thread = self.app.conversation_at(&current).unwrap().id.clone();
        assert_ne!(new_thread, old_thread);
        assert!(!self
            .app
            .task_completion_authority
            .contains_key(&self.agent_id));
        assert!(!self
            .app
            .pending_task_completion_authority
            .contains_key(&self.agent_id));
        new_thread
    }

    fn assign_another_task(&mut self, title: &str) -> String {
        let id = filed(&mut self.app, &self.project_id, title)["id"]
            .as_str()
            .unwrap()
            .to_string();
        let assigned = self.app.handle(req(
            "tasks.assign",
            json!({
                "task_id":id,"assignee":{"kind":"agent","agent_id":self.agent_id},
            }),
        ));
        assert_eq!(assigned["ok"], true, "{assigned}");
        id
    }
}

#[test]
fn completion_fence_rejects_pre_reset_receipt_reconstruction() {
    let mut task = DispatchedTask::new();
    // Keep the second assignment admitted but unsettled, as at a recovery
    // boundary. The receipt can survive independently of the resident marker.
    task.app
        .stop_task_delivery_after_for_test(crate::app::tracker::DeferredDeliveryBoundary::Posted);
    task.call(
        "tasks.assign",
        json!({"assignee":{"kind":"agent","agent_id":task.agent_id}}),
    );
    let operation = task.operation_id();
    let receipt = task
        .app
        .tracker_store()
        .unwrap()
        .operation(&operation)
        .unwrap()
        .unwrap();
    let new_thread = task.reset_target();
    assert!(task.app.operation_receipt(&operation).unwrap().is_none());
    let agents = task
        .app
        .entity_agents(&task.entity_id)
        .unwrap()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    task.app
        .tracker_store()
        .unwrap()
        .accept_thread_post(&task.entity_id, &agents, &receipt)
        .unwrap();
    let address = task
        .app
        .resolve_conversation_address(&task.entity_id, Some(&task.agent_id))
        .unwrap();
    assert_eq!(task.app.conversation_at(&address).unwrap().id, new_thread);
    task.app
        .record_native_operation_seen(&task.entity_id, &task.agent_id, &operation)
        .unwrap();
    task.assert_complete_leaves_task("in_progress");
    task.call(
        "tasks.assign",
        json!({"assignee":{"kind":"agent","agent_id":task.agent_id}}),
    );
    task.consume_turn();
    task.assert_complete_reviews_task();
}

#[test]
fn completion_fence_rechecks_reset_generation_of_captured_turn() {
    let mut task = DispatchedTask::new();
    task.consume_turn();
    let captured = task
        .app
        .task_completion_authority
        .remove(&task.agent_id)
        .unwrap();
    task.reset_target();
    // A callback holding the preceding turn's marker must not restore it.
    task.app
        .task_completion_authority
        .insert(task.agent_id.clone(), captured);
    task.assert_complete_leaves_task("in_progress");
}

#[test]
fn completion_fence_rejects_late_complete_from_abandoned_owner() {
    let mut task = DispatchedTask::new();
    task.consume_turn();
    task.abandon_owner();
    task.assert_complete_leaves_task("in_progress");
}

#[test]
fn completion_fence_rejects_native_reconstruction_on_abandoned_owner() {
    let mut task = DispatchedTask::new();
    let operation = task.operation_id();
    task.abandon_owner();
    task.app
        .record_native_operation_seen(&task.entity_id, &task.agent_id, &operation)
        .unwrap();
    task.assert_complete_leaves_task("in_progress");
}

#[test]
fn completion_fence_keeps_consumed_pending_task_when_another_assignment_is_accepted() {
    let mut task = DispatchedTask::new();
    task.consume_turn();
    task.app
        .record_agent_working_since(&task.entity_id, &task.agent_id, Some("task A".into()));
    let second = task.assign_another_task("task B already consumed");
    task.consume_turn();
    let third = task.assign_another_task("task C still unconsumed");
    task.assert_complete_reviews_task();
    task.app
        .record_agent_working_since(&task.entity_id, &task.agent_id, None);
    task.app
        .record_agent_working_since(&task.entity_id, &task.agent_id, Some("task B".into()));
    task.app
        .move_held_task_on_complete(&task.entity_id, &task.agent_id);
    let second = task.app.handle(req("tasks.get", json!({"task_id":second})));
    let third = task.app.handle(req("tasks.get", json!({"task_id":third})));
    assert_eq!(second["result"]["task"]["status"], "in_review", "{second}");
    assert_eq!(third["result"]["task"]["status"], "in_progress", "{third}");
}

#[test]
fn completion_fence_two_queued_assignments_keep_unrelated_turn_fenced() {
    let mut task = DispatchedTask::new();
    // Finish A, then start a turn with no task completion authority.
    task.consume_turn();
    task.assert_complete_reviews_task();
    task.app.record_agent_working_since(
        &task.entity_id,
        &task.agent_id,
        Some("unrelated turn".into()),
    );
    let second = task.assign_another_task("task B awaiting consumption");
    let operation = task
        .app
        .tracker_store()
        .unwrap()
        .current_delivery_intent(&second)
        .unwrap()
        .unwrap()
        .operation_id;
    let third = task.assign_another_task("task C awaiting consumption");
    // C has replaced B's unconsumed pending marker when B's Seen arrives.
    task.app
        .record_native_operation_seen(&task.entity_id, &task.agent_id, &operation)
        .unwrap();
    task.app
        .move_held_task_on_complete(&task.entity_id, &task.agent_id);
    for id in [&second, &third] {
        let task = task.app.handle(req("tasks.get", json!({"task_id":id})));
        assert_eq!(task["result"]["task"]["status"], "in_progress", "{task}");
    }
    // Waiting again must also remain fenced against another old Complete.
    task.task_id = second;
    task.call("tasks.update", json!({"status":"ready"}));
    task.add_blocker();
    task.call(
        "tasks.assign",
        json!({"assignee":{"kind":"agent","agent_id":task.agent_id}}),
    );
    task.assert_complete_leaves_task("ready");
    task.app
        .record_agent_working_since(&task.entity_id, &task.agent_id, None);
    task.app
        .record_agent_working_since(&task.entity_id, &task.agent_id, Some("task B".into()));
    task.assert_complete_leaves_task("ready");
}

#[test]
fn completion_fence_requires_delivery_consumption() {
    let mut task = DispatchedTask::new();
    task.assert_complete_leaves_task("in_progress");
    task.consume_turn();
    task.assert_complete_reviews_task();
}

#[test]
fn completion_fence_does_not_promote_an_abandoned_queue_mark() {
    let mut task = DispatchedTask::new();
    let mut pending = task.app.take_pending_turns();
    while let Some((_, mark)) = pending.next_turn() {
        mark.settle(&mut task.app);
    }
    task.app.record_agent_working_since(
        &task.entity_id,
        &task.agent_id,
        Some("unrelated turn".into()),
    );
    task.assert_complete_leaves_task("in_progress");
}

#[test]
fn completion_fence_receipt_replay_cannot_rearm_a_completed_turn() {
    let mut task = DispatchedTask::new();
    let operation = task.operation_id();
    task.app
        .record_native_operation_seen(&task.entity_id, &task.agent_id, &operation)
        .unwrap();
    task.assert_complete_reviews_task();
    task.call("tasks.update", json!({"status":"in_progress"}));
    task.app
        .record_native_operation_seen(&task.entity_id, &task.agent_id, &operation)
        .unwrap();
    task.assert_complete_leaves_task("in_progress");
}

#[test]
fn completion_fence_reconstructs_from_checked_native_receipt() {
    let mut task = DispatchedTask::new();
    let operation = task.operation_id();
    task.app.task_completion_authority.clear();
    task.app.pending_task_completion_authority.clear();
    task.app.dispatched_task.clear();
    task.app.record_agent_working_since(
        &task.entity_id,
        &task.agent_id,
        Some(crate::store::now_rfc3339()),
    );
    task.app
        .record_native_operation_seen(&task.entity_id, &task.agent_id, &operation)
        .unwrap();
    task.assert_complete_reviews_task();
}

#[test]
fn completion_fence_rejects_unknown_and_foreign_native_receipts() {
    let mut task = DispatchedTask::new();
    let operation = task.operation_id();
    assert!(task
        .app
        .record_native_operation_seen(&task.entity_id, &task.agent_id, "unknown")
        .is_err());
    assert!(task
        .app
        .record_native_operation_seen(&task.entity_id, "foreign-agent", &operation)
        .is_err());
    task.assert_complete_leaves_task("in_progress");
}

#[test]
fn completion_fence_promotes_seen_pending_on_next_working_boundary() {
    let mut task = DispatchedTask::new();
    task.consume_turn();
    task.app
        .record_agent_working_since(&task.entity_id, &task.agent_id, Some("old turn".into()));
    task.call(
        "tasks.assign",
        json!({"assignee":{"kind":"agent","agent_id":task.agent_id}}),
    );
    task.consume_turn();
    // Seen while the old interval is still Working belongs to the queued turn.
    task.assert_complete_leaves_task("in_progress");
    task.app
        .record_agent_working_since(&task.entity_id, &task.agent_id, None);
    task.app
        .record_agent_working_since(&task.entity_id, &task.agent_id, Some("new turn".into()));
    task.assert_complete_reviews_task();
}

#[test]
fn completion_fence_cannot_review_consumed_work_with_reopened_prerequisite() {
    let (mut task, prerequisite) = DispatchedTask::prepared(true);
    task.consume_turn();
    let reopened = task.app.handle(req(
        "tasks.reopen",
        json!({"task_id":prerequisite.unwrap()}),
    ));
    assert_eq!(reopened["ok"], true, "{reopened}");
    let project = task.app.tracker_project_path(&task.project_id).unwrap();
    assert!(
        task.app
            .tracker_store()
            .unwrap()
            .waiting_on_summary(&project, &task.task_id)
            .unwrap()
            .blocked
    );
    task.assert_complete_leaves_task("in_progress");
}

#[test]
fn completion_fence_survives_return_to_ready() {
    let mut task = DispatchedTask::new();
    task.consume_turn();
    task.call("tasks.update", json!({"status":"ready"}));
    task.assert_complete_leaves_task("ready");
}

#[test]
fn completion_fence_survives_done_then_resume_without_assignment() {
    let mut task = DispatchedTask::new();
    task.consume_turn();
    task.call("tasks.update", json!({"status":"done"}));
    task.call("tasks.update", json!({"status":"in_progress"}));
    task.assert_complete_leaves_task("in_progress");
}

#[test]
fn completion_fence_survives_close_then_reopen() {
    let mut task = DispatchedTask::new();
    task.consume_turn();
    task.call("tasks.close", json!({}));
    task.call("tasks.reopen", json!({}));
    task.assert_complete_leaves_task("in_progress");
}

#[test]
fn completion_fence_survives_unassignment_then_same_agent_assignment_waiting() {
    let mut task = DispatchedTask::new();
    task.consume_turn();
    task.call("tasks.assign", json!({"assignee":null}));
    task.call("tasks.update", json!({"status":"ready"}));
    task.add_blocker();
    task.call(
        "tasks.assign",
        json!({"assignee":{"kind":"agent","agent_id":task.agent_id}}),
    );
    task.assert_complete_leaves_task("ready");
}

#[test]
fn completion_fence_survives_target_removal() {
    let mut task = DispatchedTask::new();
    task.consume_turn();
    let removed = task.app.handle(req(
        "agent.remove",
        json!({"entity_id":task.entity_id,"agent_id":task.agent_id}),
    ));
    assert_eq!(removed["ok"], true, "{removed}");
    task.assert_complete_leaves_task("in_progress");
}

#[test]
fn completion_fence_keeps_new_wait_in_ready() {
    let mut task = DispatchedTask::new();
    task.consume_turn();
    task.call("tasks.update", json!({"status":"ready"}));
    let project = task.add_blocker();
    task.assert_complete_leaves_task("ready");
    assert!(
        task.app
            .tracker_store()
            .unwrap()
            .waiting_on_summary(&project, &task.task_id)
            .unwrap()
            .blocked
    );
}

#[test]
fn completion_fence_survives_same_agent_reassignment_waiting_after_consumed_turn() {
    let mut task = DispatchedTask::new();
    task.consume_turn();
    task.app.record_agent_working_since(
        &task.entity_id,
        &task.agent_id,
        Some(crate::store::now_rfc3339()),
    );
    task.call("tasks.update", json!({"status":"ready"}));
    task.add_blocker();
    let reassigned = task.app.handle(req(
        "tasks.assign",
        json!({"task_id":task.task_id,"assignee":{"kind":"agent","agent_id":task.agent_id}}),
    ));
    assert_eq!(reassigned["ok"], true, "{reassigned}");
    assert_eq!(
        reassigned["result"]["dispatch"],
        Value::Null,
        "{reassigned}"
    );
    task.assert_complete_leaves_task("ready");
}

#[test]
fn completion_fence_preserves_running_generation_until_reassigned_turn_is_consumed() {
    let mut task = DispatchedTask::new();
    task.consume_turn();
    task.app.record_agent_working_since(
        &task.entity_id,
        &task.agent_id,
        Some(crate::store::now_rfc3339()),
    );
    task.call(
        "tasks.assign",
        json!({"assignee":{"kind":"agent","agent_id":task.agent_id}}),
    );
    task.assert_complete_leaves_task("in_progress");

    task.app
        .record_agent_working_since(&task.entity_id, &task.agent_id, None);
    task.consume_turn();
    task.app
        .move_held_task_on_complete(&task.entity_id, &task.agent_id);
    let after = task
        .app
        .handle(req("tasks.get", json!({"task_id":task.task_id})));
    assert_eq!(after["result"]["task"]["status"], "in_review", "{after}");
}
