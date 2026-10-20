use super::project_agent::{items, project_agent, workspace};
use super::tracker::{filed, tracked};
use super::*;

fn failed(state: &mut AppState, owner: &str, agent_id: &str, summary: &str) {
    failed_operation(state, owner, agent_id, None, summary);
}

fn failed_operation(
    state: &mut AppState,
    owner: &str,
    agent_id: &str,
    operation_id: Option<&str>,
    summary: &str,
) {
    let key = TabKey::agent(&state.runs[owner].worktree.path, agent_id);
    record_activity(
        state,
        &key,
        owner,
        agent_id,
        &crate::harness::ActivityReport::own_work(crate::harness::AgentActivity::TurnFailed {
            summary: summary.into(),
            operation_id: operation_id.map(str::to_string),
            episode_id: None,
        }),
    );
}

fn workspace_agent(state: &mut AppState, workspace_id: &str) -> (String, String) {
    let ensured = state.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace_id}),
    ));
    assert_eq!(ensured["ok"], true, "{ensured}");
    let owner = ensured["result"]["run_id"].as_str().unwrap().to_string();
    let added = state.handle(req("agent.add", json!({"entity_id": owner})));
    assert_eq!(added["ok"], true, "{added}");
    (
        owner,
        added["result"]["agent"]["id"].as_str().unwrap().to_string(),
    )
}

