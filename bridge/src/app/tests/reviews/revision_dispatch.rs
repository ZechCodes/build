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

fn publish_next_revision(app: &AppState, task_id: &str) -> String {
    let store = app.store.as_ref().unwrap();
    let review = store.load_review(task_id).unwrap().unwrap();
    let mut next = review.snapshots.last().unwrap().clone();
    next.id = uuid::Uuid::new_v4().to_string();
    next.author = Actor::Build;
    let id = next.id.clone();
    store
        .save_review_received_snapshot(task_id, review.version, next, &review.bindings)
        .unwrap();
    id
}

fn revision_message(app: &mut AppState, owner: &str, agent: &str, snapshot_id: &str) -> String {
    let page = app.handle(req(
        "thread.page",
        json!({"entity_id":owner,"agent_id":agent,"limit":50}),
    ));
    let operation_id = format!("review-revision-{snapshot_id}");
    let messages: Vec<_> = page["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "message" && item["data"]["operation_id"] == operation_id)
        .collect();
    assert_eq!(messages.len(), 1, "{page}");
    messages[0]["data"]["body"].as_str().unwrap().to_string()
}

#[test]
fn coalesced_revisions_keep_the_last_delivered_baseline_for_agent_and_coordinator() {
    for coordinator in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let (_repo, mut app, project) = tracked(home.path());
        let workspace_id = workspace(&mut app, &project, "coalesced revisions");
        let (owner, agent) = project_agent(&mut app, &project);
        let mut params = opening::params(&workspace_id);
        params.reviewer = Some(if coordinator {
            ReviewReviewer::ProjectAgent
        } else {
            ReviewReviewer::Agent {
                agent_id: agent.clone(),
            }
        });
        let opened = opening::open_direct(&mut app, params, Actor::User);
        let task_id = opened["task"]["id"].as_str().unwrap();
        let first_id = opened["review"]["snapshots"][0]["id"].as_str().unwrap();
        let second = publish_next_revision(&app, task_id);
        let third = publish_next_revision(&app, task_id);
        app.drain_review_revision_dispatches().unwrap();
        app.drain_review_revision_dispatches().unwrap();
        assert!(app
            .store
            .as_ref()
            .unwrap()
            .operation(&format!("review-revision-{second}"))
            .unwrap()
            .is_none());
        assert_eq!(
            app.store
                .as_ref()
                .unwrap()
                .review_revision_dispatch_result(&second)
                .unwrap()
                .0,
            "superseded"
        );
        let body = revision_message(&mut app, &owner, &agent, &third);
        assert!(
            body.contains("Revision 3 is ready; read `since` from revision 1"),
            "{body}"
        );
        assert!(body.contains(first_id), "{body}");
        let fourth = publish_next_revision(&app, task_id);
        app.drain_review_revision_dispatches().unwrap();
        let body = revision_message(&mut app, &owner, &agent, &fourth);
        assert!(
            body.contains("Revision 4 is ready; read `since` from revision 3"),
            "{body}"
        );
        assert!(body.contains(&third), "{body}");

        // A newly assigned reviewer has no delivered baseline of its own.
        let replacement = app.handle(req("agent.add", json!({"entity_id":owner})))["result"]
            ["agent"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let store = app.store.as_ref().unwrap();
        let mut task = store.load_tracker_task(task_id).unwrap().unwrap();
        task.assignee = Some(crate::tracker::Assignee::Agent {
            agent_id: replacement.clone(),
        });
        store.save_tracker_task_activity(&task, &[], &[]).unwrap();
        let fifth = publish_next_revision(&app, task_id);
        app.drain_review_revision_dispatches().unwrap();
        let body = revision_message(&mut app, &owner, &replacement, &fifth);
        assert!(body.contains("Revision 5 is ready"), "{body}");
        assert!(body.contains("Read the full current revision"), "{body}");
        assert!(body.contains(&fifth), "{body}");
        assert!(!body.contains("read `since`"), "{body}");
    }
}

#[test]
fn permanent_revision_dispatch_failure_is_terminal_and_not_attempted_on_each_drain() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let workspace_id = workspace(&mut app, &project, "missing revision reviewer");
    let (_, agent) = project_agent(&mut app, &project);
    let mut params = opening::params(&workspace_id);
    params.reviewer = Some(ReviewReviewer::Agent { agent_id: agent });
    let opened = opening::open_direct(&mut app, params, Actor::User);
    let task_id = opened["task"]["id"].as_str().unwrap();
    let foreign_source = init_repo_named(home.path(), "foreign-reviewer");
    let foreign_project = added_project(&mut app, &foreign_source);
    let (_, foreign_agent) = project_agent(&mut app, &foreign_project);
    let store = app.store.as_ref().unwrap();
    let mut task = store.load_tracker_task(task_id).unwrap().unwrap();
    task.assignee = Some(crate::tracker::Assignee::Agent {
        agent_id: foreign_agent,
    });
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    let next = publish_next_revision(&app, task_id);
    app.drain_review_revision_dispatches().unwrap();
    let store = app.store.as_ref().unwrap();
    let failed = store.review_revision_dispatch_result(&next).unwrap();
    assert_eq!(failed.0, "failed");
    assert!(failed.1.is_some());
    assert!(store
        .pending_review_revision_dispatches()
        .unwrap()
        .is_empty());
    app.drain_review_revision_dispatches().unwrap();
    assert_eq!(
        app.store
            .as_ref()
            .unwrap()
            .review_revision_dispatch_result(&next)
            .unwrap(),
        failed
    );
}

