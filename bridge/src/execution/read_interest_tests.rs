//! Real shared execution checks for additive per-request read interest.

use super::*;

fn with_read_interest(request: RequestContext, live: Arc<AtomicBool>) -> RequestContext {
    let opening = request.session().opening().clone();
    request
        .narrow_read_interest(&opening, Arc::new(move || live.load(Ordering::SeqCst)))
        .unwrap()
}

async fn submit_with_read_interest(
    service: &ExecutionService,
    session: &SessionLease,
    output: Arc<dyn ReplySink>,
    method: &str,
    params: Value,
    interest: Arc<AtomicBool>,
) -> Call {
    let request = service.sessions().request(session, output, None).unwrap();
    timeout(
        WATCHDOG,
        service.submit(
            with_read_interest(request, interest),
            Invocation::decode(method, &params).unwrap(),
        ),
    )
    .await
    .expect("narrowed request admission must settle")
    .unwrap()
}

#[tokio::test]
async fn retired_read_interest_prunes_queued_read_without_cancelling_mutations() {
    let (handler, mut entered, release, effects) = blocked_handler();
    let service = single_worker(handler);
    let session = open(&service);
    let (blocking_sink, _blocking_output, _blocking_dropped) = sink();
    let mut blocking = submit(&service, &session, None, blocking_sink, "user.present").await;
    let _producer = timeout(WATCHDOG, entered.recv()).await.unwrap().unwrap();
    let interested = Arc::new(AtomicBool::new(true));
    let (read_sink, mut read_output, read_dropped) = sink();
    let mut read = submit_with_read_interest(
        &service,
        &session,
        read_sink,
        "board.list",
        json!({}),
        interested.clone(),
    )
    .await;
    let (ordinary_sink, _ordinary_output, _ordinary_dropped) = sink();
    let mut ordinary = submit(&service, &session, None, ordinary_sink, "user.present").await;
    let (restricted_sink, _restricted_output, _restricted_dropped) = sink();
    let mut restricted_mutation = submit_with_read_interest(
        &service,
        &session,
        restricted_sink,
        "user.present",
        json!({}),
        interested.clone(),
    )
    .await;
    interested.store(false, Ordering::SeqCst);
    release.send(()).unwrap();
    reply(&mut blocking).await;
    reply(&mut ordinary).await;
    reply(&mut restricted_mutation).await;
    // Keep the read's Call alive through queue drain: cancellation must not be
    // what prevents the retired read from running or producing output.
    let outcome = timeout(WATCHDOG, read.reply()).await;
    drop(read);
    timeout(WATCHDOG, read_dropped).await.unwrap().unwrap();
    assert!(outcome.expect("retired read must drain").is_err());
    assert_eq!(
        effects.load(Ordering::SeqCst),
        3,
        "only the initial and two independently live mutations may execute"
    );
    assert!(read_output.try_recv().is_err());
}

#[tokio::test]
async fn retired_read_interest_stops_immutable_snapshot_before_callback() {
    let snapshots = Arc::new(AtomicUsize::new(0));
    let witnessed = snapshots.clone();
    let handler = FrameHandler::new(FrameClock::new(), |_, _, _| {
        panic!("immutable read must not enter mutable dispatch")
    })
    .with_snapshot_reads(move |frame| {
        witnessed.fetch_add(1, Ordering::SeqCst);
        json!({"id":frame.payload["id"],"ok":true,"result":{}})
    });
    let service = ExecutionService::new(handler);
    let session = open(&service);
    let (output, mut received, dropped) = sink();
    let request = service.sessions().request(&session, output, None).unwrap();
    let request = with_read_interest(request, Arc::new(AtomicBool::new(false)));
    let outcome = service
        .submit(
            request,
            Invocation::decode("board.list", &json!({})).unwrap(),
        )
        .await;
    let refused = match outcome {
        Err(_) => true,
        Ok(mut call) => timeout(WATCHDOG, call.reply())
            .await
            .expect("retired snapshot must settle")
            .is_err(),
    };
    timeout(WATCHDOG, dropped).await.unwrap().unwrap();
    assert!(
        refused,
        "retired read unexpectedly returned a usable snapshot"
    );
    assert_eq!(
        snapshots.load(Ordering::SeqCst),
        0,
        "retired read must be checked before entering the snapshot callback"
    );
    assert!(received.try_recv().is_err());
}

