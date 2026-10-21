use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[test]
fn restored_owner_cannot_lend_stale_cause_to_later_unknown_raw_borrow() {
    let fixture = fixture();
    let base = Instant::now();
    let ticks = Arc::new(AtomicU64::new(0));
    let clock = Arc::clone(&ticks);
    let registry = WriteWaitRegistry::with_clock(Arc::new(move || {
        base + Duration::from_millis(clock.load(Ordering::Acquire))
    }));
    let write = registry.begin("publication-stale-borrow-cause", "thread.post", base);
    let id = write.id();
    let context = write.context();
    let prior = {
        let _bound = context.bind_sync();
        let mut app = fixture.state.lock().unwrap();
        let mut peer = app.take_run(OTHER).unwrap();
        peer.run.goal = "native restored peer goal".into();
        app.finish_run_mutation(OTHER.into(), peer).unwrap();
        app.read_view.load()
    };
    // The first take really settled through the native SQL/publication path.
    // This later raw absence has no recorded semantic borrower.
    let held = fixture.state.lock().unwrap().runs.remove(OTHER).unwrap();
    let committed = {
        let _bound = context.bind_sync();
        let mut app = fixture.state.lock().unwrap();
        let mut active = app.take_run(OWNER).unwrap();
        active.run.goal = "unknown predecessor goal".into();
        app.finish_run_mutation(OWNER.into(), active)
    };
    let retained = fixture.state.lock().unwrap().read_view.load();
    let durable = durable_goal(&fixture);
    write.acknowledged();
    write.complete();
    ticks.store(6_000, Ordering::Release);
    let delayed = registry
        .active_snapshot()
        .into_iter()
        .find(|row| row.id == id);
    let restored = fixture
        .state
        .lock()
        .unwrap()
        .finish_run_mutation(OTHER.into(), held);
    committed.unwrap();
    restored.unwrap();
    // Native restoration and all borrowed work are settled before assertions.
    assert_eq!(published_goal(&prior, OTHER), "native restored peer goal");
    assert_eq!(durable, "unknown predecessor goal");
    assert!(Arc::ptr_eq(&prior, &retained));
    assert_ne!(published_goal(&prior, OWNER), "unknown predecessor goal");
    let delayed = delayed.expect("the committed owner still owes publication");
    assert!(delayed.root_finished);
    assert_eq!(delayed.known_wait_union, Duration::from_secs(6));
    assert_eq!(delayed.own_external_union, Duration::ZERO);
    assert!(
        delayed.breached(),
        "unknown borrowers must remain conservative"
    );
    assert!(proof(&fixture, &context));
    assert!(registry.active_snapshot().is_empty());
    fixture.assert_full_oracle();
}
