//! The app mutex is a blocking lock, and no runtime worker waits on it.

#[cfg(test)]
type SectionObserver = tokio::sync::mpsc::UnboundedSender<std::thread::ThreadId>;

#[cfg(test)]
fn section_observers(
) -> &'static std::sync::Mutex<std::collections::HashMap<tokio::runtime::Id, SectionObserver>> {
    static OBSERVERS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<tokio::runtime::Id, SectionObserver>>,
    > = std::sync::OnceLock::new();
    OBSERVERS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[cfg(test)]
pub(in crate::app) struct SectionObservation(tokio::runtime::Id);

#[cfg(test)]
impl Drop for SectionObservation {
    fn drop(&mut self) {
        section_observers().lock().unwrap().remove(&self.0);
    }
}

/// Observe the blocking-pool thread entering a section on this test's runtime.
#[cfg(test)]
pub(in crate::app) fn observe_sections_for_test() -> (
    SectionObservation,
    tokio::sync::mpsc::UnboundedReceiver<std::thread::ThreadId>,
) {
    let runtime = tokio::runtime::Handle::current().id();
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    assert!(
        section_observers()
            .lock()
            .unwrap()
            .insert(runtime, sender)
            .is_none(),
        "only one section observer per test runtime"
    );
    (SectionObservation(runtime), receiver)
}

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
    #[cfg(test)]
    let runtime = tokio::runtime::Handle::current().id();
    let observed_section = move || {
        #[cfg(test)]
        if let Some(observer) = section_observers().lock().unwrap().get(&runtime).cloned() {
            let _ = observer.send(std::thread::current().id());
        }
        section()
    };
    match tokio::task::spawn_blocking(observed_section).await {
        Ok(value) => value,
        Err(joined) => match joined.try_into_panic() {
            Ok(panic) => std::panic::resume_unwind(panic),
            Err(cancelled) => panic!("the runtime shut down under a locked section: {cancelled}"),
        },
    }
}
