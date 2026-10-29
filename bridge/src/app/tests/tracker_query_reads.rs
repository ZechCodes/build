//! Aggregates and reference lookup share a bounded projection snapshot.
use super::tracker::{filed, tracked};
use super::*;

fn answer(app: &mut AppState, method: &str, params: Value) -> Value {
    let response = app.handle(req(method, params));
    assert_eq!(response["ok"], true, "{response}");
    response["result"].clone()
}

#[test]
fn aggregate_and_lookup_return_the_same_projection_basis_and_explicit_misses() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let task = filed(&mut app, &project, "reference title");
    let counts = answer(
        &mut app,
        "tasks.counts",
        json!({"project_id":project,"agent_ids":["absent","absent"]}),
    );
    assert_eq!(counts["unfinished_open_count"], 1);
    assert_eq!(counts["watched_unread_total"], 0);
    assert_eq!(counts["agents"].as_array().unwrap().len(), 1);
    assert_eq!(counts["agents"][0]["unfinished_open_count"], 0);
    let found = answer(
        &mut app,
        "tasks.lookup",
        json!({"project_id":project,"number":1}),
    );
    assert_eq!(found["task"]["id"], task["id"]);
    assert_eq!(found["task"]["title"], "reference title");
    assert!(found["task"].get("body").is_none());
    assert!(found.get("timeline").is_none());
    assert_eq!(found["basis"], counts["basis"]);
    assert!(serde_json::to_vec(&found).unwrap().len() <= 65_536);
    let absent = answer(
        &mut app,
        "tasks.lookup",
        json!({"project_id":project,"number":999}),
    );
    assert!(absent["task"].is_null());
    assert_eq!(absent["identities"], json!({}));
    assert_eq!(absent["basis"], counts["basis"]);
}

#[test]
fn aggregate_requests_reject_invalid_or_unbounded_agent_buckets() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    for agents in [
        json!([""]),
        json!(["x".repeat(129)]),
        json!((0..101).map(|n| format!("a{n}")).collect::<Vec<_>>()),
    ] {
        let response = app.handle(req(
            "tasks.counts",
            json!({"project_id":project,"agent_ids":agents}),
        ));
        assert_eq!(response["error_code"], "invalid_params", "{response}");
    }
    for number in [json!(0), json!(-1), json!(1.5), json!(null)] {
        let response = app.handle(req(
            "tasks.lookup",
            json!({"project_id":project,"number":number}),
        ));
        assert_eq!(response["error_code"], "invalid_params", "{response}");
    }
}

#[tokio::test]
async fn delayed_aggregate_snapshot_keeps_its_basis_after_a_write_commits() {
    let root = tempfile::tempdir().unwrap();
    let store = crate::store::Store::new(root.path()).unwrap();
    let mut task = crate::tracker::Task::drafted(
        "/basis",
        "snapshot fence",
        crate::tracker::Actor::User,
        "2026-10-01T00:00:00Z",
    );
    task = store.create_tracker_task(task, &[]).unwrap();
    let mut reader = store.read_pool().acquire().await;
    let old_basis = reader
        .read(|snapshot| {
            let before = snapshot.task_query_basis().unwrap();
            assert_eq!(
                snapshot.task_counts("/basis", &[]).unwrap()[0].unfinished_open_count,
                1
            );
            task.status = "done".into();
            store.save_tracker_task_activity(&task, &[], &[]).unwrap();
            // The committed write can be acknowledged while this old read remains in flight.
            assert_eq!(snapshot.task_query_basis().unwrap(), before);
            assert_eq!(
                snapshot.task_counts("/basis", &[]).unwrap()[0].unfinished_open_count,
                1
            );
            before
        })
        .unwrap();
    reader
        .read(|snapshot| {
            assert_ne!(snapshot.task_query_basis().unwrap(), old_basis);
            assert_eq!(
                snapshot.task_counts("/basis", &[]).unwrap()[0].unfinished_open_count,
                0
            );
        })
        .unwrap();
    // A replacement store cannot compare generations with this store.
    let other_root = tempfile::tempdir().unwrap();
    let other = crate::store::Store::new(other_root.path()).unwrap();
    assert_ne!(
        store.task_query_basis().unwrap(),
        other.task_query_basis().unwrap()
    );
}
