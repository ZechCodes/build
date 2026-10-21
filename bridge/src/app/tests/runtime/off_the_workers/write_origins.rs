//! Cancellation of an async observer cannot retire its still-running worker.
use super::*;
use crate::timing::write_wait::{current_context, IntervalCategory, WriteWaitRegistry};
use std::time::Instant;

struct HeldDecide {
    gate: crate::app::OffLockGate,
    finished: tokio::sync::oneshot::Sender<()>,
}
impl OffLockJob for HeldDecide {
    type Claim = ();
    type Decided = ();
    fn claim(&mut self) {}
    fn decide(self) {
        self.gate.arrive();
        let _ = self.finished.send(());
    }
    fn apply(_: &mut AppState, _: (), _: ()) {}
    fn abandon(_: &mut AppState, _: ()) {}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn logical_write_cancelled_off_lock_waiter_retains_running_decide_child() {
    let dir = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(AppState::new_unrooted(
        dir.path(),
        "main",
        true,
        "unused",
    )));
    let registry = WriteWaitRegistry::new();
    let original = registry.begin("cancelled-decide", "off-lock-control", Instant::now());
    let (gate, held) = crate::app::OffLockGate::new();
    let (finished, done) = tokio::sync::oneshot::channel();
    let task = {
        let _binding = original.context().bind_sync();
        assert!(current_context().is_some());
        spawn_off_lock(state, HeldDecide { gate, finished })
            .ok()
            .unwrap()
    };
    held.wait_for_arrival();
    original.complete();
    task.abort();
    let cancelled = task.await.is_err_and(|error| error.is_cancelled());
    let during = registry
        .snapshot()
        .into_iter()
        .find(|row| row.original_id == "cancelled-decide")
        .unwrap();
    held.release();
    let worker_finished = tokio::time::timeout(Duration::from_secs(5), done)
        .await
        .is_ok_and(|done| done.is_ok());
    // Wait for the actual worker's external span to close, not just the
    // async caller's cancelled JoinHandle.
    let deadline = Instant::now() + Duration::from_secs(5);
    let worker_closed = loop {
        let closed = registry
            .snapshot()
            .iter()
            .find(|row| row.original_id == "cancelled-decide")
            .is_some_and(|row| row.active.is_empty());
        if closed || Instant::now() >= deadline {
            break closed;
        }
        tokio::task::yield_now().await;
    };
    // All holders and both actual stages have ended before assertions.
    assert!(cancelled && worker_finished && worker_closed);
    assert!(during.root_finished);
    assert!(
        during
            .active
            .iter()
            .any(|interval| interval.category == IntervalCategory::OwnExternal),
        "actual decide phase: {during:?}"
    );
    assert!(
        !during.outstanding.is_empty(),
        "a running blocking worker must own its origin after async cancellation: {during:?}"
    );
}
