mod followups;
mod next_stage;
use super::*;
use std::sync::mpsc;

struct Case {
    registry: Arc<WriteWaitRegistry>,
    base: Instant,
    nanos: Arc<AtomicU64>,
}
impl Case {
    fn new() -> Self {
        let base = Instant::now();
        let nanos = Arc::new(AtomicU64::new(0));
        let clock = Arc::clone(&nanos);
        let registry = WriteWaitRegistry::with_clock(Arc::new(move || {
            base + Duration::from_nanos(clock.load(Ordering::Acquire))
        }));
        Self {
            registry,
            base,
            nanos,
        }
    }
    fn advance(&self, duration: Duration) {
        self.nanos
            .fetch_add(duration.as_nanos().try_into().unwrap(), Ordering::Release);
    }
    fn seconds(&self, seconds: u64) {
        self.advance(Duration::from_secs(seconds));
    }
    fn begin(&self, original: &str) -> LogicalWrite {
        self.registry.begin(original, "thread.post", self.base)
    }
    fn row(&self, id: WriteId) -> WriteSnapshot {
        self.registry
            .snapshot()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap()
    }
}
fn lock(context: &WriteContext) -> WaitGuard {
    context.interval(IntervalCategory::KnownWait(WaitKind::AppLock))
}

#[test]
fn overlapping_waits_charge_wall_union_and_keep_individual_ids() {
    let case = Case::new();
    let write = case.begin("overlap");
    let context = write.context();
    let first = lock(&context);
    case.seconds(2);
    let second = context.interval(IntervalCategory::KnownWait(WaitKind::StoreWriter));
    let row = case.row(write.id());
    assert_eq!(row.active.len(), 2);
    assert!(row.active.iter().any(|wait| wait.id == first.id()));
    assert!(row.active.iter().any(|wait| wait.id == second.id()));
    case.seconds(2);
    drop(first);
    case.seconds(2);
    drop(second);
    let row = case.row(write.id());
    assert_eq!(row.known_wait_union, Duration::from_secs(6));
    assert_eq!(row.max_chargeable_interval, Duration::from_secs(4));
    assert!(row.cumulative_breached);
    assert!(!row.interval_breached);
    write.complete();
}

#[test]
fn three_two_second_waits_breach_cumulative_without_interval_breach() {
    let case = Case::new();
    let write = case.begin("2+2+2");
    for _ in 0..3 {
        let wait = lock(&write.context());
        case.seconds(2);
        drop(wait);
        case.seconds(1);
    }
    let row = case.row(write.id());
    assert_eq!(row.conservative_upper, Duration::from_secs(6));
    assert_eq!(row.max_chargeable_interval, Duration::from_secs(2));
    assert!(row.cumulative_breached);
    assert!(!row.interval_breached);
    write.complete();
}

#[test]
fn exact_budget_passes_but_open_interval_one_nanosecond_over_is_sticky() {
    let case = Case::new();
    let write = case.begin("strict-boundary");
    let id = write.id();
    let wait = lock(&write.context());
    case.advance(WAIT_BUDGET);
    let exact = case.row(id);
    assert_eq!(exact.conservative_upper, WAIT_BUDGET);
    assert!(!exact.breached());
    case.advance(Duration::from_nanos(1));
    let open = case.row(id);
    assert!(open.interval_breached && open.cumulative_breached);
    drop(wait);
    write.complete();
    case.seconds(10);
    let retained = case.row(id);
    assert!(retained.breached());
    assert_eq!(
        retained.conservative_upper,
        WAIT_BUDGET + Duration::from_nanos(1)
    );
}

#[test]
fn opaque_sql_is_separate_and_conservative_overlap_is_not_hidden() {
    let case = Case::new();
    let write = case.begin("sql-upper");
    let context = write.context();
    let external = context.interval(IntervalCategory::OwnExternal);
    let known = lock(&context);
    case.seconds(1);
    let sql = context.interval(IntervalCategory::SqlService);
    case.seconds(3);
    drop(sql);
    drop(known);
    case.seconds(6);
    drop(external);
    let row = case.row(write.id());
    assert_eq!(row.known_wait_union, Duration::from_secs(4));
    assert_eq!(row.sql_service_union, Duration::from_secs(3));
    assert_eq!(row.blocked_or_sql_union, Duration::from_secs(4));
    assert_eq!(row.conservative_upper, Duration::from_secs(7));
    assert_eq!(row.own_external_union, Duration::from_secs(10));
    assert_eq!(row.max_chargeable_interval, Duration::from_secs(4));
    assert!(row.cumulative_breached);
    assert!(!row.interval_breached);
    write.complete();
}

