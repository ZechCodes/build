//! Explicit completed-trace unit controls only. No native runtime claims.
use super::*;

#[test]
fn full_frame_hold_max_allows_199999_and_rejects_200000_and_200001_microseconds() {
    for (held_us, expected_breaches) in [(199_999, 0), (200_000, 1), (200_001, 1)] {
        let observe = observation(&["boundary"]);
        observe
            .frame(completed_frame(&["boundary"], held_us))
            .unwrap();
        let rows = rows(&observe);
        super::super::validate_final_rows(&rows, &observe);
        let failures = hold_failures(&observe);
        assert_eq!(
            failures.len(),
            expected_breaches,
            "strict full-frame held_us <200000 requirement; held_us={held_us}; unit data only"
        );
        if let Some(failure) = failures.first() {
            assert_eq!(failure.measured_us, Some(held_us));
            assert_eq!(failure.limit_us, Some(FULL_FRAME_HOLD_MAX_US));
            assert_eq!(failure.original_id.as_deref(), Some("boundary"));
        }
    }
}

#[test]
fn full_frame_hold_rejects_225000_despite_same_original_150000_finite_service() {
    let observe = observation(&["same-original-fs"]);
    {
        let mut ledger = observe.ledger.lock().unwrap();
        let row = ledger.rows.get_mut("same-original-fs").unwrap();
        row.slot.finite_target_us = Some(150_000);
        row.slot.finite_ceiling_us = Some(200_000);
        row.recipe.needs_registry_root = true;
        row.milestones_us.insert(Stage::Completed, 225_000);
    }
    observe
        .frame(completed_frame(&["same-original-fs"], 225_000))
        .unwrap();
    observe
        .service(ServiceTrace {
            original_id: "same-original-fs".into(),
            registry_interval_id: "unit-finite-interval".into(),
            registry_origins: vec!["unit-server-root".into()],
            kind: "Filesystem".into(),
            target_us: 150_000,
            ceiling_us: 200_000,
            actual_us: 150_000,
            succeeded: true,
            abandoned: false,
            operation_before_padding_us: Some(10_000),
            finite_service: Interval {
                begin_us: 20_000,
                end_us: 170_000,
            },
        })
        .unwrap();
    observe
        .union(
            "same-original-fs",
            UnionTrace {
                write_ids: vec!["unit-server-root".into()],
                complete: true,
                ..UnionTrace::default()
            },
        )
        .unwrap();
    let rows = rows(&observe);
    assert_eq!(rows[0].services[0].actual_us, 150_000);
    assert_eq!(
        rows[0].frames[0].original_ids,
        vec![rows[0].services[0].original_id.clone()]
    );
    assert_eq!(
        225_000 - rows[0].services[0].actual_us,
        75_000,
        "subtracting finite service would incorrectly turn this full hold green"
    );
    super::super::validate_final_rows(&rows, &observe);
    let failures = hold_failures(&observe);
    assert_eq!(
        failures.len(),
        1,
        "225000us full hold must reject; paired service is not a deduction"
    );
    assert_eq!(failures[0].measured_us, Some(225_000));
    assert_eq!(failures[0].limit_us, Some(200_000));
    assert_eq!(failures[0].original_id.as_deref(), Some("same-original-fs"));
    assert_eq!(
        observe.ledger.lock().unwrap().violations.len(),
        1,
        "the valid paired finite service must not cause unrelated service failures"
    );
}