#[tokio::test]
async fn newest_retired_read_interest_does_not_suppress_a_live_folded_caller() {
    let (handler, mut entered, release, effects) = blocked_handler();
    let service = single_worker(handler);
    let session = open(&service);
    let (blocking_sink, _blocking_output, _blocking_dropped) = sink();
    let mut blocking = submit(&service, &session, None, blocking_sink, "user.present").await;
    let _producer = timeout(WATCHDOG, entered.recv()).await.unwrap().unwrap();
    let (older_sink, mut older_output, _older_dropped) = sink();
    let mut older = submit_with_read_interest(
        &service,
        &session,
        older_sink,
        "board.list",
        json!({}),
        Arc::new(AtomicBool::new(true)),
    )
    .await;
    let latest_interest = Arc::new(AtomicBool::new(true));
    let (newest_sink, mut newest_output, newest_dropped) = sink();
    let mut newest = submit_with_read_interest(
        &service,
        &session,
        newest_sink,
        "board.list",
        json!({}),
        latest_interest.clone(),
    )
    .await;
    let folds = service.dispatcher.folded_reads_waiting();
    latest_interest.store(false, Ordering::SeqCst);
    release.send(()).unwrap();
    reply(&mut blocking).await;
    reply(&mut older).await;
    let newest_answer = timeout(WATCHDOG, newest.reply()).await;
    // The newest Call stays alive until the computation finishes, so only its
    // additional read proof can suppress delivery to that caller.
    drop(newest);
    timeout(WATCHDOG, newest_dropped).await.unwrap().unwrap();
    assert_eq!(folds, 1, "both requests must share one queued fold");
    assert!(newest_answer
        .expect("retired folded caller must settle")
        .is_err());
    assert_eq!(
        effects.load(Ordering::SeqCst),
        2,
        "a surviving fold computes once beside the initial blocked mutation"
    );
    let event = timeout(WATCHDOG, older_output.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(event.is_live());
    assert_eq!(
        event.payload()["id"],
        json!(older.context().request_order())
    );
    assert_eq!(event.payload()["ok"], true);
    assert_eq!(event.payload()["result"], json!({"served":true}));
    assert!(newest_output.try_recv().is_err());
}

#[tokio::test]
async fn retired_read_interest_cannot_enter_sql_worker_after_capacity_wait() {
    let (entered, mut arrivals) = mpsc::unbounded_channel();
    let (release, released) = blocking::channel();
    let released = Mutex::new(released);
    let starts = Arc::new(AtomicUsize::new(0));
    let counted = starts.clone();
    let retired_starts = Arc::new(AtomicUsize::new(0));
    let retired_counted = retired_starts.clone();
    let handler = FrameHandler::new(FrameClock::new(), |_, _, _| {
        panic!("SQL reads must use the existing dedicated executor")
    })
    .with_sql_reads(None, move |_, frame, _, _| {
        counted.fetch_add(1, Ordering::SeqCst);
        let task_id = frame.payload["params"]["task_id"].as_str().unwrap();
        if task_id.starts_with("held-") {
            entered.send(()).unwrap();
            released
                .lock()
                .unwrap()
                .recv_timeout(WATCHDOG)
                .expect("test must release all occupied SQL workers");
        } else if task_id == "retired" {
            retired_counted.fetch_add(1, Ordering::SeqCst);
        }
        json!({"id":frame.payload["id"],"ok":true,"result":{"sql":true}})
    });
    let service = ExecutionService::new(handler);
    let session = open(&service);
    let mut holders = Vec::new();
    let mut holder_outputs = Vec::new();
    let mut holder_budgets = Vec::new();
    for index in 0..crate::execution::sql::SQL_WORKERS {
        let (output, received, dropped) = sink();
        let request = service.sessions().request(&session, output, None).unwrap();
        let call = service
            .submit(
                request,
                Invocation::decode("tasks.get", &json!({"task_id":format!("held-{index}")}))
                    .unwrap(),
            )
            .await
            .unwrap();
        holders.push(call);
        holder_outputs.push(received);
        holder_budgets.push(dropped);
        timeout(WATCHDOG, arrivals.recv()).await.unwrap().unwrap();
    }
    let interested = Arc::new(AtomicBool::new(true));
    let (read_sink, mut read_output, read_dropped) = sink();
    let mut read = submit_with_read_interest(
        &service,
        &session,
        read_sink,
        "tasks.get",
        json!({"task_id":"retired"}),
        interested.clone(),
    )
    .await;
    let (sentinel_sink, mut sentinel_output, _sentinel_dropped) = sink();
    let request = service
        .sessions()
        .request(&session, sentinel_sink, None)
        .unwrap();
    let mut sentinel = service
        .submit(
            request,
            Invocation::decode("tasks.get", &json!({"task_id":"live"})).unwrap(),
        )
        .await
        .unwrap();
    let waiting = service.dispatcher.sql_depths().0;
    interested.store(false, Ordering::SeqCst);
    for _ in 0..crate::execution::sql::SQL_WORKERS {
        release.send(()).unwrap();
    }
    for holder in &mut holders {
        reply(holder).await;
    }
    reply(&mut sentinel).await;
    let event = timeout(WATCHDOG, sentinel_output.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(event.is_live());
    assert_eq!(
        event.payload()["id"],
        json!(sentinel.context().request_order())
    );
    assert_eq!(event.payload()["ok"], true);
    assert_eq!(event.payload()["result"], json!({"sql":true}));
    let outcome = timeout(WATCHDOG, read.reply()).await;
    drop(read);
    timeout(WATCHDOG, read_dropped).await.unwrap().unwrap();
    assert_eq!(
        waiting, 2,
        "both reads must wait behind the actual eight SQL workers"
    );
    assert!(outcome.expect("retired SQL interest must drain").is_err());
    assert_eq!(retired_starts.load(Ordering::SeqCst), 0);
    assert_eq!(
        starts.load(Ordering::SeqCst),
        crate::execution::sql::SQL_WORKERS + 1
    );
    assert!(read_output.try_recv().is_err());
}

#[derive(Clone, Copy, Debug)]
enum NarrowedAuthorityEnd {
    Cancellation,
    OpeningClose,
    Expiry,
    Revocation,
}

#[tokio::test]
async fn true_read_interest_cannot_bypass_request_or_opening_authority() {
    for end in [
        NarrowedAuthorityEnd::Cancellation,
        NarrowedAuthorityEnd::OpeningClose,
        NarrowedAuthorityEnd::Expiry,
        NarrowedAuthorityEnd::Revocation,
    ] {
        let (handler, mut entered, release, effects) = blocked_handler();
        let service = single_worker(handler);
        let live = Arc::new(AtomicBool::new(true));
        let session = service
            .sessions()
            .open(caller(
                live.clone(),
                Instant::now() + Duration::from_secs(60),
            ))
            .unwrap();
        let (blocking_sink, _blocking_output, _blocking_dropped) = sink();
        let mut blocking = submit(&service, &session, None, blocking_sink, "user.present").await;
        let _producer = timeout(WATCHDOG, entered.recv()).await.unwrap().unwrap();
        let (read_sink, mut read_output, read_dropped) = sink();
        let mut read = submit_with_read_interest(
            &service,
            &session,
            read_sink,
            "board.list",
            json!({}),
            Arc::new(AtomicBool::new(true)),
        )
        .await;
        let (mutation_sink, mut mutation_output, mutation_dropped) = sink();
        let mut mutation = submit_with_read_interest(
            &service,
            &session,
            mutation_sink,
            "user.present",
            json!({}),
            Arc::new(AtomicBool::new(true)),
        )
        .await;
        match end {
            NarrowedAuthorityEnd::Cancellation => read.cancel(),
            NarrowedAuthorityEnd::OpeningClose => service.sessions().close(&session),
            NarrowedAuthorityEnd::Expiry => {
                tokio::time::pause();
                tokio::time::advance(Duration::from_secs(61)).await;
                tokio::time::resume();
            }
            NarrowedAuthorityEnd::Revocation => live.store(false, Ordering::SeqCst),
        }
        release.send(()).unwrap();
        let running = timeout(WATCHDOG, blocking.reply()).await;
        let outcome = timeout(WATCHDOG, read.reply()).await;
        let mutation_outcome = timeout(WATCHDOG, mutation.reply()).await;
        // A successful Call outcome can wake before the ReplySink enqueue.
        // Keep its handle alive until the actual independent mutation reply is
        // handed off, rather than turning this into another cancellation race.
        let mutation_delivered = if matches!(end, NarrowedAuthorityEnd::Cancellation)
            && matches!(&mutation_outcome, Ok(Ok(_)))
        {
            let event = timeout(WATCHDOG, mutation_output.recv())
                .await
                .expect("live mutation must deliver its independent reply")
                .expect("live mutation sink remains open");
            assert!(event.is_live());
            assert_eq!(
                event.payload()["id"],
                json!(mutation.context().request_order())
            );
            assert_eq!(event.payload()["ok"], true);
            assert_eq!(event.payload()["result"], json!({"served":true}));
            true
        } else {
            false
        };
        drop(read);
        drop(mutation);
        timeout(WATCHDOG, async {
            read_dropped.await.unwrap();
            mutation_dropped.await.unwrap();
        })
        .await
        .expect("drained read and mutation must release their reply budgets");
        let _ = running.expect("already-running work must settle after release");
        assert!(outcome.expect("dead read must drain").is_err(), "{end:?}");
        let mutation_outcome = mutation_outcome.expect("queued mutation must drain");
        let expected_effects = match end {
            NarrowedAuthorityEnd::Cancellation => {
                assert!(
                    mutation_outcome.is_ok(),
                    "cancelling a read cannot cancel another request"
                );
                assert!(mutation_delivered);
                2
            }
            _ => {
                assert!(
                    mutation_outcome.is_err(),
                    "true read proof bypassed {end:?} for a mutation"
                );
                assert!(mutation_output.try_recv().is_err(), "{end:?}");
                1
            }
        };
        assert_eq!(effects.load(Ordering::SeqCst), expected_effects, "{end:?}");
        assert!(read_output.try_recv().is_err(), "{end:?}");
    }
}
