use super::*;
use crate::timing::write_wait::deep_test_support::TrackedWrite;
use crate::timing::write_wait::{current_context, IntervalCategory, WaitKind};
use std::time::Duration;

#[test]
fn deep_store_mutex_is_observed_before_sql_service_starts() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let tracked = TrackedWrite::new("store-mutex");
    let held = store.writer_for_test();
    let context = tracked.context();
    let worker_store = store.clone();
    let worker = std::thread::spawn(move || {
        let _scope = context.bind_sync();
        worker_store
            .load_user_session()
            .map(|session| session.is_none())
    });
    let observed = tracked.wait_for(IntervalCategory::KnownWait(WaitKind::StoreWriter));
    let waiting = tracked.row();
    drop(held);
    let result = worker.join().unwrap();
    // No test assertion is permitted before releasing the real writer guard.
    assert!(result.unwrap());
    assert!(
        observed,
        "real Store mutex wait was not visible while active"
    );
    assert_eq!(waiting.sql_service_union, Duration::ZERO);
    assert!(tracked.row().known_wait_union > Duration::ZERO);
    assert!(tracked.row().sql_service_union > Duration::ZERO);
    assert!(tracked.row().active.is_empty());
    tracked.complete();
}

#[test]
fn deep_store_opaque_sql_service_is_separate_from_mutex_wait() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let timeout: u64 = store
        .writer_for_test()
        .pragma_query_value(None, "busy_timeout", |row| row.get(0))
        .unwrap();
    assert_eq!(
        timeout, 5000,
        "qualification must preserve production SQLite timeout"
    );
    let other = rusqlite::Connection::open(directory.path().join(DB_FILE)).unwrap();
    let tracked = TrackedWrite::new("sqlite-service-fault");
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    let context = tracked.context();
    let worker_store = store.clone();
    let (done, completed) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _scope = context.bind_sync();
        let result = worker_store
            .save_user_session(&crate::session_summary::UserSession::default().updated(722));
        let _ = done.send(());
        result
    });
    let observed = tracked.wait_for(IntervalCategory::SqlService);
    let waiting = tracked.row();
    // Keep the independent writer held until SQLite proves actual contention.
    // This is an intentional five-second service fault, not a supported-load sample.
    let completed_while_held = completed.recv_timeout(Duration::from_secs(7));
    let rollback = other.execute_batch("ROLLBACK");
    drop(other);
    let result = worker.join().unwrap();
    assert!(rollback.is_ok());
    assert!(
        completed_while_held.is_ok(),
        "SQLite's unchanged busy timeout must finish the owned worker"
    );
    assert!(
        matches!(result, Err(StoreError::Db(rusqlite::Error::SqliteFailure(error, _)))
        if error.code == rusqlite::ErrorCode::DatabaseBusy)
    );
    assert!(
        observed,
        "opaque SQLite service bound missing while the real writer is held"
    );
    assert!(waiting
        .active
        .iter()
        .all(|entry| entry.category != IntervalCategory::KnownWait(WaitKind::StoreWriter)));
    assert!(tracked.row().sql_service_union > Duration::ZERO);
    assert!(tracked.row().active.is_empty());
    store
        .save_user_session(&crate::session_summary::UserSession::default().updated(722))
        .unwrap();
    assert_eq!(
        store.load_user_session().unwrap().unwrap().last_activity_ms,
        Some(722)
    );
    tracked.complete();
}

#[test]
fn deep_store_poison_errors_and_untracked_calls_leave_no_intervals() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let tracked = TrackedWrite::new("store-unwind");
    let context = tracked.context();
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _scope = context.bind_sync();
        let _connection = store.connection();
        panic!("exercise actual Store guard unwinding");
    }));
    assert!(failed.is_err());
    assert!(current_context().is_none());
    assert!(tracked.row().active.is_empty());
    {
        let _scope = context.bind_sync();
        assert!(store.connection().execute_batch("not valid SQL").is_err());
        assert!(store.load_user_session().unwrap().is_none());
    }
    assert!(current_context().is_none());
    assert!(tracked.row().active.is_empty());
    let previous = tracked.row();
    assert!(store.load_user_session().unwrap().is_none());
    assert_eq!(
        tracked.row(),
        previous,
        "unattributed Store use must not inherit an origin"
    );
    tracked.complete();
}
