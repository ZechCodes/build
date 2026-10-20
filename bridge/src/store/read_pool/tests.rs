use super::*;
use crate::store::Store;
use std::time::Duration;

#[tokio::test]
async fn review_metadata_uses_one_readonly_snapshot_without_the_writer_connection() {
    let fixture = crate::reviews::sync::reconcile::tests::Fixture::new();
    fixture.sync();
    let review = fixture.review();
    let sync = fixture.store.load_review_sync_observations(&review.task_id).unwrap();
    let intents = fixture.store.load_review_merge_intents(&review.task_id).unwrap();
    let mut reader = fixture.store.read_pool().acquire().await;
    let _writer = fixture.store.connection();
    reader.read(|read| {
        assert_eq!(read.load_review(&review.task_id).unwrap(), Some(review.clone()));
        assert_eq!(read.load_review_sync_observations(&review.task_id).unwrap(), sync);
        assert_eq!(read.load_review_merge_intents(&review.task_id).unwrap(), intents);
    }).unwrap();
}

#[tokio::test]
async fn eight_readers_are_independent_and_the_ninth_waits_fairly() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let pool = store.read_pool();
    let mut borrowers = Vec::new();
    for _ in 0..READ_CONNECTIONS {
        borrowers.push(pool.acquire().await);
    }
    let first = pool.acquire();
    let second = pool.acquire();
    tokio::pin!(first, second);
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut first)
        .await
        .is_err());
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut second)
        .await
        .is_err());
    drop(borrowers.pop());
    let returned = tokio::time::timeout(Duration::from_secs(1), &mut first)
        .await
        .unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut second)
        .await
        .is_err());
    drop(returned);
    tokio::time::timeout(Duration::from_secs(1), &mut second)
        .await
        .unwrap();
}

#[tokio::test]
async fn read_only_queries_progress_while_the_writer_holds_an_uncommitted_change() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut reader = store.read_pool().acquire().await;
    let mut writer = store.connection();
    let write = writer
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    write
        .execute(
            "INSERT INTO meta (key, value) VALUES ('uncommitted', '1')",
            [],
        )
        .unwrap();
    reader
        .read(|read| {
            assert!(read.connection().is_readonly("main").unwrap());
            assert_eq!(
                read.connection()
                    .query_row("PRAGMA query_only", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                1
            );
            assert_eq!(
                read.connection()
                    .query_row(
                        "SELECT COUNT(*) FROM meta WHERE key = 'uncommitted'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                0
            );
            assert!(read
                .connection()
                .execute("INSERT INTO meta VALUES ('forbidden', '1')", [])
                .is_err());
            assert!(read
                .connection()
                .execute("CREATE TEMP TABLE forbidden (id)", [])
                .is_err());
        })
        .unwrap();
    write.commit().unwrap();
    reader
        .read(|read| {
            assert_eq!(
                read.connection()
                    .query_row(
                        "SELECT COUNT(*) FROM meta WHERE key = 'uncommitted'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                1
            )
        })
        .unwrap();
}

#[tokio::test]
async fn one_request_keeps_its_snapshot_and_the_next_request_gets_a_fresh_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let pool = store.read_pool();
    let mut reader = pool.acquire().await;
    reader
        .read(|read| {
            let count = || {
                read.connection()
                    .query_row("SELECT COUNT(*) FROM meta WHERE key = 'later'", [], |r| {
                        r.get::<_, i64>(0)
                    })
                    .unwrap()
            };
            assert_eq!(count(), 0);
            store
                .connection()
                .execute("INSERT INTO meta VALUES ('later', '1')", [])
                .unwrap();
            assert_eq!(count(), 0);
        })
        .unwrap();
    drop(reader);
    pool.acquire()
        .await
        .read(|read| {
            assert_eq!(
                read.connection()
                    .query_row("SELECT COUNT(*) FROM meta WHERE key = 'later'", [], |r| r
                        .get::<_, i64>(
                        0
                    ))
                    .unwrap(),
                1
            )
        })
        .unwrap();
}

#[tokio::test]
async fn cancellation_removes_the_waiter_without_losing_a_connection() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let pool = store.read_pool();
    let mut borrowers = Vec::new();
    for _ in 0..READ_CONNECTIONS {
        borrowers.push(pool.acquire().await);
    }
    {
        let cancelled = pool.acquire();
        tokio::pin!(cancelled);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut cancelled)
                .await
                .is_err()
        );
    }
    let next = pool.acquire();
    tokio::pin!(next);
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut next)
        .await
        .is_err());
    drop(borrowers.pop());
    let returned = tokio::time::timeout(Duration::from_secs(1), next)
        .await
        .unwrap();
    drop(returned);
    drop(borrowers);
    assert_eq!(pool.inner.permits.available_permits(), READ_CONNECTIONS);
}

#[tokio::test]
async fn unwinding_a_query_rolls_back_its_snapshot_before_the_connection_returns() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let pool = store.read_pool();
    let mut reader = pool.acquire().await;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        reader
            .read::<()>(|read| {
                read.connection()
                    .query_row("SELECT COUNT(*) FROM meta", [], |row| row.get::<_, i64>(0))
                    .unwrap();
                panic!("cancel request during query");
            })
            .unwrap();
    }));
    assert!(result.is_err());
    assert!(reader.connection.as_ref().unwrap().is_autocommit());
    drop(reader);
    assert_eq!(pool.inner.permits.available_permits(), READ_CONNECTIONS);
    pool.acquire()
        .await
        .read(|read| assert!(!read.connection().is_autocommit()))
        .unwrap();
}
