use super::tracker::{filed, tracked};
use super::tracker_tools::{call, coding_agent};
use super::{req, AppState};
use crate::mcp::{BridgeAction, DoneServer};
use serde_json::{json, Value};

fn tool(name: &str, arguments: Value) -> BridgeAction {
    let server = DoneServer::new("test");
    let handled = server.handle_message(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}}).to_string());
    handled
        .action
        .unwrap_or_else(|| panic!("{name} refused: {:?}", handled.reply))
}
fn rpc(app: &mut AppState, name: &str, params: Value) -> Value {
    let result = app.handle(req(name, params));
    assert_eq!(result["ok"], true, "{result}");
    result["result"].clone()
}

#[test]
fn waiting_on_rpc_mcp_contract_round_trips_and_keeps_tasks_link_retired() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut app, project) = tracked(tmp.path());
    let blocker = filed(&mut app, &project, "prerequisite");
    let before_runs = app.runs.len();
    let before_workspaces = app.workspaces.list(Some(&project)).len();
    let task = rpc(
        &mut app,
        "tasks.create",
        json!({"project_id":project,"title":"waiting","waiting_on":[blocker["id"]],"assignee":{"kind":"new_project_agent","agent_name":"Held worker"}}),
    );
    assert_eq!(task["task"]["dependencies"]["blocked"], true);
    assert_eq!(
        task["task"]["dependencies"]["waiting_on"][0]["task_id"],
        blocker["id"]
    );
    assert_eq!(task["task"]["deferred_assignment"]["state"], "waiting");
    assert_eq!(task["dispatch"], Value::Null);
    assert_eq!(app.runs.len(), before_runs);
    assert_eq!(app.workspaces.list(Some(&project)).len(), before_workspaces);
    let reverse = rpc(
        &mut app,
        "tasks.waiting_on.blocks",
        json!({"task_id":blocker["id"],"limit":1}),
    );
    assert_eq!(reverse["blocks"][0]["task_id"], task["task"]["id"]);
    let who = coding_agent(&mut app, &project, "reader");
    let reverse = call(
        &mut app,
        &who,
        tool(
            "list_task_dependents",
            json!({"task_id":blocker["id"],"limit":1}),
        ),
    )
    .unwrap();
    assert_eq!(reverse["blocks"][0]["task_id"], task["task"]["id"]);
    let other = filed(&mut app, &project, "other prerequisite");
    call(
        &mut app,
        &who,
        tool(
            "link_task",
            json!({"task_id":task["task"]["id"],"waiting_on":[other["id"]]}),
        ),
    )
    .unwrap();
    let removed = call(
        &mut app,
        &who,
        tool(
            "unlink_task",
            json!({"task_id":task["task"]["id"],"waiting_on":[other["id"]]}),
        ),
    )
    .unwrap();
    assert_eq!(
        removed["task"]["dependencies"]["waiting_on"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let retired = app.handle(req(
        "tasks.link",
        json!({"task_id":task["task"]["id"],"branch":"test"}),
    ));
    assert_eq!(retired["error_code"], "unknown_method");
}

#[test]
fn waiting_on_rpc_mcp_contract_refusals_are_atomic_and_scoped() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut app, project) = tracked(tmp.path());
    let task = filed(&mut app, &project, "dependent");
    let blocker = filed(&mut app, &project, "blocker");
    let who = coding_agent(&mut app, &project, "writer");
    let cycle = call(
        &mut app,
        &who,
        tool(
            "link_task",
            json!({"task_id":task["id"],"branch":"partial","waiting_on":[blocker["id"],task["id"]]}),
        ),
    );
    assert!(cycle.is_err(), "{cycle:?}");
    let reread = rpc(&mut app, "tasks.get", json!({"task_id":task["id"]}));
    assert_eq!(reread["task"]["links"]["branches"], json!([]));
    assert_eq!(reread["task"]["dependencies"]["waiting_on"], json!([]));
    let refused = app.handle(req("tasks.create", json!({"project_id":project,"title":"must not exist","waiting_on":[blocker["id"],"task-missing"],"assignee":{"kind":"new_project_agent","agent_name":"must not start"}})));
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(
        rpc(&mut app, "tasks.list", json!({"project_id":project}))["tasks"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    for forged in ["actor", "blocked", "generation", "unblock", "operation"] {
        let mut params = json!({"task_id":task["id"],"waiting_on":[blocker["id"]]});
        params[forged] = json!(true);
        assert_eq!(
            app.handle(req("tasks.waiting_on.add", params))["error_code"],
            "invalid_params"
        );
    }
    let removed = rpc(
        &mut app,
        "tasks.waiting_on.remove",
        json!({"task_id":task["id"],"waiting_on":[blocker["id"]]}),
    );
    assert_eq!(removed["task"]["dependencies"]["waiting_on"], json!([]));
}

#[test]
fn waiting_on_rpc_mcp_contract_foreign_endpoints_and_cursors_are_fenced() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut app, project) = tracked(tmp.path());
    let (_foreign_home, foreign_repo) = super::init_repo();
    let foreign_project = super::project_agent::added_project(&mut app, &foreign_repo);
    let foreign = filed(&mut app, &foreign_project, "secret foreign title");
    let a = filed(&mut app, &project, "first");
    let b = filed(&mut app, &project, "second");
    let blocker = filed(&mut app, &project, "blocker");
    let who = coding_agent(&mut app, &project, "scoped writer");
    for name in ["tasks.waiting_on.add", "tasks.waiting_on.remove"] {
        let response = app.handle(req(
            name,
            json!({"task_id":a["id"],"waiting_on":[blocker["id"],foreign["id"]]}),
        ));
        assert_eq!(response["ok"], false, "{response}");
        assert!(!response.to_string().contains("secret foreign title"));
    }
    for (name, args) in [
        (
            "link_task",
            json!({"task_id":a["id"],"branch":"must not persist","waiting_on":[foreign["id"]]}),
        ),
        (
            "unlink_task",
            json!({"task_id":a["id"],"waiting_on":[foreign["id"]]}),
        ),
        ("list_task_dependents", json!({"task_id":foreign["id"]})),
        (
            "create_task",
            json!({"title":"must not exist","waiting_on":[foreign["id"]]}),
        ),
    ] {
        let error = call(&mut app, &who, tool(name, args)).unwrap_err();
        assert!(error.contains("unknown task_id"), "{error}");
        assert!(!error.contains("secret foreign title"));
    }
    for task in [&a, &b] {
        rpc(
            &mut app,
            "tasks.waiting_on.add",
            json!({"task_id":task["id"],"waiting_on":[blocker["id"],blocker["id"]]}),
        );
    }
    let first = rpc(
        &mut app,
        "tasks.waiting_on.blocks",
        json!({"task_id":blocker["id"],"limit":1}),
    );
    assert_eq!(first["blocks"].as_array().unwrap().len(), 1);
    let cursor = first["next_cursor"].as_str().unwrap();
    let second = rpc(
        &mut app,
        "tasks.waiting_on.blocks",
        json!({"task_id":blocker["id"],"limit":1,"cursor":cursor}),
    );
    assert_eq!(second["blocks"].as_array().unwrap().len(), 1);
    assert_ne!(
        first["blocks"][0]["task_id"],
        second["blocks"][0]["task_id"]
    );
    assert_eq!(
        app.handle(req(
            "tasks.waiting_on.blocks",
            json!({"task_id":foreign["id"],"cursor":cursor})
        ))["ok"],
        false
    );
    for limit in [0, 101] {
        assert_eq!(
            app.handle(req(
                "tasks.waiting_on.blocks",
                json!({"task_id":blocker["id"],"limit":limit})
            ))["error_code"],
            "invalid_params"
        );
    }
    let too_many = (0..21).map(|n| format!("task-{n}")).collect::<Vec<_>>();
    assert_eq!(
        app.handle(req(
            "tasks.waiting_on.add",
            json!({"task_id":a["id"],"waiting_on":too_many})
        ))["error_code"],
        "invalid_params"
    );
    assert_eq!(
        rpc(&mut app, "tasks.get", json!({"task_id":a["id"]}))["task"]["dependencies"]
            ["waiting_on"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn waiting_on_rpc_mcp_contract_retry_preserves_saved_intent_and_refuses_replacement() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut app, project) = tracked(tmp.path());
    let blocker = filed(&mut app, &project, "blocker");
    let created = rpc(
        &mut app,
        "tasks.create",
        json!({"project_id":project,"title":"dependent","waiting_on":[blocker["id"]],"assignee":{"kind":"new_project_agent","agent_name":"Saved target"},"note":"saved note"}),
    );
    let id = created["task"]["id"].as_str().unwrap();
    let intent_id = created["task"]["deferred_assignment"]["id"]
        .as_str()
        .unwrap();
    let saved = app
        .tracker_store()
        .unwrap()
        .load_delivery_intent(intent_id)
        .unwrap()
        .unwrap();
    app.tracker_store()
        .unwrap()
        .fail_delivery_intent(intent_id, saved.generation, "injected failure")
        .unwrap();
    for extra in [
        json!({"assignee":null}),
        json!({"note":"forged"}),
        json!({"note":null}),
        json!({"operation_id":null}),
        json!({"operation_id":"replacement"}),
    ] {
        let mut args = json!({"task_id":id,"retry_deferred_id":intent_id});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(
            app.handle(req("tasks.assign", args.clone()))["error_code"],
            "invalid_params"
        );
        assert!(DoneServer::new("test").handle_message(&json!({"id":1,"method":"tools/call","params":{"name":"assign_task","arguments":args}}).to_string()).action.is_none());
    }
    let retried = rpc(
        &mut app,
        "tasks.assign",
        json!({"task_id":id,"retry_deferred_id":intent_id}),
    );
    assert_eq!(retried["task"]["deferred_assignment"]["id"], intent_id);
    assert_eq!(retried["task"]["deferred_assignment"]["state"], "waiting");
    let after = app
        .tracker_store()
        .unwrap()
        .load_delivery_intent(intent_id)
        .unwrap()
        .unwrap();
    assert_eq!(after.target, saved.target);
    assert_eq!(after.note, saved.note);
    assert_eq!(after.actor, saved.actor);
    assert_eq!(after.sender, saved.sender);
    assert_eq!(after.generation, saved.generation);
    assert_eq!(after.operation_id, saved.operation_id);
    assert_eq!(
        app.handle(req(
            "tasks.assign",
            json!({"task_id":id,"retry_deferred_id":intent_id})
        ))["ok"],
        false
    );
    rpc(
        &mut app,
        "tasks.assign",
        json!({"task_id":id,"assignee":null}),
    );
    assert_eq!(
        app.handle(req(
            "tasks.assign",
            json!({"task_id":id,"retry_deferred_id":intent_id})
        ))["ok"],
        false
    );
}

