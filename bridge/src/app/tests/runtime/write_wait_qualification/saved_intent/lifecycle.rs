//! Native saved-assignment lifecycle accounting. This is a fault/contract
//! cohort; no cancelled, refused, interrupted or injected-stall sample is a
//! successful write-wait qualification sample.
use super::{fixture::Fixture, HeldService, SAFETY};
use crate::app::tests::runtime::write_wait_qualification::rpc::RpcHarness;
use crate::app::AppState;
use crate::timing::write_wait::{
    IntervalCategory, WaitKind, WriteCohort, WriteSnapshot, WriteWaitRegistry, WAIT_BUDGET,
};
use futures_util::FutureExt;
use serde_json::{json, Value};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

mod publication;

fn clock() -> (Arc<WriteWaitRegistry>, Arc<AtomicU64>) {
    let offset = Arc::new(AtomicU64::new(0));
    let now = Arc::clone(&offset);
    let registry = WriteWaitRegistry::with_clock(Arc::new(move || {
        Instant::now() + Duration::from_secs(now.load(Ordering::SeqCst))
    }));
    (registry, offset)
}

async fn row(
    registry: &WriteWaitRegistry,
    original: &str,
    terminal: bool,
) -> Option<WriteSnapshot> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let found = registry
            .snapshot()
            .into_iter()
            .find(|row| row.original_id == original);
        if found
            .as_ref()
            .is_some_and(|row| row.root_finished && (!terminal || row.completed_at.is_some()))
            || Instant::now() >= deadline
        {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn assert_finished(row: &WriteSnapshot, cancelled: bool) {
    assert!(row.root_finished && row.completed_at.is_some(), "{row:?}");
    assert!(
        row.outstanding.is_empty() && row.active.is_empty(),
        "{row:?}"
    );
    assert_eq!(row.cancelled, cancelled, "{row:?}");
    assert!(!row.abandoned && !row.breached(), "{row:?}");
    assert!(row.conservative_upper <= WAIT_BUDGET);
}

async fn retry(rpc: &RpcHarness, fixture: &Fixture, original: &str) -> Value {
    rpc.request(
        0,
        original,
        "tasks.assign",
        json!({
            "task_id":fixture.intent.task_id,"retry_deferred_id":fixture.intent.id,
        }),
    )
    .await
    .reply
}

async fn task(rpc: &RpcHarness, fixture: &Fixture, original: &str) -> Value {
    rpc.request(
        0,
        original,
        "tasks.get",
        json!({"task_id":fixture.intent.task_id}),
    )
    .await
    .reply
}

fn reconcile(fixture: &Fixture) -> Result<(), String> {
    let state = Arc::clone(&fixture.state);
    std::thread::spawn(move || {
        for _ in 0..32 {
            if !AppState::pump_waiting_on_changes_for_test(&state)? {
                return Ok(());
            }
        }
        Err("bounded lifecycle reconciliation did not finish".into())
    })
    .join()
    .unwrap_or_else(|_| Err("reconciliation worker panicked".into()))
}

fn step(fixture: &Fixture) -> Option<Result<bool, String>> {
    let state = Arc::clone(&fixture.state);
    std::thread::spawn(move || {
        let (pulse, _arrival) = mpsc::channel();
        AppState::task_delivery_step_for_test(&state, pulse)()
    })
    .join()
    .unwrap_or_else(|_| Some(Err("service worker panicked".into())))
}

struct BackgroundFault {
    holding: bool,
    reached: bool,
    locked: bool,
    active: Option<WriteSnapshot>,
    watchdog: Option<WriteSnapshot>,
    service: Option<Result<bool, String>>,
    second: Option<Result<bool, String>>,
}

async fn background_fault(
    fixture: &Fixture,
    registry: &Arc<WriteWaitRegistry>,
    offset: &AtomicU64,
) -> BackgroundFault {
    let (send, receive) = mpsc::channel();
    let watchdog = registry.watchdog(
        Duration::from_millis(5),
        Arc::new(move |row| {
            if row.cohort == WriteCohort::BackgroundWork && row.method == "tasks.delivery_service" {
                let _ = send.send(row);
            }
        }),
    );
    let (mut held, arrived, holding) = HeldService::start(Arc::clone(&fixture.state));
    let reached = arrived.recv_timeout(SAFETY).is_ok();
    let deadline = Instant::now() + Duration::from_secs(2);
    let locked = loop {
        let locked = registry.active_snapshot().iter().any(|row| {
            row.cohort == WriteCohort::BackgroundWork
                && row.method == "tasks.delivery_service"
                && row
                    .active
                    .iter()
                    .any(|span| span.category == IntervalCategory::KnownWait(WaitKind::AppLock))
        });
        if locked || Instant::now() >= deadline {
            break locked;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    };
    if reached {
        offset.fetch_add(6, Ordering::SeqCst);
    }
    let active = registry.active_snapshot().into_iter().find(|row| {
        row.cohort == WriteCohort::BackgroundWork && row.method == "tasks.delivery_service"
    });
    let report = receive.recv_timeout(Duration::from_secs(2)).ok();
    let service = held.finish();
    drop(watchdog);
    let second = step(fixture);
    BackgroundFault {
        holding,
        reached,
        locked,
        active,
        watchdog: report,
        service,
        second,
    }
}

impl BackgroundFault {
    fn assert_delivered(&self) {
        assert!(self.holding && self.reached && self.locked);
        assert_eq!(self.service, Some(Ok(true)));
        assert_eq!(self.second, Some(Ok(false)));
        let active = self
            .active
            .as_ref()
            .expect("actual background reached held app lock");
        let report = self
            .watchdog
            .as_ref()
            .expect("watchdog reports background before release");
        assert!(active.breached() && active.known_wait_union > WAIT_BUDGET);
        assert_eq!(active.id, report.id);
    }
}

struct Effects {
    original: crate::store::DeliveryIntent,
    saved: crate::store::DeliveryIntent,
    receipt: bool,
    task: Value,
    messages: usize,
    turns: usize,
    idle: bool,
    opens: usize,
    clean: bool,
    cleanup_error: Option<String>,
}

async fn finish(fixture: Fixture, rpc: RpcHarness) -> Effects {
    let current = task(&rpc, &fixture, "qual/lifecycle/final-task").await;
    let page = rpc
        .request(
            0,
            "qual/lifecycle/final-page",
            "thread.page",
            json!({
                "entity_id":fixture.owner,"agent_id":fixture.agent,"limit":100,
            }),
        )
        .await
        .reply;
    let (saved, receipt, idle) = {
        let app = fixture.state.lock().unwrap();
        let store = app.tracker_store().unwrap();
        (
            store
                .load_delivery_intent(&fixture.intent.id)
                .unwrap()
                .unwrap(),
            store
                .operation(&fixture.intent.operation_id)
                .unwrap()
                .is_some(),
            app.delivery_queue.is_idle(),
        )
    };
    let messages = page["result"]["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|item| {
                    item["type"] == "message"
                        && item["data"]["operation_id"] == fixture.intent.operation_id
                        && item["data"]["from_task"]["task_id"] == fixture.intent.task_id
                })
                .count()
        })
        .unwrap_or(usize::MAX);
    let turns = fixture
        .warm
        .log
        .turns()
        .iter()
        .filter(|turn| turn.contains(&fixture.intent.operation_id))
        .count();
    let opens = fixture.warm.opens.load(Ordering::SeqCst);
    let original = fixture.intent.clone();
    fixture.warm.end();
    let cleanup = AssertUnwindSafe(rpc.finish_report()).catch_unwind().await;
    let transport_clean = cleanup.as_ref().is_ok_and(|report| report.is_clean());
    let owned_cleanup = fixture.finish_owned(transport_clean).await;
    let cleanup_error = owned_cleanup.as_ref().err().cloned();
    Effects {
        original,
        saved,
        receipt,
        task: current,
        messages,
        turns,
        idle,
        opens,
        clean: owned_cleanup.is_ok() && page["ok"] == true,
        cleanup_error,
    }
}

