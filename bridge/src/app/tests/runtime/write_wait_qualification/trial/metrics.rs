//! Off-source split of the frozen coordinator; no execution evidence.
use super::*;

pub(super) fn distribution(mut samples: Vec<(String, u64)>, limit: Option<u64>) -> Distribution {
    samples.sort_by_key(|(_, us)| *us);
    let n = samples.len();
    let percentile = |p: usize| {
        if n == 0 {
            None
        } else {
            Some(samples[(n * p).div_ceil(100).saturating_sub(1)].1)
        }
    };
    Distribution {
        samples: n,
        p50_us: percentile(50),
        p95_us: percentile(95),
        p99_us: percentile(99),
        max_us: samples.last().map(|(_, us)| *us),
        violations: samples
            .into_iter()
            .filter(|(_, us)| limit.is_some_and(|bound| *us > bound))
            .collect(),
    }
}

pub(super) fn counts<'a>(
    rows: impl Iterator<Item = &'a Row>,
    cutoff: Option<u64>,
) -> BTreeMap<Traffic, TrafficCounts> {
    let mut result = [Traffic::Write, Traffic::Read, Traffic::Exempt]
        .into_iter()
        .map(|traffic| (traffic, TrafficCounts::default()))
        .collect::<BTreeMap<_, _>>();
    for row in rows {
        let count = result.get_mut(&row.slot.traffic).unwrap();
        count.intended += 1;
        let has = |stage| {
            row.milestones_us
                .get(&stage)
                .is_some_and(|at| cutoff.is_none_or(|cutoff| *at <= cutoff))
        };
        let offered = has(Stage::Offered);
        let sent = has(Stage::WireSent);
        let done = has(Stage::Completed);
        let admitted = has(Stage::Admitted);
        count.offered += usize::from(offered);
        count.dispatch_started += usize::from(has(Stage::DispatchStarted));
        count.sent += usize::from(sent);
        count.server_intake += usize::from(has(Stage::ServerIntake));
        count.admitted += usize::from(admitted);
        count.durable_acknowledged += usize::from(has(Stage::DurableAck));
        count.completed += usize::from(done);
        count.verified_success += usize::from(done && row.error.is_none() && row.proof.is_some());
        count.failed += usize::from(done && row.error.is_some());
        count.pending_unsubmitted += usize::from(offered && !sent);
        count.sent_unfinished += usize::from(sent && !done);
        count.admitted_unfinished += usize::from(admitted && !done);
        count.not_offered += usize::from(!offered);
    }
    for count in result.values_mut() {
        count.offered_per_second = count.offered as f64 / SECONDS as f64;
        count.sent_per_second = count.sent as f64 / SECONDS as f64;
        count.admitted_per_second = count.admitted as f64 / SECONDS as f64;
        count.completed_per_second = count.completed as f64 / SECONDS as f64;
    }
    result
}

type MetricSamples = BTreeMap<String, Vec<(String, u64)>>;
type MethodKey = (Traffic, String, String, String);
#[path = "metrics/endpoints.rs"]
mod endpoints;
#[path = "metrics/frames.rs"]
mod frames;
#[path = "metrics/waits.rs"]
mod waits;

pub(super) fn method_metrics(rows: &[Row]) -> Vec<MethodMetrics> {
    let mut groups = BTreeMap::<MethodKey, Vec<&Row>>::new();
    for row in rows {
        groups.entry(method_key(row)).or_default().push(row);
    }
    groups
        .into_iter()
        .map(|(key, group)| summarize_method(key, group))
        .collect()
}
fn method_key(row: &Row) -> MethodKey {
    let cohort = match row
        .proof
        .as_ref()
        .and_then(|proof| proof.read_cohort.as_ref())
    {
        Some(ReadCohort::ReadyBounded) => "ready_bounded",
        Some(ReadCohort::ExplicitPending) => "explicit_pending",
        Some(ReadCohort::LegacyUnbounded) => "legacy_unbounded",
        None => "unclassified",
    };
    (
        row.slot.traffic,
        row.recipe.method.clone(),
        row.recipe.scenario.clone(),
        cohort.into(),
    )
}
fn summarize_method(
    (traffic, method, scenario, cohort): MethodKey,
    group: Vec<&Row>,
) -> MethodMetrics {
    let mut samples = MetricSamples::new();
    let mut seen_frames = BTreeSet::new();
    for row in &group {
        endpoints::append(row, &mut samples);
        frames::append(row, &mut samples, &mut seen_frames);
        waits::append(row, &mut samples);
    }
    let metrics = samples
        .into_iter()
        .map(|(metric, values)| {
            let limit = metric_limit(&metric, traffic, &cohort);
            (metric, distribution(values, limit))
        })
        .collect();
    let counts = counts(group.into_iter(), None).remove(&traffic).unwrap();
    MethodMetrics {
        traffic,
        method,
        scenario,
        read_cohort: (traffic == Traffic::Read).then_some(cohort),
        counts,
        metrics,
    }
}
fn metric_limit(metric: &str, traffic: Traffic, cohort: &str) -> Option<u64> {
    match metric {
        "known_wait_union"
        | "wait_plus_sql_union"
        | "conservative_wait_upper"
        | "intake_to_encoded_reply" => Some(WRITE_BOUND_US),
        // Evidence lists all slow ready samples; only p99 is a100ms gate.
        "sent_to_parsed_reply" | "due_to_parsed_reply" => {
            Some(if traffic == Traffic::Read && cohort == "ready_bounded" {
                READ_BOUND_US
            } else {
                WRITE_BOUND_US
            })
        }
        _ => None,
    }
}
fn push(samples: &mut MetricSamples, row: &Row, metric: &str, value: u64) {
    samples
        .entry(metric.into())
        .or_default()
        .push((row.slot.original_id.clone(), value));
}
