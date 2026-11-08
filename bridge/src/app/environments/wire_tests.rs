use crate::app::{AppState, Frame};
use serde_json::{json, Value};

fn state(root: &std::path::Path) -> AppState {
    let context = crate::harness::HarnessContext::resolved(root.join("mcp.sock"), root.join("state")).unwrap();
    AppState::new_unrooted_configured(root.join("worktrees"), "main", true, context)
}

fn rpc(app: &mut AppState, method: &str, params: Value) -> Value {
    app.handle(Frame {
        session_id: "test".into(), message_id: "test".into(), frame_type: "data".into(),
        sender: "client".into(), created_at: "test".into(),
        payload: json!({"method":method,"id":"1","params":params}),
    })
}

#[test]
fn implicit_environment_from_legacy_project_create() {
    let root = tempfile::tempdir().unwrap();
    let mut app = state(root.path());
    let created = rpc(&mut app, "project.create", json!({"name":"legacy","parent":root.path()}));
    assert_eq!(created["ok"], true, "{created}");
    let listed = rpc(&mut app, "environment.list", json!({}));
    assert_eq!(listed["ok"], true, "{listed}");
    let environments = listed["result"]["environments"].as_array().unwrap();
    assert_eq!(environments.len(), 1);
    assert_eq!(environments[0]["project_ids"], json!([created["result"]["project_id"]]));
    assert_eq!(environments[0]["home_path"], created["result"]["path"]);
    assert_eq!(app.projects.environments().count(), 1);
}

#[test]
fn environment_list_rows_match_fixture() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("bridge");
    std::fs::create_dir(&home).unwrap();
    let mut app = state(root.path());
    let project = app.add_project(home.clone(), "main".into());
    let listed = rpc(&mut app, "environment.list", json!({}));
    assert_eq!(listed["ok"], true, "{listed}");
    let fixture: Value = serde_json::from_slice(&std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/api/v1/environment.list.json")).unwrap()).unwrap();
    let row = &listed["result"]["environments"][0];
    let keys = |value: &Value| value.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(keys(row), keys(&fixture["result"]["environments"][0]));
    assert_eq!(row["home_path"], json!(home));
    assert_eq!(row["name"], "bridge");
    assert_eq!(row["project_ids"], json!([project]));
    assert_eq!(row["sources"][0]["path"], row["home_path"]);
    assert_eq!(row["is_git"], false);
    assert_eq!(row["isolation"], Value::Null);
    assert_eq!(row["isolation_effective"], "worktree");
    assert!(row["device"].is_string());
    let sibling = app.add_environment_sibling_for_test(&project);
    let listed = rpc(&mut app, "environment.list", json!({}));
    assert_eq!(listed["result"]["environments"].as_array().unwrap().len(), 1);
    assert_eq!(listed["result"]["environments"][0]["project_ids"], json!([project, sibling]));
}

#[test]
fn board_list_includes_environments() {
    let root = tempfile::tempdir().unwrap();
    let mut app = state(root.path());
    app.add_project(root.path().to_path_buf(), "main".into());
    let board = rpc(&mut app, "board.list", json!({}));
    assert!(board["result"]["environments"].is_array(), "{board}");
    let listed = rpc(&mut app, "environment.list", json!({}));
    assert_eq!(board["result"]["environments"], listed["result"]["environments"]);
}

#[test]
fn project_and_workspace_rows_carry_environment_membership_and_counts() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let mut app = state(root.path()).with_task_store(root.path().join("store")).unwrap();
    let project = app.add_project(home, "main".into());
    let filed = rpc(&mut app, "tasks.create", json!({"project_id":project,"title":"open task"}));
    assert_eq!(filed["ok"], true, "{filed}");
    let workspace = rpc(&mut app, "workspace.create", json!({"project_id":project,"name":"first"}));
    assert_eq!(workspace["ok"], true, "{workspace}");
    let projects = rpc(&mut app, "project.list", json!({}));
    let row = &projects["result"]["projects"][0];
    assert_eq!(row["environment_id"], "env-1");
    assert_eq!(row["environment_name"], "home");
    assert_eq!(row["project_key"], json!(app.project_for(&project).unwrap().project_key));
    assert_eq!(row["diverged"], false);
    assert_eq!(row["open_tasks"], 1);
    assert_eq!(row["total_tasks"], 1);
    assert_eq!(row["workspaces"], 1);
    assert!(row["device"].is_string());
    let params = json!({"workspace_id":workspace["result"]["workspace_id"]});
    let get = rpc(&mut app, "workspace.get", params);
    let list = rpc(&mut app, "workspace.list", json!({}));
    assert_eq!(get["result"]["environment_id"], "env-1");
    assert_eq!(list["result"]["workspaces"][0]["environment_id"], "env-1");
    let closed = rpc(&mut app, "tasks.close", json!({"task_id":filed["result"]["task"]["id"]}));
    assert_eq!(closed["ok"], true, "{closed}");
    let projects = rpc(&mut app, "project.list", json!({}));
    assert_eq!(projects["result"]["projects"][0]["open_tasks"], 0);
    assert_eq!(projects["result"]["projects"][0]["total_tasks"], 1);
}