#[test]
fn full_frame_hold_counts_unknown_background_and_shared_observer_ids_once() {
    let observe = observation(&["known-a", "known-b"]);
    let shared = observe
        .frame(completed_frame(
            &["known-a", "known-b", "unknown-origin"],
            200_001,
        ))
        .unwrap();
    let unmatched = observe
        .frame(completed_frame(&["unknown-only"], 200_002))
        .unwrap();
    let background = observe.frame(completed_frame(&[], 200_003)).unwrap();
    let rows = rows(&observe);
    let other_frames = observe.ledger.lock().unwrap().other_frames.clone();
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .all(|row| row.frames.len() == 1 && row.frames[0].observer_id == shared));
    assert_eq!(
        other_frames
            .iter()
            .map(|frame| frame.observer_id)
            .collect::<Vec<_>>(),
        vec![shared, unmatched, background]
    );
    let audit = validate_frame_holds(&rows, &other_frames, &observe);
    assert_eq!(
        audit,
        FrameHoldAudit {
            unique_frames: 3,
            over_limit_frames: 3,
            max_held_us: 200_003
        },
        "shared collector observation appears in two client rows and other_frames, but counts once; unmatched/background also count"
    );
    let failures = hold_failures(&observe);
    assert_eq!(
        failures.len(),
        3,
        "retain every distinct violating collector observation"
    );
    assert!(failures.iter().any(
        |failure| failure.original_id.as_deref() == Some("unknown-only")
            && failure.measured_us == Some(200_002)
    ));
    assert!(failures
        .iter()
        .any(|failure| failure.original_id.is_none() && failure.measured_us == Some(200_003)));
    for observer_id in [shared, unmatched, background] {
        assert_eq!(
            failures
                .iter()
                .filter(|failure| failure
                    .detail
                    .contains(&format!("observer_id={observer_id};")))
                .count(),
            1,
            "each retained failure names its harness collector-observation ID exactly once"
        );
    }
}

fn observation(ids: &[&str]) -> Observation {
    let (wake, _receiver) = mpsc::unbounded_channel();
    Observation {
        start: Instant::now(),
        ledger: Arc::new(Mutex::new(Ledger {
            rows: ids
                .iter()
                .enumerate()
                .map(|(sequence, id)| ((*id).into(), row(id, sequence)))
                .collect(),
            violations: Vec::new(),
            latched: BTreeSet::new(),
            other_frames: Vec::new(),
        })),
        wake,
        next_frame_observer: Arc::new(AtomicU64::new(1)),
    }
}

fn row(id: &str, sequence: usize) -> Row {
    Row {
        slot: Slot {
            original_id: id.into(),
            sequence,
            traffic: Traffic::Write,
            route: Route::SpaRpc,
            caller: 0,
            owner: 0,
            due_us: 0,
            class: Some(WriteClass::FilesystemProcess),
            finite_target_us: None,
            finite_ceiling_us: None,
            method: Some("UnitCompletedFrame".into()),
        },
        recipe: Recipe {
            method: "UnitCompletedFrame".into(),
            scenario: "explicit-unit-data-not-native".into(),
            recipe_key: "unit-frame-only".into(),
            predecessor: None,
            needs_admission: false,
            needs_durable_ack: false,
            needs_registry_root: false,
            expected_read_cohort: None,
            durable_operation_id: None,
        },
        milestones_us: BTreeMap::new(),
        proof: None,
        error: None,
        frames: Vec::new(),
        services: Vec::new(),
        waits: None,
    }
}

fn completed_frame(ids: &[&str], held_us: u64) -> FrameTrace {
    FrameTrace {
        observer_id: 0, // Observation.frame assigns the actual harness observation ID.
        original_ids: ids.iter().map(|id| (*id).into()).collect(),
        protocol: "explicit-unit-completed-frame".into(),
        method: "UnitCompletedFrame".into(),
        sampled_at_us: 1_000_000,
        queued_us: 0,
        permit_wait_us: None,
        pool_wait_us: None,
        lock_wait_us: 0,
        held_us,
        total_us: held_us,
    }
}

fn rows(observe: &Observation) -> Vec<Row> {
    observe
        .ledger
        .lock()
        .unwrap()
        .rows
        .values()
        .cloned()
        .collect()
}

fn hold_failures(observe: &Observation) -> Vec<Violation> {
    observe
        .ledger
        .lock()
        .unwrap()
        .violations
        .iter()
        .filter(|failure| failure.kind == "app_lock_full_frame_hold_max")
        .cloned()
        .collect()
}
