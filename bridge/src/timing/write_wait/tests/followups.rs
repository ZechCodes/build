use super::*;

#[test]
fn classification_preserves_original_record_open_wait_and_sticky_budget() {
    for cohort in [
        WriteCohort::SemanticWrite,
        WriteCohort::ExemptRead,
        WriteCohort::ExemptProtocol,
    ] {
        let case = Case::new();
        let write = case.registry.begin_pending("opaque-correlation", case.base);
        let id = write.id();
        let wait = lock(&write.context());
        case.seconds(6);
        let pending = case.row(id);
        assert_eq!(pending.cohort, WriteCohort::PendingIntake);
        write.classify("native-72202", "thread.post", cohort);
        let classified = case.row(id);
        assert_eq!(classified.original_id, "native-72202");
        assert_eq!(classified.method, "thread.post");
        assert_eq!(classified.cohort, cohort);
        assert_eq!(classified.id, id);
        assert_eq!(classified.intake, case.base);
        assert_eq!(classified.active[0].id, wait.id());
        assert_eq!(classified.conservative_upper, Duration::from_secs(6));
        assert!(pending.breached() && classified.breached());
        drop(wait);
        write.complete();
        let archived = case.row(id);
        assert_eq!(archived.cohort, cohort);
        assert!(archived.breached(), "exemption never erases raw evidence");
    }
}

#[test]
fn classification_is_once_only_and_rejection_does_not_poison_registry() {
    let case = Case::new();
    let write = case.registry.begin_pending("opaque", case.base);
    write.classify("native", "user.present", WriteCohort::SemanticWrite);
    write.classify("native", "user.present", WriteCohort::SemanticWrite);
    let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        write.classify("replacement", "thread.post", WriteCohort::ExemptRead);
    }));
    assert!(
        rejected.is_err(),
        "conflicting reclassification must be explicit misuse"
    );
    let row = case.row(write.id());
    assert_eq!(row.original_id, "native");
    assert_eq!(row.method, "user.present");
    assert_eq!(row.cohort, WriteCohort::SemanticWrite);
    write.complete();
    let unrelated = case.begin("still-usable");
    unrelated.complete();
    assert_eq!(case.registry.snapshot().len(), 2);
}

#[test]
fn original_ids_lookup_preserves_coalesced_identity_after_roots_finish() {
    let case = Case::new();
    let first = case.begin("qual/small/trial1/write/00001");
    let second = case.begin("qual/small/trial1/write/00002");
    let child = case
        .registry
        .coalesced(&[first.id(), second.id(), first.id()]);
    first.complete();
    second.complete();
    assert_eq!(
        child.context().original_ids(),
        vec![
            "qual/small/trial1/write/00001".to_owned(),
            "qual/small/trial1/write/00002".to_owned(),
        ]
    );
    child.complete();
}

#[test]
fn raw_interval_export_retains_exact_shared_ids_kinds_and_times() {
    let case = Case::new();
    let first = case.begin("first");
    let second = case.begin("second");
    let origins = vec![first.id(), second.id()];
    let child = case.registry.coalesced(&origins);
    let obligation = child.id();
    let known = child.interval(IntervalCategory::KnownWait(WaitKind::StoreWriter));
    let known_id = known.id();
    case.seconds(2);
    drop(known);
    let sql = child.interval(IntervalCategory::SqlService);
    let sql_id = sql.id();
    case.seconds(1);
    drop(sql);
    first.complete();
    second.complete();
    child.complete();
    let intervals = case.registry.completed_intervals();
    assert_eq!(
        intervals.len(),
        2,
        "coalesced intervals are stored once, not perorigin"
    );
    assert_eq!(
        intervals[0],
        CompletedInterval {
            id: known_id,
            origins: origins.clone(),
            obligation,
            category: IntervalCategory::KnownWait(WaitKind::StoreWriter),
            started: case.base,
            ended: case.base + Duration::from_secs(2),
        }
    );
    assert_eq!(
        intervals[1],
        CompletedInterval {
            id: sql_id,
            origins,
            obligation,
            category: IntervalCategory::SqlService,
            started: case.base + Duration::from_secs(2),
            ended: case.base + Duration::from_secs(3),
        }
    );
    assert!(case.registry.active_snapshot().is_empty());
    assert_eq!(case.registry.snapshot().len(), 2);
}

