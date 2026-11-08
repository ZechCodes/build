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
    assert!(spawns.lock().unwrap().is_empty());
}

#[test]
fn restore_restores_everything() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let task = filed(&mut app, &project, "Retained task");
    let (owner, pin) = project_agent(&mut app, &project);
    let archived = call(&mut app, "project.archive", json!({"project_id":project}));
    assert!(archived["project"]["archived_at"].is_number(), "{archived}");
    let listed = call(&mut app, "project.list", json!({}));
    assert_eq!(
        listed["projects"][0]["archived_at"],
        archived["project"]["archived_at"]
    );
    assert_eq!(app.project_pin(&owner), Some(pin.as_str()));
    call(&mut app, "tasks.get", json!({"task_id":task["id"]}));
    let restored = call(&mut app, "project.restore", json!({"project_id":project}));
    assert!(restored["project"]["archived_at"].is_null());
    call(
        &mut app,
        "tasks.comment",
        json!({"task_id":task["id"],"body":"Restored"}),
    );
    let workspace = workspace(&mut app, &project, "Restored checkout");
    call(
        &mut app,
        "workspace.rename",
        json!({"workspace_id":workspace,"name":"Renamed"}),
    );
    assert!(app
        .agent_action(&owner, &pin, BridgeAction::ListProjectAgents)
        .is_ok());
    assert_eq!(app.project_pin(&owner), Some(pin.as_str()));
}
