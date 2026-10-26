//! Real stream-json reader to the offline failure pump, using only a fake child.
use super::project_agent::{items, project_agent, workspace};
use super::tracker::{filed, tracked};
use super::*;
use crate::harness::adk::AdkSession;

#[tokio::test]
async fn adk_prompt_too_long_reaches_attention_task_and_coordinator_without_a_client() {
    let temp = tempfile::tempdir().unwrap();
    let (_home, mut app, project) = tracked(temp.path());
    let (coordinator_owner, coordinator) = project_agent(&mut app, &project);
    let ws = workspace(&mut app, &project, "offline ADK overflow");
    let task = filed(&mut app, &project, "Over-window ADK turn");
    let task_id = task["id"].as_str().unwrap().to_string();
    let assigned = app.handle(req(
        "tasks.assign",
        json!({"task_id":task_id, "assignee":{"kind":"new_agent", "workspace_id":ws}}),
    ));
    assert_eq!(assigned["ok"], true, "{assigned}");
    let dispatch = &assigned["result"]["dispatch"];
    let owner = dispatch["entity_id"].as_str().unwrap().to_string();
    let agent = dispatch["agent_id"].as_str().unwrap().to_string();
    let operation = dispatch["operation_id"].as_str().unwrap().to_string();
    let watched = app.handle(req(
        "conversation.watch",
        json!({"entity_id": owner, "agent_id": agent}),
    ));
    assert_eq!(watched["ok"], true, "{watched}");
    // The normal delivery path has consumed the assignment's task authority.
    super::tracker_activity::consume_assignment_turns(&mut app);
    let input = temp.path().join("adk-input");
    let spec = HarnessSpec::new("sh").arg("-c").arg(r#"
        printf '%s\n' '{"type":"system","subtype":"init","session_id":"synthetic-overflow","model":"synthetic"}'
        while IFS= read -r line; do
            printf '%s\n' "$line" >> "$1"
            printf '%s\n' '{"type":"assistant","error":"invalid_request","message":{"content":[{"type":"text","text":"Prompt is too long"}]}}'
            printf '%s\n' '{"type":"result","subtype":"success","is_error":true,"duration_ms":40,"result":"Prompt is too long"}'
        done
    "#).arg("adk-overflow").arg(input.to_string_lossy());
    let choice = ModelChoice {
        provider: AgentProvider::ClaudeAdk,
        ..ModelChoice::default()
    };
    let (session, activity) = AdkSession::spawn(&spec, None, &choice).unwrap();
    wait_for(Duration::from_secs(5), || {
        (session.status() == AgentStatus::Waiting).then_some(())
    })
    .await
    .expect("the fake child initializes");
    let root = app.runs[&owner].worktree.path.clone();
    let key = insert_agent_tab(&mut app, &root, &owner, &agent, session);
    let coordinator_root = app.runs[&coordinator_owner].worktree.path.clone();
    let coordinator_log = SessionLog::default();
    insert_agent_tab(
        &mut app,
        &coordinator_root,
        &coordinator_owner,
        &coordinator,
        DictatedSession::reporting(AgentStatus::Waiting).recording_into(&coordinator_log),
    );
    let worker = Arc::clone(&app.session_registry.test_tab(&key).unwrap().session);
    let state = app.shared();
    // No client frame or synthetic TurnFailed: the real ADK reader owns it.
    spawn_activity_pump(&state, key, Some(activity));
    worker
        .send_turn(&Turn::new("wake already stuck agent").with_operation_id(operation))
        .unwrap();
    wait_for(Duration::from_secs(5), || {
        let mut app = state.lock().unwrap();
        items(&mut app, &owner, &agent)
            .iter()
            .any(|row| row["data"]["outcome"] == "failed")
            .then_some(())
    })
    .await
    .expect("the surviving refused ADK turn becomes visible attention");
    wait_for(
        crate::app::runtime::delivery::queue::NOTICE_SETTLE_WINDOW + Duration::from_secs(5),
        || (!coordinator_log.turns().is_empty()).then_some(()),
    )
    .await
    .expect("the coordinator receives the failure without a client");
    assert_eq!(
        worker.status(),
        AgentStatus::Waiting,
        "provider refusal keeps the child alive"
    );
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
        !reports.iter().any(|row| row["data"]["from_build"] == true),
        "no self-directed failure notice"
    );
    let thread = app.agent_conversation(&owner, Some(&agent)).unwrap();
    let unread = app.agent_unread(
        &owner,
        app.runs[&owner].agents.by_id(&agent).unwrap(),
        thread,
    );
    assert_eq!(unread.reason, Some("run_failed"));
    assert!(unread.count > 0, "the Inbox has unread attention");
    let entry = super::board::attention::board_entry(&mut app, &owner);
    assert_eq!(entry["needs_attention"], true, "{entry}");
    assert_eq!(entry["unread_reason"], "run_failed", "{entry}");
    let read = app.handle(req("tasks.get", json!({"task_id":task_id})));
    let comments: Vec<_> = read["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["type"] == "comment")
        .collect();
    assert_eq!(comments.len(), 1, "{read}");
    assert!(comments[0]["body"]
        .as_str()
        .unwrap()
        .contains("Prompt is too long"));
    let notices = items(&mut app, &coordinator_owner, &coordinator);
    assert_eq!(
        notices
            .iter()
            .filter(|row| row["data"]["from_build"] == true
                && row["data"]["body"]
                    .as_str()
                    .is_some_and(|body| body.contains("Prompt is too long")))
            .count(),
        1
    );
    assert_eq!(coordinator_log.turns().len(), 1);
    assert_eq!(
        std::fs::read_to_string(input).unwrap().lines().count(),
        1,
        "the failed agent is never woken to read its own failure"
    );
    worker.end();
}
