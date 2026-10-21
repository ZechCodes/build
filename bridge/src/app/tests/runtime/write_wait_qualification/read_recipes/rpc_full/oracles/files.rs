use super::*;

pub(super) fn check(method: &str, value: &Value, e: &Witnesses) -> bool {
    let i = &e.inputs;
    match method {
        "fs.list" => {
            assert_eq!(value["path"], i.repository.to_str().unwrap());
            row(value, "/entries", "name", "qualification-child");
        }
        "fs.tree" => {
            row(value, "/entries", "name", &i.stable_file);
            row(value, "/entries", "name", "qualification-child");
        }
        "fs.read" => {
            assert_eq!(value["path"], i.stable_file);
            bytes(value, &e.file_bytes);
            assert_eq!(value["size"], e.file_bytes.len());
            assert_eq!(value["range"]["offset"], 0);
        }
        "github.repos" => {
            let repo = row(
                value,
                "/repos",
                "name_with_owner",
                "qualification/read-witness",
            );
            assert_eq!(repo["url"], "https://github.com/qualification/read-witness");
            assert_eq!(repo["private"], true);
        }
        "task.doc" => assert_eq!(value["contents"], e.ordinary_doc),
        "task.stage_doc" => {
            assert_eq!(value["task_id"], i.staged_plan);
            assert_eq!(value["stage_id"], i.stage);
            assert_eq!(value["contents"], e.staged_doc);
        }
        _ => return false,
    }
    true
}
