use super::*;
use crate::app::tests::tracker::tracked;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_encrypted_write_returns_a_final_reply_and_the_supplied_registry_origin() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, app, project) = tracked(root.path());
    let registry = WriteWaitRegistry::new();
    let rpc = RpcHarness::new(app.shared(), Arc::clone(&registry));
    let original = "qual/rpc-control/write/00000";
    let proof = rpc
        .request(
            0,
            original,
            "tasks.create",
            json!({
                "project_id":project,"title":"qualified actual wire mutation",
            }),
        )
        .await;
    assert_eq!(proof.original_id, original);
    assert_eq!(proof.method, "tasks.create");
    assert_eq!(proof.reply["id"], original);
    assert_eq!(proof.reply["ok"], true, "{}", proof.reply);
    assert!(proof.final_reply_at >= proof.sent_at);
    let task = proof.reply["result"]["task"]["id"].as_str().unwrap();
    let read = rpc
        .request(
            0,
            "qual/rpc-control/read/00000",
            "tasks.get",
            json!({"task_id":task}),
        )
        .await;
    assert_eq!(
        read.reply["result"]["task"]["title"],
        "qualified actual wire mutation"
    );
    rpc.finish().await;
    let row = registry
        .snapshot()
        .into_iter()
        .find(|row| row.original_id == original)
        .expect("actual carrier intake must attach the exact supplied collector");
    assert_eq!(row.method, "tasks.create");
    assert!(row.root_finished && row.outstanding.is_empty() && row.active.is_empty());
    assert!(!row.cancelled && !row.abandoned && !row.breached());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_identical_reads_keep_each_original_final_reply() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, app, project) = tracked(root.path());
    let registry = WriteWaitRegistry::new();
    let rpc = RpcHarness::new(app.shared(), registry);
    let params = json!({"project_id":project,"limit":100});
    let (one, two, three) = tokio::join!(
        rpc.request(0, "qual/fold/read/00001", "tasks.list", params.clone()),
        rpc.request(0, "qual/fold/read/00002", "tasks.list", params.clone()),
        rpc.request(1, "qual/fold/read/00003", "tasks.list", params),
    );
    for (proof, id) in [
        (one, "qual/fold/read/00001"),
        (two, "qual/fold/read/00002"),
        (three, "qual/fold/read/00003"),
    ] {
        assert_eq!(proof.original_id, id);
        assert_eq!(proof.reply["id"], id);
        assert_eq!(proof.reply["ok"], true, "{}", proof.reply);
    }
    rpc.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_encrypted_driver_settles_the_attached_original_client_watch() {
    use super::super::client_watch::ClientWatch;
    let root = tempfile::tempdir().unwrap();
    let (_repo, app, project) = tracked(root.path());
    let registry = WriteWaitRegistry::new();
    let state = app.shared();
    let released = Arc::downgrade(&state);
    let mut rpc = RpcHarness::new(state, registry);
    let watch = ClientWatch::new(Arc::new(Instant::now));
    let original = "qual/driver-watch/write/00000";
    watch.register_at(original, Instant::now());
    let guard = watch.watch();
    rpc.attach_client_watch(watch.clone());
    // This actual offer precedes transport/credit waiting; the adapter may not
    // reset it or turn an acceptance receipt into a final response.
    watch.offer(original);
    let proof = rpc
        .request(
            0,
            original,
            "tasks.create",
            json!({
                "project_id":project,"title":"actual attached-watch mutation"
            }),
        )
        .await;
    // Join actual carrier receivers before checking missing instrumentation.
    rpc.finish().await;
    let cleanup_deadline = Instant::now() + std::time::Duration::from_secs(10);
    while released.upgrade().is_some() && Instant::now() < cleanup_deadline {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    let state_released = released.upgrade().is_none();
    let report = guard.finish();
    let rows = watch.snapshot();
    let row = rows.iter().find(|row| row.original_id == original).unwrap();
    let reconciliation = watch.reconcile_final();
    assert_eq!(proof.reply["ok"], true, "{}", proof.reply);
    assert_eq!(
        proof.reply["result"]["task"]["title"],
        "actual attached-watch mutation"
    );
    assert!(
        state_released,
        "temporary root remains owned until shared work exits"
    );
    assert!(!report.panicked);
    assert!(
        row.sent_at.is_some() && row.final_at.is_some(),
        "actual wire send/decrypted-final callbacks must drive the attached watch: {row:?}"
    );
    assert!(
        reconciliation.violations.is_empty(),
        "{:?}",
        reconciliation.violations
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn completed_bootstrap_can_precede_actual_trial_watch_attachment() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, app, project) = tracked(root.path());
    let registry = WriteWaitRegistry::new();
    let state = app.shared();
    let released = Arc::downgrade(&state);
    let mut rpc = RpcHarness::new(state, Arc::clone(&registry));
    let bootstrap = rpc
        .request(
            0,
            "qual/bootstrap/watch-control",
            "tasks.create",
            json!({"project_id":project,"title":"actual completed bootstrap"}),
        )
        .await;
    let bootstrap_saved = saved_created_task(&rpc, &bootstrap.reply);
    let bootstrap_until = Instant::now() + Duration::from_secs(1);
    while registry.snapshot().iter().any(|row| {
        row.original_id == "qual/bootstrap/watch-control"
            && (!row.root_finished || !row.outstanding.is_empty() || !row.active.is_empty())
    }) && Instant::now() < bootstrap_until
    {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let bootstrap_ready_before_attach = registry.snapshot().iter().any(|row| {
        row.original_id == "qual/bootstrap/watch-control"
            && row.root_finished
            && row.outstanding.is_empty()
            && row.active.is_empty()
            && !row.cancelled
            && !row.abandoned
    });
    let watch = ClientWatch::new(Arc::new(Instant::now));
    let original = "qual/after-bootstrap/write/00000";
    watch.register_at(original, Instant::now());
    let guard = watch.watch();
    let attached = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rpc.attach_client_watch(watch.clone());
    }));
    // A refused prerequisite cannot run dependent trial work. The intended
    // missing behavior is asserted only after every actual receiver is joined.
    let trial =
        if attached.is_ok() {
            watch.offer(original);
            let proof = rpc.request(1, original, "tasks.create",
            json!({"project_id":project,"title":"actual measured mutation after bootstrap"})).await;
            let saved = saved_created_task(&rpc, &proof.reply);
            Some((proof, saved))
        } else {
            None
        };
    let cleanup = rpc.finish_report().await;
    let until = Instant::now() + Duration::from_secs(10);
    while released.upgrade().is_some() && Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let state_released = released.upgrade().is_none();
    let watched = guard.finish();
    let rows = watch.snapshot();
    assert_eq!(bootstrap.reply["ok"], true, "{}", bootstrap.reply);
    assert_eq!(bootstrap_saved.unwrap().title, "actual completed bootstrap");
    assert!(
        bootstrap_ready_before_attach,
        "actual bootstrap root was not settled before attachment"
    );
    assert!(cleanup.is_clean(), "{cleanup:?}");
    assert!(
        state_released,
        "fixture roots stay owned through shared work"
    );
    assert!(!watched.panicked);
    assert!(
        attached.is_ok(),
        "trial watch must attach after actual completed bootstrap requests"
    );
    let (trial, saved) = trial.unwrap();
    assert_eq!(trial.reply["ok"], true, "{}", trial.reply);
    assert_eq!(
        saved.unwrap().title,
        "actual measured mutation after bootstrap"
    );
    let row = rows.iter().find(|row| row.original_id == original).unwrap();
    assert!(row.sent_at.is_some() && row.final_at.is_some(), "{row:?}");
    assert!(watch.reconcile_final().violations.is_empty());
    assert_eq!(
        rows.len(),
        1,
        "bootstrap stays outside the trial caller cohort"
    );
}

