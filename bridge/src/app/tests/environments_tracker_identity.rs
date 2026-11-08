//! Opaque project keys keep sibling boards and their asynchronous work separate.
use super::project_agent::project_agent;
use super::tracker::{filed, tracked};
use super::*;
use crate::app::projects::test_support::register_sibling;
use crate::tracker::{Actor, Assignee, Task};

fn seeded(app: &AppState, project: &str, title: &str) -> Task {
    let key = &app.project(project).unwrap().project_key;
    app.tracker_store()
        .unwrap()
        .create_tracker_task(
            Task::drafted(key, title, Actor::User, &crate::store::now_rfc3339()),
            &[],
        )
        .unwrap()
}

#[test]
fn sibling_projects_number_tasks_independently() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, legacy) = tracked(root.path());
    let sibling = register_sibling(&mut app, &legacy);
    let first = filed(&mut app, &legacy, "legacy first");
    let second = filed(&mut app, &sibling, "sibling first");
    assert_eq!(first["number"], 1);
    assert_eq!(second["number"], 1);
    for (project, task) in [(&legacy, &first), (&sibling, &second)] {
        let listed = app.handle(req("tasks.list", json!({"project_id":project})));
        assert_eq!(listed["ok"], true, "{listed}");
        assert_eq!(
            listed["result"]["tasks"].as_array().unwrap().len(),
            1,
            "{listed}"
        );
        assert_eq!(listed["result"]["tasks"][0]["id"], task["id"]);
        assert_eq!(listed["result"]["tasks"][0]["project_id"], project.as_str());
    }
}

#[test]
fn tracker_task_resolves_uuid_keyed_task() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, legacy) = tracked(root.path());
    let sibling = register_sibling(&mut app, &legacy);
    let task = seeded(&app, &sibling, "opaque lookup");
    let (project, resolved) = app.tracker_task(&task.id).unwrap();
    assert_eq!(project, sibling);
    assert_eq!(
        resolved.project_path,
        app.project(&sibling).unwrap().project_key
    );
    let answer = app.handle(req("tasks.get", json!({"task_id":task.id})));
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["task"]["project_id"], sibling);
}

fn intent_for(task: &Task) -> crate::store::DeliveryIntent {
    crate::store::DeliveryIntent {
        id: String::new(),
        task_id: task.id.clone(),
        project_path: task.project_path.clone(),
        generation: 0,
        target: json!({"kind":"project_agent"}),
        actor: Actor::User,
        sender: None,
        note: None,
        operation_id: String::new(),
        state: String::new(),
        reservation: Value::Null,
        admitted: None,
        error: None,
    }
}

fn thread_count(app: &AppState, owner: &str, agent: &str) -> usize {
    app.runs[owner]
        .agents
        .by_id(agent)
        .unwrap()
        .thread
        .items
        .len()
}

#[test]
fn project_agent_intent_routes_by_key() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, legacy) = tracked(root.path());
    let sibling = register_sibling(&mut app, &legacy);
    let (legacy_owner, legacy_agent) = project_agent(&mut app, &legacy);
    let (owner, coordinator) = project_agent(&mut app, &sibling);
    let mut task = seeded(&app, &sibling, "queued coordinator work");
    task.assignee = Some(Assignee::ProjectAgent);
    task.trackers = vec![coordinator.clone()];
    app.tracker_store()
        .unwrap()
        .accept_delivery_intent(&task, &[], intent_for(&task))
        .unwrap();
    let before = thread_count(&app, &owner, &coordinator);
    let legacy_before = thread_count(&app, &legacy_owner, &legacy_agent);
    let moved = app.handle(req(
        "tasks.update",
        json!({"task_id":task.id,"status":"ready"}),
    ));
    assert_eq!(moved["ok"], true, "{moved}");
    assert_eq!(
        thread_count(&app, &owner, &coordinator),
        before,
        "queued project holder suppresses generic updates until its assignment is delivered"
    );
    assert_eq!(
        thread_count(&app, &legacy_owner, &legacy_agent),
        legacy_before
    );
    let receipt = app
        .tracker_store()
        .unwrap()
        .current_delivery_intent(&task.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        receipt.project_path,
        app.project(&sibling).unwrap().project_key
    );
}

