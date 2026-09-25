//! The app mutex is a blocking lock, and no runtime worker waits on it.

/// Run `section` — work that takes the app mutex — on the runtime's blocking
/// pool, and wait for it without holding a worker.
///
/// A worker parked on the lock behind a slow frame is a worker the I/O driver
/// cannot run on, and with every worker parked the data channels stall, the
/// terminals stop painting and `bridge.stats` cannot answer (issue #131). A
/// panic in `section` resumes here, as it would have had the section run on
/// the caller's task.
pub(in crate::app) async fn off_the_workers<T: Send + 'static>(
    section: impl FnOnce() -> T + Send + 'static,
) -> T {
    match tokio::task::spawn_blocking(section).await {
        Ok(value) => value,
        Err(joined) => match joined.try_into_panic() {
            Ok(panic) => std::panic::resume_unwind(panic),
            Err(cancelled) => panic!("the runtime shut down under a locked section: {cancelled}"),
        },
    }
}