#[test]
fn unavailable_coordinator_retries_when_due_without_writing_on_skipped_polls() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let workspace_id = workspace(&mut app, &project, "retry coordinator");
    let (owner, agent) = project_agent(&mut app, &project);
    let mut params = opening::params(&workspace_id);
    params.reviewer = Some(ReviewReviewer::ProjectAgent);
    let opened = opening::open_direct(&mut app, params, Actor::User);
    let next = publish_next_revision(&app, opened["task"]["id"].as_str().unwrap());
    app.runs.get_mut(&owner).unwrap().pinned_agent_id = None;
    let now = crate::agent::now_ms() as i64;
    app.drain_review_revision_dispatches_at(now).unwrap();
    let schedule = app
        .store
        .as_ref()
        .unwrap()
        .review_revision_dispatch_schedule(&next)
        .unwrap();
    assert_eq!((schedule.0, schedule.1), (1, now + 30_000));
    assert_eq!(
        app.store
            .as_ref()
            .unwrap()
            .review_revision_dispatch_result(&next)
            .unwrap()
            .0,
        "pending"
    );
    let changes = app.store.as_ref().unwrap().revision_dispatch_sql_changes();
    app.drain_review_revision_dispatches_at(now + 29_999)
        .unwrap();
    assert_eq!(
        app.store.as_ref().unwrap().revision_dispatch_sql_changes(),
        changes
    );
    app.runs.get_mut(&owner).unwrap().pinned_agent_id = Some(agent.clone());
    app.drain_review_revision_dispatches_at(now + 30_000)
        .unwrap();
    assert!(revision_message(&mut app, &owner, &agent, &next).contains("Revision 2 is ready"));
    assert_eq!(
        app.store
            .as_ref()
            .unwrap()
            .review_revision_dispatch_result(&next)
            .unwrap()
            .0,
        "delivered"
    );
}

