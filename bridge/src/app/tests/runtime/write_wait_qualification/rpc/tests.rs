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
