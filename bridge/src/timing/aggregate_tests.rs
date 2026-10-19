use super::*;

#[test]
fn per_verb_intervals_report_counts_sums_and_distributions() {
    let record = MethodRecord::new("tasks.list");
    for millis in [1, 2, 3, 4, 10] {
        record.record(&Spent {
            queued: Duration::from_millis(millis),
            lock_wait: Duration::from_millis(millis * 2),
            held: Duration::from_millis(millis * 3),
            total: Duration::from_millis(millis * 6),
            permit_wait: None,
            pool_wait: None,
        });
    }
    let stats = record.stats();
    for (phase, sum, p50, max) in [
        ("queued", 20.0, 5.0, 10.0),
        ("lock_wait", 40.0, 10.0, 20.0),
        ("held", 60.0, 10.0, 30.0),
    ] {
        assert_eq!(stats[phase]["count"], 5, "{phase}: {stats}");
        assert_eq!(stats[phase]["sum_ms"], sum, "{phase}: {stats}");
        assert_eq!(stats[phase]["p50_ms"], p50, "{phase}: {stats}");
        assert_eq!(stats[phase]["p95_ms"], max, "{phase}: {stats}");
        assert_eq!(stats[phase]["max_ms"], max, "{phase}: {stats}");
    }
    // Existing consumers retain the same total and hold keys and meanings.
    assert_eq!(stats["served"], 5);
    assert_eq!(stats["p50_ms"], 25.0);
    assert_eq!(stats["p95_ms"], 60.0);
    assert_eq!(stats["max_ms"], 60.0);
    assert_eq!(stats["held_p99_ms"], 30.0);
    assert_eq!(stats["held_max_ms"], 30.0);
}

#[test]
fn frames_without_app_lock_use_recorded_zero_wait_and_hold() {
    let (clock, _) = recording_clock();
    drop(clock.frame("bridge.stats"));
    let stats = clock.stats();
    for phase in ["lock_wait", "held"] {
        assert_eq!(stats["methods"]["bridge.stats"][phase], json!({
            "count": 1, "sum_ms": 0.0, "p50_ms": 0.0,
            "p95_ms": 0.0, "max_ms": 0.0,
        }));
    }
}

#[test]
fn every_registered_method_keeps_its_record_after_unknown_name_flood() {
    let (clock, _) = recording_clock();
    for n in 0..1_000 {
        drop(clock.frame(&format!("unknown.{n}")));
    }
    let registered = crate::api::v1::methods().iter().map(|(name, _)| *name)
        .chain(crate::api::LEGACY_METHODS.iter().copied())
        .chain(crate::api::QA_METHODS.iter().copied());
    for name in registered {
        drop(clock.frame(name));
        assert_eq!(clock.stats()["methods"][name]["served"], 1, "{name}");
    }
    let stats = clock.stats();
    assert_eq!(stats["methods"]["other"]["served"], 1_000);
    assert_eq!(stats["methods"].as_object().unwrap().len(),
        crate::api::v1::methods().len() + crate::api::LEGACY_METHODS.len()
            + crate::api::QA_METHODS.len() + 1);
}

#[test]
fn slow_frame_format_remains_unchanged() {
    let spent = Spent {
        queued: Duration::from_millis(100),
        lock_wait: Duration::from_millis(200),
        held: Duration::from_millis(300),
        total: Duration::from_millis(650),
        permit_wait: Some(Duration::from_millis(20)),
        pool_wait: Some(Duration::from_millis(30)),
    };
    assert_eq!(slow_frame_line("tasks.list", &spent, 2),
        "slow frame tasks.list total=650.0ms queued=100.0ms lock_wait=200.0ms held=300.0ms permit_wait=20.0ms pool_wait=30.0ms waiting=2");
}
