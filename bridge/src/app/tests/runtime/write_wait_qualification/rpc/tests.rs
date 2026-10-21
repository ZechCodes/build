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
