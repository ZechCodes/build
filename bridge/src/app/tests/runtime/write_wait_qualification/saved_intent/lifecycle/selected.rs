//! A native selected service job owns cleanup even after its intent is cancelled.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selected_delivery_keeps_original_through_native_post_selection_cancellation() {
    const ORIGINAL: &str = "qual/lifecycle/selected-before-cancel";
    let fixture = Fixture::new();
    let (registry, _) = clock();
    let rpc = RpcHarness::new(Arc::clone(&fixture.state), Arc::clone(&registry));
    let reply = retry(&rpc, &fixture, ORIGINAL).await;
    let after_ack = row(&registry, ORIGINAL, false).await;
    // Exercise the real service's supported two-phase boundary. Its actual
    // returned work is held, with no app guard and no fabricated ownership.
    let selected = fixture.state.lock().unwrap().prepare_task_delivery();
    let selected_intent = selected
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .map(|work| (work.intent.id.clone(), work.intent.operation_id.clone()));
    let cancelled = rpc
        .request(
            0,
            "qual/lifecycle/selected-cancellation",
            "tasks.assign",
            json!({"task_id":fixture.intent.task_id,"assignee":null}),
        )
        .await
        .reply;
    let observed = task(&rpc, &fixture, "qual/lifecycle/selected-cancelled-read").await;
    let while_selected = row(&registry, ORIGINAL, false).await;
    let state = Arc::clone(&fixture.state);
    let finished = std::thread::spawn(move || {
        let work = selected?.ok_or("actual service did not select the ready retry")?;
        let result = work.git.as_ref().map(crate::app::DeferredGit::run);
        let clock = Arc::clone(&state.lock().unwrap().frame_clock);
        let finished = state
            .lock()
            .unwrap()
            .finish_prepared_task_delivery(work, result);
        crate::app::DeliveryRunner::drain(&state, &clock.frame("qualification.selected_cleanup"));
        finished
    })
    .join()
    .unwrap_or_else(|_| Err("selected service cleanup panicked".into()));
    let second = step(&fixture);
    let terminal = row(&registry, ORIGINAL, true).await;
    let effects = finish(fixture, rpc).await;

    eprintln!(
        "saved_intent_lifecycle_contract_expectations={}",
        json!({
            "case":"selected_cancellation", "offered_semantic":2,
            "expected_cancelled_semantic":1,"expected_cancellation_commands":1,
            "expected_selected_cleanup":1,"qualification_successes":0,
        })
    );
    assert_eq!(reply["ok"], true);
    assert!(after_ack.is_some_and(|row| row.root_finished && !row.outstanding.is_empty()));
    assert_eq!(
        selected_intent,
        Some((
            effects.original.id.clone(),
            effects.original.operation_id.clone()
        ))
    );
    assert_eq!(cancelled["ok"], true);
    assert_eq!(observed["ok"], true);
    assert!(observed["result"]["task"]["assignee"].is_null());
    assert!(observed["result"]["task"]["deferred_assignment"].is_null());
    assert!(
        finished.is_ok(),
        "actual selected cleanup failed: {finished:?}"
    );
    assert_eq!(second, Some(Ok(false)));
    assert!(effects.clean && effects.idle, "{:?}", effects.cleanup_error);
    assert_eq!(effects.saved.state, "cancelled");
    assert!(!effects.receipt);
    assert_eq!((effects.messages, effects.turns, effects.opens), (0, 0, 0));
    let pending = while_selected.expect("same original survives selected service cleanup");
    assert!(pending.root_finished && pending.cancelled, "{pending:?}");
    assert!(
        pending.completed_at.is_none() && !pending.outstanding.is_empty(),
        "{pending:?}"
    );
    assert!(
        !pending
            .active
            .iter()
            .any(|span| span.category == IntervalCategory::KnownWait(WaitKind::Dispatch)),
        "selected cancelled work must not masquerade as queued dispatch: {pending:?}"
    );
    let terminal = terminal.expect("original is archived after native selected cleanup");
    assert_eq!(terminal.id, pending.id);
    assert_finished(&terminal, true);
}
