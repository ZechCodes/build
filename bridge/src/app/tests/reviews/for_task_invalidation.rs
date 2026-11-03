use super::*;
use crate::carrier::{OutboundEnvelope, SessionSender};

async fn assert_parents_invalidated(
    bus: &crate::changes::ChangeBus,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<OutboundEnvelope>,
    key: &str,
    parents: &[&str],
) -> std::collections::BTreeSet<String> {
    tokio::time::timeout(Duration::from_secs(10), bus.settle_for_test())
        .await
        .unwrap();
    let mut pushed = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        pushed.push(SessionSender::decrypt_push(key, &envelope));
    }
    let ids = pushed
        .iter()
        .filter(|frame| frame["type"] == "changes")
        .flat_map(|frame| frame["items"].as_array().into_iter().flatten())
        .flat_map(|item| item["tasks"]["task_ids"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    for parent in parents {
        assert!(
            ids.contains(*parent),
            "parent {parent} must refresh: {pushed:?}"
        );
    }
    ids
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relink_opinion_and_close_refresh_both_parent_chips_and_canonical_workspace_link() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracked(home.path());
    let workspace_id = workspace(&mut app, &project, "live work-task chip");
    let work = filed(&mut app, &project, "Implement the feature");
    let next_work = filed(&mut app, &project, "New work scope");
    let mut asked = opening::params(&workspace_id);
    asked.for_task_id = Some(work["id"].as_str().unwrap().to_owned());
    let retry_params = serde_json::to_value(&asked).unwrap();
    let opened = opening::open_direct(&mut app, asked, crate::tracker::Actor::User);
    let project_path = app.tracker_project_path(&project).unwrap();
    let state = app.shared();
    let handler = AppState::handler(Arc::clone(&state));
    let (sender, mut rx, key) = SessionSender::observable("work-task-reader");
    assert_eq!(
        handler.call(sender.clone(), req("session.hello", json!({})))["ok"],
        true
    );
    assert_eq!(handler.call(sender.clone(), req("changes.subscribe", json!({
        "subscription_id":"work-task-review", "scope":{"kind":"entity","id":project}, "kinds":["tasks"]
    })))["ok"], true);
    let bus = state.lock().unwrap().changes();
    bus.settle_for_test().await;
    while rx.try_recv().is_ok() {}
    state
        .lock()
        .unwrap()
        .link_task_as_user(&json!({
            "task_id":opened["task"]["id"], "parent_task_id":next_work["id"]
        }))
        .unwrap();
    let old_parent = work["id"].as_str().unwrap();
    let parent = next_work["id"].as_str().unwrap();
    assert_parents_invalidated(&bus, &mut rx, &key, &[old_parent, parent]).await;
    let old_detail = handler.call(
        sender.clone(),
        req("tasks.get", json!({"task_id":old_parent})),
    );
    assert!(old_detail["result"]["task"].get("linked_reviews").is_none());
    let new_detail = handler.call(sender.clone(), req("tasks.get", json!({"task_id":parent})));
    assert_eq!(
        new_detail["result"]["task"]["linked_reviews"][0]["task_id"],
        opened["task"]["id"]
    );
    let workspace = handler.call(
        sender.clone(),
        req("workspace.get", json!({"workspace_id":workspace_id})),
    );
    assert_eq!(
        workspace["result"]["active_review"]["parent_task_id"],
        next_work["id"]
    );
    assert_eq!(
        state
            .lock()
            .unwrap()
            .store
            .as_ref()
            .unwrap()
            .load_review_opening(&project_path, "open-1")
            .unwrap()
            .unwrap()
            .request
            .for_task_id
            .as_deref(),
        Some(old_parent)
    );
    let retried = handler.call(sender.clone(), req("tasks.review.open", retry_params));
    assert_eq!(retried["ok"], true, "{retried}");
    assert_eq!(
        retried["result"]["task"]["links"]["parent_task_id"],
        next_work["id"]
    );
    let changed = assert_parents_invalidated(&bus, &mut rx, &key, &[parent]).await;
    assert!(
        !changed.contains(old_parent),
        "retry must use the current parent: {changed:?}"
    );
    let opinion = handler.call(
        sender.clone(),
        req(
            "tasks.comment",
            json!({
                "task_id":opened["task"]["id"], "body":"Approved", "opinion":{
                    "snapshot_id":opened["review"]["snapshots"][0]["id"], "verdict":"approve"
                }
            }),
        ),
    );
    assert_eq!(opinion["ok"], true, "{opinion}");
    assert_parents_invalidated(&bus, &mut rx, &key, &[parent]).await;
    let detail = handler.call(sender.clone(), req("tasks.get", json!({"task_id":parent})));
    assert_eq!(
        detail["result"]["task"]["linked_reviews"][0]["status"],
        "approved"
    );
    let version = detail["result"]["task"]["linked_reviews"][0]["version"].clone();
    let closed = handler.call(sender.clone(), req("tasks.review.close", json!({
        "task_id":opened["task"]["id"], "expected_version":version, "description":"Review completed without applying."
    })));
    assert_eq!(closed["ok"], true, "{closed}");
    assert_parents_invalidated(&bus, &mut rx, &key, &[parent]).await;
    let detail = handler.call(sender, req("tasks.get", json!({"task_id":parent})));
    assert_eq!(
        detail["result"]["task"]["linked_reviews"][0]["status"],
        "closed"
    );
}