#[test]
fn collector_reports_its_own_mutator_overhead_separately() {
    let case = Case::new();
    let write = case.begin("overhead");
    let wait = lock(&write.context());
    case.seconds(1);
    drop(wait);
    write.complete();
    let overhead = case.registry.collector_overhead();
    assert_eq!(overhead.writer_acquisitions, 4);
    assert!(overhead.writer_mutation_hold > Duration::ZERO);
    assert!(overhead.max_writer_lock_wait <= overhead.writer_lock_wait);
    assert!(overhead.max_writer_mutation_hold <= overhead.writer_mutation_hold);
    let _raw = case.registry.completed_intervals();
    let _rows = case.registry.snapshot();
    assert_eq!(
        case.registry.collector_overhead(),
        overhead,
        "exports are outside measured mutators"
    );
}

fn retained_snapshot(close_at: u64, evaluate_at: u64) -> (WriteSnapshot, WaitId) {
    let case = Case::new();
    let write = case.begin("retained-active-root");
    let id = write.id();
    let wait = lock(&write.context());
    let wait_id = wait.id();
    let retained = case.registry.capture_active();
    case.seconds(close_at);
    let writer = case.registry.writer.lock().unwrap();
    let close = std::thread::spawn(move || drop(wait));
    let deadline = Instant::now();
    while case.registry.closing.load(Ordering::Acquire) != wait_id.0
        && deadline.elapsed() < Duration::from_secs(1)
    {
        std::thread::yield_now();
    }
    let arrived = case.registry.closing.load(Ordering::Acquire) == wait_id.0;
    if evaluate_at > close_at {
        case.seconds(evaluate_at - close_at);
    }
    let captured = retained.snapshot_at(case.base + Duration::from_secs(evaluate_at));
    // Unblock and join before assertions, including failure of the stage witness.
    drop(writer);
    close.join().unwrap();
    write.complete();
    assert!(
        arrived,
        "guard close reached the existing collector acquisition"
    );
    (
        captured.into_iter().find(|row| row.id == id).unwrap(),
        wait_id,
    )
}

#[test]
fn retained_root_caps_closed_four_second_wait_before_blocked_mutation() {
    let (row, _) = retained_snapshot(4, 10);
    assert_eq!(row.known_wait_union, Duration::from_secs(4));
    assert!(row.active.is_empty());
    assert!(
        !row.breached(),
        "stale immutable root must not extend an ended interval"
    );
}

#[test]
fn retained_root_still_latches_real_six_second_breach_after_guard_closes() {
    let (row, _) = retained_snapshot(6, 10);
    assert_eq!(row.known_wait_union, Duration::from_secs(6));
    assert!(row.active.is_empty());
    assert!(row.interval_breached && row.cumulative_breached);
}

#[test]
fn retained_root_evaluation_before_close_keeps_interval_active_at_that_time() {
    let (row, wait_id) = retained_snapshot(6, 4);
    assert_eq!(row.known_wait_union, Duration::from_secs(4));
    assert_eq!(row.active[0].id, wait_id);
    assert!(
        !row.breached(),
        "later close must not rewrite the sampled past"
    );
}

#[test]
fn unclassified_intake_remains_visible_to_independent_watchdog() {
    let case = Case::new();
    let write = case.registry.begin_pending("opaque-arrival", case.base);
    let id = write.id();
    let wait = write
        .context()
        .interval(IntervalCategory::KnownWait(WaitKind::Admission));
    let (send, receive) = mpsc::channel();
    let watchdog = case.registry.watchdog(
        Duration::from_millis(1),
        Arc::new(move |row| {
            let _ = send.send(row);
        }),
    );
    let writer = case.registry.writer.lock().unwrap();
    case.seconds(6);
    let observed = receive.recv_timeout(Duration::from_secs(1));
    drop(writer);
    drop(watchdog);
    drop(wait);
    write.cancel();
    let row = observed.expect("pending intake must stay visible before decryption/classification");
    assert_eq!(row.id, id);
    assert_eq!(row.cohort, WriteCohort::PendingIntake);
    assert_eq!(row.method, "rpc.unclassified");
    assert_eq!(row.intake, case.base);
    assert!(row.breached());
}
