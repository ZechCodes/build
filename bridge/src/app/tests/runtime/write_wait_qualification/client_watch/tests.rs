use super::*;

fn fixture() -> (ClientWatch, Arc<std::sync::Mutex<Instant>>) {
    let at = Arc::new(std::sync::Mutex::new(Instant::now()));
    let clock = Arc::clone(&at);
    (
        ClientWatch::new(Arc::new(move || *clock.lock().unwrap())),
        at,
    )
}

#[test]
fn unfinished_client_before_server_registration_breaches_original_deadline() {
    let (watch, clock) = fixture();
    watch.offer("qual/small/trial1/write/00000");
    *clock.lock().unwrap() += Duration::from_secs(5);
    assert!(!watch.snapshot()[0].breached, "five seconds is inclusive");
    *clock.lock().unwrap() += Duration::from_nanos(1);
    let rows = watch.snapshot();
    assert!(
        rows[0].sent_at.is_none(),
        "server registration need not exist"
    );
    assert!(
        rows[0].breached,
        "unfinished caller cannot disappear from completed histograms"
    );
}

#[test]
fn late_completion_without_a_prior_poll_retains_breach_and_original_times() {
    let (watch, clock) = fixture();
    let original = "qual/small/trial1/write/00001";
    watch.offer(original);
    let offered = *clock.lock().unwrap();
    *clock.lock().unwrap() += Duration::from_secs(1);
    watch.sent(original);
    let sent = *clock.lock().unwrap();
    *clock.lock().unwrap() += Duration::from_secs(5);
    watch.complete(original);
    let row = watch
        .snapshot()
        .pop()
        .expect("completed callers remain archived");
    assert_eq!(row.original_id, original);
    assert_eq!(row.offered_at, Some(offered));
    assert_eq!(row.sent_at, Some(sent));
    assert!(row.finished_at.is_some());
    assert!(
        row.breached,
        "late completion is a failure even without earlier polling"
    );
}

#[test]
fn cancellation_and_cleanup_cannot_erase_an_already_overdue_caller() {
    let (watch, clock) = fixture();
    let original = "qual/large/trial3/write/00002";
    watch.offer(original);
    watch.sent(original);
    *clock.lock().unwrap() += Duration::from_secs(6);
    watch.cancel(original);
    let row = watch
        .snapshot()
        .pop()
        .expect("cancelled caller remains evidence");
    assert!(row.cancelled);
    assert!(row.breached);
}

#[test]
fn intended_due_time_detects_late_scheduler_before_actual_offer_or_server_registration() {
    let (watch, clock) = fixture();
    let base = *clock.lock().unwrap();
    let original = "qual/small/trial1/write/00040";
    watch.register_at(original, base + Duration::from_secs(10));
    *clock.lock().unwrap() = base + Duration::from_secs(6);
    assert!(
        !watch.snapshot()[0].breached,
        "future due time has zero age"
    );
    *clock.lock().unwrap() = base + Duration::from_secs(15);
    assert!(
        !watch.snapshot()[0].breached,
        "exact five seconds is inclusive"
    );
    *clock.lock().unwrap() += Duration::from_nanos(1);
    assert!(
        watch.snapshot()[0].breached,
        "no offer or server origin is needed"
    );
    watch.offer(original);
    watch.sent(original);
    let row = &watch.snapshot()[0];
    assert_eq!(row.intended_at, base + Duration::from_secs(10));
    assert!(row.offered_at.unwrap() > row.intended_at);
    assert!(
        row.breached,
        "late offer cannot reset the original deadline"
    );
}

#[test]
fn independent_client_watch_observes_unregistered_pending_without_snapshot_or_runtime_poll() {
    let (watch, clock) = fixture();
    let original = "qual/small/trial1/write/independent";
    let due = *clock.lock().unwrap();
    watch.register_at(original, due);
    watch.offer(original);
    let guard = watch.watch();
    // Only the fake clock changes. Neither a collector/app lock nor a caller
    // snapshot or Tokio task drives the watchdog's observation.
    *clock.lock().unwrap() += Duration::from_secs(5) + Duration::from_nanos(1);
    let deadline = Instant::now() + Duration::from_millis(250);
    while !guard.failed() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    let observed = guard.failed();
    let joined = guard.finish();
    assert!(
        observed && joined.failed && !joined.panicked && joined.polls > 0,
        "independent watchdog must retain unfinished client breach before server registration"
    );
}

#[test]
fn decoded_observer_preserves_actual_timestamp_without_converting_transport_failure_to_reply() {
    let (watch, clock) = fixture();
    let (events, decoded) = mpsc::channel();
    watch
        .observe_decoded_replies(Arc::new(move |id, actual_at| {
            events.send((id.to_owned(), actual_at)).unwrap();
        }))
        .unwrap();
    for original in [
        "decoded-ok",
        "decoded-error",
        "transport-error",
        "cancelled",
    ] {
        watch.offer(original);
        watch.sent(original);
    }
    let actual_at = *clock.lock().unwrap();
    *clock.lock().unwrap() += Duration::from_millis(7);
    watch.decoded_final_at("decoded-ok", FinalOutcome::Reply, actual_at);
    watch.decoded_final_at("decoded-error", FinalOutcome::ProtocolError, actual_at);
    watch.complete_with_outcome("transport-error", FinalOutcome::TransportError);
    watch.cancel("cancelled");
    let events = decoded.try_iter().collect::<Vec<_>>();
    assert_eq!(
        events,
        vec![
            ("decoded-ok".into(), actual_at),
            ("decoded-error".into(), actual_at),
        ],
        "only validated decoded replies notify; the actual parsed timestamp is unchanged"
    );
    assert!(watch.observe_decoded_replies(Arc::new(|_, _| {})).is_err());
    let rows = watch.snapshot();
    assert!(
        rows.iter()
            .find(|row| row.original_id == "cancelled")
            .unwrap()
            .cancelled
    );
    assert_eq!(
        rows.iter()
            .find(|row| row.original_id == "transport-error")
            .unwrap()
            .final_outcome,
        Some(FinalOutcome::TransportError)
    );
}
