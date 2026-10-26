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
    edges(&app, &project, &dependent, std::slice::from_ref(&blocker));
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
    assert_unblocked_attention(&read, &last);
}

fn assert_unblocked_attention(read: &Value, last: &str) {
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
            failure_kind: None,
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

#[test]
fn waiting_notices_and_reminders_undelivered_states() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let (owner, agent) = project_agent(&mut app, &project);
    let dependent = id(&filed(&mut app, &project, "queued task"));
    let blocker = id(&filed(&mut app, &project, "last blocker"));
    edges(&app, &project, &dependent, std::slice::from_ref(&blocker));
    call(
        &mut app,
        "tasks.assign",
        json!({"task_id":dependent,"assignee":{"kind":"agent","agent_id":agent}}),
    );
    call(
        &mut app,
        "tasks.update",
        json!({"task_id":blocker,"status":"done"}),
    );
    while app
        .tracker_store()
        .unwrap()
        .reconcile_waiting_on(100)
        .unwrap()
        .pending
    {}
    let intent = app
        .tracker_store()
        .unwrap()
        .current_delivery_intent(&dependent)
        .unwrap()
        .unwrap();
    assert_eq!(intent.state, "queued");
    assert_eq!(
        call(&mut app, "tasks.get", json!({"task_id":dependent}))["task"]["deferred_assignment"]
            ["state"],
        "queued"
    );
    app.remind_of_open_tasks(&owner, &agent);
    assert!(
        !has_reminder(&mut app, &owner, &agent),
        "queued assignment is still undelivered"
    );
    app.tracker_store()
        .unwrap()
        .fail_delivery_intent(&intent.id, intent.generation, "temporary fixture failure")
        .unwrap();
    assert_eq!(
        call(&mut app, "tasks.get", json!({"task_id":dependent}))["task"]["deferred_assignment"]
            ["state"],
        "failed"
    );
    app.remind_of_open_tasks(&owner, &agent);
    assert!(
        !has_reminder(&mut app, &owner, &agent),
        "failed assignment is still undelivered"
    );
    app.retry_task_delivery(&intent.id).unwrap();
    pump(&mut app);
    assert!(
        call(&mut app, "tasks.get", json!({"task_id":dependent}))["task"]
            .get("deferred_assignment")
            .is_none(),
        "successful delivery removes the public pending assignment"
    );
    app.remind_of_open_tasks(&owner, &agent);
    assert!(
        has_reminder(&mut app, &owner, &agent),
        "delivered actionable task still produces reminder"
    );
}
fn has_reminder(app: &mut AppState, owner: &str, agent: &str) -> bool {
    call(
        app,
        "thread.page",
        json!({"entity_id":owner,"agent_id":agent,"limit":100}),
    )["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| {
            m["data"]["body"]
                .as_str()
                .unwrap_or("")
                .contains("reported Complete")
        })
}

#[test]
fn waiting_reads_bound_reverse_preview_and_share_summaries() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let blocker = id(&filed(&mut app, &project, "one shared blocker"));
    for number in 0..21 {
        let dependent = id(&filed(&mut app, &project, &format!("dependent {number}")));
        edges(&app, &project, &dependent, std::slice::from_ref(&blocker));
    }
    let read = call(&mut app, "tasks.get", json!({"task_id":blocker}));
    let dependencies = &read["task"]["dependencies"];
    assert_eq!(dependencies["blocks_count"], 21);
    assert_eq!(dependencies["blocks"].as_array().unwrap().len(), 20);
    assert!(dependencies["blocks_next_cursor"].is_string());
    let list = call(&mut app, "tasks.list", json!({"project_id":project}));
    let task = list["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == blocker)
        .unwrap();
    assert_eq!(&task["dependencies"], dependencies);
    let watched = row(&mut app, &blocker);
    assert_eq!(&watched["dependencies"], dependencies);
}

