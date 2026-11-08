//! Archive is a retained project with one mutation refusal across entry points.
use super::project_agent::{project_agent, workspace};
use super::project_coordinator::add_worker;
use super::tracker::{filed, tracked};
use super::*;
use crate::mcp::BridgeAction;

fn call(app: &mut AppState, method: &str, params: Value) -> Value {
    let reply = app.handle(req(method, params));
    assert_eq!(reply["ok"], true, "{reply}");
    reply["result"].clone()
}

#[test]
fn archive_refused_names_each_blocker() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let (owner, pin) = project_agent(&mut app, &project);
    let worker = add_worker(&mut app, &owner, json!({"name":"Editor"}));
    let workspace = workspace(&mut app, &project, "Unfinished checkout");
    let review = call(
        &mut app,
        "tasks.review.open",
        json!({"workspace_id":workspace,
        "request_id":"archive-review", "title":"Pending verdict", "description":"Review"}),
    );
    let prerequisite = filed(&mut app, &project, "Prerequisite");
    let waiting = call(
        &mut app,
        "tasks.create",
        json!({"project_id":project,
        "title":"Pending intent", "waiting_on":[prerequisite["id"]],
        "assignee":{"kind":"new_project_agent","agent_name":"Waiting worker"}}),
    );
    let attention = call(
        &mut app,
        "tasks.create",
        json!({"project_id":project,
        "title":"Needs you", "assignee":{"kind":"user"}}),
    );
    for agent in [&pin, &worker] {
        call(
            &mut app,
            "thread.post",
            json!({"entity_id":owner,"agent_id":agent,"body":"Queued turn"}),
        );
    }
    let refused = app.handle(req("project.archive", json!({"project_id":project})));
    assert_eq!(refused["error_code"], "conflict", "{refused}");
    let message = refused["error"].as_str().unwrap();
    for blocker in [
        "finish or delete workspace Unfinished checkout".to_string(),
        "stop worker Editor".into(),
        "stop coordinator".into(),
        format!("pending delivery for #{}", waiting["task"]["number"]),
        format!("answer #{}", attention["task"]["number"]),
        format!("give a verdict on #{}", review["task"]["number"]),
    ] {
        assert!(message.contains(&blocker), "missing {blocker}: {message}");
    }
    assert!(app.project(&project).unwrap().archived_at.is_none());
}

#[test]
fn archived_project_refuses_mutations_on_every_path() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let task = filed(&mut app, &project, "Retained task");
    let (owner, pin) = project_agent(&mut app, &project);
    let workspace = workspace(&mut app, &project, "Retained checkout");
    app.projects.at_mut(0).archived_at = Some(1);
    for (method, params) in [
        (
            "tasks.create",
            json!({"project_id":project,"title":"Refused"}),
        ),
        (
            "tasks.update",
            json!({"task_id":task["id"],"title":"Refused"}),
        ),
        (
            "tasks.comment",
            json!({"task_id":task["id"],"body":"Refused"}),
        ),
        (
            "tasks.assign",
            json!({"task_id":task["id"],"assignee":{"kind":"user"}}),
        ),
        (
            "tasks.waiting_on.add",
            json!({"task_id":task["id"],"waiting_on":[]}),
        ),
        (
            "tasks.review.close",
            json!({"task_id":task["id"],"expected_version":1,"description":"Refused"}),
        ),
        (
            "tasks.review.open",
            json!({"workspace_id":workspace,"title":"Refused","description":"Refused","request_id":"refused"}),
        ),
        (
            "project.add_source",
            json!({"project_id":project,"path":home.path()}),
        ),
        (
            "project.sync_source",
            json!({"project_id":project,"source_id":"source-1"}),
        ),
        (
            "workspace.create",
            json!({"project_id":project,"name":"Refused"}),
        ),
        (
            "workspace.rename",
            json!({"workspace_id":workspace,"name":"Refused"}),
        ),
        ("agent.add", json!({"entity_id":owner})),
        (
            "thread.post",
            json!({"entity_id":owner,"agent_id":pin,"body":"Refused"}),
        ),
    ] {
        let refused = app.handle(req(method, params));
        assert_eq!(
            refused["error_code"], "project_archived",
            "{method}: {refused}"
        );
        assert!(refused["error"]
            .as_str()
            .unwrap()
            .ends_with("is archived; restore it first"));
    }
    let refused = app.agent_action(
        &owner,
        &pin,
        BridgeAction::CreateWorkspace {
            name: "Refused".into(),
            isolation: None,
        },
    );
    let refused = crate::app::mcp::mcp_action_response(refused);
    assert_eq!(refused["error_code"], "project_archived", "{refused}");
    assert!(app
        .agent_action(&owner, &pin, BridgeAction::ListProjectAgents)
        .is_ok());
    call(&mut app, "tasks.get", json!({"task_id":task["id"]}));
}

