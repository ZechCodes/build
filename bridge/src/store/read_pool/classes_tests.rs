use super::*;
use std::time::Duration;

#[tokio::test]
async fn cancelled_class_waiter_leaves_both_shared_permits_reusable() {
    let root = tempfile::tempdir().unwrap();
    let store = super::super::Store::new(root.path()).unwrap();
    let pool = store.read_pool();
    let clone = store.read_pool();
    let first = pool.acquire_class(ReadClass::TaskDetails).await;
    let second = clone.acquire_class(ReadClass::TaskDetails).await;
    {
        let cancelled = clone.acquire_class(ReadClass::TaskDetails);
        tokio::pin!(cancelled);
        assert!(futures_util::poll!(cancelled.as_mut()).is_pending());
        // A saturated class has no effect on a different class.
        let different = pool.acquire_class(ReadClass::Comment);
        tokio::pin!(different);
        assert!(futures_util::poll!(different.as_mut()).is_ready());
    }
    drop((first, second));
    let replacements = tokio::time::timeout(Duration::from_secs(1), async {
        (
            pool.acquire_class(ReadClass::TaskDetails).await,
            clone.acquire_class(ReadClass::TaskDetails).await,
        )
    })
    .await
    .expect("cancelled waiter retained a class permit");
    let excess = pool.acquire_class(ReadClass::TaskDetails);
    tokio::pin!(excess);
    assert!(futures_util::poll!(excess.as_mut()).is_pending());
    drop(replacements);
}