#[test]
fn waiting_on_invalidation_fanout_contract() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let blocker = id(&filed(&mut app, &project, "fanout blocker"));
    let mut dependents = Vec::new();
    for number in 0..crate::changes::TASKS_PER_FLUSH + 1 {
        let dependent = id(&filed(&mut app, &project, &format!("fanout {number}")));
        edges(&app, &project, &dependent, std::slice::from_ref(&blocker));
        dependents.push(dependent);
    }
    let state = app.shared();
    while AppState::pump_waiting_on_changes_for_test(&state).unwrap() {}
    let handler = AppState::handler(Arc::clone(&state));
    let changes = state.lock().unwrap().changes();
    let (session, mut rx, key) = SessionSender::observable("waiting-fanout");
    assert_eq!(
        handler.call(
            session.clone(),
            req("session.hello", json!({"changes":"subscriptions"}))
        )["ok"],
        true
    );
    assert_eq!(handler.call(session.clone(),req("changes.subscribe",json!({
        "subscription_id":"waiting-tasks","scope":{"kind":"entity","id":project},"kinds":["tasks"]
    })))["ok"],true);
    changes.flush();
    while rx.try_recv().is_ok() {}
    assert_eq!(
        handler.call(
            session,
            req("tasks.update", json!({"task_id":blocker,"status":"done"}))
        )["ok"],
        true
    );
    while AppState::pump_waiting_on_changes_for_test(&state).unwrap() {}
    changes.flush();
    let mut task_pushes = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        let push = SessionSender::decrypt_push(&key, &envelope);
        if push["type"] == "changes" {
            task_pushes.extend(
                push["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|item| item.get("tasks").is_some())
                    .cloned(),
            );
        }
    }
    assert_eq!(task_pushes.len(), 1, "{task_pushes:?}");
    assert_eq!(task_pushes[0]["entity_id"], project);
    assert_eq!(task_pushes[0]["tasks"]["truncated"], true);
    assert!(
        task_pushes[0]["tasks"]["task_ids"]
            .as_array()
            .unwrap()
            .len()
            <= crate::changes::TASKS_PER_FLUSH
    );
    let store = state.lock().unwrap().tracker_store().unwrap().clone();
    let path = state
        .lock()
        .unwrap()
        .tracker_project_path(&project)
        .unwrap();
    let summaries = store.waiting_on_summaries(&path, &dependents).unwrap();
    assert!(summaries.values().all(|summary| !summary.blocked));
}

