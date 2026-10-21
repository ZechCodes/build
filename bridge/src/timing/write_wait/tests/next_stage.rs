use super::*;

#[test]
fn subset_intervals_preserve_one_live_owner_and_exact_causal_origins() {
    let case = Case::new();
    let first = case.begin("retirement-cause");
    let second = case.begin("retirement-predecessor");
    let ids = [first.id(), second.id()];
    let joined = WriteContext::coalesced(&[first.context(), second.context()]);
    let owner = joined.id();
    first.complete();
    second.complete();
    let context = joined.context();
    let own = context.interval_for(&[ids[0], ids[0]], IntervalCategory::OwnExternal);
    let predecessor =
        context.interval_for(&[ids[1]], IntervalCategory::KnownWait(WaitKind::Retirement));
    case.seconds(6);
    let first = case.row(ids[0]);
    let second = case.row(ids[1]);
    drop(own);
    drop(predecessor);
    joined.complete();
    assert_eq!(first.outstanding, vec![owner]);
    assert_eq!(second.outstanding, vec![owner]);
    assert_eq!(first.known_wait_union, Duration::ZERO);
    assert_eq!(first.own_external_union, Duration::from_secs(6));
    assert!(!first.breached());
    assert_eq!(second.known_wait_union, Duration::from_secs(6));
    assert!(second.breached());
    let raw = case.registry.completed_intervals();
    assert_eq!(raw.len(), 2);
    assert_eq!(raw[0].origins, vec![ids[0]]);
    assert_eq!(raw[1].origins, vec![ids[1]]);
    assert!(raw.iter().all(|row| row.obligation == owner));
}

#[test]
fn invalid_interval_subsets_leave_owner_and_collector_usable() {
    let case = Case::new();
    let one = case.begin("one");
    let other = case.begin("other");
    let context = one.context();
    for subset in [vec![], vec![other.id()], vec![one.id(), other.id()]] {
        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            context.interval_for(&subset, IntervalCategory::OwnExternal);
        }));
        assert!(rejected.is_err());
    }
    assert!(case.registry.completed_intervals().is_empty());
    assert!(case.row(one.id()).active.is_empty());
    let wait = context.interval_for(&[one.id()], IntervalCategory::OwnExternal);
    case.seconds(1);
    drop(wait);
    one.complete();
    other.complete();
    assert_eq!(case.registry.completed_intervals().len(), 1);
    assert!(case.registry.active_snapshot().is_empty());
}

#[test]
fn explicit_optional_notice_preserves_raw_wait_without_qualification_breach() {
    let case = Case::new();
    let origin = case.begin("published-source");
    let notice = origin.context().begin_optional_notice(
        "audited-optional-notice",
        "optional.complete_reminder",
        case.base,
    );
    let id = notice.id();
    assert_ne!(id, origin.id());
    let wait = lock(&notice.context());
    case.seconds(6);
    let active = case.row(id);
    drop(wait);
    notice.complete();
    origin.complete();
    assert_eq!(active.cohort, WriteCohort::OptionalNotice);
    assert_eq!(active.conservative_upper, Duration::from_secs(6));
    assert_eq!(active.max_chargeable_interval, Duration::from_secs(6));
    assert!(!active.breached());
    assert!(!case.registry.has_breaches());
    assert_eq!(case.registry.completed_intervals()[0].origins, vec![id]);
    assert_eq!(
        case.row(id).completed_at,
        Some(case.base + Duration::from_secs(6))
    );
}

#[test]
fn optional_coalesced_work_cannot_exempt_its_required_original() {
    let case = Case::new();
    let required = case.begin("required");
    let optional =
        required
            .context()
            .begin_optional_notice("optional", "optional.notice", case.base);
    let ids = [required.id(), optional.id()];
    let joined = WriteContext::coalesced(&[required.context(), optional.context()]);
    let wait = lock(&joined.context());
    required.complete();
    optional.complete();
    case.seconds(6);
    let required = case.row(ids[0]);
    let optional = case.row(ids[1]);
    drop(wait);
    joined.complete();
    assert_eq!(required.conservative_upper, Duration::from_secs(6));
    assert_eq!(optional.conservative_upper, Duration::from_secs(6));
    assert!(required.breached() && case.registry.has_breaches());
    assert!(!optional.breached());
    assert_eq!(optional.cohort, WriteCohort::OptionalNotice);
}

#[test]
fn terminal_timestamp_is_last_child_settlement_not_ack_or_report_time() {
    let case = Case::new();
    let write = case.begin("terminal-child");
    let id = write.id();
    let child = write.child();
    case.seconds(1);
    write.acknowledged();
    write.complete();
    assert_eq!(case.row(id).completed_at, None);
    case.seconds(2);
    child.cancel();
    case.seconds(10);
    let row = case.row(id);
    assert!(row.cancelled && row.root_finished);
    assert_eq!(
        row.acknowledged_at,
        Some(case.base + Duration::from_secs(1))
    );
    assert_eq!(row.completed_at, Some(case.base + Duration::from_secs(3)));
}

#[test]
fn terminal_timestamp_uses_guard_end_before_delayed_collector_mutation() {
    let case = Case::new();
    let write = case.begin("terminal-open-interval");
    let id = write.id();
    let wait = lock(&write.context());
    let wait_id = wait.id();
    case.seconds(1);
    write.complete();
    assert_eq!(case.row(id).completed_at, None);
    case.seconds(3);
    let writer = case.registry.writer.lock().unwrap();
    let close = std::thread::spawn(move || drop(wait));
    let deadline = Instant::now();
    while case.registry.closing.load(Ordering::Acquire) != wait_id.0
        && deadline.elapsed() < Duration::from_secs(1)
    {
        std::thread::yield_now();
    }
    let arrived = case.registry.closing.load(Ordering::Acquire) == wait_id.0;
    case.seconds(6);
    drop(writer);
    close.join().unwrap();
    assert!(
        arrived,
        "end cap precedes the blocked collector acquisition"
    );
    let row = case.row(id);
    assert_eq!(row.known_wait_union, Duration::from_secs(4));
    assert_eq!(row.completed_at, Some(case.base + Duration::from_secs(4)));
    assert!(!row.breached());
}

#[test]
fn policy_utilities_use_the_existing_registry_and_configured_clock() {
    let case = Case::new();
    let other = Case::new();
    let write = case.begin("policy-source");
    let context = write.context();
    assert!(context.belongs_to(&case.registry));
    assert!(!context.belongs_to(&other.registry));
    case.seconds(3);
    assert_eq!(case.registry.now(), case.base + Duration::from_secs(3));
    write.complete();
}
