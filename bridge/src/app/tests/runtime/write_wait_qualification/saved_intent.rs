//! A durable retry ACK must retain its original until the actual service finishes.
use super::rpc::RpcHarness;
use crate::app::AppState;
use crate::timing::write_wait::{
    IntervalCategory, WaitKind, WriteSnapshot, WriteWaitRegistry, WAIT_BUDGET,
};
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

mod fixture;

const ORIGINAL: &str = "qualification/saved-retry/original";
const SAFETY: Duration = Duration::from_secs(20);

struct HeldService {
    release: Option<mpsc::Sender<()>>,
    holder: Option<std::thread::JoinHandle<()>>,
    service: Option<std::thread::JoinHandle<Option<Result<bool, String>>>>,
}

impl HeldService {
    fn start(state: Arc<Mutex<AppState>>) -> (Self, mpsc::Receiver<()>, bool) {
        let (pulse, arrived) = mpsc::channel();
        let service_step = AppState::task_delivery_step_for_test(&state, pulse);
        let (held, ready) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let held_state = Arc::clone(&state);
        let holder = std::thread::spawn(move || {
            let _guard = held_state.lock().unwrap();
            let _ = held.send(());
            let _ = released.recv_timeout(SAFETY);
        });
        let holding = ready.recv_timeout(SAFETY).is_ok();
        let service = std::thread::spawn(service_step);
        (
            Self {
                release: Some(release),
                holder: Some(holder),
                service: Some(service),
            },
            arrived,
            holding,
        )
    }

    fn finish(&mut self) -> Option<Result<bool, String>> {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        let holder = self.holder.take().map(|holder| holder.join());
        let service = self.service.take().map(|service| service.join());
        // Both workers are joined even when either panics. No assertion can
        // strand the real service behind a mutex held by the test.
        if holder.is_some_and(|result| result.is_err()) {
            return Some(Err("test app holder panicked".into()));
        }
        service.and_then(|result| {
            result.unwrap_or_else(|_| Some(Err("delivery service panicked".into())))
        })
    }
}

