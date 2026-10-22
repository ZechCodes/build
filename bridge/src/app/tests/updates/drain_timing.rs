//! Delivery admission charges the existing lock before testing update policy.
use super::*;
use crate::timing::FrameClock;
use std::sync::mpsc;

const WATCHDOG: Duration = Duration::from_secs(5);
const WAIT: Duration = Duration::from_millis(250);

#[tokio::test]
async fn drain_admission_lock_wait_is_reported_before_closed_gate_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let updates = update_fixture(dir.path(), false);
    updates.check().await.unwrap();
    updates
        .install(crate::update::InstallWhen::Idle, true)
        .await
        .unwrap();
    assert_eq!(
        updates.tick(false).await.unwrap().state,
        UpdateState::Installing
    );
    assert!(updates.admission().try_enter().is_none());
    let clock = FrameClock::reporting_to(Arc::new(|_| {}));
    let (entered, arrived) = mpsc::channel();
    clock.observe_requests(Arc::new(move |_, phase, _| {
        if phase == "delivery_admission_start" {
            let _ = entered.send(());
        }
    }));
    let samples = clock.capture_phases();
    let mut app = AppState::new_unrooted(dir.path().join("worktrees"), "main", true, "unused.sock")
        .with_update_service(Arc::clone(&updates));
    app.frame_clock = Arc::clone(&clock);
    app.delivery_queue.enqueue(pending_turn(dir.path()));
    // Passive shared state: background delivery must not consume the sentinel.
    let state = Arc::new(Mutex::new(app));
    let before = state.lock().unwrap().read_view.load();
    let blocker = state.lock().unwrap();
    let work = Arc::clone(&state);
    let (complete, completed) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut timer = clock.frame("user.present");
        timer.bind_request(&req("user.present", json!({})));
        DeliveryRunner::drain(&work, &timer);
        drop(timer);
        let _ = complete.send(());
    });
    // Store all observations, release the real blocker and join, THEN assert.
    // The marker is directly before the admission lock in the actual drain.
    let arrival = arrived.recv_timeout(WATCHDOG);
    let stayed_blocked = arrival.is_ok()
        && matches!(
            completed.recv_timeout(WAIT),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
    drop(blocker);
    let joined = worker.join();
    assert!(
        arrival.is_ok(),
        "drain never reached its exact admission acquisition"
    );
    assert!(
        stayed_blocked,
        "drain must wait on the held AppState acquisition"
    );
    joined.unwrap();
    let captured = samples.snapshot();
    let frame = &captured["user.present"];
    assert_eq!(frame["entered.lock"]["count"], 1,
        "closed admission returns before the queue lock; this is only the formerly raw acquisition: {captured}");
    assert!(
        frame["frame.lock_wait"]["sum_ms"].as_f64().unwrap() >= 200.0,
        "the coordinated admission wait is charged to lock_wait: {captured}"
    );
    let app = state.lock().unwrap();
    assert_eq!(
        app.delivery_queue.queued_len(),
        1,
        "closed admission preserves queued work"
    );
    assert!(Arc::ptr_eq(&before, &app.read_view.load()));
    assert!(updates.admission().try_enter().is_none());
}

#[test]
fn drain_admission_timing_keeps_open_gate_settling_maintenance() {
    let dir = tempfile::tempdir().unwrap();
    let updates = update_fixture(dir.path(), false);
    assert!(updates.admission().try_enter().is_some());
    let clock = FrameClock::reporting_to(Arc::new(|_| {}));
    let mut app = AppState::new_unrooted(dir.path().join("worktrees"), "main", true, "unused.sock")
        .with_update_service(Arc::clone(&updates));
    app.frame_clock = Arc::clone(&clock);
    app.delivery_queue
        .enqueue_after_settle_window(pending_turn(dir.path()));
    let due = app
        .delivery_queue
        .settle_wake_due()
        .expect("seed has a real future wake");
    app.delivery_queue.settle_wake_fired(due);
    assert!(app.update_has_working_agents());
    let state = Arc::new(Mutex::new(app));
    let before = state.lock().unwrap().read_view.load();
    let timer = clock.frame("user.present");
    DeliveryRunner::drain(&state, &timer);
    drop(timer);
    let mut app = state.lock().unwrap();
    assert!(
        app.update_has_working_agents(),
        "future notice remains settling"
    );
    assert!(
        app.delivery_queue.settle_wake_due().is_none(),
        "drain requested the existing wake exactly once"
    );
    assert!(Arc::ptr_eq(&before, &app.read_view.load()));
    assert!(updates.admission().try_enter().is_some());
}
