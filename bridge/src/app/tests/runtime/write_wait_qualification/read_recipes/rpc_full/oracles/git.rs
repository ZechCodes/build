use super::*;

pub(super) fn check(method: &str, value: &Value, e: &Witnesses) -> bool {
    let i = &e.inputs;
    match method {
        "git.log" => {
            let commit = row(value, "/commits", "hash", &i.commit);
            assert_eq!(commit["subject"], e.commit_subject);
            assert!(array(value, "/commits").len() <= 20);
        }
        "git.show" => {
            assert_eq!(value["hash"], i.commit);
            assert_eq!(value["subject"], e.commit_subject);
            assert!(text(value, "/patch").contains(&e.committed_file));
            assert_eq!(value["range"]["offset"], 0);
        }
        "git.status" => {
            assert!(file(value, &i.diff_file));
            nonempty(value, "/status_key");
            assert_ne!(value["unchanged"], true);
        }
        "git.diff" => {
            let entry = row(value, "/files", "path", &i.diff_file);
            assert!(text(entry, "/patch").contains(&e.diff_needle));
            assert_eq!(entry["range"]["offset"], 0);
        }
        "git.changeset_diff" => {
            assert!(file(value, &i.diff_file));
            assert!(text(value, "/patch").contains(&e.diff_needle));
            nonempty(value, "/diff_key");
        }
        "git.refs" => {
            row(value, "/refs", "full_ref", &e.branch_ref);
            row(value, "/refs", "full_ref", &e.upstream_ref);
        }
        "git.unpushed" => {
            assert!(file(value, &e.committed_file));
            nonempty(value, "/diff_key");
            assert!(value.get("patch").is_none_or(Value::is_null));
            assert!(value["base"].is_object());
        }
        "worktree.diff" => {
            assert_eq!(value["worktree_id"], i.external_worktree);
            assert!(file(value, &e.committed_file) || file(value, &i.diff_file));
            nonempty(value, "/diff_key");
        }
        "run.diff" => {
            assert!(file(value, &e.committed_file) || file(value, &i.diff_file));
            nonempty(value, "/diff_key");
        }
        "task.stage_diff" => {
            assert_eq!(value["task_id"], i.staged_plan);
            assert_eq!(value["run_id"], i.stage_run);
            assert_eq!(value["stage_id"], i.stage);
            assert!(file(value, &e.committed_file));
            assert_eq!(value["status"], "available");
            assert_eq!(value["start_sha"], e.stage_start);
            assert_eq!(value["completion_sha"], e.stage_end);
        }
        _ => return false,
    }
    true
}