#[test]
fn waiting_on_rpc_mcp_contract_refusals_name_scoped_blockers_and_stale_retry_conflicts() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut app, project) = tracked(tmp.path());
    let blocker = filed(&mut app, &project, "blocker");
    let task = rpc(&mut app, "tasks.create", json!({"project_id":project,"title":"held","waiting_on":[blocker["id"]],"assignee":{"kind":"new_project_agent","agent_name":"held worker"}}));
    let id = &task["task"]["id"];
    let blocked = app.handle(req("tasks.update",json!({"task_id":id,"status":"in_progress"})));
    assert_eq!(blocked["error_code"], "conflict", "{blocked}");
    assert_eq!(blocked["details"]["task_id"], *id, "{blocked}");
    assert_eq!(blocked["details"]["waiting_on"][0]["task_id"],blocker["id"], "{blocked}");
    let cycle = app.handle(req("tasks.waiting_on.add",json!({"task_id":blocker["id"],"waiting_on":[id]})));
    assert_eq!(cycle["error_code"],"invalid_params","{cycle}");
    assert_eq!(cycle["details"]["task_id"],blocker["id"],"{cycle}");
    assert_eq!(cycle["details"]["waiting_on"],json!([id]),"{cycle}");
    let stale = app.handle(req("tasks.assign",json!({"task_id":id,"retry_deferred_id":task["task"]["deferred_assignment"]["id"]})));
    assert_eq!(stale["error_code"],"conflict","{stale}");
}
