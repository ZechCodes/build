use super::*;

const A: &str = "qual/task-cuts/retry";
const B: &str = "qual/task-cuts/cancel";
const C: &str = "qual/task-cuts/title-one";
const D: &str = "qual/task-cuts/title-two";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn carried_task_cuts_reject_false_credit_and_newer_full_cut_coalesces() {
    let fixture = Fixture::new();
    fixture.add_publication_peer(PEER);
    let (registry, _) = clock();
    let rpc = RpcHarness::new(Arc::clone(&fixture.state), Arc::clone(&registry));
    let retry_reply = retry(&rpc, &fixture, A).await;
    let baseline = fixture.state.lock().unwrap().read_view.load();
    let old = prepare(&fixture);
    let peer = BorrowedPeer::take(&fixture);
    let cancellation = rpc
        .request(
            0,
            B,
            "tasks.assign",
            json!({
                "task_id":fixture.intent.task_id,"assignee":null,
            }),
        )
        .await
        .reply;
    let first = rpc
        .request(
            0,
            C,
            "tasks.update",
            json!({
                "task_id":fixture.intent.task_id,"title":"first pending task cut",
            }),
        )
        .await
        .reply;
    let second = rpc
        .request(
            0,
            D,
            "tasks.update",
            json!({
                "task_id":fixture.intent.task_id,"title":"newest coalesced task cut",
            }),
        )
        .await
        .reply;
    let sql = task(&rpc, &fixture, "qual/task-cuts/sql").await;
    let blocked = fixture.state.lock().unwrap().read_view.load();
    for id in [A, B, C, D] {
        let _ = row(&registry, id, false).await;
    }
    peer.restore_for_candidate();

    // Old capture is really accepted, but its queued task does not cover any
    // of the later cancellation/title mutations now durable in SQL.
    publish(&fixture, old);
    let older = fixture.state.lock().unwrap().read_view.load();
    let mut phases = vec![("older accepted", capture(&registry))];
    let dropped = prepare(&fixture);
    drop(dropped);
    phases.push(("dropped", capture(&registry)));

    // Isolate board replacement: no later setter can conceal a missing
    // board-put invalidation. The full capture below repairs this deliberate
    // board/cache disagreement using native authoritative preparation.
    let board_only = prepare(&fixture).put(
        "board.list",
        "",
        baseline.get_owned("board.list", "").unwrap(),
    );
    publish(&fixture, board_only);
    let board_only_view = fixture.state.lock().unwrap().read_view.load();
    phases.push(("board overwritten", capture(&registry)));

    // Independently replace only the global cache with an older captured
    // revision. A correct board setter must not hide a missing cache fence.
    let global_only = prepare(&fixture).set_global(baseline.global.as_ref().unwrap().clone());
    publish(&fixture, global_only);
    let global_only_view = fixture.state.lock().unwrap().read_view.load();
    phases.push(("global overwritten", capture(&registry)));

    // Overwrite via the real Delta setters, preserving a coherent old board,
    // facts and global cache. This must invalidate the newer carried receipt.
    let overwritten = prepare(&fixture)
        .put(
            "board.list",
            "",
            baseline.get_owned("board.list", "").unwrap(),
        )
        .set_facts(Arc::clone(&baseline.facts))
        .set_global(baseline.global.as_ref().unwrap().clone());
    publish(&fixture, overwritten);
    let overwritten_view = fixture.state.lock().unwrap().read_view.load();
    phases.push(("overwritten", capture(&registry)));

    let rejected = prepare(&fixture);
    let (advanced_ok, advanced) = {
        let app = fixture.state.lock().unwrap();
        let current = app.read_view.load();
        // An actual newer cache-only CAS has no task capture or receipt.
        let accepted = app.read_view.publish_delta(
            SnapshotDelta::new(app.read_view.next_version())
                .set_global(current.global.as_ref().unwrap().clone()),
        );
        (accepted, app.read_view.load())
    };
    publish(&fixture, rejected);
    let after_rejection = fixture.state.lock().unwrap().read_view.load();
    phases.push(("rejected", capture(&registry)));

    // One actual newest full capture covers cancellation and both title writes.
    publish(&fixture, prepare(&fixture));
    let final_view = fixture.state.lock().unwrap().read_view.load();
    let mut terminal = Vec::new();
    for id in [A, B, C, D] {
        terminal.push(row(&registry, id, true).await);
    }
    let restored = settle_peer(&fixture);
    let service = step(&fixture);
    let effects = finish(fixture, rpc).await;

    eprintln!(
        "task_publication_adversarial={{\"case\":\"carried_cuts\",\"qualification_successes\":0}}"
    );
    for reply in [&retry_reply, &cancellation, &first, &second, &sql] {
        assert_eq!(reply["ok"], true, "{reply}");
    }
    assert_no_delivery(&effects);
    assert!(restored.is_ok(), "{restored:?}");
    assert_eq!(service, Some(Ok(false)));
    assert_eq!(sql["result"]["task"]["title"], "newest coalesced task cut");
    assert!(sql["result"]["task"]["deferred_assignment"].is_null());
    let old_row = board_task(&baseline, &effects.original.task_id).unwrap();
    assert_eq!(old_row["deferred_assignment"]["state"], "queued");
    for retained in [
        &baseline,
        &blocked,
        &older,
        &board_only_view,
        &overwritten_view,
        &after_rejection,
    ] {
        assert_eq!(
            board_task(retained, &effects.original.task_id),
            Some(old_row.clone())
        );
    }
    assert_eq!(
        board_task(&global_only_view, &effects.original.task_id).unwrap()["title"],
        "newest coalesced task cut"
    );
    assert!(advanced_ok && Arc::ptr_eq(&advanced, &after_rejection));
    let final_row = board_task(&final_view, &effects.original.task_id).unwrap();
    assert_eq!(final_row["title"], "newest coalesced task cut");
    assert!(final_row["assignee"].is_null() && final_row["deferred_assignment"].is_null());
    assert_phases(&phases, &terminal);
}

fn capture(registry: &WriteWaitRegistry) -> Vec<Option<WriteSnapshot>> {
    [A, B, C, D]
        .into_iter()
        .map(|id| snapshot(registry, id))
        .collect()
}
fn assert_phases(
    phases: &[(&str, Vec<Option<WriteSnapshot>>)],
    terminal: &[Option<WriteSnapshot>],
) {
    for (phase, rows) in phases {
        for (id, row) in [A, B, C, D].into_iter().zip(rows) {
            assert_pending(row, id);
            assert!(
                row.as_ref().unwrap().active.iter().any(|span| {
                    span.category == IntervalCategory::KnownWait(WaitKind::Publication)
                }),
                "phase={phase}, original={id}, row={row:?}"
            );
        }
    }
    for (index, row) in terminal.iter().enumerate() {
        let row = row
            .as_ref()
            .expect("accepted newest cut archives each original");
        assert_finished(row, index == 0);
        assert_eq!(row.id, phases[0].1[index].as_ref().unwrap().id);
    }
}
