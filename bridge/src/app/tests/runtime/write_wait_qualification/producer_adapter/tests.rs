//! Harness scheduling controls only. The callbacks below are real owned
//! invocations, but their completion is not a product-effect qualification.
use super::super::trial_complete::Envelope;
use super::*;
use std::sync::mpsc;
use std::thread::ThreadId;

struct Paused {
    producer: Arc<ActualProducer>,
    registry: Arc<WriteWaitRegistry>,
    start: Instant,
    entered: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
    calls: Arc<Mutex<Vec<(u64, ThreadId)>>>,
}

fn paused(window: Duration, period: Duration) -> Paused {
    let registry = WriteWaitRegistry::new();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&calls);
    let (entered, receive_entered) = mpsc::channel();
    let (release, receive_release) = mpsc::channel();
    let body = Box::new(move |ordinal| {
        recorded
            .lock()
            .unwrap()
            .push((ordinal, std::thread::current().id()));
        if ordinal == 0 {
            entered.send(()).map_err(|error| error.to_string())?;
            receive_release
                .recv_timeout(Duration::from_secs(5))
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    });
    let producer = ActualProducer::new(
        TrialName {
            envelope: Envelope::Small,
            number: 1,
        },
        Config {
            period,
            ..CONFIG[1]
        },
        window,
        Arc::clone(&registry),
        tokio::runtime::Handle::current(),
        body,
    )
    .unwrap();
    let start = Instant::now() + Duration::from_millis(10);
    producer.start_at(start).unwrap();
    Paused {
        producer,
        registry,
        start,
        entered: receive_entered,
        release,
        calls,
    }
}

async fn join(producer: &ActualProducer) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !producer.joined() {
        assert!(Instant::now() < deadline, "owned callback did not drain");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn assert_native_callbacks(fixture: &Paused, ordinals: &[u64], period: Duration) {
    let calls = fixture.calls.lock().unwrap();
    assert_eq!(
        calls.iter().map(|call| call.0).collect::<Vec<_>>(),
        ordinals
    );
    assert!(calls.iter().all(|call| call.1 == calls[0].1));
    let rows = fixture.registry.snapshot();
    assert_eq!(rows.len(), ordinals.len());
    for (row, ordinal) in rows.iter().zip(ordinals) {
        assert_eq!(
            row.original_id,
            format!("qual/small_trial_1/background/delivery/{ordinal:05}")
        );
        assert_eq!(row.intake, fixture.start + period * *ordinal as u32);
        assert!(row.root_finished && row.completed_at.is_some());
        assert!(row.outstanding.is_empty() && row.active.is_empty());
        assert!(!row.cancelled && !row.abandoned);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offered_callback_drains_after_window_on_its_original_due_time() {
    let period = Duration::from_millis(20);
    let window = period * 2;
    let fixture = paused(window, period);
    let entered = fixture.entered.recv_timeout(Duration::from_secs(5));
    tokio::time::sleep_until((fixture.start + window + period).into()).await;
    let before = fixture.producer.snapshot();
    let released = fixture.release.send(());
    join(&fixture.producer).await;
    let after = fixture.producer.snapshot();

    entered.unwrap();
    released.unwrap();
    assert_eq!(
        (before.intended, before.started, before.completed),
        (2, 1, 0)
    );
    assert_eq!(before.active_ordinal, Some(0));
    assert_eq!(before.pending_not_started, 1);
    assert_eq!(before.missed_at_stop, 0);
    assert!(!before.stop_requested);
    assert_eq!(
        (
            after.planned,
            after.intended,
            after.started,
            after.completed
        ),
        (2, 2, 2, 2)
    );
    assert_eq!(
        (
            after.failed,
            after.pending_not_started,
            after.missed_at_stop
        ),
        (0, 0, 0)
    );
    assert!(after.body_released && after.joined && after.first_error.is_none());
    assert!(after.maximum_start_lag_us >= period.as_micros() as u64);
    assert_native_callbacks(&fixture, &[0, 1], period);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_abort_keeps_active_callback_and_counts_due_but_unstarted_work() {
    let period = Duration::from_millis(10);
    let fixture = paused(Duration::from_secs(1), period);
    let entered = fixture.entered.recv_timeout(Duration::from_secs(5));
    tokio::time::sleep_until((fixture.start + period * 4).into()).await;
    fixture.producer.stop_offering();
    let stopped = fixture.producer.snapshot();
    let released = fixture.release.send(());
    join(&fixture.producer).await;
    let after = fixture.producer.snapshot();

    entered.unwrap();
    released.unwrap();
    assert!(stopped.stop_requested);
    assert_eq!(stopped.active_ordinal, Some(0));
    assert!(stopped.intended >= 4 && stopped.intended < stopped.planned);
    assert_eq!(stopped.missed_at_stop, stopped.intended - 1);
    assert_eq!(after.intended, stopped.intended);
    assert_eq!((after.started, after.completed, after.failed), (1, 1, 0));
    assert_eq!(after.missed_at_stop, stopped.missed_at_stop);
    assert_eq!(after.pending_not_started, 0);
    assert!(after.body_released && after.joined && after.first_error.is_none());
    assert_native_callbacks(&fixture, &[0], period);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_deadline_uses_original_due_without_changing_callback_outcome() {
    let period = Duration::from_millis(20);
    let fixture = paused(period * 2, period);
    let entered = fixture.entered.recv_timeout(Duration::from_secs(5));
    // Test the atomic observer at exact synthetic observation times. This is
    // a boundary control, not a claim that a physical five-second stall ran.
    let boundary = fixture.producer.snapshot_at(fixture.start + WAIT_BUDGET);
    let over = fixture
        .producer
        .snapshot_at(fixture.start + WAIT_BUDGET + Duration::from_micros(1));
    let released = fixture.release.send(());
    join(&fixture.producer).await;
    let after = fixture.producer.snapshot();

    entered.unwrap();
    released.unwrap();
    assert_eq!(boundary.oldest_owed_ordinal, Some(0));
    assert_eq!(boundary.oldest_owed_age_us, Some(5_000_000));
    assert!(boundary.deadline_breach.is_none());
    assert_eq!(over.oldest_owed_age_us, Some(5_000_001));
    assert!(over
        .deadline_breach
        .as_deref()
        .is_some_and(|error| error.contains("ordinal0")));
    assert!(after.oldest_owed_ordinal.is_none());
    assert_eq!((after.started, after.completed, after.failed), (2, 2, 0));
    assert!(after.first_error.is_none());
    assert_native_callbacks(&fixture, &[0, 1], period);
}
