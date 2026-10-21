//! Test-only acquisition accounting. Production lock expressions remain unchanged.
use super::WaitKind;
use std::sync::{LockResult, Mutex, MutexGuard};

pub(crate) fn acquire_mutex<T>(mutex: &Mutex<T>, _kind: WaitKind) -> LockResult<MutexGuard<'_, T>> {
    // Test-first passthrough: attribution is added after the real contention
    // controls demonstrate that uninstrumented acquisition is invisible.
    mutex.lock()
}

/// Run a real leaf operation on this thread while another thread holds its
/// actual mutex. The registry clock advances only after the waiter starts;
/// collection and release remain independent of the blocked operation.
pub(crate) fn assert_blocked_acquisition<T: Send>(
    mutex: &Mutex<T>,
    original_id: &str,
    operation: impl FnOnce(),
) {
    use super::{IntervalCategory, WriteWaitRegistry};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::{Duration, Instant};

    let base = Instant::now();
    let seconds = Arc::new(AtomicU64::new(0));
    let clock_seconds = Arc::clone(&seconds);
    let registry = WriteWaitRegistry::with_clock(Arc::new(move || {
        base + Duration::from_secs(clock_seconds.load(Ordering::Acquire))
    }));
    let write = registry.begin(original_id, "publication.leaf", base);
    let id = write.id();
    let context = write.context();
    let (ready, ready_rx) = mpsc::channel();
    let (started, started_rx) = mpsc::channel();
    let (breach, breach_rx) = mpsc::channel();
    let watchdog = registry.watchdog(
        Duration::from_millis(1),
        Arc::new(move |row| {
            let _ = breach.send(row);
        }),
    );
    let (active, reported) = std::thread::scope(|scope| {
        let registry = &registry;
        let seconds = &seconds;
        let blocker = scope.spawn(move || {
            let held = mutex.lock().unwrap();
            ready.send(()).unwrap();
            let started = started_rx.recv_timeout(Duration::from_secs(5));
            let deadline = Instant::now() + Duration::from_secs(1);
            while started.is_ok() && Instant::now() < deadline {
                if registry
                    .active_snapshot()
                    .iter()
                    .any(|row| row.id == id && !row.active.is_empty())
                {
                    break;
                }
                std::thread::yield_now();
            }
            seconds.store(6, Ordering::Release);
            let active = registry.active_snapshot();
            let reported = breach_rx.recv_timeout(Duration::from_secs(1)).ok();
            drop(held);
            (active, reported)
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let binding = context.bind_sync();
        started.send(()).unwrap();
        operation();
        drop(binding);
        blocker.join().unwrap()
    });
    // All actual blockers have been released and joined before any assertion.
    seconds.store(16, Ordering::Release);
    write.complete();
    drop(watchdog);
    let active = active.iter().find(|row| row.id == id).unwrap();
    assert_eq!(active.original_id, original_id);
    assert_eq!(active.active.len(), 1, "{original_id}: missing leaf wait");
    assert_eq!(
        active.active[0].category,
        IntervalCategory::KnownWait(WaitKind::ResourceLock)
    );
    assert_eq!(active.known_wait_union, Duration::from_secs(6));
    assert!(active.interval_breached && active.cumulative_breached);
    assert_eq!(reported.unwrap().id, id, "watchdog missed {original_id}");
    let finished = registry
        .snapshot()
        .into_iter()
        .find(|row| row.id == id)
        .unwrap();
    assert!(finished.root_finished && finished.active.is_empty());
    assert_eq!(
        finished.known_wait_union,
        Duration::from_secs(6),
        "{original_id}: mutex hold or later work was charged as acquisition"
    );
}

#[test]
fn mutex_acquisition_records_real_blocking_and_excludes_guard_hold() {
    let mutex = Mutex::new(0);
    assert_blocked_acquisition(&mutex, "acquire-real-mutex", || {
        let mut held = acquire_mutex(&mutex, WaitKind::ResourceLock).unwrap();
        *held += 1;
        let context = super::current_context().unwrap();
        let row = context.registry.active_snapshot().remove(0);
        assert!(
            row.active.is_empty(),
            "acquisition remained open while holding the mutex"
        );
        drop(held);
    });
    assert_eq!(*mutex.lock().unwrap(), 1);
}

#[test]
fn mutex_acquisition_preserves_poison_and_the_original_guard() {
    let mutex = Mutex::new(1);
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            let mut held = mutex.lock().unwrap();
            *held = 2;
            panic!("poison the actual leaf mutex");
        });
        assert!(worker.join().is_err());
    });
    let poisoned = acquire_mutex(&mutex, WaitKind::ResourceLock).unwrap_err();
    assert_eq!(*poisoned.into_inner(), 2);
    assert!(mutex.is_poisoned());
}
