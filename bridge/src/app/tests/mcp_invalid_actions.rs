//! Authenticated control requests always receive an answer, including requests
//! from a newer adapter whose action the current daemon cannot deserialize.

use super::*;
use crate::app::mcp::{
    handle_authenticated_mcp_frame, handle_done_stream, ControlPlane, Turn, MCP_CONTROL_METHOD,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

const TOKEN: &str = "test-mcp-capability-672";
const PRIVATE_INPUT: &str = "private-request-content-672";

#[derive(Clone, Copy)]
enum Surface {
    Coding,
    Router,
    Project,
}

fn fixture(surface: Surface) -> (tempfile::TempDir, Arc<Mutex<AppState>>, String) {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let agent = match surface {
        Surface::Coding => {
            let owner = file_legacy_task(&mut app, "malformed coding action");
            primary_agent_id(&app, &owner)
        }
        Surface::Router => super::routing::router::captured(&mut app, "malformed router action").1,
        Surface::Project => {
            let project = app.project_at(0).id.clone();
            super::project_agent::project_agent(&mut app, &project).1
        }
    };
    app.delivery_queue.clear_queued();
    app.session_registry
        .test_install_token(agent.clone(), TOKEN.into());
    assert!(app.addressed_session(agent.clone()).is_some());
    (dir, app.shared(), agent)
}

fn frame(agent: &str, payload: Value) -> Value {
    let mut frame = json!({ "task_id": agent, "session_token": TOKEN });
    frame
        .as_object_mut()
        .unwrap()
        .extend(payload.as_object().unwrap().clone());
    frame
}

fn parse_error() -> Value {
    json!({ "ok": false, "error": "invalid MCP action request" })
}

async fn answer(state: &Arc<Mutex<AppState>>, frame: &Value) -> Option<Value> {
    let clock = Arc::clone(&state.lock().unwrap().frame_clock);
    let plane = ControlPlane::new(Arc::clone(&clock));
    let timer = clock.frame(MCP_CONTROL_METHOD);
    let mut turn = Turn::take(&plane, &timer).await;
    handle_authenticated_mcp_frame(state, frame, &timer, &mut turn).await
}

async fn assert_invalid_actions(surface: Surface) {
    let (_dir, state, agent) = fixture(surface);
    for payload in [
        json!({ "request": { "action": PRIVATE_INPUT } }),
        json!({ "request": { "action": "set_topic", "topic": { "private": PRIVATE_INPUT } } }),
        json!({ "request": { "action": "set_topic" } }),
        json!({ "request": PRIVATE_INPUT }),
        json!({ "request": null }),
        json!({}),
    ] {
        let response = answer(&state, &frame(&agent, payload)).await;
        assert_eq!(response, Some(parse_error()));
    }
}

#[tokio::test]
async fn malformed_coding_actions_receive_explicit_errors() {
    assert_invalid_actions(Surface::Coding).await;
}

#[tokio::test]
async fn malformed_router_actions_receive_explicit_errors() {
    assert_invalid_actions(Surface::Router).await;
}

#[tokio::test]
async fn malformed_project_actions_receive_explicit_errors() {
    assert_invalid_actions(Surface::Project).await;
}

#[tokio::test]
async fn malformed_actions_do_not_bypass_session_authentication() {
    for surface in [Surface::Coding, Surface::Router, Surface::Project] {
        let (_dir, state, agent) = fixture(surface);
        let mut frame = frame(&agent, json!({ "request": { "action": PRIVATE_INPUT } }));
        frame["session_token"] = json!("wrong-test-capability");
        assert_eq!(
            answer(&state, &frame).await,
            Some(json!({ "ok": false, "error": "unauthorized MCP session" }))
        );
    }
}

#[tokio::test]
async fn valid_done_reports_remain_notifications() {
    for surface in [Surface::Coding, Surface::Router, Surface::Project] {
        let (_dir, state, agent) = fixture(surface);
        let report = json!({ "report": { "status": "blocked", "summary": "test completion" } });
        assert_eq!(answer(&state, &frame(&agent, report)).await, None);
    }
}

#[tokio::test]
async fn malformed_action_receives_a_socket_reply_without_waiting_for_the_client_deadline() {
    let (_dir, state, agent) = fixture(Surface::Coding);
    let plane = ControlPlane::new(Arc::clone(&state.lock().unwrap().frame_clock));
    let (client, daemon) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(handle_done_stream(state, daemon, plane));
    let frame = frame(&agent, json!({ "request": { "action": PRIVATE_INPUT } }));
    let mut client = tokio::io::BufReader::new(client);
    client
        .get_mut()
        .write_all(format!("{frame}\n").as_bytes())
        .await
        .unwrap();
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(5), client.read_line(&mut response))
        .await
        .expect("a malformed action must produce a response, not a silent wait")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&response).unwrap(),
        parse_error()
    );
    drop(client);
    server.await.unwrap();
}
