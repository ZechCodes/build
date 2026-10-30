//! Raw JSON compatibility controls for GET's opt-in unavailable-only phase.
use super::tracker::{filed, tracked};
use super::*;

fn fixture() -> (tempfile::TempDir, tempfile::TempDir, AppState, Value) {
    let root = tempfile::tempdir().unwrap();
    let (repo, mut state, project) = tracked(root.path());
    let task = filed(&mut state, &project, "GET snapshot option");
    state.initialize_read_context();
    (root, repo, state, task)
}

async fn routes(state: &mut AppState, params: Value) -> (Value, Value) {
    let sync = state.handle(req("tasks.get", params.clone()));
    let connection = state.tracker_store().unwrap().read_pool().acquire().await;
    let sql = state
        .read_service()
        .call(req("tasks.get", params), Some(connection));
    (sync, sql)
}

fn result_bytes(reply: &Value) -> Vec<u8> {
    assert_eq!(reply["ok"], true, "{reply}");
    serde_json::to_vec(&reply["result"]).unwrap()
}

#[tokio::test]
async fn get_absent_and_false_preserve_legacy_bytes_on_both_routes() {
    let (_root, _repo, mut state, task) = fixture();
    let (sync_absent, sql_absent) = routes(&mut state, json!({"task_id":task["id"]})).await;
    let (sync_false, sql_false) = routes(
        &mut state,
        json!({"task_id":task["id"],"watch_snapshot":false}),
    )
    .await;
    assert_eq!(result_bytes(&sync_absent), result_bytes(&sync_false));
    assert_eq!(result_bytes(&sql_absent), result_bytes(&sql_false));
    for reply in [sync_absent, sync_false, sql_absent, sql_false] {
        assert!(reply["result"].get("watch_snapshot").is_none());
        assert!(reply["result"]["task"].get("watch_facts").is_none());
        assert!(reply["result"]["task"].get("watch_unavailable").is_none());
    }
}

#[tokio::test]
async fn get_true_unready_returns_only_bounded_unavailable_on_both_routes() {
    let (_root, _repo, mut state, task) = fixture();
    let (sync_legacy, sql_legacy) = routes(&mut state, json!({"task_id":task["id"]})).await;
    let (sync, sql) = routes(
        &mut state,
        json!({"task_id":task["id"],"watch_snapshot":true}),
    )
    .await;
    let expected = json!({"watch":"coherence_unavailable","total":"coherence_unavailable", "attention":"coherence_unavailable","activity":"coherence_unavailable"});
    for (reply, legacy) in [(sync, sync_legacy), (sql, sql_legacy)] {
        assert_eq!(reply["ok"], true, "{reply}");
        assert!(reply["result"].get("watch_snapshot").is_none());
        assert!(reply["result"]["task"].get("watch_facts").is_none());
        let unavailable = &reply["result"]["task"]["watch_unavailable"];
        assert_eq!(unavailable, &expected);
        assert!(serde_json::to_vec(unavailable).unwrap().len() <= 256);
        let mut ordinary = reply["result"]["task"].clone();
        ordinary
            .as_object_mut()
            .unwrap()
            .remove("watch_unavailable");
        assert_eq!(ordinary, legacy["result"]["task"]);
        assert_eq!(reply["result"]["timeline"], legacy["result"]["timeline"]);
    }
}

#[tokio::test]
async fn get_rejects_null_and_nonboolean_option_on_both_routes() {
    let (_root, _repo, mut state, task) = fixture();
    for option in [json!(null), json!("yes"), json!(1), json!([]), json!({})] {
        let (sync, sql) = routes(
            &mut state,
            json!({"task_id":task["id"],"watch_snapshot":option}),
        )
        .await;
        for reply in [sync, sql] {
            assert_eq!(reply["ok"], false, "{reply}");
            assert_eq!(reply["error_code"], "invalid_params", "{reply}");
            assert!(
                !reply["error"]
                    .as_str()
                    .unwrap_or("")
                    .contains("unknown param"),
                "declared option must fail type validation: {reply}"
            );
        }
    }
}

#[tokio::test]
async fn get_false_keeps_large_legacy_timeline_on_both_routes() {
    let (_root, _repo, mut state, task) = fixture();
    let body = "history ".repeat(300);
    for _ in 0..34 {
        let reply = state.handle(req(
            "tasks.comment",
            json!({"task_id":task["id"],"body":body}),
        ));
        assert_eq!(reply["ok"], true, "{reply}");
    }
    state.initialize_read_context();
    let (sync_absent, sql_absent) = routes(&mut state, json!({"task_id":task["id"]})).await;
    let (sync_false, sql_false) = routes(
        &mut state,
        json!({"task_id":task["id"],"watch_snapshot":false}),
    )
    .await;
    assert!(result_bytes(&sync_absent).len() > 65_536);
    assert_eq!(result_bytes(&sync_absent), result_bytes(&sync_false));
    assert_eq!(result_bytes(&sql_absent), result_bytes(&sql_false));
}

#[test]
fn get_option_does_not_change_other_task_id_verbs_or_advertise_capability() {
    let (_root, _repo, mut state, task) = fixture();
    assert!(!crate::api::capabilities(false).contains(&"tasks.watchSnapshot.get.v1"));
    for method in ["tasks.watch", "tasks.unwatch", "tasks.reopen"] {
        let reply = state.handle(req(
            method,
            json!({"task_id":task["id"],"watch_snapshot":false}),
        ));
        assert_eq!(reply["ok"], false, "{method}: {reply}");
        assert_eq!(reply["error_code"], "invalid_params", "{method}: {reply}");
        assert_eq!(
            reply["details"]["params"],
            json!(["watch_snapshot"]),
            "{method}: {reply}"
        );
    }
}
