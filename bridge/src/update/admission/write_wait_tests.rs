use super::*;
use crate::timing::write_wait::deep_test_support::TrackedWrite;
use crate::timing::write_wait::{current_context, IntervalCategory, WaitKind};
use std::time::{Duration, Instant};

#[test]
fn deep_admission_mutex_and_final_lease_drop_keep_original_context() {
    let gate = AdmissionGate::new(false);
    let tracked = TrackedWrite::new("admission-mutex");
    let held = gate.state.lock().unwrap();
    let worker_gate = Arc::clone(&gate);
    let context = tracked.context();
    let worker = std::thread::spawn(move || {
        let _scope = context.bind_sync();
        worker_gate.try_enter()
    });
    let entering = tracked.wait_for(IntervalCategory::KnownWait(WaitKind::Admission));
    drop(held);
    let lease = worker.join().unwrap();
    let admitted = lease.is_some();
    // Cleanup must also be attributed after the original sync binding ends.
    let held = gate.state.lock().unwrap();
    let dropper = std::thread::spawn(move || drop(lease));
    let dropping = tracked.wait_for(IntervalCategory::KnownWait(WaitKind::Admission));
    drop(held);
    dropper.join().unwrap();
    assert!(admitted);
    assert!(entering, "actual admission state mutex wait missing");
    assert!(
        dropping,
        "last lease's raw state lock lost the original cause"
    );
    assert_eq!(gate.state.lock().unwrap().active, 0);
    assert!(tracked.row().active.is_empty());
    tracked.complete();
}

async fn wait_for_notify(tracked: &TrackedWrite) -> bool {
    let deadline = Instant::now() + Duration::from_millis(750);
    while Instant::now() < deadline {
        if tracked
            .row()
            .active
            .iter()
            .any(|entry| entry.category == IntervalCategory::KnownWait(WaitKind::Admission))
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    false
}

#[tokio::test(flavor = "current_thread")]
async fn deep_admission_notify_uses_explicit_context_without_tls_across_await() {
    let directory = tempfile::tempdir().unwrap();
    let store = crate::store::Store::new(directory.path()).unwrap();
    let gate = AdmissionGate::new(true);
    let tracked = TrackedWrite::new("admission-notify");
    let worker_gate = Arc::clone(&gate);
    let context = tracked.context();
    let worker = tokio::spawn(async move {
        worker_gate
            .enter_when_open_with_context(Some(context))
            .await
    });
    let observed = wait_for_notify(&tracked).await;
    let before = tracked.row().active;
    let clean_tls = current_context().is_none();
    let unrelated = store.load_user_session();
    let after = tracked.row().active;
    gate.reopen();
    let joined = worker.await;
    let joined_ok = joined.is_ok();
    drop(joined);
    assert!(unrelated.unwrap().is_none());
    assert!(joined_ok);
    assert!(observed, "closed gate's actual Notify wait was not visible");
    assert!(
        clean_tls,
        "a pending async admission leaked its synchronous origin"
    );
    assert_eq!(
        after, before,
        "unattributed same-thread Store work acquired an origin"
    );
    assert_eq!(gate.state.lock().unwrap().active, 0);
    assert!(tracked.row().active.is_empty());
    tracked.complete();
}
