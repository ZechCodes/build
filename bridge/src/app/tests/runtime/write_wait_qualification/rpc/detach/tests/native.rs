//! Actual full read fixture, actual auxiliary term.close, actual receiver joins.
use super::*;
use crate::app::tests::runtime::write_wait_qualification::fixture::{
    Envelope, QualificationFixture,
};
use futures_util::FutureExt;
use std::panic::AssertUnwindSafe;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "actual full read fixture; run explicitly under nice -n 10"]
async fn quiet_trial_watch_detaches_before_native_full_read_cleanup() {
    crate::git_fixture::environment::isolated_git_test!();
    const READ: &str = "qual/detach/full/read";
    const WRITE: &str = "qual/detach/full/write";
    let mut fixture = QualificationFixture::ready(Envelope::Small).await;
    let ready = fixture
        .read_inputs
        .take()
        .expect("genuine full read fixture");
    let task = ready.witness.inputs.task.clone();
    let watch = ClientWatch::new(Arc::new(Instant::now));
    let due = Instant::now() + Duration::from_millis(10);
    watch.register_cohort_at(READ, due, TrafficCohort::Read);
    watch.register_at(WRITE, due);
    let guard = watch.watch();
    fixture.rpc.attach_client_watch(watch.clone());
    tokio::time::sleep_until(tokio::time::Instant::from_std(due)).await;
    watch.offer(READ);
    let read = fixture
        .rpc
        .request(0, READ, "tasks.get", json!({"task_id":task}))
        .await;
    watch.offer(WRITE);
    let write = fixture
        .rpc
        .request(
            1,
            WRITE,
            "tasks.update",
            json!({
                "task_id":task,"title":"Quiet detach native witness",
            }),
        )
        .await;
    let terminal = server_terminal(&fixture.rpc, WRITE).await;
    let report = guard.finish();
    let detached = detach_when_quiet(&fixture.rpc).await;
    // Always attempt actual read teardown; no synthetic enrollment for its
    // term.close and no replacement of its native close/reap assertions.
    let read_cleanup = AssertUnwindSafe(ready.shutdown(&fixture.rpc))
        .catch_unwind()
        .await;
    drop(ready);
    let cleanup = fixture.cleanup_owned_resources().await;
    let after_cleanup = std::panic::catch_unwind(AssertUnwindSafe(|| watch.reconcile_final()));

    eprintln!(
        "quiet_detach_control={{\"case\":\"native_full_read_cleanup\",\"qualification_successes\":0}}"
    );
    let audit = detached
        .expect("quiet originals must permit explicit audited detach")
        .expect("the actual trial watch was attached");
    assert!(read_cleanup.is_ok(), "native full read cleanup failed");
    assert!(cleanup.is_empty(), "{cleanup:?}");
    assert!(terminal && !report.panicked && !report.failed);
    assert_eq!(read.reply["ok"], true, "{}", read.reply);
    assert_eq!(write.reply["ok"], true, "{}", write.reply);
    assert!(audit.violations.is_empty(), "{:?}", audit.violations);
    assert_eq!(audit.rows.len(), 2);
    let write = audit
        .rows
        .iter()
        .find(|row| row.original_id == WRITE)
        .unwrap();
    assert!(write.server_registered_at.is_some() && write.admitted_at.is_some());
    let after = after_cleanup.expect("watch evidence survives auxiliary cleanup");
    assert!(after.violations.is_empty(), "{:?}", after.violations);
    assert_eq!(
        after.rows.len(),
        2,
        "auxiliary cleanup never enters the trial population"
    );
    for before in &audit.rows {
        let after = after
            .rows
            .iter()
            .find(|row| row.original_id == before.original_id)
            .unwrap();
        assert_eq!(before.final_at, after.final_at);
        assert_eq!(before.server_registered_at, after.server_registered_at);
        assert_eq!(before.admitted_at, after.admitted_at);
        assert!(!after.original_id.starts_with("qual/cleanup/"));
    }
}