#[test]
fn dependency_change_reconciles_for_uuid_keyed_sibling() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, legacy) = tracked(root.path());
    let sibling = register_sibling(&mut app, &legacy);
    let legacy_task = seeded(&app, &legacy, "other board");
    let dependent = seeded(&app, &sibling, "dependent");
    let mut blocker = seeded(&app, &sibling, "blocker");
    let store = app.tracker_store().unwrap().clone();
    let now = crate::store::now_rfc3339();
    // Cross-project edges remain refused by the existing graph contract.
    assert!(store
        .edit_waiting_on(
            &dependent.project_path,
            &dependent.id,
            std::slice::from_ref(&legacy_task.id),
            &[],
            &Actor::User,
            &now
        )
        .is_err());
    store
        .edit_waiting_on(
            &dependent.project_path,
            &dependent.id,
            std::slice::from_ref(&blocker.id),
            &[],
            &Actor::User,
            &now,
        )
        .unwrap();
    let state = app.shared();
    while AppState::pump_waiting_on_changes_for_test(&state).unwrap() {}
    let changes = state.lock().unwrap().changes();
    let handler = AppState::handler(Arc::clone(&state));
    let (session, mut rx, key) = SessionSender::observable("sibling-dependencies");
    assert_eq!(
        handler.call(
            session.clone(),
            req("session.hello", json!({"changes":"subscriptions"}))
        )["ok"],
        true
    );
    assert_eq!(
        handler.call(
            session,
            req(
                "changes.subscribe",
                json!({
                    "subscription_id":"both-projects", "scope":{"kind":"all"}, "kinds":["tasks"]
                })
            )
        )["ok"],
        true
    );
    changes.flush();
    while rx.try_recv().is_ok() {}
    blocker.status = "done".into();
    store
        .save_tracker_task_activity(&blocker, &[], &[])
        .unwrap();
    while AppState::pump_waiting_on_changes_for_test(&state).unwrap() {}
    changes.flush();
    let mut projects = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        let push = SessionSender::decrypt_push(&key, &envelope);
        if let Some(items) = push["items"].as_array() {
            projects.extend(
                items
                    .iter()
                    .filter(|item| item.get("tasks").is_some())
                    .map(|item| item["entity_id"].as_str().unwrap().to_string()),
            );
        }
    }
    assert_eq!(projects, vec![sibling]);
    assert!(
        !store
            .waiting_on_summary(&dependent.project_path, &dependent.id)
            .unwrap()
            .blocked
    );
}

#[test]
fn revision_dispatch_recovers_for_uuid_keyed_project() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, legacy) = tracked(root.path());
    let sibling = register_sibling(&mut app, &legacy);
    let task = seeded(&app, &sibling, "failed revision handoff");
    let store = app.tracker_store().unwrap().clone();
    store.writer_for_test().execute("INSERT INTO review_revision_dispatches(task_id,snapshot_id,previous_snapshot_id,number,reviewer,state,error) VALUES(?1,'uuid-revision','uuid-previous',2,'reviewer','failed','reviewer unavailable')", [&task.id]).unwrap();
    app.drain_review_revision_dispatches().unwrap();
    let notices = store
        .load_tracker_timeline(&task.id)
        .unwrap()
        .into_iter()
        .filter(|entry| {
            matches!(entry, crate::tracker::TimelineEntry::Comment(comment)
            if comment.review_notice == Some(crate::tracker::ReviewNoticeKind::RevisionUndelivered))
        })
        .count();
    assert_eq!(notices, 1);
    assert!(store.load_tracker_task(&task.id).unwrap().unwrap().watched);
    assert!(store.failed_review_revision_notices().unwrap().is_empty());
    app.drain_review_revision_dispatches().unwrap();
    assert_eq!(
        store
            .load_tracker_timeline(&task.id)
            .unwrap()
            .iter()
            .filter(|entry| matches!(entry, crate::tracker::TimelineEntry::Comment(_)))
            .count(),
        1
    );
}

