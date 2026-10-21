use super::*;

pub(super) fn check(method: &str, value: &Value, e: &Witnesses) -> bool {
    let i = &e.inputs;
    match method {
        "task.stages" => {
            assert_eq!(value["task_id"], i.staged_plan);
            let stage = row(value, "/stages", "id", &i.stage);
            assert_eq!(stage["start_sha"], e.stage_start);
            assert_eq!(stage["completion_sha"], e.stage_end);
            assert_eq!(stage["publication"], "local");
        }
        "thread.revision" => {
            assert_eq!(value["revision_id"], i.revision);
            assert_eq!(value["artifact"], "plan");
            assert_eq!(value["contents"], e.revision_contents);
        }
        "stream.events" => {
            let events = array(value, "/events");
            assert_eq!(events.len(), 9);
            for (index, event) in events.iter().enumerate() {
                assert_eq!(event["seq"], index + 1);
                if index < 8 {
                    assert_eq!(event["kind"], "output");
                    assert_eq!(event["data"]["text"], format!("chunk-{index:06}"));
                } else {
                    assert_eq!(event["kind"], "done");
                }
            }
            assert_eq!(value["head"], 9);
            assert_eq!(value["next"], 9);
            assert_eq!(value["complete"], true);
        }
        "stream.state" => {
            let output = (0..8)
                .map(|n| format!("chunk-{n:06}"))
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(value["stream_id"], i.stream);
            assert_eq!(value["count"], 8);
            assert_eq!(value["head"], 9);
            assert_eq!(value["complete"], true);
            assert_eq!(value["output_len"], output.len());
            assert_eq!(
                value["checksum"],
                crate::app::streams::sha256_hex(output.as_bytes())
            );
        }
        "workspace.git_init_options" => {
            assert_eq!(value["workspace_id"], i.workspace);
            assert_eq!(value["source_id"], i.source);
            assert_eq!(value["workspace"]["is_git"], true);
            assert_eq!(value["source"]["is_git"], true);
        }
        _ => return false,
    }
    true
}