#[test]
fn failed_revision_notice_replays_one_reserved_comment_identity_after_store_reopen() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let workspace_id = workspace(&mut app, &project, "revision failure replay");
    let (_, agent) = project_agent(&mut app, &project);
    let mut params = opening::params(&workspace_id);
    params.reviewer = Some(ReviewReviewer::Agent {
        agent_id: agent.clone(),
    });
    let opened = opening::open_direct(&mut app, params, Actor::User);
    let task_id = opened["task"]["id"].as_str().unwrap();
    let next = publish_next_revision(&app, task_id);
    let now = crate::agent::now_ms() as i64;
    let store = app.store.as_ref().unwrap();
    store
        .settle_review_revision_dispatch_at(
            &next,
            Some(&agent),
            Some("reviewer belongs to another project"),
            false,
            now,
        )
        .unwrap();
    let reserved = store
        .review_revision_dispatch_schedule(&next)
        .unwrap()
        .2
        .unwrap();
    assert!(reserved.starts_with("tc-"));
    // Restart after the failed row reserved its notice but before the notice write.
    app.store = Some(crate::store::Store::new(home.path().join("store")).unwrap());
    assert_eq!(
        app.store
            .as_ref()
            .unwrap()
            .review_revision_dispatch_schedule(&next)
            .unwrap()
            .2
            .as_deref(),
        Some(reserved.as_str())
    );
    app.drain_review_revision_dispatches_at(now).unwrap();
    app.drain_review_revision_dispatches_at(now + 1).unwrap();
    let notes: Vec<_> = app
        .store
        .as_ref()
        .unwrap()
        .load_tracker_timeline(task_id)
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry {
            crate::tracker::TimelineEntry::Comment(comment)
                if comment.review_notice
                    == Some(crate::tracker::ReviewNoticeKind::RevisionUndelivered) =>
            {
                Some(comment)
            }
            _ => None,
        })
        .collect();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].id, reserved);
    assert!(notes[0].mentions_user);
    assert!(notes[0].body.contains(&format!(
        "Revision 2 couldn’t be sent to @agent:{agent}: reviewer belongs to another project"
    )));
}

#[test]
fn exhausted_transient_revision_attempts_back_off_to_ten_minutes_then_notify_once() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let workspace_id = workspace(&mut app, &project, "exhausted coordinator");
    let (owner, _) = project_agent(&mut app, &project);
    let mut params = opening::params(&workspace_id);
    params.reviewer = Some(ReviewReviewer::ProjectAgent);
    let opened = opening::open_direct(&mut app, params, Actor::User);
    let task_id = opened["task"]["id"].as_str().unwrap();
    let next = publish_next_revision(&app, task_id);
    app.runs.get_mut(&owner).unwrap().pinned_agent_id = None;
    let mut now = crate::agent::now_ms() as i64;
    for (attempt, delay) in [30_000, 60_000, 120_000, 240_000, 480_000, 600_000]
        .into_iter()
        .enumerate()
    {
        app.drain_review_revision_dispatches_at(now).unwrap();
        let schedule = app
            .store
            .as_ref()
            .unwrap()
            .review_revision_dispatch_schedule(&next)
            .unwrap();
        assert_eq!((schedule.0, schedule.1), (attempt as i64 + 1, now + delay));
        now += delay;
    }
    app.drain_review_revision_dispatches_at(now).unwrap();
    app.drain_review_revision_dispatches_at(now + 600_000)
        .unwrap();
    assert_eq!(
        app.store
            .as_ref()
            .unwrap()
            .review_revision_dispatch_schedule(&next)
            .unwrap()
            .0,
        7
    );
    assert_eq!(
        app.store
            .as_ref()
            .unwrap()
            .review_revision_dispatch_result(&next)
            .unwrap()
            .0,
        "failed"
    );
    let timeline = app
        .store
        .as_ref()
        .unwrap()
        .load_tracker_timeline(task_id)
        .unwrap();
    assert_eq!(timeline.iter().filter(|entry| matches!(entry,crate::tracker::TimelineEntry::Comment(comment) if comment.review_notice == Some(crate::tracker::ReviewNoticeKind::RevisionUndelivered) && comment.mentions_user)).count(),1);
}
