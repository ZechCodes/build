use super::*;

const FIRST: &str = "qual/task-borrow/first";
const SECOND: &str = "qual/task-borrow/second";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_publication_repartitions_real_borrowers_and_pauses_queue_for_install() {
    let fixture = Fixture::new();
    fixture.add_publication_peer(PEER);
    let base = Instant::now();
    let ticks = Arc::new(AtomicU64::new(0));
    let time = Arc::clone(&ticks);
    let registry = WriteWaitRegistry::with_clock(Arc::new(move || {
        base + Duration::from_secs(time.load(Ordering::SeqCst))
    }));
    let rpc = RpcHarness::new(Arc::clone(&fixture.state), Arc::clone(&registry));
    let first = registry.begin(FIRST, "tasks.update", base);
    let first_context = first.context();
    let initial = fixture.state.lock().unwrap().read_view.load();
    let (first_peer, first_reply) = {
        let _binding = first_context.bind_sync();
        let peer = BorrowedPeer::take(&fixture);
        let reply = update(&fixture, "first borrower title");
        (peer, reply)
    };
    let second = registry.begin(SECOND, "tasks.update", base);
    let second_context = second.context();
    let second_reply = {
        let _binding = second_context.bind_sync();
        update(&fixture, "second borrower title")
    };
    ticks.store(3, Ordering::SeqCst);
    let first_partition = [snapshot(&registry, FIRST), snapshot(&registry, SECOND)];
    first_peer.restore_for_candidate();
    // The same native owner is now borrowed by a different live mutation.
    // A stale own-work cause must not survive this new take_run boundary.
    let second_peer = {
        let _binding = second_context.bind_sync();
        BorrowedPeer::take(&fixture)
    };
    first.admitted();
    first.acknowledged();
    first.complete();
    second.admitted();
    second.acknowledged();
    second.complete();
    ticks.store(9, Ordering::SeqCst);
    let after_ack = [snapshot(&registry, FIRST), snapshot(&registry, SECOND)];
    let stale = fixture.state.lock().unwrap().read_view.load();
    let sampled = Arc::new(Mutex::new(Vec::new()));
    let sample = Arc::clone(&sampled);
    let observed_registry = Arc::clone(&registry);
    let installer = observe_install_receipts(move |_| {
        sample.lock().unwrap().push((
            current_context()
                .map(|context| context.original_ids())
                .unwrap_or_default(),
            [
                snapshot(&observed_registry, FIRST),
                snapshot(&observed_registry, SECOND),
            ],
        ));
    });
    let restored = second_peer.restore();
    drop(installer);
    let accepted = fixture.state.lock().unwrap().read_view.load();
    let terminal = [snapshot(&registry, FIRST), snapshot(&registry, SECOND)];
    let effects = finish(fixture, rpc).await;

    eprintln!("task_publication_adversarial={{\"case\":\"borrow_partition\",\"qualification_successes\":0}}");
    assert!(effects.clean && effects.idle, "{:?}", effects.cleanup_error);
    assert!(restored.is_ok(), "{restored:?}");
    assert_eq!(effects.saved.state, "failed");
    assert!(!effects.receipt);
    assert_eq!((effects.messages, effects.turns, effects.opens), (0, 0, 0));
    assert_eq!(first_reply["ok"], true, "{first_reply}");
    assert_eq!(second_reply["ok"], true, "{second_reply}");
    assert_eq!(
        board_task(&stale, &effects.original.task_id),
        board_task(&initial, &effects.original.task_id)
    );
    assert_eq!(
        board_task(&accepted, &effects.original.task_id).unwrap()["title"],
        "second borrower title"
    );
    assert_partitions(&first_partition, &after_ack);
    let sampled = sampled.lock().unwrap();
    assert!(
        !sampled.is_empty(),
        "the real accepted installer was observed"
    );
    for (origins, rows) in sampled.iter() {
        assert!(rows.iter().all(Option::is_some));
        assert!(origins.iter().any(|id| id == FIRST) && origins.iter().any(|id| id == SECOND));
        for row in rows.iter().flatten() {
            assert!(row.completed_at.is_none() && !row.outstanding.is_empty());
            assert!(!row.active.iter().any(|span| matches!(
                span.category,
                IntervalCategory::OwnExternal | IntervalCategory::KnownWait(WaitKind::Publication)
            )));
        }
    }
    for row in terminal.iter().flatten() {
        assert!(row.root_finished && row.completed_at.is_some());
        assert!(row.outstanding.is_empty() && row.active.is_empty());
        assert!(!row.cancelled && !row.abandoned);
    }
    assert!(terminal.iter().all(Option::is_some));
}

fn update(fixture: &Fixture, title: &str) -> Value {
    fixture.state.lock().unwrap().handle(crate::app::tests::req(
        "tasks.update",
        json!({"task_id":fixture.intent.task_id,"title":title}),
    ))
}
fn assert_partitions(before: &[Option<WriteSnapshot>; 2], after: &[Option<WriteSnapshot>; 2]) {
    let first = before[0].as_ref().unwrap();
    let second = before[1].as_ref().unwrap();
    assert_eq!(first.own_external_union, Duration::from_secs(3));
    assert_eq!(first.known_wait_union, Duration::ZERO);
    assert_eq!(second.known_wait_union, Duration::from_secs(3));
    assert_eq!(second.own_external_union, Duration::ZERO);
    assert_pending(&after[0], FIRST);
    assert_pending(&after[1], SECOND);
    let first = after[0].as_ref().unwrap();
    let second = after[1].as_ref().unwrap();
    assert_eq!(first.own_external_union, Duration::from_secs(3));
    assert_eq!(first.known_wait_union, Duration::from_secs(6));
    assert!(first.breached());
    assert_eq!(second.known_wait_union, Duration::from_secs(3));
    assert_eq!(second.own_external_union, Duration::from_secs(6));
    assert!(!second.breached());
}
