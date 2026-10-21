//! Published MCP lists bypass writer turns and never run list maintenance.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn mcp_response_envelope_preserves_owned_payload_allocations() {
    let title = "already assembled response content ".repeat(100);
    let allocation = title.as_ptr();
    let result = Value::String(title);
    let reply = crate::app::mcp::mcp_action_response(Ok(result));
    assert_eq!(reply["ok"], true);
    assert_eq!(
        reply["result"].as_str().unwrap().as_ptr(),
        allocation,
        "wrapping an owned response must move its payload without serializing a copy"
    );
}

pub(super) fn router(fixture: &Fixture) -> (String, String) {
    let mut app = fixture.state.lock().unwrap();
    let (capture, agent) =
        super::super::routing::router::captured(&mut app, "published read-lane route");
    app.test_install_mcp_token(agent.clone(), TOKEN.into());
    app.publish_all_snapshot_projection();
    (capture, agent)
}

pub(super) fn router_frame(agent: &str, action: BridgeAction) -> Value {
    json!({"task_id":agent,"session_token":TOKEN,"request":action})
}

fn published(app: &AppState, method: &str, project: Option<&str>) -> Value {
    app.snapshot_read(
        method,
        &project.map_or_else(|| json!({}), |id| json!({"project_id":id})),
    )
    .expect("fixture must initialize the real producer root")
    .unwrap()
}

// Independent legacy router wire oracle over the producer's complete board.
fn work_digest(board: &Value) -> Value {
    json!({"work":board["items"].as_array().unwrap().iter()
        .filter(|row| row["kind"] != "capture")
        .map(|row| json!({
            "kind":row["kind"],
            "entity_id":row["run_id"].as_str()
                .or_else(|| row["task_id"].as_str())
                .or_else(|| row["worktree_id"].as_str()),
            "project_id":row["project_id"], "project":row["project"],
            "branch":row["branch"], "task_id":row["task_id"],
            "title":row["title"], "state":row["state"], "working":row["working"],
        })).collect::<Vec<_>>()})
}

