//! Delayed accounting-callback unit control, paired with native failed/retry
//! Store transitions and delivery effects. No old physical Work is executed.
use super::*;

const OLD: &str = "qual/task-identity/old";
const NEW: &str = "qual/task-identity/new";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delayed_old_attempt_callback_keeps_reused_id_retry_owned() {
    let fixture = Fixture::new();
    fixture.add_publication_peer(PEER);
    let (registry, _) = clock();
    let rpc = RpcHarness::new(Arc::clone(&fixture.state), Arc::clone(&registry));
    let first = retry(&rpc, &fixture, OLD).await;
    let initial = row(&registry, OLD, false).await;
    let (origins, old, failed, failed_board) = {
        let mut app = fixture.state.lock().unwrap();
        let (project, _) = app.tracker_task(&fixture.intent.task_id).unwrap();
        let store = app.tracker_store().unwrap();
        let origins = store.delivery_origins.clone();
        let old = origins.attempt(&fixture.intent.id).unwrap();
        store
            .fail_delivery_intent(
                &fixture.intent.id,
                fixture.intent.generation,
                "delayed accounting callback control",
            )
            .unwrap();
        let failed = store
            .load_delivery_intent(&fixture.intent.id)
            .unwrap()
            .unwrap();
        app.note_tasks_changed(&project, &fixture.intent.task_id);
        let board = board_task(&app.read_view.load(), &fixture.intent.task_id);
        (origins, old, failed, board)
    };
    let second = retry(&rpc, &fixture, NEW).await;
    let newer = row(&registry, NEW, false).await;
    let (new, retried) = {
        let app = fixture.state.lock().unwrap();
        let store = app.tracker_store().unwrap();
        (
            origins.attempt(&fixture.intent.id).unwrap(),
            store
                .load_delivery_intent(&fixture.intent.id)
                .unwrap()
                .unwrap(),
        )
    };
    // Invoke the actual accounting callback retained from the prior attempt.
    // Durable IDs deliberately match; only Arc identity fences its removal.
    origins.finish(&old, false);
    let retained = origins.attempt(&fixture.intent.id);
    let old_done = row(&registry, OLD, true).await;
    let new_pending = row(&registry, NEW, false).await;
    let service = step(&fixture);
    let idle = step(&fixture);
    let new_done = row(&registry, NEW, true).await;
    let effects = finish(fixture, rpc).await;

    eprintln!("task_publication_adversarial={{\"case\":\"delayed_accounting_callback\",\"qualification_successes\":0}}");
    effects.assert_delivery();
    assert_eq!(service, Some(Ok(true)));
    assert_eq!(idle, Some(Ok(false)));
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(second["ok"], true, "{second}");
    assert_eq!(failed.state, "failed");
    assert_eq!(retried.state, "queued");
    assert_eq!(
        failed_board.unwrap()["deferred_assignment"]["state"],
        "failed"
    );
    assert_eq!(
        (&failed.id, failed.generation, &failed.operation_id),
        (&retried.id, retried.generation, &retried.operation_id)
    );
    assert!(!Arc::ptr_eq(&old, &new));
    assert!(retained
        .as_ref()
        .is_some_and(|entry| Arc::ptr_eq(entry, &new)));
    assert_pending(&initial, OLD);
    assert_pending(&newer, NEW);
    assert_pending(&new_pending, NEW);
    let new_pending = new_pending.unwrap();
    assert!(new_pending
        .active
        .iter()
        .any(|span| span.category == IntervalCategory::KnownWait(WaitKind::Dispatch)));
    assert_eq!(new_pending.id, newer.unwrap().id);
    let old_done = old_done.unwrap();
    let new_done = new_done.unwrap();
    assert_finished(&old_done, false);
    assert_finished(&new_done, false);
    assert_eq!(old_done.id, initial.unwrap().id);
    assert_eq!(new_done.id, new_pending.id);
    assert_ne!(old_done.id, new_done.id);
}
