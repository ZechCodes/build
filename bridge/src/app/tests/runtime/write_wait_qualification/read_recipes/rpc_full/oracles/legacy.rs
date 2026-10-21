use super::*;

pub(super) fn check(method: &str, value: &Value, e: &Witnesses) -> bool {
    let i = &e.inputs;
    match method {
        "archived.list" => {
            let archived = row(value, "/items", "run_id", &i.archived_run);
            assert_eq!(archived["state"], e.archived_state);
        }
        "task.get" => {
            assert_eq!(value["task_id"], i.ordinary_plan);
            assert_eq!(value["plan_id"], i.ordinary_plan);
            assert!(value["thread"].is_object());
            assert!(value["agents"].is_array());
        }
        "term.list" => {
            let terminal = row(value, "/terminals", "term_id", &e.shell_id);
            assert_eq!(terminal["cols"], 80);
            assert_eq!(terminal["rows"], 24);
        }
        _ => return false,
    }
    true
}