#[tokio::test]
async fn archived_project_agent_not_started() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let (owner, pin) = project_agent(&mut app, &project);
    let worker = add_worker(&mut app, &owner, json!({"name":"Editor"}));
    app.projects.at_mut(0).archived_at = Some(1);
    let root = app.entity_agent_root(&owner).unwrap();
    let state = app.shared();
    let spawns = super::project_agent_base::record_spawns(&state);
    for agent in [&pin, &worker] {
        let result = ensure_agent_tab(
            &state,
            &root,
            &owner,
            agent,
            &ModelChoice::default(),
            "start",
        );
        assert!(result.is_err(), "archived agent started");
        let error = result.err().unwrap();
        assert!(error.contains("is archived; restore it first"), "{error}");
    }
    let response = super::workflow::lifecycle_offlock::frame_on_a_thread(
        &state,
        "archived-start",
        "agent.start",
        json!({"id":owner,"agent_id":pin}),
    )
    .recv_timeout(std::time::Duration::from_secs(5))
    .unwrap();
    assert_eq!(response["error_code"], "project_archived", "{response}");
    let refusal = crate::app::creation::admit(
        &state,
        "workspace.create",
        json!({"project_id":project}),
        crate::api::writes::WriteMetadata {
            operation_id: uuid::Uuid::new_v4().to_string(),
            after: None,
        },
    );
    assert_eq!(refusal.unwrap_err().code(), "project_archived");
    assert!(spawns.lock().unwrap().is_empty());
}

#[test]
fn restore_restores_everything() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let task = filed(&mut app, &project, "Retained task");
    let (owner, pin) = project_agent(&mut app, &project);
    let retained_workspace = workspace(&mut app, &project, "Finished checkout");
    let review = call(
        &mut app,
        "tasks.review.open",
        json!({"workspace_id":retained_workspace,
        "request_id":"retained-review", "title":"Retained review", "description":"Keep this history"}),
    );
    let review_task = review["task"]["id"].clone();
    call(
        &mut app,
        "tasks.review.close",
        json!({"task_id":review_task,"expected_version":1,"description":"Finished review"}),
    );
    call(
        &mut app,
        "tasks.comment",
        json!({"task_id":task["id"],"body":"Retained comment"}),
    );
    let mut finished = app.workspaces.get(&retained_workspace).unwrap().clone();
    app.workspaces.record_finished(&mut finished).unwrap();
    app.workspaces.apply_local_capabilities(finished).unwrap();
    let before = call(&mut app, "tasks.get", json!({"task_id":task["id"]}));
    let sibling = crate::app::projects::test_support::register_sibling(&mut app, &project);
    let archived = call(&mut app, "project.archive", json!({"project_id":project}));
    assert!(archived["project"]["archived_at"].is_number(), "{archived}");
    let listed = call(&mut app, "project.list", json!({}));
    let row = listed["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["project_id"] == project)
        .unwrap();
    assert_eq!(row["archived_at"], archived["project"]["archived_at"]);
    assert_eq!(
        call(&mut app, "tasks.get", json!({"task_id":task["id"]})),
        before
    );
    assert_eq!(
        call(&mut app, "tasks.review.get", json!({"task_id":review_task}))["review"]["task_id"],
        review_task
    );
    assert_eq!(
        call(
            &mut app,
            "workspace.get",
            json!({"workspace_id":retained_workspace})
        )["status"],
        "finished"
    );
    // Environment-reference maintenance follows through an unarchived sibling.
    call(
        &mut app,
        "project.update_source",
        json!({"project_id":sibling,"source_id":"source-1","name":"Shared label"}),
    );
    assert_eq!(
        app.project(&project).unwrap().sources[0].name,
        "Shared label"
    );
    assert!(app.project(&project).unwrap().archived_at.is_some());
    let restored = call(&mut app, "project.restore", json!({"project_id":project}));
    assert!(restored["project"]["archived_at"].is_null());
    call(
        &mut app,
        "tasks.comment",
        json!({"task_id":task["id"],"body":"Restored"}),
    );
    call(
        &mut app,
        "workspace.rename",
        json!({"workspace_id":retained_workspace,"name":"Renamed retained checkout"}),
    );
    app.agent_action(
        &owner,
        &pin,
        BridgeAction::CreateWorkspace {
            name: "Restored MCP checkout".into(),
            isolation: None,
        },
    )
    .unwrap();
    assert_eq!(app.project_pin(&owner), Some(pin.as_str()));
}

