//! Review admission must refuse before obtaining SQL or external job inputs.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_review_deletion_admission_refuses_before_sql_capacity_or_external_work() {
    let fixture = Fixture::new();
    let actions = review_actions(&fixture);
    let pool = {
        let mut app = fixture.state.lock().unwrap();
        app.project_deletion_in_progress = true;
        app.publish_task_snapshot_projection();
        app.require_store().unwrap().read_pool()
    };
    let mut connections = Vec::new();
    for _ in 0..SQL_READERS {
        connections.push(pool.acquire().await);
    }
    sentry(&fixture);
    let before = fixture.state.lock().unwrap().read_view.load();
    let turns = fixture.held_turns().await;
    for action in actions {
        assert_eq!(
            fixture.reply(fixture.frame(action)).await,
            json!({"ok":false,"error":crate::app::rpc::PROJECT_DELETION_IN_PROGRESS})
        );
        assert_no_drain(&fixture, &before);
    }
    assert_eq!(turns.len(), 2);
    drop(connections);
}
