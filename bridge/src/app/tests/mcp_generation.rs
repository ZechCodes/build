//! A frame may authenticate before a reset and wait for the mutation lock.
//! Its authenticated generation must survive that wait unchanged.

use super::*;
use crate::app::mcp::{
    authenticated_mcp_owner, handle_coding_mcp_frame, AddressedSession, ControlPlane, Turn,
    MCP_CONTROL_METHOD,
};
use crate::mcp::BridgeAction;

fn admitted_frame(
    app: &mut AppState,
    owner: &str,
    agent: &str,
    token: &str,
    payload: Value,
) -> (Value, AddressedSession) {
    app.session_registry
        .test_install_token(agent.into(), token.into());
    let mut frame = json!({ "task_id": agent, "session_token": token });
    frame
        .as_object_mut()
        .unwrap()
        .extend(payload.as_object().unwrap().clone());
    let authenticated = authenticated_mcp_owner(&frame, &app.session_registry).unwrap();
    let endpoint = app.addressed_session(authenticated.into()).unwrap();
    assert!(
        matches!(&endpoint, AddressedSession::Coding(generation) if generation.entity_id == owner)
    );
    (frame, endpoint)
}

fn clear_agent(app: &mut AppState, owner: &str, agent: &str) {
    let thread = app.agent_conversation(owner, Some(agent)).unwrap();
    let params = json!({
        "project_id": app.projects.project_id_of(owner).unwrap(),
        "entity_id": owner, "agent_id": agent, "conversation_id": agent,
        "expected_thread_id": thread.id,
    });
    let answer = app.handle(req("conversation.reset", params));
    assert_eq!(answer["ok"], true, "{answer:?}");
    assert!(!app
        .session_registry
        .token_matches(agent, "old-session-token"));
}

async fn finish_admitted_frame(
    app: AppState,
    frame: &Value,
    endpoint: &AddressedSession,
) -> (Arc<Mutex<AppState>>, Option<Value>) {
    let state = Arc::new(Mutex::new(app));
    let clock = Arc::clone(&state.lock().unwrap().frame_clock);
    let plane = ControlPlane::new(Arc::clone(&clock));
    let mut turn = Turn::take(&plane).await;
    let timer = clock.frame(MCP_CONTROL_METHOD);
    let AddressedSession::Coding(generation) = endpoint else {
        panic!("a coding endpoint")
    };
    let response = handle_coding_mcp_frame(&state, frame, generation, &timer, &mut turn).await;
    (state, response)
}

#[tokio::test]
async fn mcp_generation_rejects_done_authenticated_before_clear() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut app, "old done race");
    let agent = primary_agent_id(&app, &owner);
    let report =
        super::project_agent::terminal(crate::mcp::DoneStatus::Completed, "old private answer");
    let (frame, endpoint) = admitted_frame(
        &mut app,
        &owner,
        &agent,
        "old-session-token",
        json!({ "report": report }),
    );
    clear_agent(&mut app, &owner, &agent);

    let (state, response) = finish_admitted_frame(app, &frame, &endpoint).await;
    let app = state.lock().unwrap();
    assert!(
        app.agent_conversation(&owner, Some(&agent))
            .unwrap()
            .items
            .is_empty(),
        "an admitted old report cannot repopulate the cleared conversation"
    );
    assert_eq!(response.unwrap()["ok"], false);
}

#[tokio::test]
async fn mcp_generation_rejects_message_agent_authenticated_before_clear() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut app, "old agent send race");
    let agent = primary_agent_id(&app, &owner);
    let target = app.handle(req("agent.add", json!({ "entity_id": owner })))["result"]["agent"]
        ["id"]
        .as_str()
        .unwrap()
        .to_string();
    let action = BridgeAction::MessageAgent {
        agent_id: target.clone(),
        body: "old private outbound answer".into(),
    };
    let (frame, endpoint) = admitted_frame(
        &mut app,
        &owner,
        &agent,
        "old-session-token",
        json!({ "request": action }),
    );
    clear_agent(&mut app, &owner, &agent);

    let (state, response) = finish_admitted_frame(app, &frame, &endpoint).await;
    let app = state.lock().unwrap();
    assert!(
        app.agent_conversation(&owner, Some(&agent))
            .unwrap()
            .items
            .is_empty(),
        "an old send cannot write its sender copy into the new generation"
    );
    assert!(
        app.agent_conversation(&owner, Some(&target))
            .unwrap()
            .items
            .is_empty(),
        "a retired sender cannot reach another conversation"
    );
    assert_eq!(response.unwrap()["ok"], false);
}

#[test]
fn mcp_generation_rejects_deferred_task_dispatch_after_sender_clear() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut app, project_id) = super::tracker::tracked(tmp.path());
    let (owner, agent) = super::project_agent::project_agent(&mut app, &project_id);
    let task = super::tracker::filed(&mut app, &project_id, "Old deferred private task");
    let action = BridgeAction::TrackerAssignTask {
        task_id: task["id"].as_str().unwrap().into(),
        assignee: json!({ "kind": "new_workspace", "isolation": "worktree" }),
        note: Some("old private assignment note".into()),
        track: None,
        notify_user: None,
    };
    let (answer, deferred) = app.agent_action_deferring(&owner, &agent, action);
    answer.unwrap();
    let deferred = deferred.expect("workspace creation runs outside the app mutex");
    clear_agent(&mut app, &owner, &agent);
    let done = deferred.run();
    let settled = app.apply_deferred(MCP_CONTROL_METHOD, &Value::Null, done);

    assert!(
        settled.is_err(),
        "an old sender's deferred dispatch must be refused: {settled:?}"
    );
    let stored = app.handle(req("tasks.get", json!({ "task_id": task["id"] })));
    assert_eq!(stored["result"]["task"]["assignee"], Value::Null);
    assert!(app
        .agent_conversation(&owner, Some(&agent))
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
async fn mcp_generation_accepts_the_fresh_endpoint_after_clear() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut app, "fresh endpoint");
    let agent = primary_agent_id(&app, &owner);
    clear_agent(&mut app, &owner, &agent);
    let action = BridgeAction::PostThreadMessage {
        body: "fresh answer".into(),
        still_working: false,
        options: Vec::new(),
    };
    let (frame, endpoint) = admitted_frame(
        &mut app,
        &owner,
        &agent,
        "fresh-session-token",
        json!({ "request": action }),
    );

    let (state, response) = finish_admitted_frame(app, &frame, &endpoint).await;
    assert_eq!(response.unwrap()["ok"], true);
    let app = state.lock().unwrap();
    let items = &app.agent_conversation(&owner, Some(&agent)).unwrap().items;
    assert_eq!(items.len(), 1);
    assert!(serde_json::to_value(items)
        .unwrap()
        .to_string()
        .contains("fresh answer"));
}
