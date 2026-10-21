//! Real pending/decoded-receiver boundaries; failed finals remain failed evidence.
use super::*;
use crate::app::tests::tracker::tracked;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn detach_refuses_pending_and_active_receiver_and_preserves_failed_finals() {
    crate::git_fixture::environment::isolated_git_test!();
    const PENDING: &str = "qual/detach/pending";
    const RECEIVING: &str = "qual/detach/receiving";
    const FAILED: &str = "qual/detach/failed";
    let root = tempfile::tempdir().unwrap();
    let (_repo, app, project) = tracked(root.path());
    let registry = WriteWaitRegistry::new();
    let state = app.shared();
    let weak = Arc::downgrade(&state);
    let mut rpc = RpcHarness::new(state, registry);
    let watch = ClientWatch::new(Arc::new(Instant::now));
    let due = Instant::now() + Duration::from_millis(10);
    watch.register_at(PENDING, due);
    watch.register_cohort_at(RECEIVING, due, TrafficCohort::Read);
    let (mut receiver_pause, receiver_wait) = Pause::new();
    let receiver_wait = Mutex::new(Some(receiver_wait));
    watch
        .observe_decoded_replies(Arc::new(move |id, _| {
            if id == RECEIVING {
                if let Some(wait) = receiver_wait.lock().unwrap().take() {
                    wait();
                }
            }
        }))
        .unwrap();
    let guard = watch.watch();
    rpc.attach_client_watch(watch.clone());
    let rpc = Arc::new(rpc);
    tokio::time::sleep_until(tokio::time::Instant::from_std(due)).await;
    let (mut app_pause, app_wait) = Pause::new();
    let state = Arc::clone(&rpc.state);
    let held = std::thread::spawn(move || {
        let _app = state.lock().unwrap();
        app_wait();
    });
    let app_reached = app_pause.reached();
    watch.offer(PENDING);
    let sending = Arc::clone(&rpc);
    let request = tokio::spawn(async move {
        sending
            .request(
                0,
                PENDING,
                "tasks.create",
                json!({"project_id":project,"title":"pending detach witness"}),
            )
            .await
    });
    let until = Instant::now() + Duration::from_secs(2);
    while !rpc.pending().iter().any(|id| id == PENDING) && Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let pending_present = rpc.pending().iter().any(|id| id == PENDING);
    let pending_refusal = rpc.detach_client_watch();
    app_pause.release();
    let app_joined = held.join().is_ok();
    let created = request.await.unwrap();
    let created_terminal = server_terminal(&rpc, PENDING).await;
    let task = created.reply["result"]["task"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    watch.offer(RECEIVING);
    let receiving = Arc::clone(&rpc);
    let read = tokio::spawn(async move {
        receiving
            .request(1, RECEIVING, "tasks.get", json!({"task_id":task}))
            .await
    });
    let receiver_reached = receiver_pause.reached();
    let receiver_pending_empty = rpc.pending().is_empty();
    let all_watch_receipts_present = watch.unfinished_final_originals().is_empty();
    let receiver_refusal = rpc.detach_client_watch();
    receiver_pause.release();
    let read = read.await.unwrap();
    let failure_due = Instant::now() + Duration::from_millis(1);
    watch.register_at(FAILED, failure_due);
    tokio::time::sleep_until(tokio::time::Instant::from_std(failure_due)).await;
    watch.offer(FAILED);
    let failed = rpc
        .request(
            2,
            FAILED,
            "tasks.update",
            json!({"task_id":"missing-detach-task","title":"refused"}),
        )
        .await;
    let failed_terminal = server_terminal(&rpc, FAILED).await;
    let watched = guard.finish();
    let detached = detach_when_quiet(&rpc).await;
    // Deliberate late duplicate through real encryption/intake/decoder; no
    // pending entry or new trial original is fabricated for this fault.
    let opening = &rpc.openings[1];
    let envelope = client_request(
        &opening.key,
        &opening.id,
        "data",
        json!({
            "id":RECEIVING,"method":"tasks.get","params":{"task_id":"missing-detach-task"},
        }),
    );
    let duplicate_sent = opening.wire.accept(&rpc.intake, envelope).await.is_ok();
    let until = Instant::now() + Duration::from_secs(2);
    while opening.receive_errors.lock().unwrap().is_empty() && Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let rpc = match Arc::try_unwrap(rpc) {
        Ok(rpc) => rpc,
        Err(_) => panic!("all original senders joined"),
    };
    let cleanup = rpc.finish_report().await;
    let until = Instant::now() + Duration::from_secs(10);
    while weak.upgrade().is_some() && Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    eprintln!(
        "quiet_detach_control={{\"case\":\"pending_and_receiver_faults\",\"qualification_successes\":0}}"
    );
    assert!(
        weak.upgrade().is_none(),
        "native state must be released before roots drop"
    );
    assert!(app_reached && app_joined && pending_present && created_terminal);
    assert!(
        app_pause.timely.load(Ordering::Acquire) && receiver_pause.timely.load(Ordering::Acquire)
    );
    let pending_errors = pending_refusal.expect_err("actual pending original forbids detach");
    assert!(pending_errors.iter().any(|error| error.contains("pending")));
    assert!(receiver_reached && receiver_pending_empty && all_watch_receipts_present);
    let receiver_errors = receiver_refusal.expect_err("active decoded callback forbids detach");
    assert!(
        receiver_errors
            .iter()
            .any(|error| error.contains("in flight")),
        "{receiver_errors:?}"
    );
    assert_eq!(created.reply["ok"], true);
    assert_eq!(read.reply["ok"], true);
    assert_eq!(failed.reply["ok"], false);
    assert!(failed_terminal && !watched.panicked);
    let audit = detached
        .expect("terminal failure permits audited cleanup")
        .unwrap();
    assert_eq!(audit.rows.len(), 3);
    assert!(audit.violations.iter().any(|error| error.contains(FAILED)));
    assert!(
        !audit
            .violations
            .iter()
            .any(|error| error.contains("unmatched"))
    );
    assert!(duplicate_sent);
    assert_eq!(cleanup.joined_receivers, 4);
    assert!(cleanup.pending_at_close.is_empty() && cleanup.pending_after_join.is_empty());
    assert!(
        cleanup
            .errors
            .iter()
            .any(|error| error.contains("duplicate/unknown"))
    );
    assert!(Arc::ptr_eq(
        cleanup.watch_before_cleanup.as_ref().unwrap(),
        &audit
    ));
    let final_audit = cleanup.watch_after_cleanup.unwrap();
    assert_eq!(final_audit.rows.len(), 3);
    assert!(
        final_audit
            .violations
            .iter()
            .any(|error| error.contains(FAILED))
    );
    assert!(
        final_audit
            .violations
            .iter()
            .any(|error| error.contains(RECEIVING) && error.contains("unmatched"))
    );
}