#[test]
fn a_terminal_provider_failure_reports_once_to_its_task_and_coordinator() {
    let temp = tempfile::tempdir().unwrap();
    let (_home, mut state, project_id) = tracked(temp.path());
    let (coordinator_owner, coordinator) = project_agent(&mut state, &project_id);
    let ws = workspace(&mut state, &project_id, "failure");
    let task = filed(&mut state, &project_id, "Resume stalled agent");
    let task_id = task["id"].as_str().unwrap();
    let assigned = state.handle(req(
        "tasks.assign",
        json!({
            "task_id": task_id,
            "assignee": {"kind": "new_agent", "workspace_id": ws},
        }),
    ));
    assert_eq!(assigned["ok"], true, "{assigned}");
    let owner = assigned["result"]["dispatch"]["entity_id"]
        .as_str()
        .unwrap();
    let agent = assigned["result"]["dispatch"]["agent_id"].as_str().unwrap();
    super::tracker_activity::consume_assignment_turns(&mut state);
    for _ in 0..2 {
        failed(
            &mut state,
            owner,
            agent,
            "Provider retry exhausted: server overloaded",
        );
    }
    let reports = items(&mut state, owner, agent);
    assert_eq!(
        reports
            .iter()
            .filter(|row| row["data"]["outcome"] == "failed")
            .count(),
        1
    );
    let task = state.handle(req("tasks.get", json!({"task_id": task_id})));
    let comments = task["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["type"] == "comment")
        .collect::<Vec<_>>();
    assert_eq!(comments.len(), 1, "{task}");
    assert!(comments[0]["body"]
        .as_str()
        .unwrap()
        .contains("server overloaded"));
    assert_eq!(task["result"]["task"]["status"], "in_progress", "{task}");
    let notices = items(&mut state, &coordinator_owner, &coordinator);
    assert_eq!(
        notices
            .iter()
            .filter(|row| row["data"]["from_build"] == true
                && row["data"]["body"]
                    .as_str()
                    .is_some_and(|body| body.contains("server overloaded")))
            .count(),
        1
    );
    assert!(
        !state.dispatched_task.contains_key(agent),
        "a failed turn spends only its consumed task authority"
    );
}

#[test]
fn a_provider_failure_answers_the_dispatching_agent_without_forwarding_normal_reports() {
    let temp = tempfile::tempdir().unwrap();
    let (_home, mut state, project_id) = tracked(temp.path());
    let (coordinator_owner, coordinator) = project_agent(&mut state, &project_id);
    let requester_ws = workspace(&mut state, &project_id, "dispatcher");
    let (requester_owner, requester) = workspace_agent(&mut state, &requester_ws);
    let ws = workspace(&mut state, &project_id, "worker");
    let (owner, agent) = workspace_agent(&mut state, &ws);
    let sent = state
        .agent_action(
            &requester_owner,
            &requester,
            crate::mcp::BridgeAction::MessageAgent {
                attachments: Vec::new(),
                agent_id: agent.clone(),
                body: "repair the bridge".into(),
            },
        )
        .unwrap();
    let operation = sent["operation_id"].as_str().unwrap();
    state
        .record_native_operation_seen(&owner, &agent, operation)
        .unwrap();
    failed(
        &mut state,
        &owner,
        &agent,
        "Provider rejected request: invalid credentials",
    );
    failed(
        &mut state,
        &owner,
        &agent,
        "Provider rejected request: invalid credentials",
    );
    let notices = items(&mut state, &requester_owner, &requester);
    assert_eq!(
        notices
            .iter()
            .filter(|row| row["data"]["from_build"] == true
                && row["data"]["body"]
                    .as_str()
                    .is_some_and(|body| body.contains("invalid credentials")))
            .count(),
        1
    );
    assert!(!items(&mut state, &coordinator_owner, &coordinator)
        .iter()
        .any(|row| row["data"]["body"]
            .as_str()
            .is_some_and(|body| body.contains("invalid credentials"))));
}

fn assign_failure_task(state: &mut AppState, project: &str, ws: &str) -> (String, String, String) {
    let task = filed(state, project, "Failed turn authority");
    let task_id = task["id"].as_str().unwrap().to_string();
    let assigned = state.handle(req(
        "tasks.assign",
        json!({
            "task_id": task_id, "assignee": {"kind":"new_agent", "workspace_id":ws},
        }),
    ));
    assert_eq!(assigned["ok"], true, "{assigned}");
    let dispatch = &assigned["result"]["dispatch"];
    let owner = dispatch["entity_id"].as_str().unwrap().to_string();
    let agent = dispatch["agent_id"].as_str().unwrap().to_string();
    super::tracker_activity::consume_assignment_turns(state);
    (task_id, owner, agent)
}

#[test]
fn a_failure_cannot_comment_on_a_task_reassigned_after_its_turn_was_consumed() {
    let temp = tempfile::tempdir().unwrap();
    let (_home, mut state, project) = tracked(temp.path());
    project_agent(&mut state, &project);
    let ws = workspace(&mut state, &project, "old holder");
    let (task, owner, agent) = assign_failure_task(&mut state, &project, &ws);
    let changed = state.handle(req(
        "tasks.assign",
        json!({
            "task_id":task, "assignee":{"kind":"new_agent", "workspace_id":ws},
        }),
    ));
    assert_eq!(changed["ok"], true, "{changed}");
    failed(&mut state, &owner, &agent, "old turn failed");
    let read = state.handle(req("tasks.get", json!({"task_id":task})));
    assert!(
        !read["result"]["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["type"] == "comment"),
        "{read}"
    );
    assert!(
        !state.dispatched_task.contains_key(&agent),
        "a stale failed dispatch cannot survive as completion authority"
    );
}

#[test]
fn a_new_conversation_generation_cannot_reuse_a_failed_turns_task_authority() {
    let temp = tempfile::tempdir().unwrap();
    let (_home, mut state, project) = tracked(temp.path());
    project_agent(&mut state, &project);
    let ws = workspace(&mut state, &project, "reset holder");
    let (task, owner, agent) = assign_failure_task(&mut state, &project, &ws);
    failed(&mut state, &owner, &agent, "provider failed");
    let thread = state.agent_conversation(&owner, Some(&agent)).unwrap();
    let reset = state.handle(req(
        "conversation.reset",
        json!({
            "project_id":project, "entity_id":owner, "agent_id":agent,
            "conversation_id":agent, "expected_thread_id":thread.id,
        }),
    ));
    assert_eq!(reset["ok"], true, "{reset}");
    failed(&mut state, &owner, &agent, "provider failed");
    let read = state.handle(req("tasks.get", json!({"task_id":task})));
    assert_eq!(
        read["result"]["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["type"] == "comment")
            .count(),
        1,
        "{read}"
    );
    assert_eq!(
        items(&mut state, &owner, &agent)
            .iter()
            .filter(|row| row["data"]["outcome"] == "failed")
            .count(),
        1
    );
}

#[test]
fn a_saved_self_requester_cannot_wake_the_failed_agent_with_its_failure() {
    let temp = tempfile::tempdir().unwrap();
    let (_home, mut state, project) = tracked(temp.path());
    let (coordinator_owner, coordinator) = project_agent(&mut state, &project);
    let ws = workspace(&mut state, &project, "self requester");
    let (owner, agent) = workspace_agent(&mut state, &ws);
    state.thread_post_from_agent(&json!({
        "entity_id":owner, "agent_id":agent, "body":"legacy saved operation", "operation_id":"op-self-requester",
    }), crate::operation::OperationRequester {
        entity_id:owner.clone(), agent_id:agent.clone(), conversation_id:agent.clone(),
    }).unwrap();
    state
        .record_native_operation_seen(&owner, &agent, "op-self-requester")
        .unwrap();
    failed(
        &mut state,
        &owner,
        &agent,
        "self requester provider failure",
    );
    assert!(!items(&mut state, &owner, &agent)
        .iter()
        .any(|row| row["data"]["from_build"] == true));
    assert_eq!(
        items(&mut state, &coordinator_owner, &coordinator)
            .iter()
            .filter(|row| row["data"]["body"]
                .as_str()
                .is_some_and(|body| body.contains("self requester provider failure")))
            .count(),
        1
    );
}

#[test]
fn a_delayed_failure_spends_its_own_task_and_preserves_a_newer_consumed_turn() {
    let temp = tempfile::tempdir().unwrap();
    let (_home, mut state, project) = tracked(temp.path());
    project_agent(&mut state, &project);
    let ws = workspace(&mut state, &project, "queued failures");
    let (first_task, owner, agent) = assign_failure_task(&mut state, &project, &ws);
    let first_operation = state.task_failure_operation(&agent).unwrap().to_string();
    state.record_agent_working_since(&owner, &agent, Some("2026-10-13T22:00:00Z".into()));
    let second = filed(&mut state, &project, "Newer consumed task");
    let second_task = second["id"].as_str().unwrap().to_string();
    let assigned = state.handle(req(
        "tasks.assign",
        json!({
            "task_id":second_task, "assignee":{"kind":"agent", "agent_id":agent},
        }),
    ));
    assert_eq!(assigned["ok"], true, "{assigned}");
    super::tracker_activity::consume_assignment_turns(&mut state);
    state.record_agent_working_since(&owner, &agent, None);
    state.record_agent_working_since(&owner, &agent, Some("2026-10-13T22:01:00Z".into()));
    let second_operation = state.task_failure_operation(&agent).unwrap().to_string();
    let state_before = state.runs[&owner].run.state;
    for _ in 0..2 {
        failed_operation(
            &mut state,
            &owner,
            &agent,
            Some(&first_operation),
            "old provider turn failed",
        );
    }
    let first = state.handle(req("tasks.get", json!({"task_id":first_task})));
    let second = state.handle(req("tasks.get", json!({"task_id":second_task})));
    assert_eq!(
        first["result"]["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["type"] == "comment")
            .count(),
        1,
        "{first}"
    );
    assert!(
        !second["result"]["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["type"] == "comment"),
        "{second}"
    );
    assert_eq!(
        state.task_failure_operation(&agent),
        Some(second_operation.as_str())
    );
    assert_eq!(
        state.runs[&owner]
            .agents
            .by_id(&agent)
            .unwrap()
            .working_since
            .as_deref(),
        Some("2026-10-13T22:01:00Z")
    );
    assert_eq!(state.runs[&owner].run.state, state_before);
    assert_eq!(
        items(&mut state, &owner, &agent)
            .iter()
            .filter(|row| row["data"]["outcome"] == "failed")
            .count(),
        1
    );
}

#[test]
fn a_failure_creates_the_coordinator_notice_when_no_project_conversation_exists_yet() {
    let temp = tempfile::tempdir().unwrap();
    let (_home, mut state, project) = tracked(temp.path());
    let ws = workspace(&mut state, &project, "no coordinator yet");
    let (_, owner, agent) = assign_failure_task(&mut state, &project, &ws);
    failed(&mut state, &owner, &agent, "coordinator fallback failure");
    let coordinator_owner = state
        .project_conversation_run(&project)
        .expect("the failure reaches a coordinator");
    let coordinator = state.project_pin(&coordinator_owner).unwrap().to_string();
    assert_eq!(
        items(&mut state, &coordinator_owner, &coordinator)
            .iter()
            .filter(|row| row["data"]["body"]
                .as_str()
                .is_some_and(|body| body.contains("coordinator fallback failure")))
            .count(),
        1
    );
}

#[tokio::test]
async fn the_failure_pump_consumes_its_receipt_and_delivers_without_a_client() {
    let temp = tempfile::tempdir().unwrap();
    let (_home, mut app, project) = tracked(temp.path());
    let (coordinator_owner, coordinator) = project_agent(&mut app, &project);
    let ws = workspace(&mut app, &project, "offline failure pump");
    let task = filed(&mut app, &project, "Offline failure notice");
    let task_id = task["id"].as_str().unwrap().to_string();
    let assigned = app.handle(req(
        "tasks.assign",
        json!({
            "task_id":task_id, "assignee":{"kind":"new_agent", "workspace_id":ws},
        }),
    ));
    assert_eq!(assigned["ok"], true, "{assigned}");
    let dispatch = &assigned["result"]["dispatch"];
    let owner = dispatch["entity_id"].as_str().unwrap().to_string();
    let agent = dispatch["agent_id"].as_str().unwrap().to_string();
    let operation = dispatch["operation_id"].as_str().unwrap().to_string();
    app.delivery_queue.clear_queued();
    let root = app.runs[&owner].worktree.path.clone();
    let (_, receipts) = tokio::sync::watch::channel(crate::harness::TurnReceiptSnapshot {
        seen_operation_ids: vec![operation.clone()],
        uncertain_operation_ids: Vec::new(),
    });
    let worker_log = SessionLog::default();
    let key = insert_agent_tab(
        &mut app,
        &root,
        &owner,
        &agent,
        DictatedSession::reporting(AgentStatus::Waiting)
            .watching_receipts(receipts)
            .recording_into(&worker_log),
    );
    let coordinator_root = app.runs[&coordinator_owner].worktree.path.clone();
    let coordinator_log = SessionLog::default();
    insert_agent_tab(
        &mut app,
        &coordinator_root,
        &coordinator_owner,
        &coordinator,
        DictatedSession::reporting(AgentStatus::Waiting).recording_into(&coordinator_log),
    );
    let state = app.shared();
    let (activity, receiver) = broadcast::channel(4);
    // Deliberately start no receipt pump and issue no client frame.
    spawn_activity_pump(&state, key.clone(), Some(receiver));
    activity
        .send(crate::harness::ActivityReport::own_work(
            crate::harness::AgentActivity::TurnFailed {
                summary: "offline provider failure".into(),
                operation_id: Some(operation),
                episode_id: None,
            },
        ))
        .unwrap();
    wait_for(
        crate::app::runtime::delivery::queue::NOTICE_SETTLE_WINDOW + Duration::from_secs(5),
        || (!coordinator_log.turns().is_empty()).then_some(()),
    )
    .await
    .expect("the failure notice wakes its coordinator without a client RPC");
    let read = state
        .lock()
        .unwrap()
        .handle(req("tasks.get", json!({"task_id":task_id})));
    assert_eq!(
        read["result"]["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["type"] == "comment")
            .count(),
        1,
        "{read}"
    );
    assert!(
        worker_log.turns().is_empty(),
        "the failed holder must not be woken to read its own failure"
    );
    drop(activity);
    wait_for(Duration::from_secs(5), || {
        (!state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(&key)
            .unwrap()
            .live)
            .then_some(())
    })
    .await
    .expect("the failed session stream is closed");
    let mut app = state.lock().unwrap();
    let reports = items(&mut app, &owner, &agent);
    assert_eq!(
        reports
            .iter()
            .filter(|row| row["data"]["outcome"] == "failed")
            .count(),
        1
    );
    assert!(
        !reports
            .iter()
            .any(|row| row["data"]["event"] == "interrupted"),
        "EOF cannot add another terminal failure after the report"
    );
}