#[test]
fn session_summaries_rebind_by_key() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, legacy) = tracked(root.path());
    let sibling = register_sibling(&mut app, &legacy);
    for (project, stamp) in [(&legacy, 100_i64), (&sibling, 900_i64)] {
        let key = app.project(project).unwrap().project_key.clone();
        app.tracker_store().unwrap().writer_for_test().execute(
            "INSERT INTO device_pooled_sessions(kind,key,session_started_ms,last_activity_ms) VALUES('project',?1,?2,?2)",
            rusqlite::params![key, stamp]).unwrap();
    }
    app.rebuild_session_summaries().unwrap();
    assert_eq!(app.session_summary(&legacy).last_activity_ms, Some(100));
    assert_eq!(app.session_summary(&sibling).last_activity_ms, Some(900));
}

#[test]
fn verdict_delivery_routes_by_key() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, legacy) = tracked(root.path());
    let sibling = register_sibling(&mut app, &legacy);
    let (legacy_owner, legacy_agent) = project_agent(&mut app, &legacy);
    let (owner, agent) = project_agent(&mut app, &sibling);
    let key = &app.project(&sibling).unwrap().project_key;
    assert_eq!(
        app.review_verdict_recipient_for_task(
            "task-unused",
            "workspace-missing",
            key,
            Some(&agent)
        )
        .unwrap(),
        Some((owner, agent))
    );
    assert_eq!(
        app.review_verdict_recipient_for_task(
            "task-unused",
            "workspace-missing",
            key,
            Some(&legacy_agent)
        )
        .unwrap(),
        None
    );
    let legacy_key = &app.project(&legacy).unwrap().project_key;
    assert_eq!(
        app.review_verdict_recipient_for_task(
            "task-unused",
            "workspace-missing",
            legacy_key,
            Some(&legacy_agent)
        )
        .unwrap(),
        Some((legacy_owner, legacy_agent))
    );
}

#[test]
fn attachment_containment_stays_in_attachment_store() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, legacy) = tracked(root.path());
    let _sibling = register_sibling(&mut app, &legacy);
    let attachment_root = app.local_attachments_dir();
    std::fs::create_dir_all(&attachment_root).unwrap();
    let held = attachment_root.join("kept.txt");
    std::fs::write(&held, "attached").unwrap();
    assert_eq!(
        app.resolve_task_attachment(held.to_str().unwrap()).unwrap(),
        held
    );
    let outside = app
        .project(&legacy)
        .unwrap()
        .home_path
        .join("source-only.txt");
    std::fs::write(&outside, "source").unwrap();
    assert!(app
        .resolve_task_attachment(outside.to_str().unwrap())
        .is_err());
    assert!(app
        .resolve_task_attachment("../../source-only.txt")
        .is_err());
    #[cfg(unix)]
    {
        let escape = attachment_root.join("escape.txt");
        std::os::unix::fs::symlink(&outside, &escape).unwrap();
        assert!(app
            .resolve_task_attachment(escape.to_str().unwrap())
            .is_err());
    }
}

