use super::*;
use crate::api::v1::reviews::ReviewReviewer;
use crate::tracker::Actor;

#[test]
fn new_revision_redispatches_agent_assignee_once_with_previous_revision_baseline() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let workspace_id = workspace(&mut app, &project, "revision delivery");
    let (owner, agent) = project_agent(&mut app, &project);
    let mut params = opening::params(&workspace_id);
    params.reviewer = Some(ReviewReviewer::Agent {
        agent_id: agent.clone(),
    });
    let opened = opening::open_direct(&mut app, params, Actor::User);
    let review: crate::reviews::records::Review =
        serde_json::from_value(opened["review"].clone()).unwrap();
    let previous = review.snapshots[0].id.clone();
    let mut next = review.snapshots[0].clone();
    next.id = uuid::Uuid::new_v4().to_string();
    next.author = Actor::Build;
    let next_id = next.id.clone();
    app.store
        .as_ref()
        .unwrap()
        .save_review_received_snapshot(&review.task_id, review.version, next, &review.bindings)
        .unwrap();
    app.drain_review_revision_dispatches().unwrap();
    app.drain_review_revision_dispatches().unwrap();
    let operation_id = format!("review-revision-{next_id}");
    let page = app.handle(req(
        "thread.page",
        json!({"entity_id":owner,"agent_id":agent,"limit":50}),
    ));
    let deliveries: Vec<_> = page["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "message" && item["data"]["operation_id"] == operation_id)
        .collect();
    assert_eq!(deliveries.len(), 1, "{page}");
    let body = deliveries[0]["data"]["body"].as_str().unwrap();
    assert!(body.contains("Revision 2 is ready"), "{body}");
    assert!(body.contains("read `since` from revision 1"), "{body}");
    assert!(body.contains(&previous), "{body}");
    assert!(body.contains(&next_id), "{body}");
    assert!(app
        .store
        .as_ref()
        .unwrap()
        .pending_review_revision_dispatches()
        .unwrap()
        .is_empty());
}

#[test]
fn user_review_inbox_assignment_yields_after_verdict_and_returns_with_revision() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let workspace_id = workspace(&mut app, &project, "user revision");
    let mut params = opening::params(&workspace_id);
    params.reviewer = Some(ReviewReviewer::User);
    let opened = opening::open_direct(&mut app, params, Actor::User);
    let review: crate::reviews::records::Review =
        serde_json::from_value(opened["review"].clone()).unwrap();
    let row = |app: &AppState| {
        app.watched_task_rows()
            .into_iter()
            .find(|row| row["task_id"] == review.task_id)
            .unwrap()
    };
    assert_eq!(row(&app)["assigned_to_user"], true);
    review_call(
        &mut app,
        "tasks.comment",
        json!({"task_id":review.task_id,"body":"Fix the remaining issue", "opinion":{"snapshot_id":review.snapshots[0].id,"verdict":"request_changes"}}),
    );
    assert_eq!(row(&app)["assigned_to_user"], false);
    let current = app
        .store
        .as_ref()
        .unwrap()
        .load_review(&review.task_id)
        .unwrap()
        .unwrap();
    let mut next = review.snapshots[0].clone();
    next.id = uuid::Uuid::new_v4().to_string();
    app.store
        .as_ref()
        .unwrap()
        .save_review_received_snapshot(&review.task_id, current.version, next, &review.bindings)
        .unwrap();
    assert_eq!(row(&app)["assigned_to_user"], true);
}
