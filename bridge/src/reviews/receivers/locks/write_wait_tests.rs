use super::*;
use crate::timing::write_wait::deep_test_support::TrackedWrite;
use crate::timing::write_wait::{IntervalCategory, WaitKind};

#[cfg(unix)]
#[test]
fn deep_native_flock_wait_is_visible_before_directory_release() {
    use std::os::fd::AsRawFd;
    let directory = tempfile::tempdir().unwrap();
    // Separate open-file descriptions are essential: try_clone would share ownership.
    let holder = File::open(directory.path()).unwrap();
    let waiter = File::open(directory.path()).unwrap();
    let tracked = TrackedWrite::new("native-directory-flock");
    assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
    let context = tracked.context();
    let worker = std::thread::spawn(move || {
        let _scope = context.bind_sync();
        lock_recovery_directory(&waiter)
    });
    let observed = tracked.wait_for(IntervalCategory::KnownWait(WaitKind::ResourceLock));
    let unlocked = unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_UN) };
    drop(holder);
    let joined = worker.join().unwrap();
    assert_eq!(unlocked, 0);
    assert!(joined.is_ok());
    assert!(
        observed,
        "actual kernel flock contention had no live interval"
    );
    assert!(tracked.row().active.is_empty());
    tracked.complete();
}
