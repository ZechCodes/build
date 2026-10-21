use super::fixture::SavedFixture;
use super::task_control::{assert_task_witnesses, task_requests};
use crate::app::accepted_records::AcceptedRecords;
use crate::app::tests::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_task_writes_capture_changed_and_native_noop_records_per_original() {
    isolated_git_test!(concurrent_task_writes_capture_changed_and_native_noop_records_per_original);
    let records = AcceptedRecords::new();
    let fixture = SavedFixture::new(Arc::clone(&records));
    let store = fixture.store.clone();
    let baseline = store.load_tracker_task(&fixture.task).unwrap().unwrap();
    let project = fixture.project.clone();
    let caller = fixture.caller.clone();
    let replies = fixture.compete(task_requests(&fixture.task)).await;
    let cleanup = fixture.finish().await;
    assert!(cleanup.is_ok(), "saved-record fixture cleanup: {cleanup:?}");
    let witnessed = assert_task_witnesses(baseline, &project, &caller, replies, &records, &store);
    assert_eq!(witnessed.len(), 4);
}
