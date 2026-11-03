use super::*;
use crate::carrier::{OutboundEnvelope, SessionSender};

async fn assert_parent_invalidated(
    bus: &crate::changes::ChangeBus,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<OutboundEnvelope>,
    key: &str,
    parent: &str,
) {
    tokio::time::timeout(Duration::from_secs(10), bus.settle_for_test())
        .await
        .unwrap();
    let mut pushed = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        pushed.push(SessionSender::decrypt_push(key, &envelope));
    }
    assert!(
        pushed
            .iter()
            .filter(|frame| frame["type"] == "changes")
            .flat_map(|frame| frame["items"].as_array().into_iter().flatten())
            .flat_map(|item| item["tasks"]["task_ids"].as_array().into_iter().flatten())
            .any(|id| id == parent),
        "parent {parent} must refresh: {pushed:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_opinion_and_close_invalidate_parent_and_refresh_linked_status() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracked(home.path());
    let workspace_id = workspace(&mut app, &project, "live work-task chip");
    let work = filed(&mut app, &project, "Implement the feature");
    let mut asked = opening::params(&workspace_id);
    asked.for_task_id = Some(work["id"].as_str().unwrap().to_owned());
    let opened = opening::open_direct(&mut app, asked, crate::tracker::Actor::User);
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
    let parent = work["id"].as_str().unwrap();
    assert_parent_invalidated(&bus, &mut rx, &key, parent).await;
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
    assert_parent_invalidated(&bus, &mut rx, &key, parent).await;
    let detail = handler.call(sender, req("tasks.get", json!({"task_id":parent})));
    assert_eq!(
        detail["result"]["task"]["linked_reviews"][0]["status"],
        "closed"
    );
}
