//! Accounting controls only: real owned callbacks are not product-effect credit.
use super::*;
use crate::app::tests::runtime::write_wait_qualification::producer_adapter::{
    ActualBody, ConcreteProducers, ProductionBodies,
};
use crate::timing::write_wait::WriteWaitRegistry;

type Calls = Arc<Mutex<BTreeMap<&'static str, Vec<u64>>>>;

fn callback(name: &'static str, calls: &Calls, fail_status: bool) -> ActualBody {
    let calls = Arc::clone(calls);
    Box::new(move |ordinal| {
        calls.lock().unwrap().entry(name).or_default().push(ordinal);
        if fail_status && name == "status_observations" && ordinal == 0 {
            Err("deliberate actual callback failure for final-report control".into())
        } else {
            Ok(())
        }
    })
}

async fn actual_report(stop_early: bool, fail_status: bool) -> ProbeSnapshot {
    let calls = Arc::new(Mutex::new(BTreeMap::new()));
    let registry = WriteWaitRegistry::new();
    let producers = ConcreteProducers::pipeline_control(
        TrialName {
            envelope: Envelope::Small,
            number: 1,
        },
        Arc::clone(&registry),
        ProductionBodies {
            status_observations: callback("status_observations", &calls, fail_status),
            delivery_consumption: callback("delivery_consumption", &calls, false),
            source_git_refresh: callback("source_git_refresh", &calls, false),
            watcher_publication: callback("watcher_publication", &calls, false),
        },
    )
    .unwrap();
    let start = Instant::now()
        + if stop_early {
            Duration::from_secs(1)
        } else {
            Duration::from_millis(10)
        };
    for producer in &producers.handles {
        producer.start_at(start).unwrap();
    }
    if stop_early {
        for producer in &producers.handles {
            producer.stop_offering();
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while !producers.handles.iter().all(|producer| producer.joined()) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let drained = producers.handles.iter().all(|producer| producer.joined());
    if !drained {
        for producer in &producers.handles {
            producer.stop_offering();
        }
        let cleanup = Instant::now() + Duration::from_secs(5);
        while !producers.handles.iter().all(|producer| producer.joined())
            && Instant::now() < cleanup
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
    let snapshots = producers.snapshots();
    assert!(
        snapshots.iter().all(|row| row.joined && row.body_released),
        "actual callback ownership retained: {snapshots:?}"
    );
    assert!(
        drained,
        "accounting control exceeded its ordinary callback drain"
    );
    let calls = calls.lock().unwrap();
    let rows = registry.snapshot();
    assert_eq!(
        rows.len() as u64,
        snapshots.iter().map(|row| row.started).sum::<u64>()
    );
    assert!(rows.iter().all(|row| row.root_finished
        && row.completed_at.is_some()
        && !row.abandoned
        && row.active.is_empty()
        && row.outstanding.is_empty()));
    let mut report = ProbeSnapshot::default();
    for row in &snapshots {
        let actual = calls.get(row.name).cloned().unwrap_or_default();
        assert_eq!(actual, (0..row.started).collect::<Vec<_>>());
        assert_eq!(row.started, row.completed + row.failed);
        assert!(row.counters_coherent);
        report.producer_counts.insert(row.name.into(), row.into());
        if let Some(error) = &row.first_error {
            report.producer_failures.push(error.clone());
        }
    }
    report
}

fn violations(report: &ProbeSnapshot, window: Duration) -> Vec<Violation> {
    let observe = Observation::for_native_control_at(&[], Instant::now());
    validate_final_producers(report, window, &observe);
    let result = observe.ledger.lock().unwrap().violations.clone();
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_joined_callbacks_populate_final_producer_report() {
    let report = actual_report(false, false).await;
    for (name, expected) in [
        ("status_observations", 8),
        ("delivery_consumption", 8),
        ("source_git_refresh", 2),
        ("watcher_publication", 2),
    ] {
        let row = &report.producer_counts[name];
        assert_eq!(
            (row.planned, row.intended, row.started, row.completed),
            (expected, expected, expected, expected)
        );
        assert!(row.joined && row.body_released && row.counters_coherent);
    }
    assert!(report.producer_failures.is_empty());
    assert!(violations(&report, Duration::from_secs(2)).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn final_report_rejects_cleanly_joined_early_stop() {
    let report = actual_report(true, false).await;
    assert!(report.producer_failures.is_empty());
    assert!(report
        .producer_counts
        .values()
        .all(|row| row.started == 0 && row.completed == 0 && row.intended == 0 && row.failed == 0));
    let failures = violations(&report, Duration::from_secs(2));
    assert_eq!(
        failures.len(),
        1,
        "joined zero-error callbacks must not replace the full finite producer plan"
    );
    assert!(failures[0]
        .detail
        .contains("intended callbacks differ from full plan"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn final_report_retains_actual_callback_failure_after_watchdog_stops() {
    let report = actual_report(false, true).await;
    assert_eq!(report.producer_counts["status_observations"].failed, 1);
    assert!(report
        .producer_failures
        .iter()
        .any(|error| error.contains("deliberate actual callback failure")));
    let failures = violations(&report, Duration::from_secs(2));
    assert_eq!(failures.len(), 1);
    assert!(failures[0]
        .detail
        .contains("deliberate actual callback failure"));
}

// The following maps are deliberately synthetic report-boundary controls,
// never evidence that 600 seconds or any native product work was executed.
fn declared_normal_report() -> ProbeSnapshot {
    let mut report = ProbeSnapshot::default();
    for (name, period_us, planned) in [
        ("status_observations", 250_000, 2400),
        ("delivery_consumption", 250_000, 2400),
        ("source_git_refresh", 1_000_000, 600),
        ("watcher_publication", 1_000_000, 600),
    ] {
        report.producer_counts.insert(
            name.into(),
            ProducerCounts {
                period_us,
                planned,
                intended: planned,
                started: planned,
                completed: planned,
                counters_coherent: true,
                joined: true,
                body_released: true,
                ..ProducerCounts::default()
            },
        );
    }
    report
}

#[test]
fn final_report_requires_pinned_normal_periods_totals_and_every_producer() {
    let original = declared_normal_report();
    assert!(violations(&original, Duration::from_secs(600)).is_empty());
    let mut altered = original.clone();
    altered
        .producer_counts
        .get_mut("status_observations")
        .unwrap()
        .period_us = 500_000;
    assert!(
        !violations(&altered, Duration::from_secs(600)).is_empty(),
        "wrong period must fail"
    );
    let mut altered = original.clone();
    let row = altered
        .producer_counts
        .get_mut("delivery_consumption")
        .unwrap();
    row.planned -= 1;
    row.intended -= 1;
    row.started -= 1;
    row.completed -= 1;
    assert!(
        !violations(&altered, Duration::from_secs(600)).is_empty(),
        "2399 successful callbacks cannot replace 2400"
    );
    let mut altered = original;
    altered.producer_counts.remove("watcher_publication");
    assert!(
        !violations(&altered, Duration::from_secs(600)).is_empty(),
        "missing producer must fail"
    );
}

#[test]
fn final_report_rejects_incoherent_unjoined_or_unreleased_snapshots() {
    for field in ["counters_coherent", "joined", "body_released"] {
        let mut report = declared_normal_report();
        let row = report
            .producer_counts
            .get_mut("source_git_refresh")
            .unwrap();
        match field {
            "counters_coherent" => row.counters_coherent = false,
            "joined" => row.joined = false,
            "body_released" => row.body_released = false,
            _ => unreachable!(),
        }
        assert!(
            !violations(&report, Duration::from_secs(600)).is_empty(),
            "final {field}=false must fail"
        );
    }
}