impl Drop for HeldService {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

async fn original_after_root(registry: &WriteWaitRegistry) -> Option<WriteSnapshot> {
    let deadline = Instant::now() + WAIT_BUDGET;
    loop {
        if let Some(row) = registry
            .snapshot()
            .into_iter()
            .find(|row| row.original_id == ORIGINAL && row.root_finished)
        {
            return Some(row);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

async fn service_lock_started(registry: &WriteWaitRegistry) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if registry.active_snapshot().iter().any(|row| {
            row.original_id == ORIGINAL
                && row
                    .active
                    .iter()
                    .any(|span| span.category == IntervalCategory::KnownWait(WaitKind::AppLock))
        }) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logical_write_saved_retry_retains_original_before_service_admission() {
    let fixture = fixture::Fixture::new();
    let offset = Arc::new(AtomicU64::new(0));
    let now = Arc::clone(&offset);
    let registry = WriteWaitRegistry::with_clock(Arc::new(move || {
        Instant::now() + Duration::from_secs(now.load(Ordering::SeqCst))
    }));
    let rpc = RpcHarness::new(Arc::clone(&fixture.state), Arc::clone(&registry));
    let retained = fixture.state.lock().unwrap().read_view.load();
    let retained_board = retained.get_owned("board.list", "");
    let reply = rpc
        .request(
            0,
            ORIGINAL,
            "tasks.assign",
            json!({
                "task_id":fixture.intent.task_id,"retry_deferred_id":fixture.intent.id,
            }),
        )
        .await;
    let after_ack = original_after_root(&registry).await;
    let retry = fixture
        .state
        .lock()
        .unwrap()
        .tracker_store()
        .unwrap()
        .load_delivery_intent(&fixture.intent.id)
        .unwrap()
        .unwrap();
    let (reported, reports) = mpsc::channel();
    let watchdog = registry.watchdog(
        Duration::from_millis(5),
        Arc::new(move |row| {
            if row.original_id == ORIGINAL {
                let _ = reported.send(row);
            }
        }),
    );
    let (mut held, arrived, holding) = HeldService::start(Arc::clone(&fixture.state));
    let reached = arrived.recv_timeout(SAFETY).is_ok();
    // The pulse proves real service execution. Wait for the actual span before
    // advancing time: its eventual lock helper may open it just after the pulse.
    // An absent span is retained as failed evidence, never manufactured here.
    let lock_started = reached && service_lock_started(&registry).await;
    if reached {
        offset.store(6, Ordering::SeqCst);
    }
    let blocked = registry
        .active_snapshot()
        .into_iter()
        .find(|row| row.original_id == ORIGINAL);
    let report = reports.recv_timeout(Duration::from_secs(2)).ok();
    let service = held.finish();
    drop(watchdog);

    // A second genuine recovery scan must not admit the saved operation again.
    let state = Arc::clone(&fixture.state);
    let again = std::thread::spawn(move || {
        let (pulse, _arrived) = mpsc::channel();
        AppState::task_delivery_step_for_test(&state, pulse)()
    })
    .join();
    let settled = fixture
        .state
        .lock()
        .unwrap()
        .tracker_store()
        .unwrap()
        .load_delivery_intent(&fixture.intent.id)
        .unwrap()
        .unwrap();
    let receipt = fixture
        .state
        .lock()
        .unwrap()
        .tracker_store()
        .unwrap()
        .operation(&fixture.intent.operation_id)
        .unwrap();
    let task = rpc
        .request(
            0,
            "qualification/saved-retry/task",
            "tasks.get",
            json!({"task_id":fixture.intent.task_id}),
        )
        .await;
    let page = rpc
        .request(
            0,
            "qualification/saved-retry/history",
            "thread.page",
            json!({"entity_id":fixture.owner,"agent_id":fixture.agent,"limit":100}),
        )
        .await;
    let operation = rpc
        .request(
            0,
            "qualification/saved-retry/receipt",
            "thread.operation",
            json!({"entity_id":fixture.owner,"operation_id":fixture.intent.operation_id}),
        )
        .await;
    let completed = registry
        .snapshot()
        .into_iter()
        .find(|row| row.original_id == ORIGINAL);
    let queue_idle = fixture.state.lock().unwrap().delivery_queue.is_idle();
    fixture.warm.end();
    rpc.finish().await;

    // Real effects, publication and all owned cleanup precede the intended RED.
    assert_eq!(reply.reply["ok"], true, "{}", reply.reply);
    assert_eq!(
        (&retry.id, retry.generation, &retry.operation_id),
        (
            &fixture.intent.id,
            fixture.intent.generation,
            &fixture.intent.operation_id
        )
    );
    assert_eq!(retry.state, "queued");
    assert!(
        holding && reached,
        "actual service reached its first held acquisition"
    );
    assert_eq!(service, Some(Ok(true)));
    assert!(matches!(again, Ok(Some(Ok(false)))), "{again:?}");
    assert_eq!(settled.state, "delivered");
    assert_eq!(
        (&settled.id, settled.generation),
        (&fixture.intent.id, fixture.intent.generation)
    );
    assert_eq!(settled.operation_id, fixture.intent.operation_id);
    let receipt = receipt.expect("actual delivery admitted its durable receipt");
    assert_eq!(receipt.operation_id, fixture.intent.operation_id);
    assert_eq!(operation.reply["ok"], true, "{}", operation.reply);
    assert_eq!(operation.reply["result"], receipt.wire_value());
    assert_eq!(task.reply["ok"], true, "{}", task.reply);
    assert!(task.reply["result"]["task"]["deferred_assignment"].is_null());
    assert_eq!(
        task.reply["result"]["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["type"] == "event" && entry["kind"] == "dispatched")
            .count(),
        1
    );
    assert_eq!(page.reply["ok"], true, "{}", page.reply);
    let messages = page.reply["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| {
            item["type"] == "message"
                && item["data"]["operation_id"] == fixture.intent.operation_id
                && item["data"]["from_task"]["task_id"] == fixture.intent.task_id
        })
        .count();
    assert_eq!(messages, 1, "{}", page.reply);
    assert_eq!(
        fixture
            .warm
            .log
            .turns()
            .iter()
            .filter(|turn| turn.contains(&fixture.intent.operation_id))
            .count(),
        1
    );
    assert!(queue_idle && fixture.warm.log.ended());
    assert_eq!(fixture.warm.opens.load(Ordering::SeqCst), 0);
    assert!(retained_board.is_some());
    assert_eq!(retained.get_owned("board.list", ""), retained_board);

    let after_ack = after_ack.expect("the actual request root finished before service start");
    assert!(after_ack.acknowledged_at.is_some());
    assert!(
        !after_ack.outstanding.is_empty(),
        "the saved ready retry must retain its original after ACK: {after_ack:?}"
    );
    assert!(
        lock_started,
        "actual service AppLock interval began before fake-clock advancement"
    );
    let blocked = blocked.expect("the saved original survives until actual service admission");
    assert_eq!(blocked.id, after_ack.id);
    assert!(
        blocked
            .active
            .iter()
            .any(|span| span.category == IntervalCategory::KnownWait(WaitKind::AppLock)),
        "{blocked:?}"
    );
    assert!(
        blocked.known_wait_union > WAIT_BUDGET && blocked.breached(),
        "{blocked:?}"
    );
    let report = report.expect("independent watchdog reports the blocked original before release");
    assert_eq!(report.id, after_ack.id);
    let completed = completed.expect("terminal original remains in the archive");
    assert_eq!(completed.id, after_ack.id);
    assert_eq!(completed.intake, after_ack.intake);
    assert!(completed.root_finished && completed.completed_at.is_some());
    assert!(completed.outstanding.is_empty() && completed.active.is_empty());
    assert!(completed.breached() && !completed.cancelled && !completed.abandoned);
}
