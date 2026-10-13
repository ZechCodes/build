//! Quiet waiting assignments, completion and durable user attention.
use super::project_agent::project_agent;
use super::tracker::{filed, tracked};
use super::*;
use crate::mcp::{BridgeAction, DoneReport, DoneStatus};
use crate::tracker::Actor;

fn id(task: &Value) -> String {
    task["id"].as_str().unwrap().to_string()
}
fn call(app: &mut AppState, method: &str, params: Value) -> Value {
    let answer = app.handle(req(method, params));
    assert_eq!(answer["ok"], true, "{answer}");
    answer["result"].clone()
}
fn edges(app: &AppState, project: &str, dependent: &str, blockers: &[String]) {
    app.tracker_store()
        .unwrap()
        .edit_waiting_on(
            &app.tracker_project_path(project).unwrap(),
            dependent,
            blockers,
            &[],
            &Actor::User,
            &crate::store::now_rfc3339(),
        )
        .unwrap();
}
fn pump(app: &mut AppState) {
    for _ in 0..1000 {
        if !app.pump_task_deliveries_for_test().unwrap() {
            return;
        }
    }
    panic!("delivery did not settle");
}
fn row(app: &mut AppState, task: &str) -> Value {
    call(app, "board.list", json!({}))["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["task_id"] == task)
        .unwrap()
        .clone()
}
fn task_messages(app: &mut AppState, owner: &str, agent: &str, task: &str) -> Vec<Value> {
    call(
        app,
        "thread.page",
        json!({"entity_id":owner,"agent_id":agent,"limit":100}),
    )["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "message")
        .map(|item| item["data"].clone())
        .filter(|message| message["from_task"]["task_id"] == task)
        .collect()
}
fn question(app: &mut AppState, owner: &str, agent: &str, task: &str) {
    app.on_agent_mcp_action(
        owner,
        agent,
        BridgeAction::TrackerCommentTask {
            task_id: task.into(),
            body: "Which route should I use?".into(),
            refs: Vec::new(),
            track: None,
            attachments: Vec::new(),
            notify_user: None,
            mention_user: Some(true),
        },
    )
    .unwrap();
}

#[test]
fn unblock_preserves_user_attention() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let (owner, agent) = project_agent(&mut app, &project);
    let dependent = id(&filed(&mut app, &project, "user held"));
    let blocker = id(&filed(&mut app, &project, "prerequisite"));
    call(
        &mut app,
        "tasks.assign",
        json!({"task_id":dependent,"assignee":{"kind":"user"}}),
    );
    let before = row(&mut app, &dependent);
    let mut saved = app
        .tracker_store()
        .unwrap()
        .load_tracker_task(&dependent)
        .unwrap()
        .unwrap();
    let timeline = app
        .tracker_store()
        .unwrap()
        .load_tracker_timeline(&dependent)
        .unwrap();
    let last = match timeline.last().unwrap() {
        crate::tracker::TimelineEntry::Comment(c) => c.id.clone(),
        crate::tracker::TimelineEntry::Event(e) => e.id.clone(),
    };
    saved.read_through = Some(last.clone());
    saved.dismissed_through = Some(last.clone());
    app.tracker_store()
        .unwrap()
        .save_tracker_task_activity(&saved, &[], &[])
        .unwrap();
    edges(&app, &project, &dependent, &[blocker.clone()]);
    pump(&mut app);
    let blocked = row(&mut app, &dependent);
    assert_eq!(blocked["dependencies"]["blocked"], true);
    assert_eq!(
        blocked["assigned_to_user"], false,
        "waiting work cannot be acted on"
    );
    assert_eq!(blocked["anchor"], before["anchor"]);
    assert_eq!(blocked["done_until_next"], true);
    assert_eq!(blocked["unread"], 0);
    question(&mut app, &owner, &agent, &dependent);
    let asked = row(&mut app, &dependent);
    assert_eq!(
        asked["unread"], 1,
        "explicit questions remain unread while blocked"
    );
    assert_eq!(asked["done_until_next"], false);
    call(
        &mut app,
        "tasks.update",
        json!({"task_id":blocker,"status":"done"}),
    );
    pump(&mut app);
    let unblocked = row(&mut app, &dependent);
    assert_eq!(unblocked["assigned_to_user"], true);
    assert_eq!(unblocked["dependencies"]["blocked"], false);
    assert_eq!(unblocked["unread"], asked["unread"]);
    assert_eq!(unblocked["anchor"], asked["anchor"]);
    let read = call(&mut app, "tasks.get", json!({"task_id":dependent}));
    assert_eq!(read["task"]["read_through"], last);
    assert_eq!(read["task"]["dismissed_through"], last);
    assert_eq!(read["task"]["watched"], true);
    assert_eq!(
        read["task"]["dependencies"]["waiting_on"][0]["resolved_by"],
        "done"
    );
    assert!(read["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "unblocked")
        .all(|e| e["mentions_user"] != true));
}

#[test]
fn waiting_notices_and_reminders() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let (owner, agent) = project_agent(&mut app, &project);
    let reviewer = app.agent_add(&json!({"entity_id":owner})).unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let dependent = id(&filed(&mut app, &project, "held until ready"));
    let first = id(&filed(&mut app, &project, "first prerequisite"));
    let last = id(&filed(&mut app, &project, "last prerequisite"));
    edges(&app, &project, &dependent, &[first.clone(), last.clone()]);
    call(
        &mut app,
        "tasks.assign",
        json!({"task_id":dependent,"assignee":{"kind":"agent","agent_id":agent}}),
    );
    app.remind_of_open_tasks(&owner, &agent);
    assert!(
        call(
            &mut app,
            "thread.page",
            json!({"entity_id":owner,"agent_id":agent,"limit":100})
        )["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| !m["data"]["body"]
                .as_str()
                .unwrap_or("")
                .contains("reported Complete")),
        "undelivered task cannot cause a reminder"
    );
    assert!(task_messages(&mut app, &owner, &agent, &dependent).is_empty());
    // Explicit tracking does not turn waiting assignment bookkeeping into work.
    app.on_agent_mcp_action(
        &owner,
        &agent,
        BridgeAction::TrackerTrackTask {
            task_id: dependent.clone(),
        },
    )
    .unwrap();
    call(
        &mut app,
        "tasks.update",
        json!({"task_id":dependent,"title":"current work title"}),
    );
    assert!(
        task_messages(&mut app, &owner, &agent, &dependent).is_empty(),
        "holder must not be nagged while waiting"
    );
    question(&mut app, &owner, &reviewer, &dependent);
    assert_eq!(
        task_messages(&mut app, &owner, &agent, &dependent).len(),
        1,
        "comments still arrive"
    );
    call(
        &mut app,
        "tasks.update",
        json!({"task_id":first,"status":"done"}),
    );
    pump(&mut app);
    assert_eq!(
        task_messages(&mut app, &owner, &agent, &dependent).len(),
        1,
        "partial release is quiet"
    );
    call(
        &mut app,
        "tasks.update",
        json!({"task_id":last,"status":"done"}),
    );
    pump(&mut app);
    assert_eq!(
        task_messages(&mut app, &owner, &agent, &dependent).len(),
        2,
        "one assignment brief"
    );
    super::tracker_activity::consume_assignment_turns(&mut app);
    call(
        &mut app,
        "tasks.update",
        json!({"task_id":first,"status":"ready"}),
    );
    pump(&mut app);
    let messages = task_messages(&mut app, &owner, &agent, &dependent);
    assert_eq!(messages.len(), 3);
    assert!(messages[2]["body"]
        .as_str()
        .unwrap()
        .contains("informational"));
    app.done_deferring_for_agent(
        &owner,
        &agent,
        DoneReport {
            status: DoneStatus::Completed,
            summary: "unrelated turn complete".into(),
            message_id: None,
        },
    );
    assert_eq!(
        call(&mut app, "tasks.get", json!({"task_id":dependent}))["task"]["status"],
        "in_progress",
        "reblocked task cannot auto move on Complete"
    );
    call(
        &mut app,
        "tasks.update",
        json!({"task_id":first,"status":"done"}),
    );
    pump(&mut app);
    assert_eq!(
        task_messages(&mut app, &owner, &agent, &dependent).len(),
        3,
        "unblock never redispatches consumed assignment"
    );
}

#[test]
fn closed_blocker_notice_contract() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let (owner, agent) = project_agent(&mut app, &project);
    let dependent = id(&filed(&mut app, &project, "coordinator assessment"));
    let closed = id(&filed(&mut app, &project, "earlier abandoned prerequisite"));
    let last = id(&filed(&mut app, &project, "last prerequisite"));
    call(&mut app, "tasks.close", json!({"task_id":closed}));
    edges(&app, &project, &dependent, &[closed.clone(), last.clone()]);
    pump(&mut app);
    call(
        &mut app,
        "tasks.update",
        json!({"task_id":last,"status":"done"}),
    );
    pump(&mut app);
    let messages = task_messages(&mut app, &owner, &agent, &dependent);
    assert_eq!(messages.len(), 1);
    let body = messages[0]["body"].as_str().unwrap();
    for text in [
        "closed without Done",
        "earlier abandoned prerequisite",
        "assess",
        "comment_task",
        "mention_user: true",
    ] {
        assert!(body.contains(text), "{body}");
    }
    assert!(
        !body.contains(&last),
        "Done is distinct from closure: {body}"
    );
    assert!(messages[0]["mentions_user"] != true);
    let read = call(&mut app, "tasks.get", json!({"task_id":dependent}));
    assert_eq!(
        read["task"]["dependencies"]["waiting_on"][0]["resolved_by"],
        "close"
    );
    assert_eq!(
        read["task"]["dependencies"]["waiting_on"][1]["resolved_by"],
        "done"
    );
    pump(&mut app);
    assert_eq!(
        task_messages(&mut app, &owner, &agent, &dependent).len(),
        1,
        "one durable coordinator notice"
    );
}
