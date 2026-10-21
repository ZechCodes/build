//! Canonical document probes must leave application state available to writes.
use super::*;

async fn final_reply(reads: &mut NativeReads) -> Value {
    loop {
        let envelope = reads.wire.outbound.recv().await.unwrap();
        let reply = SessionSender::decrypt_push(&reads.key, &envelope);
        if reply["id"] == 1 && reply.get("accepted").is_none() {
            break reply;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_plan_doc_directory_probe_does_not_block_a_real_app_lock_request() {
    let mut reads = NativeReads::new();
    let store = reads.state.lock().unwrap().store.clone().unwrap();
    let (probe, waiting) = OffLockGate::new();
    store.set_plan_docs_probe_gate(Some(probe));
    let shared = Arc::clone(&reads.state);
    let maintenance = std::thread::spawn(move || {
        let refresh = shared.lock().unwrap().refresh_snapshot_observations();
        let observed = refresh.load();
        let mut app = shared.lock().unwrap();
        app.install_workspace_review_refresh(observed);
        app.publish_snapshot_projection();
    });
    waiting.wait_for_arrival();
    reads.marks.lock().unwrap().clear();
    let started = Instant::now();
    let answer = tokio::time::timeout(Duration::from_millis(100), async {
        reads
            .wire
            .accept(
                &reads.intake,
                client_request(
                    &reads.key,
                    SESSION,
                    "data",
                    json!({"id":1,"method":"capture.cancel", "params":{"capture_id":reads.capture}}),
                ),
            )
            .await
            .unwrap();
        final_reply(&mut reads).await
    })
    .await;
    let elapsed = started.elapsed();
    // Unblock and join the producer before asserting, including on the red path.
    waiting.release();
    maintenance.join().unwrap();
    store.set_plan_docs_probe_gate(None);
    let replied_before_probe_release = answer.is_ok();
    let answer = match answer {
        Ok(answer) => answer,
        Err(_) => tokio::time::timeout(Duration::from_secs(5), final_reply(&mut reads))
            .await
            .expect("the queued write finishes after probe cleanup"),
    };
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["cancelled"], true, "{answer}");
    let marks = reads.marks.lock().unwrap();
    assert!(
        marks.iter().any(|(phase, _)| *phase == "app_hold_start"),
        "the real writer must acquire the application mutex: {marks:?}"
    );
    eprintln!(
        "{}",
        json!({"directory_probe_stalled_through_reply":replied_before_probe_release,
            "writer":"capture.cancel","reply_us":elapsed.as_micros(),"trace_marks":marks.len()})
    );
    assert!(
        replied_before_probe_release,
        "capture.cancel did not answer within 100 ms while the actual plan-doc directory probe was stalled: {elapsed:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_snapshot_publication_uses_observed_plan_docs_without_probing_directories() {
    let reads = NativeReads::new();
    let store = reads.state.lock().unwrap().store.clone().unwrap();
    let (probe, waiting) = OffLockGate::new();
    store.set_plan_docs_probe_gate(Some(probe));
    let shared = Arc::clone(&reads.state);
    let owner = reads.task.clone();
    let (finished, complete) = mpsc::channel();
    let publication = std::thread::spawn(move || {
        let mut app = shared.lock().unwrap();
        app.refresh_owner_snapshot_observations(&owner);
        app.publish_owner_snapshot_projection(&owner);
        finished.send(()).unwrap();
    });
    let completed = complete.recv_timeout(Duration::from_millis(100)).is_ok();
    let probed = waiting.has_pending_arrival();
    waiting.release();
    publication.join().unwrap();
    store.set_plan_docs_probe_gate(None);
    assert!(
        completed && !probed,
        "owner publication must use existing in-memory document observations (completed={completed}, directory_probe_started={probed})"
    );
}
