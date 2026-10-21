//! Service ceilings inspect the exact open service interval, not an outer job.
use crate::timing::write_wait::{IntervalCategory, WriteSnapshot, WriteWaitRegistry};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn active_service_breaches(
    _rows: &[WriteSnapshot],
    _ceilings: &BTreeMap<String, Duration>,
    _now: Instant,
) -> Vec<String> {
    // Implement after the actual open-interval control produces RED. Completed
    // service samples cannot prove an unfinished effect stayed in its bound.
    Vec::new()
}

fn fixture() -> (Arc<WriteWaitRegistry>, Arc<Mutex<Instant>>) {
    let clock = Arc::new(Mutex::new(Instant::now()));
    let observed = Arc::clone(&clock);
    let registry = WriteWaitRegistry::with_clock(Arc::new(move || *observed.lock().unwrap()));
    (registry, clock)
}

#[test]
fn unfinished_finite_service_breaches_its_ceiling_without_a_completion_sample() {
    let (registry, clock) = fixture();
    let original = "qual/small/trial1/write/00032";
    let intake = *clock.lock().unwrap();
    let write = registry.begin(original, "fs.write", intake);
    let active = write.context().interval(IntervalCategory::FiniteService);
    let ceilings = BTreeMap::from([(original.to_string(), Duration::from_millis(250))]);
    *clock.lock().unwrap() += Duration::from_millis(250);
    assert!(active_service_breaches(
        &registry.active_snapshot(),
        &ceilings,
        *clock.lock().unwrap()
    )
    .is_empty());
    *clock.lock().unwrap() += Duration::from_nanos(1);
    let failures = active_service_breaches(
        &registry.active_snapshot(),
        &ceilings,
        *clock.lock().unwrap(),
    );
    assert_eq!(
        failures,
        vec![original.to_string()],
        "open service cannot hide behind missing final samples"
    );
    drop(active);
    write.complete();
}

#[test]
fn outer_deferred_external_envelope_is_not_the_finite_service_interval() {
    let (registry, clock) = fixture();
    let original = "qual/large/trial2/write/00036";
    let intake = *clock.lock().unwrap();
    let write = registry.begin(original, "git.stage", intake);
    let outer = write.context().interval(IntervalCategory::OwnExternal);
    *clock.lock().unwrap() += Duration::from_secs(6);
    let service = write.context().interval(IntervalCategory::FiniteService);
    *clock.lock().unwrap() += Duration::from_millis(10);
    let ceilings = BTreeMap::from([(original.to_string(), Duration::from_millis(500))]);
    assert!(active_service_breaches(
        &registry.active_snapshot(),
        &ceilings,
        *clock.lock().unwrap()
    )
    .is_empty());
    drop(service);
    drop(outer);
    write.complete();
}