#[test]
fn own_external_work_alone_never_consumes_wait_budget() {
    let case = Case::new();
    let write = case.begin("external");
    let external = write.context().interval(IntervalCategory::OwnExternal);
    case.seconds(9);
    let row = case.row(write.id());
    assert_eq!(row.own_external_union, Duration::from_secs(9));
    assert_eq!(row.conservative_upper, Duration::ZERO);
    assert!(!row.breached());
    drop(external);
    write.complete();
}

#[test]
fn admitted_and_acknowledged_milestones_do_not_reset_waits_or_children() {
    let case = Case::new();
    let write = case.begin("ack-child");
    let id = write.id();
    let child = write.child();
    let child_id = child.id();
    let wait = lock(&child.context());
    case.seconds(2);
    write.admitted();
    write.acknowledged();
    case.seconds(2);
    write.admitted();
    write.acknowledged();
    write.complete();
    let row = case.row(id);
    assert_eq!(row.admitted_at, Some(case.base + Duration::from_secs(2)));
    assert_eq!(row.acknowledged_at, row.admitted_at);
    assert!(row.root_finished);
    assert_eq!(row.outstanding, vec![child_id]);
    assert_eq!(row.conservative_upper, Duration::from_secs(4));
    case.seconds(2);
    assert!(case.row(id).breached());
    drop(wait);
    child.complete();
    assert!(case.registry.active_snapshot().is_empty());
    assert!(case.row(id).outstanding.is_empty());
}

#[test]
fn cancellation_keeps_unfinished_child_wait_observable() {
    let case = Case::new();
    let write = case.begin("cancelled");
    let id = write.id();
    let child = write.child();
    let wait = child.interval(IntervalCategory::KnownWait(WaitKind::Permit));
    write.cancel();
    case.seconds(6);
    let row = case.row(id);
    assert!(row.cancelled && row.breached());
    assert_eq!(row.outstanding, vec![child.id()]);
    assert_eq!(row.active[0].id, wait.id());
    drop(wait);
    child.cancel();
    let row = case.row(id);
    assert!(row.cancelled && !row.abandoned);
    assert!(row.outstanding.is_empty());
}

#[test]
fn abandoned_obligation_is_sticky_and_never_reported_as_success() {
    let case = Case::new();
    let write = case.begin("abandoned");
    let id = write.id();
    let child = write.child();
    let grandchild = child.child();
    drop(child);
    grandchild.complete();
    write.complete();
    let row = case.row(id);
    assert!(row.abandoned);
    assert!(!row.cancelled);
    assert!(row.outstanding.is_empty());
    assert!(case.registry.active_snapshot().is_empty());
}

#[test]
fn coalesced_child_retains_each_original_after_both_roots_finish() {
    let case = Case::new();
    let first = case.begin("first-wire-id");
    let second = case.begin("second-wire-id");
    let ids = [first.id(), second.id()];
    let child = case.registry.coalesced(&ids);
    let wait = child.interval(IntervalCategory::KnownWait(WaitKind::ResourceLock));
    first.acknowledged();
    first.complete();
    second.complete();
    case.seconds(6);
    for (id, original) in ids.into_iter().zip(["first-wire-id", "second-wire-id"]) {
        let row = case.row(id);
        assert_eq!(row.original_id, original);
        assert_eq!(row.method, "thread.post");
        assert_eq!(row.intake, case.base);
        assert_eq!(row.outstanding, vec![child.id()]);
        assert_eq!(row.active[0].id, wait.id());
        assert!(row.breached());
    }
    drop(wait);
    child.complete();
    assert!(case.registry.active_snapshot().is_empty());
    assert_eq!(case.registry.snapshot().len(), 2);
}

#[test]
fn sync_context_restores_nested_origin_after_unwind() {
    let case = Case::new();
    assert!(current_interval(IntervalCategory::SqlService).is_none());
    let first = case.begin("outer");
    let second = case.begin("inner");
    let outer = first.context().bind_sync();
    assert_eq!(current_context().unwrap().origins(), &[first.id()]);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _inner = second.context().bind_sync();
        assert_eq!(current_context().unwrap().origins(), &[second.id()]);
        let wait = current_interval(IntervalCategory::SqlService).unwrap();
        case.seconds(2);
        drop(wait);
        panic!("exercise restoration");
    }));
    assert!(panic.is_err());
    assert_eq!(current_context().unwrap().origins(), &[first.id()]);
    let wait = current_interval(IntervalCategory::KnownWait(WaitKind::StoreWriter)).unwrap();
    case.seconds(1);
    drop(wait);
    drop(outer);
    assert!(current_interval(IntervalCategory::SqlService).is_none());
    assert_eq!(
        case.row(first.id()).known_wait_union,
        Duration::from_secs(1)
    );
    assert_eq!(
        case.row(second.id()).sql_service_union,
        Duration::from_secs(2)
    );
    first.complete();
    second.complete();
}

