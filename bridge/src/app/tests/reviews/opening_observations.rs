//! A successful opening must publish its workspace summary before replying.
use super::*;
use crate::app::mcp::{handle_authenticated_mcp_frame, ControlPlane, Turn, MCP_CONTROL_METHOD};

fn shared_handler(app: AppState) -> (Arc<Mutex<AppState>>, FrameHandler) {
    let shared = Arc::new(Mutex::new(app));
    shared.lock().unwrap().self_handle = Some(Arc::downgrade(&shared));
    let handler = AppState::handler(Arc::clone(&shared));
    (shared, handler)
}

fn call(handler: &FrameHandler, method: &str, params: Value) -> Value {
    let answer = handler.call(
        SessionSender::detached("review-opening"),
        req(method, params),
    );
    assert_eq!(answer["ok"], true, "{answer}");
    answer["result"].clone()
}

fn open_params(workspace: &str) -> Value {
    json!({
        "workspace_id": workspace,
        "request_id": "publish-workspace-review",
        "title": "Publish review discovery",
        "description": "The immediate workspace read includes this review",
    })
}

fn assert_workspace_review(handler: &FrameHandler, workspace: &str, opened: &Value) {
    assert_eq!(opened["opening_state"], "published", "{opened}");
    let detail = call(handler, "workspace.get", json!({"workspace_id": workspace}));
    let summary = &detail["active_review"];
    assert_eq!(summary["task_id"], opened["task"]["id"], "{detail}");
    assert_eq!(summary["version"], opened["review"]["version"], "{detail}");
    assert_eq!(summary["status"], "open", "{detail}");
    assert_eq!(
        summary["latest_published_snapshot_id"], opened["review"]["snapshots"][0]["id"],
        "{detail}"
    );
}

#[test]
fn rpc_open_review_refreshes_initialized_workspace_observation_before_reply() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracked(home.path());
    let workspace = workspace(&mut app, &project, "rpc discovery");
    let (shared, handler) = shared_handler(app);
    let before = call(
        &handler,
        "workspace.get",
        json!({"workspace_id": workspace}),
    );
    assert!(before.get("active_review").is_none(), "{before}");

    let samples = shared.lock().unwrap().frame_clock.capture_phases();
    let opened = call(&handler, "tasks.review.open", open_params(&workspace));
    println!("TASK727_HOLD {}", samples.snapshot()["tasks.review.open"]);

    assert_workspace_review(&handler, &workspace, &opened);
}

#[tokio::test]
async fn mcp_open_review_refreshes_initialized_workspace_observation_before_reply() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracked(home.path());
    let workspace = workspace(&mut app, &project, "mcp discovery");
    let (_owner, agent) = project_agent(&mut app, &project);
    app.test_install_mcp_token(agent.clone(), "review-opening-test-token".into());
    let (shared, handler) = shared_handler(app);
    let before = call(
        &handler,
        "workspace.get",
        json!({"workspace_id": workspace}),
    );
    assert!(before.get("active_review").is_none(), "{before}");
    let action = crate::mcp::DoneServer::for_owner(&agent)
        .handle_message(
            &json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params": {
                "name":"open_review", "arguments":open_params(&workspace),
            }})
            .to_string(),
        )
        .action
        .expect("open_review parses");
    let (clock, plane) = {
        let app = shared.lock().unwrap();
        (Arc::clone(&app.frame_clock), ControlPlane::new(&app))
    };
    let timer = clock.frame(MCP_CONTROL_METHOD);
    let mut turn = Turn::take(&plane, &timer).await;
    let answer = handle_authenticated_mcp_frame(
        &shared,
        &json!({"task_id":agent, "session_token":"review-opening-test-token", "request":action}),
        &timer,
        &mut turn,
    )
    .await
    .expect("open_review replies");
    assert_eq!(answer["ok"], true, "{answer}");

    assert_workspace_review(&handler, &workspace, &answer["result"]);
}