fn saved_created_task(rpc: &RpcHarness, reply: &Value) -> Option<crate::tracker::Task> {
    let task = reply["result"]["task"]["id"].as_str()?;
    let app = rpc.state.lock().unwrap();
    app.tracker_store()
        .ok()?
        .load_tracker_task(task)
        .ok()
        .flatten()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_rpc_decoded_notification_precedes_owned_proof_tail() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, app, project) = tracked(root.path());
    let registry = WriteWaitRegistry::new();
    let mut rpc = RpcHarness::new(app.shared(), registry);
    let watch = ClientWatch::new(Arc::new(Instant::now));
    let original = "qual/decoded-credit/write/00000";
    watch.register_at(original, Instant::now());
    let guard = watch.watch();
    let (events, mut decoded) = mpsc::unbounded_channel();
    watch
        .observe_decoded_replies(Arc::new(move |id, actual_at| {
            events.send((id.to_owned(), actual_at)).unwrap();
        }))
        .unwrap();
    rpc.attach_client_watch(watch.clone());
    watch.offer(original);
    let (finish, pending) = oneshot::channel::<()>();
    let (returned, after_reply) = oneshot::channel();
    // This is a real RPC plus a deliberately pending harness proof-tail owner;
    // it does not claim that the product itself has a delayed child here.
    let owned = tokio::spawn(async move {
        let proof = rpc
            .request(
                0,
                original,
                "tasks.create",
                json!({
                    "project_id":project,"title":"actual decoded credit boundary"
                }),
            )
            .await;
        returned.send(()).unwrap();
        pending.await.unwrap();
        let cleanup = rpc.finish_report().await;
        (proof, cleanup)
    });
    after_reply.await.unwrap();
    let first = decoded.try_recv();
    let tail_was_pending = !owned.is_finished();
    // Complete ownership cleanup before an expected notification RED.
    finish.send(()).unwrap();
    let (proof, cleanup) = owned.await.unwrap();
    let watched = guard.finish();
    assert_eq!(proof.reply["ok"], true, "{}", proof.reply);
    assert!(cleanup.is_clean(), "{cleanup:?}");
    assert!(!watched.panicked);
    let (observed_id, observed_at) =
        first.expect("validated decoded RPC final must notify before owned proof tail completion");
    assert_eq!(observed_id, original);
    assert_eq!(observed_at, proof.final_reply_at);
    assert!(tail_was_pending);
    assert!(
        decoded.try_recv().is_err(),
        "one original produces exactly one decoded event"
    );
}