#[test]
fn archived_project_refuses_retained_upload() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let caller = SessionSender::detached("archive-upload");
    let begun = app.dispatch_api_with_context("fs.uploadBegin", &json!({
        "project_id":project,"source_id":"source-1","parent":"","name":"archived-upload.txt","size":1
    }), Some(&caller)).unwrap();
    call(&mut app, "project.archive", json!({"project_id":project}));
    for (method, params) in [
        (
            "fs.uploadChunk",
            json!({"upload_id":begun["upload_id"],"offset":0,"content_b64":"eA=="}),
        ),
        ("fs.uploadFinish", json!({"upload_id":begun["upload_id"]})),
        ("fs.uploadAbort", json!({"upload_id":begun["upload_id"]})),
    ] {
        let refused = app.dispatch_api_with_context(method, &params, Some(&caller));
        assert_eq!(refused.unwrap_err().code(), "project_archived", "{method}");
    }
}

#[test]
fn project_archive_configuration_is_atomic_and_survives_restart() {
    let (_repo_home, repo) = init_repo();
    let home = tempfile::tempdir().unwrap();
    let mut app = super::project_coordinator::booted(home.path());
    let project = super::project_agent::added_project(&mut app, &repo);
    let (owner, pin) = project_agent(&mut app, &project);
    let task = filed(&mut app, &project, "Persisted history");
    let config_path = app.config_path.clone();
    let blocked_path = home.path().join("not-a-directory");
    std::fs::write(&blocked_path, "occupied").unwrap();
    app.config_path = Some(blocked_path.join("config.json"));
    assert_eq!(
        app.handle(req("project.archive", json!({"project_id":project})))["ok"],
        false
    );
    assert!(app.project(&project).unwrap().archived_at.is_none());
    app.config_path = config_path;
    let archived = call(&mut app, "project.archive", json!({"project_id":project}));
    drop(app);
    let mut app = super::project_coordinator::booted(home.path());
    assert_eq!(
        call(&mut app, "project.list", json!({}))["projects"][0]["archived_at"],
        archived["project"]["archived_at"]
    );
    assert_eq!(app.project_pin(&owner), Some(pin.as_str()));
    call(&mut app, "tasks.get", json!({"task_id":task["id"]}));
    assert_eq!(
        app.handle(req(
            "tasks.comment",
            json!({"task_id":task["id"],"body":"Refused after restart"})
        ))["error_code"],
        "project_archived"
    );
    call(&mut app, "project.restore", json!({"project_id":project}));
    drop(app);
    let mut app = super::project_coordinator::booted(home.path());
    assert!(app.project(&project).unwrap().archived_at.is_none());
    call(
        &mut app,
        "tasks.comment",
        json!({"task_id":task["id"],"body":"Restored after restart"}),
    );
}

#[test]
fn archived_project_refuses_router_dispatch() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let (capture, _) = super::routing::router::captured(&mut app, "An archived destination");
    call(&mut app, "project.archive", json!({"project_id":project}));
    let refusal = app.router_action(
        &capture,
        BridgeAction::DispatchBranch {
            project_id: project,
            branch: None,
            name: "Router worker".into(),
            instruction: "An archived destination".into(),
            rationale: None,
        },
    );
    let refusal = crate::app::mcp::mcp_action_response(refusal);
    assert_eq!(refusal["error_code"], "project_archived", "{refusal}");
}

mod deliveries;

mod reconciliation;

#[test]
fn archived_snapshot_refresh_preserves_finished_workspace_until_restore() {
    let home = tempfile::tempdir().unwrap();
    let (mut app, project) = super::tracker::tracked_with_origin(home.path());
    let ws = workspace(&mut app, &project, "Retained finished checkout");
    let mut finished = app.workspaces.get(&ws).unwrap().clone();
    app.workspaces.record_finished(&mut finished).unwrap();
    let checkout = finished.directories[0].path.clone();
    app.workspaces.apply_local_capabilities(finished).unwrap();
    call(&mut app, "project.archive", json!({"project_id":project}));
    let before = serde_json::to_value(app.workspaces.get(&ws).unwrap()).unwrap();
    std::fs::write(checkout.join("external.txt"), "external change").unwrap();
    app.refresh_workspace_snapshot_observations();
    assert_eq!(
        serde_json::to_value(app.workspaces.get(&ws).unwrap()).unwrap(),
        before
    );
    call(&mut app, "project.restore", json!({"project_id":project}));
    app.refresh_workspace_snapshot_observations();
    assert_ne!(
        app.workspaces.get(&ws).unwrap().status,
        crate::workspace::WorkspaceStatus::Finished
    );
}
