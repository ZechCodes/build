//! Watches count news from the current watch, including the news that asked
//! the user to watch, without restoring an earlier watch's unread history.

use super::project_agent::{added_project, rooted, workspace};
use super::tracker::{filed, tracked};
use super::*;
use crate::mcp::BridgeAction;

fn coding_agent(state: &mut AppState, project: &str) -> (String, String) {
    let workspace = workspace(state, project, "watch baseline");
    let conversation = state.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace}),
    ));
    let entity = conversation["result"]["run_id"].as_str().unwrap().to_owned();
    let added = state.handle(req("agent.add", json!({"entity_id": entity})));
    let agent = added["result"]["agent"]["id"].as_str().unwrap().to_owned();
    (entity, agent)
}

fn comment(state: &mut AppState, who: &(String, String), id: &str, ask: bool) {
    state.on_agent_mcp_action(&who.0, &who.1, BridgeAction::TrackerCommentTask {
        task_id: id.into(), body: "news".into(), refs: Vec::new(), track: None,
        attachments: Vec::new(), notify_user: Some(ask), mention_user: None,
    }).expect("an agent comments");
}

fn task(state: &mut AppState, id: &str) -> Value {
    state.handle(req("tasks.get", json!({"task_id": id})))["result"]["task"].clone()
}

fn assert_count(state: &mut AppState, project: &str, id: &str, count: u64) {
    assert_eq!(task(state, id)["unread_count"], count, "task get");
    let list = state.handle(req("tasks.list", json!({"project_id": project})));
    let listed = list["result"]["tasks"].as_array().unwrap().iter()
        .find(|task| task["id"] == id).unwrap();
    assert_eq!(listed["unread_count"], count, "task list");
    let board = state.handle(req("board.list", json!({})));
    let row = board["result"]["items"].as_array().unwrap().iter()
        .find(|row| row["task_id"] == id).unwrap();
    assert_eq!(row["unread"], count, "inbox row");
}

#[test]
fn explicit_task_watch_excludes_history_and_rewatch_resets_persisted_baseline() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut state, project) = tracked(tmp.path());
    let who = coding_agent(&mut state, &project);
    let created = filed(&mut state, &project, "explicit watch");
    let id = created["id"].as_str().unwrap();
    state.handle(req("tasks.unwatch", json!({"task_id": id})));
    comment(&mut state, &who, id, false);
    state.handle(req("tasks.watch", json!({"task_id": id})));
    assert_count(&mut state, &project, id, 0);
    let baseline = task(&mut state, id)["watch_started_after"].clone();
    assert!(baseline.is_string());
    comment(&mut state, &who, id, false);
    state.handle(req("tasks.watch", json!({"task_id": id})));
    assert_eq!(task(&mut state, id)["watch_started_after"], baseline);
    assert_count(&mut state, &project, id, 1);

    let path = state.projects.iter().find(|p| p.id == project).unwrap().repo_path.clone();
    drop(state);
    let mut restored = rooted(tmp.path()).with_task_store(tmp.path().join("store")).unwrap();
    let project = added_project(&mut restored, &path);
    assert_eq!(task(&mut restored, id)["watch_started_after"], baseline);
    assert_count(&mut restored, &project, id, 1);
    restored.handle(req("tasks.unwatch", json!({"task_id": id})));
    restored.handle(req("tasks.watch", json!({"task_id": id})));
    assert_count(&mut restored, &project, id, 0);
    assert_ne!(task(&mut restored, id)["watch_started_after"], baseline);
}

#[test]
fn implicit_task_watch_excludes_history_but_counts_the_triggering_agent_comment() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut state, project) = tracked(tmp.path());
    let who = coding_agent(&mut state, &project);
    let created = filed(&mut state, &project, "implicit watch");
    let id = created["id"].as_str().unwrap();
    state.handle(req("tasks.unwatch", json!({"task_id": id})));
    comment(&mut state, &who, id, false);
    comment(&mut state, &who, id, true);
    assert_count(&mut state, &project, id, 1);
    assert!(task(&mut state, id)["watch_started_after"].is_string());
}

#[test]
fn commenting_as_user_and_agent_assigning_to_user_start_from_current_history() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut state, project) = tracked(tmp.path());
    let who = coding_agent(&mut state, &project);
    for assigned in [false, true] {
        let created = filed(&mut state, &project, "automatic watch");
        let id = created["id"].as_str().unwrap();
        state.handle(req("tasks.unwatch", json!({"task_id": id})));
        comment(&mut state, &who, id, false);
        if assigned {
            state.on_agent_mcp_action(&who.0, &who.1, BridgeAction::TrackerAssignTask {
                task_id: id.into(), assignee: json!({"kind": "user"}), note: None,
                track: None, notify_user: None,
            }).expect("assigned to user");
        } else {
            state.handle(req("tasks.comment", json!({"task_id": id, "body": "hello"})));
        }
        assert_count(&mut state, &project, id, u64::from(assigned));
    }
}

#[test]
fn legacy_task_watch_uses_the_latest_watched_event_and_a_later_read_mark() {
    use crate::app::tracker::unread_since_mark;
    use crate::tracker::{Actor, Task, TaskEventKind, TimelineEntry};
    let agent = json!({"kind": "agent", "agent_id": "agent-1"});
    let mut task = Task::drafted("/repo", "legacy watch", Actor::User, "now");
    task.watched = true;
    let entries: Vec<TimelineEntry> = [
        json!({"type": "comment", "id": "tc-01K50000000000000000000001", "task_id": task.id, "author": agent, "body": "old", "created_at": "now"}),
        json!({"type": "event", "id": "te-01K50000000000000000000002", "task_id": task.id, "actor": agent, "kind": TaskEventKind::Watched.as_str(), "payload": {}, "at": "now"}),
        json!({"type": "comment", "id": "tc-01K50000000000000000000003", "task_id": task.id, "author": agent, "body": "new", "created_at": "now"}),
    ].into_iter().map(|entry| serde_json::from_value(entry).unwrap()).collect();
    assert_eq!(unread_since_mark(&task, &entries), 1);
    task.read_through = Some("tc-01K50000000000000000000003".into());
    assert_eq!(unread_since_mark(&task, &entries), 0);
    task.watched = false;
    task.read_through = None;
    assert_eq!(unread_since_mark(&task, &entries), 0);
}
