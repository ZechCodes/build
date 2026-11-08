//! Outboxes remain durable but cannot create work or attention while archived.
use super::*;
use crate::store::{ReviewVerdictDelivery, ReviewVerdictDeliveryState};

fn finish_checkout(app: &mut AppState, id: &str) {
    let mut finished = app.workspaces.get(id).unwrap().clone();
    app.workspaces.record_finished(&mut finished).unwrap();
    app.workspaces.apply_local_capabilities(finished).unwrap();
    app.delivery_queue.clear_queued();
}

#[test]
fn archive_refused_with_pending_verdict_delivery() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let workspace = workspace(&mut app, &project, "verdict outbox");
    let opened = call(
        &mut app,
        "tasks.review.open",
        json!({"workspace_id":workspace,
        "request_id":"pending-verdict", "title":"Review", "description":"Review"}),
    );
    call(
        &mut app,
        "tasks.comment",
        json!({"task_id":opened["task"]["id"],"body":"Approved",
        "opinion":{"snapshot_id":opened["review"]["snapshots"][0]["id"],"verdict":"approve"}}),
    );
    finish_checkout(&mut app, &workspace);
    assert_eq!(
        app.store
            .as_ref()
            .unwrap()
            .pending_review_verdict_deliveries(100)
            .unwrap()
            .len(),
        1
    );
    let refusal = app.handle(req("project.archive", json!({"project_id":project})));
    assert_eq!(refusal["error_code"], "conflict", "{refusal}");
    assert!(
        refusal["error"].as_str().unwrap().contains(&format!(
            "pending verdict delivery for #{}",
            opened["task"]["number"]
        )),
        "{refusal}"
    );
}

#[test]
fn archive_refused_with_pending_revision_dispatch() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let workspace = workspace(&mut app, &project, "revision outbox");
    let (_, reviewer) = project_agent(&mut app, &project);
    let opened = call(
        &mut app,
        "tasks.review.open",
        json!({"workspace_id":workspace,
        "request_id":"pending-revision", "title":"Review", "description":"Review",
        "reviewer":{"kind":"agent","agent_id":reviewer}}),
    );
    let store = app.store.clone().unwrap();
    let review = store
        .load_review(opened["task"]["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    let mut next = review.snapshots[0].clone();
    next.id = uuid::Uuid::new_v4().to_string();
    store
        .save_review_received_snapshot(&review.task_id, review.version, next, &review.bindings)
        .unwrap();
    let current = store.load_review(&review.task_id).unwrap().unwrap();
    call(
        &mut app,
        "tasks.review.close",
        json!({"task_id":review.task_id,
        "expected_version":current.version,"description":"Closed while handoff remains pending"}),
    );
    finish_checkout(&mut app, &workspace);
    assert_eq!(store.pending_review_revision_dispatches().unwrap().len(), 1);
    let refusal = app.handle(req("project.archive", json!({"project_id":project})));
    assert_eq!(refusal["error_code"], "conflict", "{refusal}");
    assert!(
        refusal["error"].as_str().unwrap().contains(&format!(
            "pending revision dispatch for #{}",
            opened["task"]["number"]
        )),
        "{refusal}"
    );
}

fn seed_verdict(app: &AppState, task: &Value) -> ReviewVerdictDelivery {
    let store = app.store.as_ref().unwrap();
    let task_id = task["id"].as_str().unwrap();
    let project_path = store.task_project_authority(task_id).unwrap().unwrap().0;
    let now = crate::store::now_rfc3339();
    let delivery = ReviewVerdictDelivery {
        comment_id: crate::tracker::new_comment_id(),
        task_id: task_id.into(),
        task_number: task["number"].as_u64().unwrap(),
        project_path,
        workspace_id: "retired-workspace".into(),
        snapshot_id: uuid::Uuid::new_v4().to_string(),
        revision: 1,
        verdict: crate::tracker::ReviewVerdict::Approve,
        reason: "Saved verdict".into(),
        reviewer: crate::tracker::Actor::User,
        batch: vec![],
        operation_id: uuid::Uuid::new_v4().to_string(),
        state: ReviewVerdictDeliveryState::Pending,
        recipient_agent_id: None,
        fallback_comment_id: None,
        retry_context: None,
        attempts: 0,
        error: None,
        created_at: now.clone(),
        updated_at: now,
    };
    store.writer_for_test().execute("INSERT INTO review_verdict_deliveries(comment_id,operation_id,task_id,state,created_at,updated_at,record) VALUES(?1,?2,?3,'pending',?4,?4,?5)",
        rusqlite::params![delivery.comment_id,delivery.operation_id,delivery.task_id,delivery.created_at,serde_json::to_string(&delivery).unwrap()]).unwrap();
    delivery
}

fn seed_revision(app: &AppState, task: &Value, failed: bool) -> String {
    let snapshot = uuid::Uuid::new_v4().to_string();
    app.store.as_ref().unwrap().writer_for_test().execute("INSERT INTO review_revision_dispatches(task_id,snapshot_id,previous_snapshot_id,number,state,failure_comment_id,error) VALUES(?1,?2,'previous',2,?3,?4,?5)",
        rusqlite::params![task["id"].as_str().unwrap(),snapshot,if failed {"failed"} else {"pending"},
        failed.then(crate::tracker::new_comment_id),failed.then_some("reviewer unavailable")]).unwrap();
    snapshot
}

#[test]
fn archive_refused_with_pending_revision_failure_notice() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let task = filed(&mut app, &project, "Unposted failure notice");
    seed_revision(&app, &task, true);
    let refusal = app.handle(req("project.archive", json!({"project_id":project})));
    assert_eq!(refusal["error_code"], "conflict", "{refusal}");
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("revision delivery notice"),
        "{refusal}"
    );
}