impl Effects {
    fn assert_delivery(&self) {
        assert!(
            self.clean && self.idle && self.receipt,
            "{:?}",
            self.cleanup_error
        );
        assert_eq!(self.opens, 0);
        assert_eq!(self.saved.state, "delivered");
        assert_eq!(
            (
                &self.saved.id,
                self.saved.generation,
                &self.saved.operation_id
            ),
            (
                &self.original.id,
                self.original.generation,
                &self.original.operation_id
            )
        );
        assert_eq!(self.messages, 1);
        assert_eq!(self.turns, 1);
        assert_eq!(self.task["ok"], true);
        assert!(self.task["result"]["task"]["deferred_assignment"].is_null());
        assert_eq!(
            self.task["result"]["timeline"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|entry| entry["type"] == "event" && entry["kind"] == "dispatched")
                .count(),
            1
        );
    }
}

/// An actual deferred constraint fails COMMIT after the retry update succeeded.
/// This does not alter the Store hook or replace the native transaction.
struct FailedCommit(rusqlite::Connection);
impl FailedCommit {
    fn install(fixture: &Fixture) -> Self {
        let conn = rusqlite::Connection::open(fixture.database_path()).unwrap();
        conn.execute_batch(
            "CREATE TABLE qual_delivery_parent(id INTEGER PRIMARY KEY);
            CREATE TABLE qual_delivery_child(parent INTEGER REFERENCES qual_delivery_parent(id)
                DEFERRABLE INITIALLY DEFERRED);
            CREATE TRIGGER qual_delivery_failed_commit AFTER UPDATE ON tracker_delivery_intents
                WHEN OLD.state='failed' AND NEW.state='queued' BEGIN
                INSERT INTO qual_delivery_child(parent) VALUES (1); END;",
        )
        .unwrap();
        Self(conn)
    }
    fn rows(&self) -> rusqlite::Result<i64> {
        self.0
            .query_row("SELECT COUNT(*) FROM qual_delivery_child", [], |row| {
                row.get(0)
            })
    }
}
impl Drop for FailedCommit {
    fn drop(&mut self) {
        let _ = self.0.execute_batch(
            "DROP TRIGGER IF EXISTS qual_delivery_failed_commit;
            DROP TABLE IF EXISTS qual_delivery_child; DROP TABLE IF EXISTS qual_delivery_parent;",
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_retry_failed_commit_creates_no_required_child_or_wakeup() {
    const ORIGINAL: &str = "qual/lifecycle/failed-commit";
    let fixture = Fixture::new();
    let wake = Arc::clone(
        &fixture
            .state
            .lock()
            .unwrap()
            .tracker_store()
            .unwrap()
            .waiting_on_wakeup,
    );
    // Setup has no autonomous worker. Consume only its existing single permit.
    let _setup_wake = tokio::time::timeout(Duration::from_millis(5), wake.notified()).await;
    let fault = FailedCommit::install(&fixture);
    let (registry, _) = clock();
    let rpc = RpcHarness::new(Arc::clone(&fixture.state), Arc::clone(&registry));
    let reply = retry(&rpc, &fixture, ORIGINAL).await;
    let terminal = row(&registry, ORIGINAL, true).await;
    let notified = tokio::time::timeout(Duration::from_millis(20), wake.notified())
        .await
        .is_ok();
    let trigger_rows = fault.rows();
    drop(fault);
    let effects = finish(fixture, rpc).await;
    eprintln!(
        "saved_intent_lifecycle_contract_expectations={}",
        json!({"case":"failed_commit",
        "offered_semantic":1,"expected_successful_semantic":0,"expected_refused_semantic":1,"qualification_successes":0})
    );
    assert!(effects.clean && effects.idle, "{:?}", effects.cleanup_error);
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(
        reply["error"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("foreign key"),
        "{reply}"
    );
    assert_eq!(
        trigger_rows.unwrap(),
        0,
        "deferred FK failure rolled back trigger and retry rows"
    );
    assert!(!notified, "failed COMMIT cannot notify delivery service");
    assert_eq!(
        serde_json::to_value(&effects.saved).unwrap(),
        serde_json::to_value(&effects.original).unwrap(),
        "failed COMMIT preserves every durable field"
    );
    assert!(!effects.receipt);
    assert_eq!((effects.messages, effects.turns, effects.opens), (0, 0, 0));
    assert_finished(
        &terminal.expect("refused original retained in archive"),
        false,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waiting_retry_finishes_before_fresh_background_release() {
    const ORIGINAL: &str = "qual/lifecycle/waiting-retry";
    let fixture = Fixture::new();
    let blocker = fixture.spare_blocker();
    // Setup changes the graph while the saved intent is still failed.
    let setup = fixture.state.lock().unwrap().handle(crate::app::tests::req(
        "tasks.waiting_on.add",
        json!({"task_id":fixture.intent.task_id,"waiting_on":[blocker]}),
    ));
    let reconciled = reconcile(&fixture);
    let (registry, offset) = clock();
    let rpc = RpcHarness::new(Arc::clone(&fixture.state), Arc::clone(&registry));
    let reply = retry(&rpc, &fixture, ORIGINAL).await;
    let waiting = task(&rpc, &fixture, "qual/lifecycle/waiting-read").await;
    let before = row(&registry, ORIGINAL, true).await;
    offset.fetch_add(6, Ordering::SeqCst);
    let dormant = row(&registry, ORIGINAL, false).await;
    let release = rpc
        .request(
            0,
            "qual/lifecycle/release",
            "tasks.update",
            json!({"task_id":blocker,"status":"done"}),
        )
        .await
        .reply;
    let release_reconciled = reconcile(&fixture);
    let fault = background_fault(&fixture, &registry, &offset).await;
    let after = row(&registry, ORIGINAL, false).await;
    let effects = finish(fixture, rpc).await;
    eprintln!(
        "saved_intent_lifecycle_contract_expectations={}",
        json!({"case":"waiting_release",
        "offered_semantic":2,"expected_successful_semantic":2,"expected_background_faults":1,"qualification_successes":0})
    );
    assert_eq!(setup["ok"], true);
    assert!(reconciled.is_ok() && release_reconciled.is_ok());
    assert_eq!(reply["ok"], true);
    assert!(reply["result"]["dispatch"].is_null());
    assert_eq!(
        waiting["result"]["task"]["deferred_assignment"]["state"],
        "waiting"
    );
    assert_eq!(release["ok"], true);
    effects.assert_delivery();
    fault.assert_delivered();
    let before = before.expect("waiting retry has original");
    assert_finished(&before, false);
    assert_eq!(
        Some(&before),
        dormant.as_ref(),
        "business dormancy does not charge resource time"
    );
    assert_eq!(
        Some(&before),
        after.as_ref(),
        "background release never revives old original"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ready_retry_reblock_publishes_waiting_before_background_release() {
    const ORIGINAL: &str = "qual/lifecycle/reblocked-retry";
    let fixture = Fixture::new();
    let blocker = fixture.spare_blocker();
    let (registry, offset) = clock();
    let rpc = RpcHarness::new(Arc::clone(&fixture.state), Arc::clone(&registry));
    let reply = retry(&rpc, &fixture, ORIGINAL).await;
    let ready = row(&registry, ORIGINAL, false).await;
    let blocked = rpc
        .request(
            0,
            "qual/lifecycle/reblock",
            "tasks.waiting_on.add",
            json!({"task_id":fixture.intent.task_id,"waiting_on":[blocker]}),
        )
        .await
        .reply;
    let reconciled = reconcile(&fixture);
    let waiting = task(&rpc, &fixture, "qual/lifecycle/reblocked-read").await;
    let published = row(&registry, ORIGINAL, true).await;
    offset.fetch_add(6, Ordering::SeqCst);
    let dormant = row(&registry, ORIGINAL, false).await;
    // Finish actual accepted domain work before asserting the missing boundary.
    let release = rpc
        .request(
            0,
            "qual/lifecycle/unblock",
            "tasks.waiting_on.remove",
            json!({"task_id":fixture.intent.task_id,"waiting_on":[blocker]}),
        )
        .await
        .reply;
    let release_reconciled = reconcile(&fixture);
    let fault = background_fault(&fixture, &registry, &offset).await;
    let after = row(&registry, ORIGINAL, false).await;
    let effects = finish(fixture, rpc).await;
    eprintln!(
        "saved_intent_lifecycle_contract_expectations={}",
        json!({"case":"ready_reblock",
        "offered_semantic":3,"expected_successful_semantic":3,"expected_background_faults":1,"qualification_successes":0})
    );
    assert_eq!(reply["ok"], true);
    assert!(ready
        .as_ref()
        .is_some_and(|row| row.root_finished && !row.outstanding.is_empty()));
    assert_eq!(blocked["ok"], true);
    assert!(reconciled.is_ok() && release_reconciled.is_ok());
    assert_eq!(
        waiting["result"]["task"]["deferred_assignment"]["state"],
        "waiting"
    );
    assert_eq!(release["ok"], true);
    effects.assert_delivery();
    fault.assert_delivered();
    let published = published.expect("reblocked original exists");
    assert_finished(&published, false);
    assert_eq!(
        Some(&published),
        dormant.as_ref(),
        "published waiting ends original resource debt"
    );
    assert_eq!(
        Some(&published),
        after.as_ref(),
        "later release is background, not old author"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ready_retry_cancellation_publishes_before_original_closes() {
    const ORIGINAL: &str = "qual/lifecycle/cancelled-retry";
    let fixture = Fixture::new();
    let (registry, offset) = clock();
    let rpc = RpcHarness::new(Arc::clone(&fixture.state), Arc::clone(&registry));
    let reply = retry(&rpc, &fixture, ORIGINAL).await;
    let ready = row(&registry, ORIGINAL, false).await;
    let cancel = rpc
        .request(
            0,
            "qual/lifecycle/cancel",
            "tasks.assign",
            json!({"task_id":fixture.intent.task_id,"assignee":null}),
        )
        .await
        .reply;
    let observed = task(&rpc, &fixture, "qual/lifecycle/cancel-read").await;
    let published = row(&registry, ORIGINAL, true).await;
    offset.fetch_add(6, Ordering::SeqCst);
    let after_time = row(&registry, ORIGINAL, false).await;
    let service = step(&fixture);
    let effects = finish(fixture, rpc).await;
    let after_cleanup = row(&registry, ORIGINAL, false).await;
    eprintln!(
        "saved_intent_lifecycle_contract_expectations={}",
        json!({"case":"ready_cancel",
        "offered_semantic":2,"expected_successful_semantic":1,"expected_cancelled_semantic":1,"qualification_successes":0})
    );
    assert_eq!(reply["ok"], true);
    assert!(ready
        .as_ref()
        .is_some_and(|row| row.root_finished && !row.outstanding.is_empty()));
    assert_eq!(cancel["ok"], true);
    assert_eq!(observed["ok"], true);
    assert!(observed["result"]["task"]["deferred_assignment"].is_null());
    assert!(observed["result"]["task"]["assignee"].is_null());
    assert_eq!(service, Some(Ok(false)));
    assert!(effects.clean && effects.idle, "{:?}", effects.cleanup_error);
    assert_eq!(effects.saved.state, "cancelled");
    assert!(!effects.receipt);
    assert_eq!((effects.messages, effects.turns, effects.opens), (0, 0, 0));
    let published = published.expect("cancelled original is never discarded");
    assert_finished(&published, true);
    assert_eq!(Some(&published), after_time.as_ref());
    assert_eq!(
        Some(&published),
        after_cleanup.as_ref(),
        "cleanup cannot manufacture cancellation"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reopened_store_recovers_delivery_with_fresh_background_origin() {
    const ORIGINAL: &str = "qual/lifecycle/pre-restart-ready-retry";
    let fixture = Fixture::restartable();
    let (old_registry, _) = clock();
    let old_rpc = RpcHarness::new(Arc::clone(&fixture.state), Arc::clone(&old_registry));
    let reply = retry(&old_rpc, &fixture, ORIGINAL).await;
    let before_restart = row(&old_registry, ORIGINAL, false).await;
    let old_cleanup = AssertUnwindSafe(old_rpc.finish_report())
        .catch_unwind()
        .await;
    let old_clean = old_cleanup.as_ref().is_ok_and(|report| report.is_clean());
    // This deliberately interrupts a required attempt. Preserve its abandoned
    // evidence; recovery must not turn this old sample into a success.
    let fixture = fixture
        .reopen(old_clean)
        .await
        .expect("restart infrastructure must release old owners; roots preserved on failure");
    let interrupted = row(&old_registry, ORIGINAL, false).await;
    let (registry, offset) = clock();
    let rpc = RpcHarness::new(Arc::clone(&fixture.state), Arc::clone(&registry));
    let fault = background_fault(&fixture, &registry, &offset).await;
    let new_rows = registry.snapshot();
    let effects = finish(fixture, rpc).await;
    let archived = row(&old_registry, ORIGINAL, false).await;
    eprintln!(
        "saved_intent_lifecycle_contract_expectations={}",
        json!({"case":"startup_recovery",
        "offered_semantic":1,"expected_successful_semantic":0,"expected_interrupted_semantic":1,"qualification_successes":0,
        "expected_recovered_background":1,"expected_background_faults":1})
    );
    assert_eq!(reply["ok"], true);
    assert!(old_clean);
    assert!(before_restart
        .as_ref()
        .is_some_and(|row| row.root_finished && !row.outstanding.is_empty()));
    effects.assert_delivery();
    fault.assert_delivered();
    assert!(new_rows.iter().all(|row| row.original_id != ORIGINAL));
    assert!(new_rows
        .iter()
        .any(|row| row.cohort == WriteCohort::BackgroundWork
            && row.method == "tasks.delivery_service"));
    let interrupted = interrupted.expect("interrupted caller remains archived in old registry");
    assert!(interrupted.abandoned && interrupted.completed_at.is_some());
    assert_eq!(
        Some(&interrupted),
        archived.as_ref(),
        "recovery cannot rewrite pre-restart evidence"
    );
}
