//! Acceptance tests exercise the running service without ChangeBus subscribers.
use super::reconcile::tests::Fixture;
use super::*;
use crate::reviews::records::Review;
use tokio::sync::mpsc;

struct Running {
    alive: Arc<AtomicBool>,
    handle: Option<ReviewSyncHandle>,
    invalidations: mpsc::UnboundedReceiver<SyncResult>,
}
impl Running {
    fn start(fixture: &Fixture) -> Self {
        let alive = Arc::new(AtomicBool::new(true));
        let living = alive.clone();
        let (sender, invalidations) = mpsc::unbounded_channel();
        let handle = spawn(
            fixture.store.clone(),
            Arc::new(move || living.load(Ordering::Acquire)),
            Arc::new(move |result| {
                let _ = sender.send(result);
            }),
        );
        Self {
            alive,
            handle: Some(handle),
            invalidations,
        }
    }
    async fn invalidated(&mut self, task: &str) {
        let result = tokio::time::timeout(Duration::from_secs(8), self.invalidations.recv())
            .await
            .expect("service reconciles without connected clients")
            .expect("service keeps its invalidation sender alive");
        assert_eq!(result.task_id, task);
        assert!(result.persisted);
    }
    async fn published(&mut self, fixture: &Fixture, expected_head: &str) -> Review {
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let result = self
                    .invalidations
                    .recv()
                    .await
                    .expect("service keeps its invalidation sender alive");
                assert_eq!(result.task_id, fixture.task_id());
                assert!(result.persisted);
                let saved = review(fixture).await;
                // Metadata-only invalidations may precede publication of the push.
                if saved.snapshots.last().unwrap().directories[0]
                    .head
                    .as_deref()
                    == Some(expected_head)
                {
                    return saved;
                }
            }
        })
        .await
        .expect("service publishes the expected head without connected clients")
    }
    async fn stop(&mut self) {
        self.alive.store(false, Ordering::Release);
        self.handle.take();
        // Let the short idle tick release metadata watchers before fixture cleanup.
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        self.handle.take();
    }
}
async fn fixture() -> Fixture {
    tokio::task::spawn_blocking(Fixture::new).await.unwrap()
}
async fn review(fixture: &Fixture) -> Review {
    let store = fixture.store.clone();
    let task = fixture.task_id().to_string();
    tokio::task::spawn_blocking(move || store.load_review(&task).unwrap().unwrap())
        .await
        .unwrap()
}
async fn push(fixture: Fixture, names: &'static [&'static str]) -> (Fixture, String) {
    tokio::task::spawn_blocking(move || {
        // Keep this fixture's reconciliation pending until the whole batch lands,
        // even when loaded test runners take longer than the debounce interval.
        let capture = (names.len() > 1).then(|| {
            super::reconcile::recovery::CaptureJournal::acquire_with_wait(
                fixture.task_id(),
                &fixture.opened.review.bindings,
                Duration::from_secs(2),
            )
            .unwrap()
        });
        let mut head = String::new();
        for name in names {
            head = fixture.commit(name);
            fixture.push();
        }
        drop(capture);
        (fixture, head)
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn startup_recovers_a_push_that_happened_before_service_registration() {
    let fixture = fixture().await;
    let (fixture, head) = push(fixture, &["before-startup.txt"]).await;
    let mut running = Running::start(&fixture);
    let saved = running.published(&fixture, &head).await;
    assert_eq!(saved.snapshots.len(), 2);
    assert_eq!(
        saved.snapshots[1].directories[0].head.as_deref(),
        Some(head.as_str())
    );
    running.stop().await;
}

#[tokio::test]
async fn terminal_pushes_publish_while_alive_and_a_burst_collapses_to_the_latest_head() {
    let fixture = fixture().await;
    let mut running = Running::start(&fixture);
    // The startup invalidation is also a barrier: metadata watchers are installed.
    running.invalidated(fixture.task_id()).await;
    let (fixture, head) = push(fixture, &["terminal-push.txt"]).await;
    let saved = running.published(&fixture, &head).await;
    assert_eq!(saved.snapshots.len(), 2);
    assert_eq!(
        saved.snapshots[1].directories[0].head.as_deref(),
        Some(head.as_str())
    );

    let (fixture, latest) = push(fixture, &["burst-first.txt", "burst-latest.txt"]).await;
    let saved = running.published(&fixture, &latest).await;
    assert_eq!(
        saved.snapshots.len(),
        3,
        "burst creates a single new snapshot"
    );
    assert_eq!(
        saved.snapshots[2].directories[0].head.as_deref(),
        Some(latest.as_str())
    );
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(review(&fixture).await.snapshots.len(), 3);
    running.stop().await;
}

#[tokio::test]
async fn dropping_service_handle_stops_future_push_admission() {
    let fixture = fixture().await;
    let mut running = Running::start(&fixture);
    running.invalidated(fixture.task_id()).await;
    // Keep the liveness condition true: this tests handle cancellation itself.
    running.handle.take();
    tokio::time::sleep(Duration::from_millis(250)).await;
    let (fixture, _) = push(fixture, &["after-stop.txt"]).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(review(&fixture).await.snapshots.len(), 1);
    assert!(running.invalidations.try_recv().is_err());
    running.stop().await;
}