fn deferred_sentinel(fixture: &Fixture) -> Arc<AtomicUsize> {
    let ran = Arc::new(AtomicUsize::new(0));
    let observe = Arc::clone(&ran);
    fixture.state.lock().unwrap().deferred_work =
        Some(crate::app::DeferredWork::External(Box::new(move || {
            observe.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"unrelated_deferred_work":true}))
        })));
    ran
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_router_snapshot_lists_reply_with_both_writer_turns_held() {
    let fixture = Fixture::new();
    let (capture, agent) = router(&fixture);
    let expected = {
        let mut app = fixture.state.lock().unwrap();
        // Exercise the old wire adapters before installing read sentinels.
        let projects = app
            .router_action(&capture, BridgeAction::ListProjects)
            .unwrap();
        let work = app.router_action(&capture, BridgeAction::ListWork).unwrap();
        app.publish_all_snapshot_projection();
        assert_eq!(projects, published(&app, "project.list", None));
        assert_eq!(work, work_digest(&published(&app, "board.list", None)));
        assert!(!work["work"].as_array().unwrap().is_empty());
        assert!(work["work"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["kind"] != "capture"));
        [projects, work]
    };
    let ran = deferred_sentinel(&fixture);
    let (before, queued) = {
        let app = fixture.state.lock().unwrap();
        assert!(
            app.delivery_queue.queued_len() > 0,
            "capture supplies a delivery sentinel"
        );
        (app.read_view.load(), app.delivery_queue.queued_len())
    };
    let held = fixture.held_turns().await;
    for (action, expected) in [BridgeAction::ListProjects, BridgeAction::ListWork]
        .into_iter()
        .zip(expected)
    {
        let answer = fixture.reply(router_frame(&agent, action.clone())).await;
        assert_eq!(
            answer,
            json!({"ok":true,"result":expected}),
            "{}",
            action.tool_name()
        );
        let app = fixture.state.lock().unwrap();
        assert!(
            Arc::ptr_eq(&before, &app.read_view.load()),
            "read ran maintenance/publication"
        );
        assert!(
            app.deferred_work.is_some(),
            "read consumed unrelated deferred work"
        );
        assert_eq!(
            app.delivery_queue.queued_len(),
            queued,
            "read drained delivery"
        );
        assert_eq!(ran.load(Ordering::SeqCst), 0);
    }
    assert_eq!(held.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_workspace_snapshot_list_keeps_caller_scope_and_ignores_foreign_extras() {
    let fixture = Fixture::new();
    let (_other_home, other_repo) = init_repo();
    let (foreign, private_workspace, expected) = {
        let mut app = fixture.state.lock().unwrap();
        let foreign = super::super::project_agent::added_project(&mut app, &other_repo);
        let workspace =
            super::super::project_agent::workspace(&mut app, &foreign, "foreign-private-list");
        let legacy = app.project_agent_workspaces(&fixture.owner).unwrap();
        app.publish_all_snapshot_projection();
        assert_eq!(
            legacy,
            published(&app, "workspace.list", Some(&fixture.project))
        );
        assert!(legacy["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["workspace_id"] == fixture.workspace));
        (foreign, workspace, legacy)
    };
    let ran = deferred_sentinel(&fixture);
    let before = fixture.state.lock().unwrap().read_view.load();
    let held = fixture.held_turns().await;
    let mut frame = fixture.frame(BridgeAction::ListWorkspaces);
    frame["request"]["project_id"] = json!(foreign);
    frame["request"]["workspace_id"] = json!(private_workspace);
    let answer = fixture.reply(frame).await;
    assert_eq!(answer, json!({"ok":true,"result":expected}));
    assert!(!answer.to_string().contains("foreign-private-list"));
    let app = fixture.state.lock().unwrap();
    assert!(Arc::ptr_eq(&before, &app.read_view.load()));
    assert!(app.deferred_work.is_some());
    assert_eq!(ran.load(Ordering::SeqCst), 0);
    assert_eq!(held.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_snapshot_lists_refuse_unknown_tokens_and_unbound_scope_without_writer_turns() {
    let fixture = Fixture::new();
    let (_capture, router_agent) = router(&fixture);
    let held = fixture.held_turns().await;
    for (agent, action) in [
        (&router_agent, BridgeAction::ListProjects),
        (&router_agent, BridgeAction::ListWork),
        (&fixture.agent, BridgeAction::ListWorkspaces),
    ] {
        let mut frame = router_frame(agent, action);
        frame["session_token"] = json!("unknown-snapshot-token");
        assert_eq!(
            fixture.reply(frame).await,
            json!({"ok":false,"error":"unauthorized MCP session"})
        );
    }
    {
        let mut app = fixture.state.lock().unwrap();
        app.projects.unbind_entity(&fixture.owner);
        app.publish_read_metadata();
    }
    let mut frame = fixture.frame(BridgeAction::ListWorkspaces);
    frame["request"]["project_id"] = json!(fixture.project);
    assert_eq!(
        fixture.reply(frame).await,
        json!({"ok":false,"error":format!("{} belongs to no project", fixture.owner)})
    );
    assert_eq!(held.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_snapshot_lists_follow_completed_producers_without_read_maintenance() {
    let fixture = Fixture::new();
    let (_capture, router_agent) = router(&fixture);
    let held = fixture.held_turns().await;
    let before_projects = fixture
        .reply(router_frame(&router_agent, BridgeAction::ListProjects))
        .await;
    let before_work = fixture
        .reply(router_frame(&router_agent, BridgeAction::ListWork))
        .await;
    let before_workspaces = fixture
        .reply(fixture.frame(BridgeAction::ListWorkspaces))
        .await;
    let (_other_home, other_repo) = init_repo();
    let (project, task, workspace, expected) = {
        let mut app = fixture.state.lock().unwrap();
        // Real producer lifecycles, not injected ReadView/chunk placeholders.
        let project = super::super::project_agent::added_project(&mut app, &other_repo);
        let workspace = super::super::project_agent::workspace(
            &mut app,
            &fixture.project,
            "new-published-workspace",
        );
        let task = super::super::tracker::filed(&mut app, &fixture.project, "new-published-task")
            ["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let expected = [
            published(&app, "project.list", None),
            work_digest(&published(&app, "board.list", None)),
            published(&app, "workspace.list", Some(&fixture.project)),
        ];
        (project, task, workspace, expected)
    };
    assert!(!before_projects["result"]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["project_id"] == project));
    assert!(!before_work["result"]["work"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["task_id"] == task));
    assert!(!before_workspaces["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["workspace_id"] == workspace));
    assert!(expected[0]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["project_id"] == project));
    assert!(expected[1]["work"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["task_id"] == task));
    assert!(expected[2]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["workspace_id"] == workspace));
    let before = fixture.state.lock().unwrap().read_view.load();
    for (frame, expected) in [
        router_frame(&router_agent, BridgeAction::ListProjects),
        router_frame(&router_agent, BridgeAction::ListWork),
        fixture.frame(BridgeAction::ListWorkspaces),
    ]
    .into_iter()
    .zip(expected)
    {
        assert_eq!(
            fixture.reply(frame).await,
            json!({"ok":true,"result":expected})
        );
        assert!(Arc::ptr_eq(
            &before,
            &fixture.state.lock().unwrap().read_view.load()
        ));
    }
    assert_eq!(held.len(), 2);
}
