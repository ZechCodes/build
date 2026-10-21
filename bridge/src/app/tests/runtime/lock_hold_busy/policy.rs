//! Qualification uses explicit hard limits and an undiluted raw writer pool.
use super::*;
use std::collections::BTreeMap;

pub(super) type RawHolds = BTreeMap<String, Vec<f64>>;

#[derive(Clone, Copy)]
pub(super) enum AggregateMode {
    Calibration,
    Enforce { limit_ms: f64 },
}

// Coordinator installs a reviewed explicit limit only after real-runner
// calibration. This mode never yields an overall passing qualification.
pub(super) const MODE: AggregateMode = AggregateMode::Calibration;

const WRITERS: [&str; 12] = [
    "mcp.control:create_task",
    "mcp.control:move_task",
    "mcp.control:message_agent",
    "mcp.control:set_topic",
    "mcp.control:comment_task",
    "mcp.control:post_thread_message",
    "entity.seen",
    "thread.post",
    "user.present",
    "agent.poll_status",
    "agent.deliver",
    "tasks.delivery_service",
];

#[derive(Default)]
struct Checks(Vec<String>);

impl Checks {
    fn require(&mut self, condition: bool, message: impl FnOnce() -> String) {
        if !condition {
            self.0.push(message());
        }
    }

    fn metric(
        &mut self,
        measurements: &Value,
        method: &str,
        phase: &str,
        field: &str,
    ) -> Option<f64> {
        let number = measurements[method][phase][field]
            .as_f64()
            .filter(|value| value.is_finite() && *value >= 0.0);
        self.require(number.is_some(), || {
            format!("{method} {phase}.{field} needs a finite nonnegative sample")
        });
        number
    }

    fn limit(
        &mut self,
        measurements: &Value,
        method: &str,
        phase: &str,
        ceiling: f64,
        strict: bool,
    ) {
        if let Some(maximum) = self.metric(measurements, method, phase, "max_ms") {
            let allowed = if strict {
                maximum < ceiling
            } else {
                maximum <= ceiling
            };
            self.require(allowed, || {
                format!(
                    "{method} {phase}.max_ms={maximum} violates {}{ceiling}ms",
                    if strict { "<" } else { "<=" }
                )
            });
        }
    }

    fn count(&mut self, measurements: &Value, method: &str, phase: &str, expected: usize) {
        let count = measurements[method][phase]["count"].as_u64();
        self.require(count == Some(expected as u64), || {
            format!("{method} {phase} requires {expected} samples, got {count:?}")
        });
    }
}

pub(super) fn assert_policy(measurements: &Value, raw: &RawHolds, mode: AggregateMode) {
    let mut checks = hard_limits(measurements);
    let pool = writer_pool(measurements, raw, &mut checks);
    mode_checks(&pool, mode, &mut checks);
    assert!(
        checks.0.is_empty(),
        "busy qualification violations:\n{}",
        checks.0.join("\n")
    );
}

fn mode_checks(pool: &[f64], mode: AggregateMode, checks: &mut Checks) {
    match mode {
        AggregateMode::Calibration => checks
            .0
            .push("aggregate writer hold limit unconfigured (calibration mode cannot pass)".into()),
        AggregateMode::Enforce { limit_ms } => {
            checks.require(limit_ms.is_finite() && limit_ms > 0.0, || {
                "aggregate writer hold limit must be finite and positive".into()
            });
            if let Some(p95) = percentile(pool, 95) {
                checks.require(p95 <= limit_ms, || {
                    format!(
                        "aggregate writer frame.held p95={p95}ms exceeds configured {limit_ms}ms"
                    )
                });
            }
        }
    }
}

fn hard_limits(measurements: &Value) -> Checks {
    let mut checks = Checks::default();
    for (method, count) in expected_request_samples() {
        checks.count(measurements, &method, "frame.held", count);
        checks.count(measurements, &method, "client.total", count);
    }
    let delivered = measurements["agent.deliver"]["frame.held"]["count"]
        .as_u64()
        .unwrap_or(0);
    checks.require(delivered > 0, || {
        "actual agent.deliver work must complete".into()
    });
    for (method, phases) in measurements.as_object().into_iter().flatten() {
        let count = phases["frame.held"]["count"].as_u64().unwrap_or(0) as usize;
        checks.require(count > 0, || {
            format!("{method} must include completed frame samples")
        });
        for phase in [
            "frame.lock_wait",
            "frame.queued",
            "frame.permit_wait",
            "frame.pool_wait",
            "frame.queue_and_lock_wait",
            "frame.queue_permit_and_lock_wait",
            "frame.total",
        ] {
            checks.count(measurements, method, phase, count);
            let _ = checks.metric(measurements, method, phase, "max_ms");
        }
        checks.limit(measurements, method, "frame.held", 200.0, true);
        checks.limit(
            measurements,
            method,
            "frame.lock_wait",
            MAX_BUSY_LOCK_WAIT_MS,
            false,
        );
        checks.limit(
            measurements,
            method,
            "frame.queue_permit_and_lock_wait",
            MAX_BUSY_RESPONSE_MS,
            false,
        );
        checks.limit(
            measurements,
            method,
            "frame.total",
            MAX_BUSY_RESPONSE_MS,
            false,
        );
        if phases.get("client.total").is_some() {
            checks.count(measurements, method, "client.total", count);
            checks.limit(
                measurements,
                method,
                "client.total",
                MAX_BUSY_RESPONSE_MS,
                false,
            );
        }
    }
    read_limits(measurements, &mut checks);
    checks
}

