use super::*;
use crate::timing::write_wait::deep_test_support::TrackedWrite;
use crate::timing::write_wait::native_retry::{assert_continuous, collect, Observer};
use std::fs::{File, TryLockError};

#[test]
fn opening_journal_retries_retain_one_wait_until_the_native_lease_is_acquired() {
    let (home, source) = init_repo();
    let request = request(home.path(), &source);
    let holder = journal::lock(&request).unwrap();
    let path = request.receiver_root.join("operations/opening.lock");
    let tracked = TrackedWrite::new("opening-journal-retries");
    let context = tracked.context();
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _context = context.bind_sync();
        let _observer = Observer::bind(sender);
        journal::lock(&request)
    });
    let attempts = collect(&receiver, 3);
    let released = holder.unlock();
    drop(holder);
    let acquired = worker.join().unwrap().unwrap();
    let competitor = File::open(&path).unwrap();
    let excluded = matches!(competitor.try_lock(), Err(TryLockError::WouldBlock));
    let ended = acquired.unlock();
    drop(acquired);
    let available = competitor.try_lock();
    let _ = competitor.unlock();
    assert!(released.is_ok() && ended.is_ok());
    assert!(
        excluded && available.is_ok(),
        "returned file lost native lease ownership"
    );
    assert_continuous(&tracked, &attempts, "review.opening.journal");
    tracked.complete();
}
