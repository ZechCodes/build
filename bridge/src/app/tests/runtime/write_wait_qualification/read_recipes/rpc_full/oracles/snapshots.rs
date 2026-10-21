use super::*;

pub(super) fn check(method: &str, value: &Value, e: &Witnesses) -> bool {
    let i = &e.inputs;
    match method {
        "agent.list" => {
            assert_eq!(value["entity_id"], i.owner);
            let agent = row(value, "/agents", "id", &i.agent);
            assert_eq!(agent["conversation_id"], i.agent);
        }
        "board.list" => {
            row(value, "/items", "task_id", &i.task);
            assert!(value["runs"].is_array());
            assert!(value["external_worktrees"].is_array());
        }
        "project.list" => {
            for id in &e.all_projects {
                row(value, "/projects", "project_id", id);
            }
        }
        "settings.get" => {
            assert_eq!(value["default_harness"], e.default_harness);
            assert!(value["agent_modes"].is_object());
        }
        "workspace.list" => {
            row(value, "/workspaces", "workspace_id", &i.workspace);
            assert!(array(value, "/workspaces")
                .iter()
                .all(|v| v["project_id"] == i.project));
        }
        "workspace.get" => {
            assert_eq!(value["workspace_id"], i.workspace);
            assert_eq!(value["project_id"], i.project);
            let directory = row(value, "/directories", "source_id", &i.source);
            assert_eq!(directory["path"], i.repository.to_str().unwrap());
            row(value, "/agents", "id", &i.agent);
        }
        "capture.get" => {
            assert_eq!(value["id"], i.capture);
            assert_eq!(value["text"], e.capture_text);
            assert!(value.get("state").is_some());
        }
        "tasks.columns" => {
            assert_eq!(value["project_id"], i.project);
            assert_eq!(value["columns"], e.expected_columns);
        }
        _ => return false,
    }
    true
}