#[test]
fn failed_delivery_invalidates_uuid_keyed_project_only() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, legacy) = tracked(root.path());
    let sibling = register_sibling(&mut app, &legacy);
    let mut task = seeded(&app, &sibling, "failed target");
    task.assignee = Some(Assignee::Agent {
        agent_id: "agent-no-longer-present".into(),
    });
    let mut intent = intent_for(&task);
    intent.target = json!({"kind":"agent","agent_id":"agent-no-longer-present"});
    let intent = app
        .tracker_store()
        .unwrap()
        .accept_delivery_intent(&task, &[], intent)
        .unwrap();
    let changes = app.changes();
    let state = app.shared();
    let handler = AppState::handler(Arc::clone(&state));
    let (session, mut rx, key) = SessionSender::observable("failed-sibling-delivery");
    assert_eq!(
        handler.call(
            session.clone(),
            req("session.hello", json!({"changes":"subscriptions"}))
        )["ok"],
        true
    );
    assert_eq!(
        handler.call(
            session,
            req(
                "changes.subscribe",
                json!({
                    "subscription_id":"failed-delivery", "scope":{"kind":"all"}, "kinds":["tasks"]
                })
            )
        )["ok"],
        true
    );
    changes.flush();
    while rx.try_recv().is_ok() {}
    assert!(state
        .lock()
        .unwrap()
        .prepare_task_delivery()
        .unwrap()
        .is_none());
    changes.flush();
    let mut projects = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        let push = SessionSender::decrypt_push(&key, &envelope);
        if let Some(items) = push["items"].as_array() {
            projects.extend(
                items
                    .iter()
                    .filter(|item| item.get("tasks").is_some())
                    .map(|item| item["entity_id"].as_str().unwrap().to_string()),
            );
        }
    }
    assert_eq!(projects, vec![sibling]);
    assert_eq!(
        state
            .lock()
            .unwrap()
            .tracker_store()
            .unwrap()
            .load_delivery_intent(&intent.id)
            .unwrap()
            .unwrap()
            .state,
        "failed"
    );
}

#[test]
fn failed_verdict_delivery_invalidates_uuid_keyed_project_only() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, legacy) = tracked(root.path());
    let sibling = register_sibling(&mut app, &legacy);
    let now = crate::store::now_rfc3339();
    let delivery: crate::store::ReviewVerdictDelivery = serde_json::from_value(json!({
        "comment_id":"uuid-verdict", "task_id":"removed-task", "task_number":1,
        "project_path":app.project(&sibling).unwrap().project_key,
        "workspace_id":"removed-workspace", "snapshot_id":"uuid-revision", "revision":2,
        "verdict":"approve", "reason":"reviewed", "reviewer":{"kind":"user"}, "batch":[],
        "operation_id":"uuid-verdict-operation", "state":"pending", "recipient_agent_id":null,
        "attempts":0, "error":null, "created_at":now, "updated_at":now,
    }))
    .unwrap();
    app.tracker_store()
        .unwrap()
        .writer_for_test()
        .execute(
            "INSERT INTO review_verdict_deliveries VALUES(?1,?2,?3,'pending',?4,?4,?5)",
            rusqlite::params![
                delivery.comment_id,
                delivery.operation_id,
                delivery.task_id,
                now,
                serde_json::to_string(&delivery).unwrap()
            ],
        )
        .unwrap();
    let changes = app.changes();
    let state = app.shared();
    let handler = AppState::handler(Arc::clone(&state));
    let (session, mut rx, key) = SessionSender::observable("failed-sibling-verdict");
    assert_eq!(
        handler.call(
            session.clone(),
            req("session.hello", json!({"changes":"subscriptions"}))
        )["ok"],
        true
    );
    assert_eq!(
        handler.call(
            session,
            req(
                "changes.subscribe",
                json!({
                    "subscription_id":"failed-verdict", "scope":{"kind":"all"}, "kinds":["tasks"]
                })
            )
        )["ok"],
        true
    );
    changes.flush();
    while rx.try_recv().is_ok() {}
    state
        .lock()
        .unwrap()
        .drain_review_verdict_deliveries()
        .unwrap();
    changes.flush();
    let mut projects = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        let push = SessionSender::decrypt_push(&key, &envelope);
        if let Some(items) = push["items"].as_array() {
            projects.extend(
                items
                    .iter()
                    .filter(|item| item.get("tasks").is_some())
                    .map(|item| item["entity_id"].as_str().unwrap().to_string()),
            );
        }
    }
    assert_eq!(projects, vec![sibling]);
    assert_eq!(
        state
            .lock()
            .unwrap()
            .tracker_store()
            .unwrap()
            .load_review_verdict_delivery("uuid-verdict")
            .unwrap()
            .unwrap()
            .state,
        crate::store::ReviewVerdictDeliveryState::Failed
    );
}