#[test]
fn watchdog_reports_open_breach_without_mutator_mutex_or_executor() {
    let case = Case::new();
    let write = case.begin("watchdog");
    let id = write.id();
    let wait = lock(&write.context());
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
    // Release every blocker and join the independent thread before assertions.
    drop(writer);
    drop(watchdog);
    drop(wait);
    write.complete();
    let row = observed.expect("open breach must be visible without collector writer lock");
    assert_eq!(row.id, id);
    assert!(row.breached());
    assert!(!row.active.is_empty());
}

#[test]
fn no_opt_in_has_no_current_origin_and_all_wait_kinds_are_explicit() {
    assert!(WriteWaitRegistry::new().active_snapshot().is_empty());
    assert!(current_interval(IntervalCategory::SqlService).is_none());
    let case = Case::new();
    let write = case.begin("categories");
    for kind in [
        WaitKind::Admission,
        WaitKind::Dispatch,
        WaitKind::Permit,
        WaitKind::BlockingPool,
        WaitKind::AppLock,
        WaitKind::StoreWriter,
        WaitKind::ResourceLock,
    ] {
        let wait = write.context().interval(IntervalCategory::KnownWait(kind));
        case.advance(Duration::from_millis(1));
        drop(wait);
    }
    assert_eq!(
        case.row(write.id()).known_wait_union,
        Duration::from_millis(7)
    );
    write.complete();
}

#[test]
fn completed_archive_does_not_expand_watchdog_active_index() {
    let case = Case::new();
    for original in 0..128 {
        let write = case.begin(&format!("completed-{original}"));
        write.acknowledged();
        write.complete();
    }
    assert_eq!(case.registry.snapshot().len(), 128);
    assert!(case.registry.active_snapshot().is_empty());
    let pending = case.begin("still-pending");
    let active = case.registry.active_snapshot();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, pending.id());
    let id = pending.id();
    drop(pending);
    assert!(case.row(id).abandoned);
    assert!(case.registry.active_snapshot().is_empty());
    assert_eq!(case.registry.snapshot().len(), 129);
}

#[test]
fn finishing_obligations_cannot_hide_an_independently_open_interval() {
    for cancelled in [false, true] {
        let case = Case::new();
        let write = case.begin("open-after-finish");
        let id = write.id();
        let child = write.child();
        let wait = lock(&child.context());
        write.complete();
        if cancelled {
            child.cancel();
        } else {
            child.complete();
        }
        case.seconds(6);
        let active = case.registry.active_snapshot();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, id);
        assert!(active[0].outstanding.is_empty());
        assert_eq!(active[0].active[0].id, wait.id());
        assert!(active[0].breached());
        assert_eq!(active[0].cancelled, cancelled);
        drop(wait);
        assert!(case.registry.active_snapshot().is_empty());
        assert!(case.row(id).breached());
    }
}

#[test]
fn attribution_context_cannot_silently_reopen_fully_finished_work() {
    let case = Case::new();
    let write = case.begin("finished-context");
    let id = write.id();
    let context = write.context();
    write.complete();
    let late_wait = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lock(&context)));
    let late_child = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| context.child()));
    assert!(
        late_wait.is_err(),
        "late interval must explicitly reject attribution misuse"
    );
    assert!(
        late_child.is_err(),
        "late child must explicitly reject attribution misuse"
    );
    assert!(case.registry.active_snapshot().is_empty());
    assert!(case.row(id).outstanding.is_empty());
}

#[test]
fn exclusive_external_work_subtracts_only_overlapping_wait_union() {
    let case = Case::new();
    let write = case.begin("external-overlap");
    let context = write.context();
    // This wait is entirely outside the external span and must not reduce it.
    let outside = lock(&context);
    case.seconds(2);
    drop(outside);
    let external = context.interval(IntervalCategory::OwnExternal);
    case.seconds(2);
    let nested = lock(&context);
    case.seconds(1);
    let sql = context.interval(IntervalCategory::SqlService);
    case.seconds(1);
    drop(nested);
    case.seconds(1);
    drop(sql);
    case.seconds(1);
    drop(external);
    let row = case.row(write.id());
    assert_eq!(row.known_wait_union, Duration::from_secs(4));
    assert_eq!(row.sql_service_union, Duration::from_secs(2));
    assert_eq!(row.blocked_or_sql_union, Duration::from_secs(5));
    assert_eq!(row.own_external_union, Duration::from_secs(6));
    // External [2,8] minus overlapping waits [4,7] leaves three seconds.
    // Subtracting outside wait [0,2] as well would incorrectly leave one.
    assert_eq!(row.own_external_active, Duration::from_secs(3));
    write.complete();
}
