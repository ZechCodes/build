//! Loaded p99 policy is independent of Ready credit for legacy MCP reads.
use super::*;
use crate::app::tests::runtime::write_wait_qualification::read_recipes::{
    mcp_method_latency_policy, LoadedReadPolicy,
};
type ReadyCases = BTreeMap<(String, String), Vec<(String, u64)>>;
pub(in super::super) fn validate_loaded_ready_reads(rows: &[Row], observe: &Observation) {
    for ((method, scenario), samples) in gather_ready_cases(rows) {
        validate_case(
            &method,
            &scenario,
            samples,
            observe,
            "ready",
            LoadedReadPolicy {
                p99_limit_us: READ_BOUND_US,
                minimum_successes: 1_000,
            },
        );
    }
    for ((method, scenario), samples) in gather_legacy_mcp_cases(rows) {
        let policy = mcp_method_latency_policy(&method).expect("reviewed MCP latency policy");
        validate_case(&method, &scenario, samples, observe, "mcp", policy);
    }
}
fn gather_ready_cases(rows: &[Row]) -> ReadyCases {
    let mut cases = ReadyCases::new();
    for row in rows {
        if row.slot.traffic != Traffic::Read
            || row.recipe.expected_read_cohort != Some(ReadCohort::ReadyBounded)
        {
            continue;
        }
        let samples = cases
            .entry((row.recipe.method.clone(), row.recipe.scenario.clone()))
            .or_default();
        append_verified(row, &ReadCohort::ReadyBounded, samples);
    }
    cases
}
fn gather_legacy_mcp_cases(rows: &[Row]) -> ReadyCases {
    let mut cases = ReadyCases::new();
    for row in rows {
        if row.slot.traffic != Traffic::Read
            || row.slot.route != Route::AgentMcp
            || row.recipe.expected_read_cohort != Some(ReadCohort::LegacyUnbounded)
            || mcp_method_latency_policy(&row.recipe.method).is_none()
        {
            continue;
        }
        let samples = cases
            .entry((row.recipe.method.clone(), row.recipe.scenario.clone()))
            .or_default();
        append_verified(row, &ReadCohort::LegacyUnbounded, samples);
    }
    cases
}
fn append_verified(row: &Row, expected: &ReadCohort, samples: &mut Vec<(String, u64)>) {
    if row.error.is_some()
        || row
            .proof
            .as_ref()
            .and_then(|proof| proof.read_cohort.as_ref())
            != Some(expected)
    {
        return;
    }
    if let Some(parsed) = row.milestones_us.get(&Stage::ParsedFinalReply) {
        samples.push((
            row.slot.original_id.clone(),
            parsed.saturating_sub(row.slot.due_us),
        ));
    }
}
fn validate_case(
    method: &str,
    scenario: &str,
    samples: Vec<(String, u64)>,
    observe: &Observation,
    cohort: &str,
    policy: LoadedReadPolicy,
) {
    let metrics = distribution(samples, Some(policy.p99_limit_us));
    if metrics.samples < policy.minimum_successes {
        observe.fail(
            None,
            &format!("{cohort}_read_sample_count/{method}/{scenario}"),
            format!(
                "{} verified {cohort} samples; require>={} for this exact method/scenario",
                metrics.samples, policy.minimum_successes
            ),
        );
    }
    if let Some(p99) = metrics.p99_us {
        if p99 > policy.p99_limit_us {
            observe.violation(Violation { original_id: None, kind: format!("{cohort}_read_p99/{method}/{scenario}"), observed_at_us: observe.now_us(), measured_us: Some(p99), limit_us: Some(policy.p99_limit_us), detail: format!("{} verified {cohort} samples; {} individual >100ms samples retained in method report", metrics.samples, metrics.violations.len()) });
        }
    }
}

#[path = "reads/policy_tests.rs"]
mod policy_tests;
