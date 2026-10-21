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

#[test]
fn deep_source_sync_final_cleanup_retains_an_obligation_after_root_completion() {
    let directory = tempfile::tempdir().unwrap();
    let identity = sync_lock_identity(directory.path());
    let tracked = TrackedWrite::new("source-sync-post-root-cleanup");
    let registry = std::sync::Arc::clone(&tracked.registry);
    let returned = {
        let _scope = tracked.context().bind_sync();
        SyncLock::acquire(directory.path(), Duration::ZERO)
            .unwrap()
            .0
    };
    tracked.complete();
    let after_ack = registry.snapshot().remove(0);
    // A dead captured context must not strand this test's source identity.
    // Catch only its drop and remove that exact hold before any assertion.
    let cleanup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(returned)));
    if cleanup.is_err() {
        let (held, released) = held_checkouts();
        held.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&identity);
        released.notify_all();
    }
    let available = SyncLock::acquire(directory.path(), Duration::ZERO);
    let released = available.is_some();
    drop(available);
    let after_cleanup = registry.snapshot().remove(0);
    assert!(
        released,
        "the exact source hold was removed before assertions"
    );
    assert!(crate::timing::write_wait::current_context().is_none());
    assert!(cleanup.is_ok(), "cleanup reused a finished root context");
    assert!(after_ack.root_finished);
    assert_eq!(after_ack.outstanding.len(), 1, "cleanup owns a live child");
    assert!(after_cleanup.outstanding.is_empty() && after_cleanup.active.is_empty());
    assert!(!after_cleanup.abandoned && !after_cleanup.cancelled);
}
