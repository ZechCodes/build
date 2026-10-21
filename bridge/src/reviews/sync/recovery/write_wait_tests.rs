use super::*;
use crate::timing::write_wait::deep_test_support::TrackedWrite;
use crate::timing::write_wait::native_retry::{assert_continuous, collect, Observer};

#[test]
fn sync_capture_retries_retain_one_wait_until_the_native_lease_is_acquired() {
    let root = tempfile::tempdir().unwrap();
    let holder = File::open(root.path()).unwrap();
    let waiter = File::open(root.path()).unwrap();
    holder.lock().unwrap();
    let tracked = TrackedWrite::new("sync-capture-retries");
    let context = tracked.context();
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _context = context.bind_sync();
        let _observer = Observer::bind(sender);
        let result = lock_capture(&waiter, "wait-control", Duration::from_secs(2));
        (waiter, result)
    });
    let attempts = collect(&receiver, 3);
    let released = holder.unlock();
    let (waiter, result) = worker.join().unwrap();
    let excluded = matches!(holder.try_lock(), Err(std::fs::TryLockError::WouldBlock));
    let ended = waiter.unlock();
    drop(waiter);
    let available = holder.try_lock();
    let _ = holder.unlock();
    assert!(released.is_ok() && ended.is_ok() && result.is_ok());
    assert!(excluded && available.is_ok());
    assert_continuous(&tracked, &attempts, "review.sync.capture");
    tracked.complete();
}

#[test]
fn sync_capture_retry_timeout_closes_wait_without_stealing_the_holder() {
    let root = tempfile::tempdir().unwrap();
    let holder = File::open(root.path()).unwrap();
    let waiter = File::open(root.path()).unwrap();
    holder.lock().unwrap();
    let tracked = TrackedWrite::new("sync-capture-timeout");
    let context = tracked.context();
    let (sender, receiver) = std::sync::mpsc::channel();
    let wait = Duration::from_millis(250);
    let started = Instant::now();
    let worker = std::thread::spawn(move || {
        let _context = context.bind_sync();
        let _observer = Observer::bind(sender);
        let result = lock_capture(&waiter, "timeout-control", wait);
        (waiter, result)
    });
    let (waiter, result) = worker.join().unwrap();
    let elapsed = started.elapsed();
    let still_excluded = matches!(waiter.try_lock(), Err(std::fs::TryLockError::WouldBlock));
    let released = holder.unlock();
    drop(holder);
    drop(waiter);
    let attempts = receiver.try_iter().collect::<Vec<_>>();
    assert_eq!(result, Err("busy: review sync capture".into()));
    assert!(elapsed >= wait, "caller-selected deadline was shortened");
    assert!(still_excluded && released.is_ok());
    assert_continuous(&tracked, &attempts, "review.sync.capture");
    tracked.complete();
}
