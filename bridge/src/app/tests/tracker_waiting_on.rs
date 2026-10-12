use super::tracker::tracked;
use crate::tracker::{Actor, Task, TaskEventKind, TimelineEntry};

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
    assert_eq!(
        store.waiting_on_summary(&project, &a.id).unwrap().blocked,
        false
    );
}