fn read_limits(measurements: &Value, checks: &mut Checks) {
    for (method, count) in read_samples() {
        checks.count(measurements, &method, "client.total", count);
        checks.limit(
            measurements,
            &method,
            "client.total",
            MAX_BUSY_MCP_READ_RESPONSE_MS,
            false,
        );
    }
}

pub(super) fn assert_read_latency(measurements: &Value) {
    let mut checks = Checks::default();
    read_limits(measurements, &mut checks);
    assert!(
        checks.0.is_empty(),
        "busy read latency violations:\n{}",
        checks.0.join("\n")
    );
}

pub(super) fn read_samples() -> Vec<(String, usize)> {
    reads::TOOLS
        .into_iter()
        .map(|tool| (format!("mcp.control:{tool}"), AGENTS * ROUNDS))
        .chain([
            ("git.show".into(), RPC_SAMPLES),
            ("tasks.review.get".into(), RPC_SAMPLES),
        ])
        .collect()
}

fn writer_pool(measurements: &Value, raw: &RawHolds, checks: &mut Checks) -> Vec<f64> {
    let read_labels = read_samples()
        .into_iter()
        .map(|(method, _)| method)
        .collect::<std::collections::BTreeSet<_>>();
    let mut pool = Vec::new();
    for (method, phases) in measurements.as_object().into_iter().flatten() {
        let count = phases["frame.held"]["count"].as_u64().unwrap_or(0) as usize;
        let values = raw.get(method).map(Vec::as_slice).unwrap_or(&[]);
        checks.require(values.len() == count && count > 0, || {
            format!(
                "{method} raw hold count={} differs from summary count={count}",
                values.len()
            )
        });
        checks.require(
            values
                .iter()
                .all(|value| value.is_finite() && *value >= 0.0),
            || format!("{method} raw holds must be finite and nonnegative"),
        );
        if WRITERS.contains(&method.as_str()) {
            pool.extend_from_slice(values);
        } else {
            checks.require(read_labels.contains(method), || {
                format!("{method} has no declared writer/read cohort classification")
            });
        }
    }
    for method in raw.keys() {
        checks.require(measurements.get(method).is_some(), || {
            format!("{method} raw holds have no completed summary")
        });
    }
    checks.require(!pool.is_empty(), || {
        "aggregate writer hold population is empty".into()
    });
    pool.retain(|value| value.is_finite() && *value >= 0.0);
    pool.sort_by(f64::total_cmp);
    pool
}

fn percentile(sorted: &[f64], percent: usize) -> Option<f64> {
    (!sorted.is_empty()).then(|| sorted[(sorted.len() * percent).div_ceil(100) - 1])
}

pub(super) fn report(measurements: &Value, raw: &RawHolds, mode: AggregateMode) -> Value {
    let mut checks = hard_limits(measurements);
    let pool = writer_pool(measurements, raw, &mut checks);
    let writers = raw
        .iter()
        .filter(|(method, _)| WRITERS.contains(&method.as_str()))
        .collect::<BTreeMap<_, _>>();
    let excluded = read_samples()
        .into_iter()
        .map(|(method, _)| {
            let count = raw.get(&method).map_or(0, Vec::len);
            (method, count)
        })
        .collect::<BTreeMap<_, _>>();
    let (name, configured, limit) = match mode {
        AggregateMode::Calibration => ("calibration", false, None),
        AggregateMode::Enforce { limit_ms } => ("enforce", true, Some(limit_ms)),
    };
    let hard_violations = checks.0.clone();
    mode_checks(&pool, mode, &mut checks);
    json!({
        "mode":name,"configured":configured,"limit_ms":limit,
        "verdict":if !configured {"unconfigured"} else if !checks.0.is_empty() {"fail"} else {"pass"},
        "quantile":"nearest rank ceil(percent*N/100)-1; raw writer frames pooled before sorting",
        "writer_methods":WRITERS,"writer_samples_ms":writers,
        "writer_distribution":{"count":pool.len(),"sum_ms":pool.iter().sum::<f64>(),"p50_ms":percentile(&pool,50),"p95_ms":percentile(&pool,95),"max_ms":pool.last()},
        "excluded_read_counts":excluded,"excluded_read_count":excluded.values().sum::<usize>(),
        "per_verb_p95_policy":"report only; strict per-verb max<200ms remains hard",
        "hard_violations":hard_violations,"violations":checks.0,
    })
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

#[test]
fn malformed_aggregate_limits_never_report_a_pass() {
    let (measurements, raw) = fixture();
    for limit_ms in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mode = AggregateMode::Enforce { limit_ms };
        let report = report(&measurements, &raw, mode);
        assert_eq!(report["verdict"], "fail", "{report}");
        assert!(failure(|| assert_policy(&measurements, &raw, mode)).is_some());
    }
}
