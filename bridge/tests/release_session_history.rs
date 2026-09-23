use build_bridge::{
    app::AppState,
    carrier::{FrameHandler, SessionSender},
    harness::HarnessContext,
    transport::Frame,
};
use serde_json::{json, Value};
use std::{path::Path, process::Command};

fn call(handler: &FrameHandler, method: &str, params: Value) -> Value {
    handler.call(
        SessionSender::detached("release-history"),
        Frame {
            session_id: "release-history".into(),
            message_id: "1".into(),
            frame_type: "rpc".into(),
            sender: "client".into(),
            created_at: "2026-09-23T00:00:00Z".into(),
            payload: json!({"id":"1", "method":method, "params":params}),
        },
    )
}

fn accepted(handler: &FrameHandler, method: &str, params: Value) -> Value {
    let response = call(handler, method, params);
    assert_eq!(response["ok"], true, "{method}: {response}");
    response["result"].clone()
}

fn app(root: &Path) -> AppState {
    AppState::new_unrooted_configured(
        root.join("wt"),
        "main",
        true,
        HarnessContext::resolved(root.join("mcp.sock"), root.to_path_buf()).unwrap(),
    )
    .with_config(root.join("config.json"))
    .unwrap()
    .with_task_store(root.join("store"))
    .unwrap()
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn frame_handler_refuses_to_release_a_workspace_after_agent_removal() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.email", "probe@example.invalid"]);
    git(&repo, &["config", "user.name", "Probe"]);
    std::fs::write(repo.join("README"), "probe").unwrap();
    git(&repo, &["add", "README"]);
    git(&repo, &["commit", "-m", "init"]);

    let handler = app(root.path()).into_handler();
    let project_id = accepted(&handler, "project.add", json!({"path":repo}))["project_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let workspace_id = accepted(
        &handler,
        "workspace.create",
        json!({
            "project_id":project_id, "name":"probe", "isolation":"worktree"
        }),
    )["workspace_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run_id = accepted(
        &handler,
        "workspace.ensure_conversation",
        json!({
            "workspace_id":workspace_id
        }),
    )["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let agent_id = accepted(&handler, "agent.add", json!({"entity_id":run_id}))["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    drop(handler);

    let store = build_bridge::store::Store::new(root.path().join("store")).unwrap();
    let mut run = store
        .load_all_runs()
        .unwrap()
        .into_iter()
        .find(|run| run.id == run_id)
        .unwrap();
    run.agents[0]
        .thread
        .post_user("anchor", None, "2026-09-01T00:00:00Z");
    store.save_run(&run).unwrap();
    drop(store);

    let handler = app(root.path()).into_handler();
    let before = accepted(&handler, "project.list", json!({}));
    accepted(
        &handler,
        "agent.remove",
        json!({"entity_id":run_id, "agent_id":agent_id}),
    );
    let release = call(&handler, "run.release", json!({"run_id":run_id}));
    assert_eq!(release["ok"], false, "{release}");
    assert!(release["error"]
        .as_str()
        .unwrap()
        .contains("delete the workspace"));
    let still_live = accepted(&handler, "workspace.list", json!({"project_id":project_id}));
    let live_project = accepted(&handler, "project.list", json!({}));
    assert_eq!(
        still_live["workspaces"][0]["session_started_ms"],
        before["projects"][0]["session_started_ms"]
    );
    assert_eq!(
        live_project["projects"][0]["session_started_ms"],
        before["projects"][0]["session_started_ms"]
    );
    drop(handler);

    let handler = app(root.path()).into_handler();
    let after = accepted(&handler, "project.list", json!({}));
    let workspaces = accepted(&handler, "workspace.list", json!({"project_id":project_id}));
    assert_eq!(
        before["projects"][0]["session_started_ms"],
        after["projects"][0]["session_started_ms"]
    );
    assert_eq!(
        before["projects"][0]["last_activity_ms"],
        after["projects"][0]["last_activity_ms"]
    );
    assert_eq!(workspaces["workspaces"][0]["workspace_id"], workspace_id);
    assert_eq!(
        workspaces["workspaces"][0]["session_started_ms"],
        before["projects"][0]["session_started_ms"]
    );
}