#[test]
fn archived_project_background_deliveries_wait_for_restore() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, archived) = tracked(home.path());
    let active = crate::app::projects::test_support::register_sibling(&mut app, &archived);
    let retained = filed(&mut app, &archived, "Retained history");
    let live = filed(&mut app, &active, "Active history");
    call(&mut app, "project.archive", json!({"project_id":archived}));
    // These rows appear after archive admission; more than a page must not starve active work.
    let mut saved = Vec::new();
    for _ in 0..33 {
        saved.push(seed_verdict(&app, &retained));
        seed_revision(&app, &retained, false);
        seed_revision(&app, &retained, true);
    }
    let active_verdict = seed_verdict(&app, &live);
    let active_revision = seed_revision(&app, &live, false);
    seed_revision(&app, &live, true);
    let store = app.store.clone().unwrap();
    let before_task = serde_json::to_value(
        store
            .load_tracker_task(retained["id"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    let before_history = serde_json::to_value(
        store
            .load_tracker_timeline(retained["id"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    for _ in 0..2 {
        app.drain_review_verdict_deliveries().unwrap();
        app.drain_review_revision_dispatches().unwrap();
    }
    assert_eq!(
        serde_json::to_value(
            store
                .load_tracker_task(retained["id"].as_str().unwrap())
                .unwrap()
        )
        .unwrap(),
        before_task
    );
    assert_eq!(
        serde_json::to_value(
            store
                .load_tracker_timeline(retained["id"].as_str().unwrap())
                .unwrap()
        )
        .unwrap(),
        before_history
    );
    for delivery in &saved {
        assert_eq!(
            store
                .load_review_verdict_delivery(&delivery.comment_id)
                .unwrap()
                .unwrap(),
            *delivery
        );
    }
    let pending_revisions: i64 = store.writer_for_test().query_row(
        "SELECT COUNT(*) FROM review_revision_dispatches WHERE task_id=?1 AND state='pending' AND attempts=0 AND retry_after_ms=0",
        [retained["id"].as_str().unwrap()], |row| row.get(0)).unwrap();
    let unposted_notices: i64 = store.writer_for_test().query_row(
        "SELECT COUNT(*) FROM review_revision_dispatches WHERE task_id=?1 AND state='failed' AND failure_notice_posted=0",
        [retained["id"].as_str().unwrap()], |row| row.get(0)).unwrap();
    assert_eq!(
        pending_revisions, 33,
        "archived revisions retain their state and retry budget"
    );
    assert_eq!(
        unposted_notices, 33,
        "archived failure notices are retained for restore"
    );
    assert_eq!(
        store
            .load_review_verdict_delivery(&active_verdict.comment_id)
            .unwrap()
            .unwrap()
            .state,
        ReviewVerdictDeliveryState::Failed
    );
    assert_eq!(
        store
            .review_revision_dispatch_result(&active_revision)
            .unwrap()
            .0,
        "failed"
    );
    assert!(!store
        .load_tracker_timeline(live["id"].as_str().unwrap())
        .unwrap()
        .is_empty());
    call(&mut app, "project.restore", json!({"project_id":archived}));
    for _ in 0..3 {
        app.drain_review_verdict_deliveries().unwrap();
        app.drain_review_revision_dispatches().unwrap();
    }
    assert_eq!(
        store
            .load_review_verdict_delivery(&saved[0].comment_id)
            .unwrap()
            .unwrap()
            .state,
        ReviewVerdictDeliveryState::Failed
    );
    assert_ne!(
        serde_json::to_value(
            store
                .load_tracker_timeline(retained["id"].as_str().unwrap())
                .unwrap()
        )
        .unwrap(),
        before_history
    );
    assert!(store
        .pending_review_revision_dispatches()
        .unwrap()
        .is_empty());
    assert!(store.failed_review_revision_notices().unwrap().is_empty());
}
