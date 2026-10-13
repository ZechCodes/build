use super::tracker::{filed, tracked};
use super::tracker_tools::{call, coding_agent};
use super::{req, AppState};
use crate::mcp::{BridgeAction, DoneServer};
use serde_json::{json, Value};

fn tool(name: &str, arguments: Value) -> BridgeAction {
    let server = DoneServer::new("test");
    let handled = server.handle_message(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}}).to_string());
    handled.action.unwrap_or_else(|| panic!("{name} refused: {:?}", handled.reply))
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
    let task = rpc(&mut app, "tasks.create", json!({"project_id":project,"title":"waiting","waiting_on":[blocker["id"]],"assignee":{"kind":"new_project_agent","agent_name":"Held worker"}}));
    assert_eq!(task["task"]["dependencies"]["blocked"], true);
    assert_eq!(task["task"]["dependencies"]["waiting_on"][0]["task_id"], blocker["id"]);
    assert_eq!(task["task"]["deferred_assignment"]["state"], "waiting");
    assert_eq!(task["dispatch"], Value::Null);
    let reverse = rpc(&mut app, "tasks.waiting_on.blocks", json!({"task_id":blocker["id"],"limit":1}));
    assert_eq!(reverse["blocks"][0]["task_id"], task["task"]["id"]);
    let who = coding_agent(&mut app, &project, "reader");
    let reverse = call(&mut app, &who, tool("list_task_dependents", json!({"task_id":blocker["id"],"limit":1}))).unwrap();
    assert_eq!(reverse["blocks"][0]["task_id"], task["task"]["id"]);
    let other = filed(&mut app, &project, "other prerequisite");
    call(&mut app, &who, tool("link_task", json!({"task_id":task["task"]["id"],"waiting_on":[other["id"]]}))).unwrap();
    let removed = call(&mut app, &who, tool("unlink_task", json!({"task_id":task["task"]["id"],"waiting_on":[other["id"]]}))).unwrap();
    assert_eq!(removed["task"]["dependencies"]["waiting_on"].as_array().unwrap().len(), 1);
    let retired = app.handle(req("tasks.link", json!({"task_id":task["task"]["id"],"branch":"test"})));
    assert_eq!(retired["error_code"], "unknown_method");
}

#[test]
fn waiting_on_rpc_mcp_contract_refusals_are_atomic_and_scoped() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut app, project) = tracked(tmp.path());
    let task = filed(&mut app, &project, "dependent");
    let blocker = filed(&mut app, &project, "blocker");
    let who = coding_agent(&mut app, &project, "writer");
    let cycle = call(&mut app, &who, tool("link_task", json!({"task_id":task["id"],"branch":"partial","waiting_on":[blocker["id"],task["id"]]})));
    assert!(cycle.is_err(), "{cycle:?}");
    let reread = rpc(&mut app, "tasks.get", json!({"task_id":task["id"]}));
    assert_eq!(reread["task"]["links"]["branches"], json!([]));
    assert_eq!(reread["task"]["dependencies"]["waiting_on"], json!([]));
    let refused = app.handle(req("tasks.create", json!({"project_id":project,"title":"must not exist","waiting_on":[blocker["id"],"task-missing"],"assignee":{"kind":"new_project_agent","agent_name":"must not start"}})));
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(rpc(&mut app, "tasks.list", json!({"project_id":project}))["tasks"].as_array().unwrap().len(), 2);
    for forged in ["actor", "blocked", "generation", "unblock", "operation"] {
        let mut params = json!({"task_id":task["id"],"waiting_on":[blocker["id"]]});
        params[forged] = json!(true);
        assert_eq!(app.handle(req("tasks.waiting_on.add", params))["error_code"], "invalid_params");
    }
    let removed = rpc(&mut app, "tasks.waiting_on.remove", json!({"task_id":task["id"],"waiting_on":[blocker["id"]]}));
    assert_eq!(removed["task"]["dependencies"]["waiting_on"], json!([]));
}
