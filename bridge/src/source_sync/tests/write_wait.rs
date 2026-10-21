use super::*;
use crate::timing::write_wait::deep_test_support::TrackedWrite;
use crate::timing::write_wait::{IntervalCategory, WaitKind};

#[test]
fn deep_source_sync_condvar_timeout_and_drop_keep_resource_waits() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().to_path_buf();
    let (held, _) = SyncLock::acquire(&path, Duration::ZERO).unwrap();
    let tracked = TrackedWrite::new("source-sync");
    let context = tracked.context();
    let worker_path = path.clone();
    let worker = std::thread::spawn(move || {
        let _scope = context.bind_sync();
        SyncLock::acquire(&worker_path, Duration::from_secs(2))
    });
    let entering = tracked.wait_for(IntervalCategory::KnownWait(WaitKind::ResourceLock));
    drop(held);
    let acquired = worker.join().unwrap();
    let (returned, waited) = acquired.unwrap();
    let (held_paths, _) = held_checkouts();
    let held_state = held_paths
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dropper = std::thread::spawn(move || drop(returned));
    let dropping = tracked.wait_for(IntervalCategory::KnownWait(WaitKind::ResourceLock));
    drop(held_state);
    dropper.join().unwrap();
    let (held, _) = SyncLock::acquire(&path, Duration::ZERO).unwrap();
    let refused = {
        let _scope = tracked.context().bind_sync();
        SyncLock::acquire(&path, Duration::ZERO).is_none()
    };
    drop(held);
    assert!(
        waited && entering,
        "real source exclusion must pass through the Condvar"
    );
    assert!(
        dropping,
        "source exclusion cleanup lost its original context"
    );
    assert!(
        refused,
        "zero-deadline contention behavior must remain unchanged"
    );
    assert!(tracked.row().active.is_empty());
    let fresh = SyncLock::acquire(&path, Duration::ZERO);
    assert!(
        fresh.is_some(),
        "cleanup released the exact source identity"
    );
    drop(fresh);
    tracked.complete();
}
