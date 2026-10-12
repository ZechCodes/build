use super::tracker::tracked;
use super::{req, AppState};
use crate::tracker::{Actor, Task, TaskEventKind, TimelineEntry};

fn setup(app: &AppState, project: &str, title: &str) -> Task {
    app.tracker_store()
        .unwrap()
        .create_tracker_task(
            Task::drafted(project, title, Actor::User, &crate::store::now_rfc3339()),
            &[],
        )
        .unwrap()
}

fn wait(app: &AppState, project: &str, task: &Task, blocker: &Task) {
    app.tracker_store()
        .unwrap()
        .edit_waiting_on(
            project,
            &task.id,
            std::slice::from_ref(&blocker.id),
            &[],
            &Actor::User,
            &crate::store::now_rfc3339(),
        )
        .unwrap();
    drain(app);
}

fn drain(app: &AppState) {
    while app
        .tracker_store()
        .unwrap()
        .reconcile_waiting_on(2)
        .unwrap()
        .pending
    {}
}

#[test]
fn waiting_on_manual_review_completion_and_workspace_finish_share_transitions() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut app, project_id) = tracked(tmp.path());
    let project = app.tracker_project_path(&project_id).unwrap();
    let dependent = setup(&app, &project, "manual dependent");
    let blocker = setup(&app, &project, "manual blocker");
    wait(&app, &project, &dependent, &blocker);
    let moved = app.handle(req(
        "tasks.update",
        serde_json::json!({"task_id":blocker.id,"status":"done"}),
    ));
    assert_eq!(moved["ok"], true, "{moved}");
    drain(&app);
    assert!(
        !app.tracker_store()
            .unwrap()
            .waiting_on_summary(&project, &dependent.id)
            .unwrap()
            .blocked
    );

    let reviewed = setup(&app, &project, "reviewed prerequisite");
    app.tracker_store()
        .unwrap()
        .save_review_snapshot(
            &reviewed.id,
            "ws-test",
            0,
            crate::reviews::model::ReviewSnapshot {
                publication: None,
                id: "rs-test".into(),
                number: 0,
                created_at: crate::store::now_rfc3339(),
                author: Actor::User,
                directories: Vec::new(),
            },
        )
        .unwrap();
    let dependent = setup(&app, &project, "review dependent");
    wait(&app, &project, &dependent, &reviewed);
    app.tracker_store()
        .unwrap()
        .complete_review(&reviewed.id, 1, &Actor::User, "accepted review")
        .unwrap();
    drain(&app);
    let summary = app
        .tracker_store()
        .unwrap()
        .waiting_on_summary(&project, &dependent.id)
        .unwrap();
    assert!(!summary.blocked);
    assert_eq!(summary.waiting_on[0].resolved_by.as_deref(), Some("done"));

    let mut finished = setup(&app, &project, "workspace prerequisite");
    finished.links.workspace_ids.push("ws-finished".into());
    app.tracker_store()
        .unwrap()
        .save_tracker_task_activity(&finished, &[], &[])
        .unwrap();
    let dependent = setup(&app, &project, "workspace dependent");
    wait(&app, &project, &dependent, &finished);
    app.close_tasks_of_finished_workspace(&project_id, "ws-finished");
    drain(&app);
    let summary = app
        .tracker_store()
        .unwrap()
        .waiting_on_summary(&project, &dependent.id)
        .unwrap();
    assert!(!summary.blocked);
    assert_eq!(summary.waiting_on[0].resolved_by.as_deref(), Some("close"));
}

#[tokio::test]
async fn waiting_on_reconciles_at_startup_without_a_client() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, app, project_id) = tracked(tmp.path());
    let project = app.tracker_project_path(&project_id).unwrap();
    let store = app.tracker_store().unwrap().clone();
    let now = crate::store::now_rfc3339();
    let a = store
        .create_tracker_task(Task::drafted(&project, "dependent", Actor::User, &now), &[])
        .unwrap();
    let mut b = store
        .create_tracker_task(Task::drafted(&project, "blocker", Actor::User, &now), &[])
        .unwrap();
    store
        .edit_waiting_on(&project, &a.id, &[b.id.clone()], &[], &Actor::User, &now)
        .unwrap();
    b.status = "done".into();
    store.save_tracker_task_activity(&b, &[], &[]).unwrap();
    let _shared = app.shared();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if store
                .load_tracker_timeline(&a.id)
                .unwrap()
                .iter()
                .any(|e| matches!(e, TimelineEntry::Event(e) if e.kind == TaskEventKind::Unblocked))
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the service reconciles durable startup work without a client");
    assert!(!store.waiting_on_summary(&project, &a.id).unwrap().blocked);
}
