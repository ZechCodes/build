//! Exact holds come from the completed timer, never independently paired maxima.
use super::*;
use crate::timing::completed_frames::{CompletedFrame, FrameProtocol};
use crate::timing::write_wait::WriteContext;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn completed_frames_pair_raw_rpc_ids_with_all_of_their_actual_holds() {
    let clock = FrameClock::reporting_to(Arc::new(|_| {}));
    let registry = clock.capture_write_waits();
    let observed = Arc::new(Mutex::new(Vec::<CompletedFrame>::new()));
    let record = observed.clone();
    // No request-phase observer is installed: pairing has its own opt-in.
    clock.observe_completed_frames(Arc::new(move |sample| {
        record.lock().unwrap().push(sample);
    }));
    let app = Arc::new(Mutex::new(0));
    let handler = FrameHandler::new(clock.clone(), move |_sender, frame, timer| {
        let (passes, delay) = if frame.payload["id"] == "qual/raw/two-holds" {
            (2, 20)
        } else {
            (1, 5)
        };
        for _ in 0..passes {
            let mut held = timer.lock(&app);
            std::thread::sleep(Duration::from_millis(delay));
            *held += 1;
        }
        json!({"id":frame.payload["id"],"ok":true})
    });
    let (intake, carrier, mut outbound, key) = opened_intake(handler, 2);
    for id in ["qual/raw/two-holds", "qual/raw/one-hold"] {
        intake
            .accept(
                client_request(
                    &key,
                    SESSION_ID,
                    "data",
                    json!({
                        "id": id, "method":"fs.write", "params":{}
                    }),
                ),
                &carrier,
            )
            .await
            .unwrap();
    }
    let mut replies = std::collections::BTreeSet::new();
    while replies.len() < 2 {
        let envelope = within_patience(outbound.recv()).await;
        let reply = SessionSender::decrypt_push(&key, &envelope);
        if reply.get("accepted").is_none() {
            assert_eq!(reply["ok"], true);
            replies.insert(reply["id"].as_str().unwrap().to_owned());
        }
    }
    // Reply transport runs after handler return, which dropped the exact timer.
    let records = observed.lock().unwrap();
    assert_eq!(records.len(), 2, "one completed sample per real RPC");
    let find = |id: &str| records.iter().find(|r| r.original_ids == [id]).unwrap();
    let two = find("qual/raw/two-holds");
    let one = find("qual/raw/one-hold");
    assert!(two.held >= Duration::from_millis(40), "{two:?}");
    assert!(one.held >= Duration::from_millis(5), "{one:?}");
    // The aggregate FrameClock uses this same timer, not a parallel stopwatch.
    let aggregate = clock.stats()["methods"]["fs.write"]["held"]["sum_ms"]
        .as_f64()
        .unwrap();
    assert!((aggregate - (two.held + one.held).as_secs_f64() * 1000.0).abs() < 0.01);
    let maximum_total = clock.stats()["methods"]["fs.write"]["max_ms"]
        .as_f64()
        .unwrap();
    assert!(
        (maximum_total - two.total.max(one.total).as_secs_f64() * 1000.0).abs() < 0.0011,
        "completed samples and legacy totals use the same Spent snapshot"
    );
    for row in [two, one] {
        assert_eq!(row.protocol, FrameProtocol::Rpc);
        assert_eq!(row.method, "fs.write");
        assert!(row.total >= row.held + row.lock_wait);
        assert!(row.total >= row.queued);
        assert!(row.permit_wait.is_none());
        // Ordinary RPC starts this timer after blocking-pool entry. Its pool
        // residence is part of `queued`, not the MCP-only optional metric.
        assert!(row.pool_wait.is_none());
        let original = registry
            .snapshot()
            .into_iter()
            .find(|write| row.original_ids == [write.original_id.as_str()])
            .unwrap();
        assert!(
            registry.completed_intervals().iter().any(|interval| {
                interval.origins.contains(&original.id)
                    && interval.category
                        == crate::timing::write_wait::IntervalCategory::KnownWait(
                            crate::timing::write_wait::WaitKind::BlockingPool,
                        )
            }),
            "RPC pool residence remains measured for {}",
            original.original_id
        );
    }
}

#[test]
fn completed_frames_keep_coalesced_identity_after_owning_work_finishes() {
    let clock = FrameClock::reporting_to(Arc::new(|_| {}));
    let registry = clock.capture_write_waits();
    let received = Arc::new(Mutex::new(Vec::new()));
    let save = received.clone();
    clock.observe_completed_frames(Arc::new(move |sample| save.lock().unwrap().push(sample)));
    let a = registry.begin("first/raw", "thread.post", std::time::Instant::now());
    let b = registry.begin("second/raw", "thread.post", std::time::Instant::now());
    let child = WriteContext::coalesced(&[a.context(), b.context()]);
    let mut timer = clock.frame("agent.deliver");
    timer.set_write_context(Some(child.context()));
    a.complete();
    b.complete();
    child.complete();
    // Looking identities up now would be stale: the timer must have copied them.
    drop(timer);
    drop(clock.frame("agent.poll_status"));
    let received = received.lock().unwrap();
    assert_eq!(received.len(), 2);
    assert_eq!(received[0].original_ids, ["first/raw", "second/raw"]);
    assert_eq!(received[0].protocol, FrameProtocol::RequiredWork);
    assert_eq!(received[0].method, "agent.deliver");
    assert!(received[1].original_ids.is_empty());
    assert_eq!(received[1].protocol, FrameProtocol::Background);
}

#[test]
fn completed_frames_distinguish_mcp_from_rpc_for_the_same_raw_id() {
    let clock = FrameClock::reporting_to(Arc::new(|_| {}));
    clock.capture_write_waits();
    let received = Arc::new(Mutex::new(Vec::new()));
    let save = received.clone();
    clock.observe_completed_frames(Arc::new(move |sample| save.lock().unwrap().push(sample)));
    let report = crate::mcp::DoneReport::new(
        crate::mcp::DoneStatus::Completed,
        "completed-frame protocol witness",
    );
    let frame = json!({"id":"qual/raw/same","report":report});
    let origin = clock.mcp_write(&frame, std::time::Instant::now()).unwrap();
    let timer = clock
        .queued_since(
            crate::timing::Priority::Foreground,
            crate::timing::WaitTimer::start().with_write_origin(Some(origin.clone())),
        )
        .start("mcp.control");
    origin.complete();
    drop(timer);
    let received = received.lock().unwrap();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].original_ids, ["qual/raw/same"]);
    assert_eq!(received[0].protocol, FrameProtocol::Mcp);
    assert_eq!(received[0].method, "mcp.control");
}
