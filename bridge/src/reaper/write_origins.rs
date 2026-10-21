//! OFFSOURCE child of reaper.rs: causal waits and required session retirement.
use super::*;
use crate::harness::{AgentStatus, HarnessError, Turn};
use crate::test_support::off_lock::{OffLockGate, OffLockGateHandle};
use crate::timing::write_wait::{
    IntervalCategory, LogicalWrite, WaitKind, WriteSnapshot, WriteWaitRegistry,
};
use std::sync::mpsc;
use std::time::Instant;

const SAFETY: Duration = Duration::from_secs(20);
const OBSERVE: Duration = Duration::from_millis(750);

struct EndingSession(OffLockGate);
impl AgentSession for EndingSession {
    fn send_turn(&self, _: &Turn) -> Result<(), HarnessError> {
        Ok(())
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Waiting
    }
    fn quiet_for(&self) -> Duration {
        Duration::ZERO
    }
    fn exited_within(&self, _: Duration) -> bool {
        false
    }
    fn end(&self) {
        self.0.arrive();
    }
    fn backdate_last_output(&self, _: Duration) {}
}
fn root(registry: &Arc<WriteWaitRegistry>, name: &str) -> LogicalWrite {
    let root = registry.begin(name, "retirement-test", Instant::now());
    root.admitted();
    root
}
fn pending_session() -> (Arc<dyn AgentSession>, OffLockGateHandle) {
    let (gate, held) = OffLockGate::new();
    (Arc::new(EndingSession(gate)), held)
}
fn row(registry: &WriteWaitRegistry, original: &str) -> WriteSnapshot {
    registry
        .snapshot()
        .into_iter()
        .find(|row| row.original_id == original)
        .unwrap()
}
fn observe(
    registry: &WriteWaitRegistry,
    original: &str,
    predicate: impl Fn(&WriteSnapshot) -> bool,
) -> WriteSnapshot {
    let deadline = Instant::now() + OBSERVE;
    loop {
        let snapshot = row(registry, original);
        if predicate(&snapshot) || Instant::now() >= deadline {
            return snapshot;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn active(row: &WriteSnapshot, category: IntervalCategory) -> bool {
    row.active
        .iter()
        .any(|interval| interval.category == category)
}
fn assert_clean(row: &WriteSnapshot) {
    assert!(row.root_finished && row.outstanding.is_empty(), "{row:?}");
    assert!(!row.abandoned && !row.cancelled, "{row:?}");
}

#[test]
fn logical_write_retirement_own_external_child_survives_root_reply() {
    let registry = WriteWaitRegistry::new();
    let original = root(&registry, "own-retirement");
    let (session, held) = pending_session();
    let retirement = {
        let _binding = original.context().bind_sync();
        Retirement::begin(session)
    };
    held.wait_for_arrival();
    original.complete();
    let pending = observe(&registry, "own-retirement", |row| {
        active(row, IntervalCategory::OwnExternal)
    });
    held.release();
    let reaped = retirement.wait(SAFETY);
    let finished = observe(&registry, "own-retirement", |row| {
        row.outstanding.is_empty()
    });
    assert!(
        reaped,
        "real end and reap callback finished before assertions"
    );
    assert!(
        pending.root_finished && !pending.outstanding.is_empty(),
        "{pending:?}"
    );
    assert!(
        active(&pending, IntervalCategory::OwnExternal),
        "{pending:?}"
    );
    assert!(
        !active(
            &pending,
            IntervalCategory::KnownWait(WaitKind::ResourceLock)
        ),
        "own reap is service"
    );
    assert_clean(&finished);
}

#[test]
fn logical_write_retirement_same_origin_condition_stays_own_external() {
    let registry = WriteWaitRegistry::new();
    let original = root(&registry, "own-condition");
    let caller_obligation = row(&registry, "own-condition").outstanding[0];
    let (session, held) = pending_session();
    let retirement = {
        let _binding = original.context().bind_sync();
        Retirement::begin(session)
    };
    held.wait_for_arrival();
    let context = original.context();
    let (started, ready) = mpsc::channel();
    let waiting = std::thread::spawn(move || {
        let _binding = context.bind_sync();
        let _ = started.send(());
        retirement.wait(SAFETY)
    });
    let reached = ready.recv_timeout(SAFETY).is_ok();
    let during_condition = observe(&registry, "own-condition", |row| {
        row.active.iter().any(|interval| {
            interval.obligation == caller_obligation
                && interval.category == IntervalCategory::OwnExternal
        })
    });
    held.release();
    let reaped = waiting.join().expect("owned waiter joined");
    original.complete();
    let finished = observe(&registry, "own-condition", |row| row.outstanding.is_empty());
    assert!(reached && reaped);
    assert!(
        during_condition
            .active
            .iter()
            .any(|interval| interval.obligation == caller_obligation
                && interval.category == IntervalCategory::OwnExternal),
        "actual caller condition entry: {during_condition:?}"
    );
    assert!(
        !active(
            &during_condition,
            IntervalCategory::KnownWait(WaitKind::ResourceLock)
        ),
        "{during_condition:?}"
    );
    assert!(
        !during_condition
            .active
            .iter()
            .any(|interval| interval.obligation == caller_obligation
                && matches!(interval.category, IntervalCategory::KnownWait(_))),
        "the caller's own condition cannot also be charged as predecessor: {during_condition:?}"
    );
    assert_clean(&finished);
}

fn predecessor(different_epoch: bool) {
    let registry = WriteWaitRegistry::new();
    let waiter_registry = if different_epoch {
        WriteWaitRegistry::new()
    } else {
        Arc::clone(&registry)
    };
    let retiring = root(&registry, "predecessor-owner");
    let waiter = root(&waiter_registry, "later-waiter");
    let (session, held) = pending_session();
    let retirement = {
        let _binding = retiring.context().bind_sync();
        Retirement::begin(session)
    };
    held.wait_for_arrival();
    if different_epoch {
        assert_eq!(
            retiring.id(),
            waiter.id(),
            "numeric IDs collide across collectors"
        );
    }
    retiring.complete();
    let context = waiter.context();
    let (started, ready) = mpsc::channel();
    let waiting = std::thread::spawn(move || {
        let _binding = context.bind_sync();
        let _ = started.send(());
        retirement.wait(SAFETY)
    });
    let reached = ready.recv_timeout(SAFETY).is_ok();
    let category = IntervalCategory::KnownWait(WaitKind::Retirement);
    let blocked = observe(&waiter_registry, "later-waiter", |row| {
        active(row, category)
    });
    let own = row(&registry, "predecessor-owner");
    held.release();
    let reaped = waiting.join().expect("owned predecessor waiter joined");
    waiter.complete();
    let owner_finished = observe(&registry, "predecessor-owner", |row| {
        row.outstanding.is_empty()
    });
    let waiter_finished = row(&waiter_registry, "later-waiter");
    assert!(reached && reaped);
    assert!(
        active(&blocked, category),
        "another write's reap is predecessor wait: {blocked:?}"
    );
    assert!(
        active(&own, IntervalCategory::OwnExternal),
        "origin retains its own external child: {own:?}"
    );
    assert_clean(&owner_finished);
    assert_clean(&waiter_finished);
}
#[test]
fn logical_write_retirement_other_origin_condition_is_predecessor_wait() {
    predecessor(false);
}
#[test]
fn logical_write_retirement_colliding_registry_ids_are_not_same_cause() {
    predecessor(true);
}

#[test]
fn logical_write_retirement_exclusion_mutex_wait_is_always_known() {
    let (session, held) = pending_session();
    let retirement = Retirement::begin(session);
    held.wait_for_arrival();
    let registry = WriteWaitRegistry::new();
    let original = root(&registry, "retirement-mutex");
    let context = original.context();
    let leaf = Arc::clone(&retirement.reaped);
    let (locked, lock_ready) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _guard = leaf.0.lock().unwrap();
        let _ = locked.send(());
        let _ = released.recv_timeout(SAFETY);
    });
    let holder_ready = lock_ready.recv_timeout(SAFETY).is_ok();
    let waiting = std::thread::spawn(move || {
        let _binding = context.bind_sync();
        retirement.wait(SAFETY)
    });
    let blocked = observe(&registry, "retirement-mutex", |row| {
        active(row, IntervalCategory::KnownWait(WaitKind::ResourceLock))
    });
    let _ = release.send(());
    holder
        .join()
        .expect("raw holder joined without poisoning leaf");
    held.release();
    let reaped = waiting.join().expect("owned raw-mutex waiter joined");
    original.complete();
    let finished = row(&registry, "retirement-mutex");
    assert!(holder_ready && reaped);
    assert!(
        active(
            &blocked,
            IntervalCategory::KnownWait(WaitKind::ResourceLock)
        ),
        "{blocked:?}"
    );
    assert!(
        !active(&blocked, IntervalCategory::OwnExternal),
        "blocking acquisition is not own service"
    );
    assert_clean(&finished);
}

/// A shared condition has different causal attribution for its two originals.
#[test]
fn logical_write_retirement_joined_originals_split_own_and_predecessor_condition() {
    use crate::timing::write_wait::WriteContext;
    let registry = WriteWaitRegistry::new();
    let owner = root(&registry, "joined-cause");
    let peer = root(&registry, "joined-peer");
    let (session, held) = pending_session();
    let retirement = {
        let _binding = owner.context().bind_sync();
        Retirement::begin(session)
    };
    held.wait_for_arrival();
    let joined = WriteContext::coalesced(&[owner.context(), peer.context()]);
    let caller = joined.id();
    let context = joined.context();
    let waiting = std::thread::spawn(move || {
        let _binding = context.bind_sync();
        retirement.wait(SAFETY)
    });
    let blocked = observe(&registry, "joined-peer", |row| {
        row.active.iter().any(|interval| {
            interval.obligation == caller
                && interval.category == IntervalCategory::KnownWait(WaitKind::Retirement)
        })
    });
    let own = row(&registry, "joined-cause");
    held.release();
    let reaped = waiting.join().expect("joined condition caller joined");
    joined.complete();
    owner.complete();
    peer.complete();
    let owner_finished = observe(&registry, "joined-cause", |row| row.outstanding.is_empty());
    let peer_finished = row(&registry, "joined-peer");
    assert!(reaped);
    assert!(
        own.active
            .iter()
            .any(|interval| interval.obligation == caller
                && interval.category == IntervalCategory::OwnExternal),
        "{own:?}"
    );
    assert!(
        !own.active
            .iter()
            .any(|interval| interval.obligation == caller
                && matches!(interval.category, IntervalCategory::KnownWait(_))),
        "{own:?}"
    );
    assert!(
        blocked
            .active
            .iter()
            .any(|interval| interval.obligation == caller
                && interval.category == IntervalCategory::KnownWait(WaitKind::Retirement)),
        "{blocked:?}"
    );
    assert!(
        !blocked
            .active
            .iter()
            .any(|interval| interval.obligation == caller
                && interval.category == IntervalCategory::OwnExternal),
        "{blocked:?}"
    );
    assert_clean(&owner_finished);
    assert_clean(&peer_finished);
}
