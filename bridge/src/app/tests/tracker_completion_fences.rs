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
        let root = tempfile::tempdir().unwrap();
        let (home, mut app, project_id) = tracked(root.path());
        let ws = workspace(&mut app, &project_id, "completion-fence");
        let task_id = filed(&mut app, &project_id, "started task")["id"]
            .as_str()
            .unwrap()
            .to_string();
        let assigned = app.handle(req(
            "tasks.assign",
            json!({"task_id":task_id,"assignee":{"kind":"new_agent","workspace_id":ws}}),
        ));
        assert_eq!(assigned["ok"], true, "{assigned}");
        let dispatch = &assigned["result"]["dispatch"];
        Self {
            _root: root,
            _home: home,
            app,
            project_id,
            task_id,
            entity_id: dispatch["entity_id"].as_str().unwrap().to_string(),
            agent_id: dispatch["agent_id"].as_str().unwrap().to_string(),
        }
    }

    fn call(&mut self, method: &str, mut params: Value) {
        params["task_id"] = json!(self.task_id);
        let answered = self.app.handle(req(method, params));
        assert_eq!(answered["ok"], true, "{answered}");
    }

    fn assert_complete_leaves_task(&mut self, status: &str) {
        let before = self.app.handle(req("tasks.get", json!({"task_id":self.task_id})));
        self.app
            .move_held_task_on_complete(&self.entity_id, &self.agent_id);
        let after = self.app.handle(req("tasks.get", json!({"task_id":self.task_id})));
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
                consumed = true;
            }
            mark.settle(&mut self.app);
        }
        assert!(consumed, "the dispatched turn was consumed");
    }
}

#[test]
fn completion_fence_survives_return_to_ready() {
    let mut task = DispatchedTask::new();
    task.call("tasks.update", json!({"status":"ready"}));
    task.assert_complete_leaves_task("ready");
}

#[test]
fn completion_fence_survives_done_then_resume_without_assignment() {
    let mut task = DispatchedTask::new();
    task.call("tasks.update", json!({"status":"done"}));
    task.call("tasks.update", json!({"status":"in_progress"}));
    task.assert_complete_leaves_task("in_progress");
}

#[test]
fn completion_fence_survives_close_then_reopen() {
    let mut task = DispatchedTask::new();
    task.call("tasks.close", json!({}));
    task.call("tasks.reopen", json!({}));
    task.assert_complete_leaves_task("in_progress");
}

#[test]
fn completion_fence_survives_unassignment_then_same_agent_assignment_waiting() {
    let mut task = DispatchedTask::new();
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
    task.call("tasks.update", json!({"status":"ready"}));
    let project = task.add_blocker();
    task.assert_complete_leaves_task("ready");
    assert!(task
        .app
        .tracker_store()
        .unwrap()
        .waiting_on_summary(&project, &task.task_id)
        .unwrap()
        .blocked);
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
    assert_eq!(reassigned["result"]["dispatch"], Value::Null, "{reassigned}");
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

    task.app.record_agent_working_since(&task.entity_id, &task.agent_id, None);
    task.consume_turn();
    task.app
        .move_held_task_on_complete(&task.entity_id, &task.agent_id);
    let after = task.app.handle(req("tasks.get", json!({"task_id":task.task_id})));
    assert_eq!(after["result"]["task"]["status"], "in_review", "{after}");
}
