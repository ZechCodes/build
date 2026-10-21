//! Explicit calibration never constitutes a passing aggregate qualification.
use super::*;
use std::collections::BTreeMap;

type RawHolds = BTreeMap<String, Vec<f64>>;

#[derive(Clone, Copy)]
enum AggregateMode {
    Calibration,
    Enforce { limit_ms: f64 },
}

// Test-first seam: preserve the current qualification policy until its real
// missing coverage and changed policy have been observed RED. The new raw pool
// and explicit mode have no acceptance effect yet; implementation follows RED.
fn assert_policy(measurements: &Value, raw: &RawHolds, mode: AggregateMode) {
    let _ = raw;
    if let AggregateMode::Enforce { limit_ms } = mode {
        let _ = limit_ms;
    }
    assert_expected_samples(measurements);
}

fn metric(count: usize, p95: f64, maximum: f64) -> Value {
    json!({"count":count,"sum_ms":p95 * count as f64,"p50_ms":p95,"p95_ms":p95,"max_ms":maximum})
}

fn fixture() -> (Value, RawHolds) {
    let mut counts = expected_request_samples()
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    for tool in reads::TOOLS {
        counts.insert(format!("mcp.control:{tool}"), AGENTS * ROUNDS);
    }
    counts.insert("agent.deliver".into(), 1);
    counts.insert("tasks.delivery_service".into(), 1);
    let mut measurements = json!({});
    let mut raw = BTreeMap::new();
    for (method, count) in counts {
        for phase in [
            "frame.held",
            "frame.lock_wait",
            "frame.queued",
            "frame.permit_wait",
            "frame.pool_wait",
            "frame.queue_and_lock_wait",
            "frame.queue_permit_and_lock_wait",
            "frame.total",
            "client.total",
        ] {
            measurements[&method][phase] = metric(count, 0.0, 0.0);
        }
        raw.insert(method, vec![0.0; count]);
    }
    (measurements, raw)
}

fn failure(work: impl FnOnce() + std::panic::UnwindSafe) -> Option<String> {
    std::panic::catch_unwind(work).err().map(|error| {
        error
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| error.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_else(|| "non-string panic".into())
    })
}

#[test]
fn every_independent_read_enforces_100ms_actual_client_latency() {
    let labels = reads::TOOLS
        .into_iter()
        .map(|tool| format!("mcp.control:{tool}"))
        .chain(["git.show".into(), "tasks.review.get".into()]);
    let mut unchecked = Vec::new();
    for label in labels {
        let (mut measurements, _) = fixture();
        let count = measurements[&label]["client.total"]["count"]
            .as_u64()
            .unwrap() as usize;
        measurements[&label]["client.total"] = metric(count, 101.0, 101.0);
        if failure(|| assert_busy_mcp_read_latency(&measurements)).is_none() {
            unchecked.push(label);
        }
    }
    assert!(
        unchecked.is_empty(),
        "busy100ms read coverage omitted: {unchecked:?}"
    );
}

#[test]
fn read_latency_100ms_boundary_remains_allowed() {
    let (mut measurements, _) = fixture();
    for tool in reads::TOOLS {
        measurements[format!("mcp.control:{tool}")]["client.total"] =
            metric(AGENTS * ROUNDS, 100.0, 100.0);
    }
    for method in ["git.show", "tasks.review.get"] {
        measurements[method]["client.total"] = metric(RPC_SAMPLES, 100.0, 100.0);
    }
    assert_busy_mcp_read_latency(&measurements);
}

#[test]
fn per_verb_p95_is_reported_without_becoming_a_separate_hard_failure() {
    let (mut measurements, mut raw) = fixture();
    let label = "mcp.control:create_task";
    measurements[label]["frame.held"] = metric(AGENTS * ROUNDS, 60.0, 60.0);
    raw.insert(label.into(), vec![60.0; AGENTS * ROUNDS]);
    assert_policy(
        &measurements,
        &raw,
        AggregateMode::Enforce { limit_ms: 100.0 },
    );
}

#[test]
fn per_verb_200ms_hard_max_applies_to_every_captured_label() {
    let (base, raw) = fixture();
    let mut unchecked = Vec::new();
    for label in base.as_object().unwrap().keys() {
        let mut measurements = base.clone();
        measurements[label]["frame.held"]["max_ms"] = json!(200.0);
        let reported = failure(|| {
            assert_policy(
                &measurements,
                &raw,
                AggregateMode::Enforce { limit_ms: 100.0 },
            )
        });
        if !reported
            .as_deref()
            .is_some_and(|message| message.contains(label) && message.contains("200"))
        {
            unchecked.push(label.clone());
        }
    }
    assert!(
        unchecked.is_empty(),
        "strict200ms hard max omitted labels: {unchecked:?}"
    );
}

#[test]
fn hard_failure_reports_all_violating_labels_before_panicking() {
    let (mut measurements, raw) = fixture();
    for label in ["mcp.control:create_task", "mcp.control:move_task"] {
        measurements[label]["frame.held"]["max_ms"] = json!(201.0);
    }
    let message = failure(|| {
        assert_policy(
            &measurements,
            &raw,
            AggregateMode::Enforce { limit_ms: 100.0 },
        )
    })
    .unwrap();
    assert!(
        message.contains("create_task") && message.contains("move_task"),
        "one hard failure hid another: {message}"
    );
}

#[test]
fn aggregate_writer_p95_uses_raw_samples_and_an_explicit_limit() {
    let (mut measurements, mut raw) = fixture();
    let label = "mcp.control:create_task";
    measurements[label]["frame.held"] = metric(AGENTS * ROUNDS, 20.0, 20.0);
    raw.insert(label.into(), vec![20.0; AGENTS * ROUNDS]);
    assert!(
        failure(|| assert_policy(
            &measurements,
            &raw,
            AggregateMode::Enforce { limit_ms: 10.0 }
        ))
        .is_some(),
        "a configured aggregate breach was ignored"
    );
}

#[test]
fn calibration_mode_reports_unconfigured_and_cannot_pass() {
    let (measurements, raw) = fixture();
    let message = failure(|| assert_policy(&measurements, &raw, AggregateMode::Calibration));
    assert!(
        message
            .as_deref()
            .is_some_and(|message| message.contains("unconfigured")),
        "uncalibrated aggregate qualification must never reportPASS: {message:?}"
    );
}
