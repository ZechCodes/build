// Intended child of harness/inventory/tests after initial rare RED.
use super::*;
use crate::timing::write_wait::WriteWaitRegistry;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn refresh_arriving_during_sweep_keeps_its_origin_until_its_generation_publishes() {
    let device = Device::new();
    let registry = WriteWaitRegistry::new();
    let (entered, observing) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let released = Mutex::new(released);
    let first_wall = AtomicBool::new(true);
    let inventory = device
        .builder()
        .clocks(Instant::now, move || {
            // observe_installations asks wall time only after capturing refresh_target.
            if first_wall.swap(false, Ordering::AcqRel) {
                entered.send(()).unwrap();
                released
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
            SystemTime::now()
        })
        .build();
    let first = registry.begin("inventory-first", "harnesses.refresh", Instant::now());
    let first_id = first.id();
    let first_request = {
        let _bound = first.context().bind_sync();
        inventory.request_refresh().request
    };
    first.complete();
    let sweeping = Arc::clone(&inventory);
    let worker = std::thread::spawn(move || sweeping.sweep());
    let reached_observation = observing.recv_timeout(Duration::from_secs(5));
    let second = registry.begin(
        "inventory-next-generation",
        "harnesses.refresh",
        Instant::now(),
    );
    let second_id = second.id();
    let second_request = {
        let _bound = second.context().bind_sync();
        inventory.request_refresh().request
    };
    second.complete();
    let released_observation = release.send(());
    let joined = worker.join();
    assert!(
        reached_observation.is_ok(),
        "sweep never reached observation"
    );
    assert!(released_observation.is_ok(), "sweep left before release");
    assert!(joined.is_ok(), "sweep failed after release");
    let first_publication = inventory.snapshot();
    let between = registry.snapshot();
    inventory.sweep();
    let final_publication = inventory.snapshot();
    let final_rows = registry.snapshot();
    assert_eq!(first_publication.refresh.completed, first_request);
    assert_eq!(final_publication.refresh.completed, second_request);
    let first = between.iter().find(|row| row.id == first_id).unwrap();
    let second = between.iter().find(|row| row.id == second_id).unwrap();
    assert!(first.root_finished && first.outstanding.is_empty() && first.active.is_empty());
    assert!(second.root_finished && !second.outstanding.is_empty());
    for id in [first_id, second_id] {
        let done = final_rows.iter().find(|row| row.id == id).unwrap();
        assert!(done.root_finished && done.outstanding.is_empty() && done.active.is_empty());
        assert!(!done.cancelled && !done.abandoned);
    }
}
