use super::*;

#[test]
fn blocked_two_revision_handoffs_compare_from_last_accepted_message() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let workspace_id = workspace(&mut app, &project, "blocked revisions");
    let (owner, agent) = project_agent(&mut app, &project);
    let mut params = opening::params(&workspace_id);
    params.reviewer = Some(ReviewReviewer::Agent {
        agent_id: agent.clone(),
    });
    let opened = opening::open_direct(&mut app, params, Actor::User);
    let task_id = opened["task"]["id"].as_str().unwrap();
    let first = opened["review"]["snapshots"][0]["id"].as_str().unwrap();
    let store = app.store.as_ref().unwrap();
    let review_task = store.load_tracker_task(task_id).unwrap().unwrap();
    let mut blocker = crate::tracker::Task::drafted(
        &review_task.project_path,
        "review prerequisite",
        Actor::User,
        &crate::store::now_rfc3339(),
    );
    blocker.status = "done".into();
    let mut blocker = store.create_tracker_task(blocker, &[]).unwrap();
    // Adding a resolved prerequisite to an active review is valid. Reopening
    // that prerequisite then blocks future reviewer deliveries, without faking
    // a task status or bypassing the dependency graph's write validation.
    store
        .edit_waiting_on(
            &review_task.project_path,
            task_id,
            std::slice::from_ref(&blocker.id),
            &[],
            &Actor::User,
            &crate::store::now_rfc3339(),
        )
        .unwrap();
    blocker.status = "backlog".into();
    store
        .save_tracker_task_activity(&blocker, &[], &[])
        .unwrap();
    while store.reconcile_waiting_on(100).unwrap().pending {}
    assert!(
        store
            .waiting_on_summary(&review_task.project_path, task_id)
            .unwrap()
            .blocked
    );

    let second = publish_next_revision(&app, task_id);
    app.drain_review_revision_dispatches().unwrap();
    let second_operation = format!("review-revision-{second}");
    let store = app.store.as_ref().unwrap();
    assert!(store.operation(&second_operation).unwrap().is_none());
    assert_eq!(
        store
            .delivery_intent_for_operation(&second_operation)
            .unwrap()
            .unwrap()
            .state,
        "waiting"
    );
    assert_eq!(
        store.review_revision_baseline(task_id, &agent, 3).unwrap(),
        Some((first.to_string(), 1))
    );
    let changes = store.revision_dispatch_sql_changes();
    app.drain_review_revision_dispatches().unwrap();
    assert_eq!(
        app.store.as_ref().unwrap().revision_dispatch_sql_changes(),
        changes,
        "waiting hand-off polling performs no writes"
    );

    let third = publish_next_revision(&app, task_id);
    app.drain_review_revision_dispatches().unwrap();
    let store = app.store.as_ref().unwrap();
    assert_eq!(
        store
            .delivery_intent_for_operation(&second_operation)
            .unwrap()
            .unwrap()
            .state,
        "cancelled"
    );
    assert!(store
        .operation(&format!("review-revision-{third}"))
        .unwrap()
        .is_none());
    assert_eq!(
        store.review_revision_baseline(task_id, &agent, 4).unwrap(),
        Some((first.to_string(), 1)),
        "neither waiting revision is a received baseline"
    );
    blocker.status = "done".into();
    store
        .save_tracker_task_activity(&blocker, &[], &[])
        .unwrap();
    pump_saved_task_deliveries(&mut app);
    let body = revision_message(&mut app, &owner, &agent, &third);
    assert!(
        body.contains("Revision 3 is ready; read `since` from revision 1"),
        "{body}"
    );
    assert!(body.contains(first), "{body}");
    let store = app.store.as_ref().unwrap();
    assert!(store.operation(&second_operation).unwrap().is_none());
    assert_eq!(
        store.review_revision_baseline(task_id, &agent, 4).unwrap(),
        Some((third, 3))
    );
}

fn fail_revision(app: &mut AppState, project: &str, name: &str, error: &str) -> (String, String) {
    let workspace_id = workspace(app, project, name);
    let (_, agent) = project_agent(app, project);
    let mut params = opening::params(&workspace_id);
    params.request_id = name.into();
    params.reviewer = Some(ReviewReviewer::Agent {
        agent_id: agent.clone(),
    });
    let opened = opening::open_direct(app, params, Actor::User);
    let task_id = opened["task"]["id"].as_str().unwrap().to_string();
    let next = publish_next_revision(app, &task_id);
    app.store
        .as_ref()
        .unwrap()
        .settle_review_revision_dispatch_at(
            &next,
            Some(&agent),
            Some(error),
            false,
            crate::agent::now_ms() as i64,
        )
        .unwrap();
    (task_id, next)
}

fn failure_notes(app: &AppState, task_id: &str) -> Vec<crate::tracker::TaskComment> {
    app.store
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
        .collect()
}

#[test]
fn failed_notice_error_does_not_prevent_later_notice_and_raw_reason_is_retained() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let (first, first_revision) =
        fail_revision(&mut app, &project, "notice first", "busy: first notice");
    let (second, second_revision) = fail_revision(
        &mut app,
        &project,
        "notice second",
        "unavailable: review reviewer is unavailable",
    );
    app.store.as_ref().unwrap().fail_next_write();
    app.drain_review_revision_dispatches().unwrap();
    assert!(failure_notes(&app, &first).is_empty());
    let notes = failure_notes(&app, &second);
    assert_eq!(notes.len(), 1);
    assert!(
        notes[0].body.ends_with(": the reviewer is unavailable"),
        "{}",
        notes[0].body
    );
    assert!(!notes[0].body.contains("unavailable:"));
    assert_eq!(
        app.store
            .as_ref()
            .unwrap()
            .review_revision_dispatch_result(&second_revision)
            .unwrap()
            .1
            .as_deref(),
        Some("unavailable: review reviewer is unavailable")
    );
    assert_eq!(
        app.store
            .as_ref()
            .unwrap()
            .failed_review_revision_notices()
            .unwrap()[0]
            .snapshot_id,
        first_revision
    );
    app.drain_review_revision_dispatches().unwrap();
    assert_eq!(failure_notes(&app, &first).len(), 1);
    assert!(failure_notes(&app, &first)[0]
        .body
        .ends_with(": first notice"));
    assert_eq!(failure_notes(&app, &second).len(), 1);
}

#[test]
fn missing_project_failure_notice_is_retired_without_starving_a_valid_project() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let (orphan, _) = fail_revision(
        &mut app,
        &project,
        "removed project notice",
        "permanent failure",
    );
    let other_source = init_repo_named(home.path(), "notice valid project");
    let other = added_project(&mut app, &other_source);
    let (valid, _) = fail_revision(
        &mut app,
        &other,
        "valid project notice",
        "permanent failure",
    );
    app.projects.remove(&project);
    app.drain_review_revision_dispatches().unwrap();
    assert!(failure_notes(&app, &orphan).is_empty());
    assert_eq!(failure_notes(&app, &valid).len(), 1);
    assert!(app
        .store
        .as_ref()
        .unwrap()
        .failed_review_revision_notices()
        .unwrap()
        .is_empty());
    let changes = app.store.as_ref().unwrap().revision_dispatch_sql_changes();
    app.drain_review_revision_dispatches().unwrap();
    assert_eq!(
        app.store.as_ref().unwrap().revision_dispatch_sql_changes(),
        changes
    );
}

fn pump_saved_task_deliveries(app: &mut AppState) {
    for _ in 0..100 {
        if !app.pump_task_deliveries_for_test().unwrap() {
            return;
        }
    }
    panic!("saved task delivery did not settle within the bounded pump");
}