fn waiting_tool(name: &str, arguments: Value) -> BridgeAction {
    crate::mcp::DoneServer::new("test").handle_message(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}}).to_string()).action.expect("valid waiting tool")
}
fn check_waiting_answer(answer: &Value, blocker: &str, intent: &Value) {
    assert_eq!(answer["task"]["dependencies"]["blocked"], true, "{answer}");
    assert!(answer["task"]["dependencies"]["waiting_on"]
        .as_array()
        .unwrap()
        .iter()
        .any(|edge| edge["task_id"] == blocker));
    assert_eq!(answer["task"]["deferred_assignment"], *intent);
}
#[test]
fn waiting_review_noop_answers_keep_waiting_contract() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let (owner, agent) = project_agent(&mut app, &project);
    let dependent = id(&filed(&mut app, &project, "no-op summaries"));
    let blocker = id(&filed(&mut app, &project, "retained blocker"));
    let other = id(&filed(&mut app, &project, "editable blocker"));
    edges(&app, &project, &dependent, std::slice::from_ref(&blocker));
    let assigned = call(
        &mut app,
        "tasks.assign",
        json!({"task_id":dependent,"assignee":{"kind":"agent","agent_id":agent}}),
    );
    let intent = assigned["task"]["deferred_assignment"].clone();
    app.on_agent_mcp_action(
        &owner,
        &agent,
        BridgeAction::TrackerTrackTask {
            task_id: dependent.clone(),
        },
    )
    .unwrap();
    let tracked = app
        .on_agent_mcp_action(
            &owner,
            &agent,
            BridgeAction::TrackerTrackTask {
                task_id: dependent.clone(),
            },
        )
        .unwrap();
    check_waiting_answer(&tracked, &blocker, &intent);
    let labelled = app
        .on_agent_mcp_action(
            &owner,
            &agent,
            waiting_tool(
                "label_task",
                json!({"task_id": dependent, "add": [], "remove": []}),
            ),
        )
        .unwrap();
    check_waiting_answer(&labelled, &blocker, &intent);
    for name in ["link_task", "unlink_task"] {
        let answered = app
            .on_agent_mcp_action(
                &owner,
                &agent,
                waiting_tool(
                    name,
                    json!({"task_id":dependent,"waiting_on":[other],"track":true}),
                ),
            )
            .unwrap();
        check_waiting_answer(&answered, &blocker, &intent);
    }
    let watched = call(&mut app, "tasks.watch", json!({"task_id":dependent}));
    check_waiting_answer(&watched, &blocker, &intent);
    let read = call(&mut app, "tasks.get", json!({"task_id":dependent}));
    let last = read["timeline"].as_array().unwrap().last().unwrap()["id"].clone();
    call(
        &mut app,
        "tasks.read_through",
        json!({"task_id":dependent,"event_id":last}),
    );
    let reread = call(
        &mut app,
        "tasks.read_through",
        json!({"task_id":dependent,"event_id":last}),
    );
    check_waiting_answer(&reread, &blocker, &intent);
}
#[test]
fn waiting_review_pending_creating_targets_are_assigned_before_pagination() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let (_, agent) = project_agent(&mut app, &project);
    let workspace = super::project_agent::workspace(&mut app, &project, "future worker");
    let blocker = id(&filed(&mut app, &project, "filter blocker"));
    let mut held = Vec::new();
    for target in [
        json!({"kind":"new_workspace","name":"future workspace"}),
        json!({"kind":"new_agent","workspace_id":workspace}),
        json!({"kind":"new_project_agent"}),
    ] {
        let created = call(
            &mut app,
            "tasks.create",
            json!({"project_id":project,"title":"future assignment","waiting_on":[blocker],"assignee":target}),
        );
        assert_eq!(created["task"]["assignee"], Value::Null);
        held.push(created["task"]["id"].clone());
    }
    let unassigned = id(&filed(&mut app, &project, "newest unassigned task"));
    let mut cursor = Value::Null;
    let mut found = Vec::new();
    loop {
        let mut params = json!({"project_id":project,"assignee":"any","limit":1});
        if !cursor.is_null() {
            params["cursor"] = cursor;
        }
        let page = call(&mut app, "tasks.list", params);
        found.extend(
            page["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|task| task["id"].clone()),
        );
        cursor = page["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!(found, held.iter().rev().cloned().collect::<Vec<_>>());
    let none = call(
        &mut app,
        "tasks.list",
        json!({"project_id":project,"assignee":"none"}),
    );
    let ids = none["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["id"].clone())
        .collect::<Vec<_>>();
    assert!(ids.contains(&json!(unassigned)));
    assert!(held.iter().all(|id| !ids.contains(id)));
    let named = call(
        &mut app,
        "tasks.list",
        json!({"project_id":project,"assignee":{"kind":"agent","agent_id":agent}}),
    );
    assert!(named["tasks"].as_array().unwrap().is_empty());
    call(
        &mut app,
        "tasks.assign",
        json!({"task_id":held[0],"assignee":null}),
    );
    let any = call(
        &mut app,
        "tasks.list",
        json!({"project_id":project,"assignee":"any"}),
    );
    assert_eq!(
        any["tasks"].as_array().unwrap().len(),
        2,
        "cancellation clears held target"
    );
}
#[test]
fn waiting_review_unblocked_notifies_other_trackers() {
    for assigned in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (_repo, mut app, project) = tracked(root.path());
        let (owner, holder) = project_agent(&mut app, &project);
        let watcher = app.agent_add(&json!({"entity_id":owner})).unwrap()["agent"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let dependent = id(&filed(&mut app, &project, "watched release"));
        let first = id(&filed(&mut app, &project, "first release prerequisite"));
        let last = id(&filed(&mut app, &project, "final release prerequisite"));
        edges(&app, &project, &dependent, &[first.clone(), last.clone()]);
        app.on_agent_mcp_action(
            &owner,
            &watcher,
            BridgeAction::TrackerTrackTask {
                task_id: dependent.clone(),
            },
        )
        .unwrap();
        if assigned {
            app.on_agent_mcp_action(
                &owner,
                &holder,
                BridgeAction::TrackerTrackTask {
                    task_id: dependent.clone(),
                },
            )
            .unwrap();
            call(
                &mut app,
                "tasks.assign",
                json!({"task_id":dependent,"assignee":{"kind":"agent","agent_id":holder}}),
            );
        }
        pump(&mut app);
        call(
            &mut app,
            "tasks.update",
            json!({"task_id":first,"status":"done"}),
        );
        pump(&mut app);
        let before = task_messages(&mut app, &owner, &watcher, &dependent).len();
        call(
            &mut app,
            "tasks.update",
            json!({"task_id":last,"status":"done"}),
        );
        pump(&mut app);
        let messages = task_messages(&mut app, &owner, &watcher, &dependent);
        let released = messages[before..]
            .iter()
            .filter(|message| message["body"].as_str().unwrap().contains("unblocked"))
            .collect::<Vec<_>>();
        assert_eq!(
            released.len(),
            1,
            "ordinary tracker receives one informational final-release notice: {messages:?}"
        );
        let body = released[0]["body"].as_str().unwrap();
        assert!(body.contains("informational"));
        assert!(!body.contains("Decide who should take it"));
        let held = task_messages(&mut app, &owner, &holder, &dependent);
        assert_eq!(
            held.len(),
            1,
            "holder receives only the assignment brief or unassigned coordinator notice"
        );
        pump(&mut app);
        assert_eq!(
            task_messages(&mut app, &owner, &watcher, &dependent).len(),
            messages.len()
        );
    }
}
#[test]
fn waiting_review_finished_unresolved_task_refuses_user_assignment() {
    for finished in ["closed", "done"] {
        let root = tempfile::tempdir().unwrap();
        let (_repo, mut app, project) = tracked(root.path());
        let (owner, agent) = project_agent(&mut app, &project);
        let dependent = id(&filed(&mut app, &project, "finished waiting task"));
        let blocker = id(&filed(&mut app, &project, "unfinished prerequisite"));
        edges(&app, &project, &dependent, std::slice::from_ref(&blocker));
        call(&mut app, "tasks.unwatch", json!({"task_id":dependent}));
        let update = if finished == "closed" {
            json!({"task_id":dependent,"state":"closed"})
        } else {
            json!({"task_id":dependent,"status":"done"})
        };
        call(&mut app, "tasks.update", update);
        let before = call(&mut app, "tasks.get", json!({"task_id":dependent}));
        let refused = app.handle(req(
            "tasks.assign",
            json!({"task_id":dependent,"assignee":{"kind":"user"}}),
        ));
        assert_eq!(
            refused["ok"], false,
            "finished unresolved task cannot be assigned: {refused}"
        );
        let tool = app.on_agent_mcp_action(
            &owner,
            &agent,
            BridgeAction::TrackerAssignTask {
                task_id: dependent.clone(),
                assignee: json!({"kind":"user"}),
                note: None,
                track: Some(true),
                notify_user: Some(true),
            },
        );
        assert!(tool.is_err());
        let after = call(&mut app, "tasks.get", json!({"task_id":dependent}));
        assert_eq!(
            after, before,
            "refusal leaves watch, tracker, assignee and timeline untouched"
        );
    }
}
