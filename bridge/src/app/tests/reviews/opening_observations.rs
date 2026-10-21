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

fn deferred_open(
    shared: &Arc<Mutex<AppState>>,
    workspace: &str,
) -> crate::app::runtime::deferred::DeferredJob {
    let (result, job) = shared
        .lock()
        .unwrap()
        .dispatch_deferring("tasks.review.open", &open_params(workspace));
    result.unwrap();
    job.unwrap()
}

#[test]
fn delayed_opening_observation_preserves_newer_close_and_replacement_review() {
    for replace in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let (_repository, mut app, project) = tracked(home.path());
        let workspace = workspace(&mut app, &project, "delayed discovery");
        let (shared, handler) = shared_handler(app);
        let opened = call(&handler, "tasks.review.open", open_params(&workspace));
        let delayed = deferred_open(&shared, &workspace).run();
        call(
            &handler,
            "tasks.review.close",
            json!({"task_id":opened["task"]["id"], "expected_version":opened["review"]["version"], "description":"Superseded"}),
        );
        if replace {
            let mut params = open_params(&workspace);
            params["request_id"] = json!("replacement-review");
            let replacement = call(&handler, "tasks.review.open", params);
            assert_ne!(replacement["task"]["id"], opened["task"]["id"]);
            assert_eq!(replacement["review"]["version"], 1);
            assert_workspace_review(&handler, &workspace, &replacement);
        }
        let expected = call(&handler, "workspace.get", json!({"workspace_id":workspace}));
        assert_eq!(
            expected["active_review"]["status"],
            if replace { "open" } else { "closed" }
        );
        shared
            .lock()
            .unwrap()
            .apply_deferred("tasks.review.open", &open_params(&workspace), delayed)
            .unwrap();

        let actual = call(&handler, "workspace.get", json!({"workspace_id":workspace}));
        assert_eq!(actual["active_review"], expected["active_review"]);
    }
}

#[test]
fn opening_observation_sql_finishes_while_app_mutex_is_held() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracked(home.path());
    let workspace = workspace(&mut app, &project, "off-lock discovery");
    let store = app.store.clone().unwrap();
    let (gate, paused) = OffLockGate::new();
    app.off_lock_review_open_observation_gate = Some(gate);
    let (shared, handler) = shared_handler(app);
    let job = deferred_open(&shared, &workspace);
    let (sent, completed) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || sent.send(job.run()).unwrap());
    paused.wait_for_arrival();
    let writer = store.writer_for_test();
    let app_guard = shared
        .try_lock()
        .expect("observation does not hold app mutex");
    paused.release();
    assert!(matches!(
        completed.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    drop(writer);
    let done = completed
        .recv_timeout(Duration::from_secs(5))
        .expect("observation SQL finishes without acquiring app mutex");
    drop(app_guard);
    worker.join().unwrap();
    let opened = shared
        .lock()
        .unwrap()
        .apply_deferred("tasks.review.open", &open_params(&workspace), done)
        .unwrap();

    assert_workspace_review(&handler, &workspace, &opened);
}

#[test]
fn failed_opening_observation_preserves_cache_and_same_request_retry_repairs_it() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracked(home.path());
    let workspace = workspace(&mut app, &project, "repair discovery");
    let store = app.store.clone().unwrap();
    let (gate, paused) = OffLockGate::new();
    let (shared, handler) = shared_handler(app);
    let opened = call(&handler, "tasks.review.open", open_params(&workspace));
    let before = call(&handler, "workspace.get", json!({"workspace_id":workspace}));
    assert_eq!(before["active_review"]["task_id"], opened["task"]["id"]);
    shared.lock().unwrap().off_lock_review_open_observation_gate = Some(gate);
    let job = deferred_open(&shared, &workspace);
    let worker = std::thread::spawn(move || job.run());
    paused.wait_for_arrival();
    let (task, record): (String, String) = {
        let writer = store.writer_for_test();
        let row = writer
            .query_row("SELECT task_id, record FROM reviews", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        writer
            .execute("UPDATE reviews SET record = '{}'", [])
            .unwrap();
        row
    };
    paused.release();
    let done = worker.join().unwrap();
    let error = shared
        .lock()
        .unwrap()
        .apply_deferred("tasks.review.open", &open_params(&workspace), done)
        .unwrap_err();
    assert!(error.contains("review"), "{error}");
    let after = call(&handler, "workspace.get", json!({"workspace_id":workspace}));
    assert_eq!(after["active_review"], before["active_review"]);
    store
        .writer_for_test()
        .execute(
            "UPDATE reviews SET record = ?1 WHERE task_id = ?2",
            [&record, &task],
        )
        .unwrap();
    shared.lock().unwrap().off_lock_review_open_observation_gate = None;

    let retried = call(&handler, "tasks.review.open", open_params(&workspace));

    assert_eq!(retried["task"]["id"], task);
    assert_workspace_review(&handler, &workspace, &retried);
}
