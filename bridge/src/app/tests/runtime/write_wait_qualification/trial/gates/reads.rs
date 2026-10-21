//! Form-specific ready p99; LegacyFull has zero Ready quota,1000 real replies.
use super::*;
type ReadyCases = BTreeMap<(String, String), Vec<(String, u64)>>;
pub(in super::super) fn validate_loaded_ready_reads(rows: &[Row], observe: &Observation) {
    for ((method, scenario), samples) in gather_ready_cases(rows) {
        validate_case(&method, &scenario, samples, observe);
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
        append_verified_ready(row, samples);
    }
    cases
}
fn append_verified_ready(row: &Row, samples: &mut Vec<(String, u64)>) {
    if row.error.is_some()
        || row
            .proof
            .as_ref()
            .and_then(|proof| proof.read_cohort.as_ref())
            != Some(&ReadCohort::ReadyBounded)
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
fn validate_case(method: &str, scenario: &str, samples: Vec<(String, u64)>, observe: &Observation) {
    let metrics = distribution(samples, Some(READ_BOUND_US));
    if metrics.samples < 1_000 {
        observe.fail(
            None,
            &format!("ready_read_sample_count/{method}/{scenario}"),
            format!(
                "{} verified ready samples; require>=1000 for this exact method/scenario",
                metrics.samples
            ),
        );
    }
    if let Some(p99) = metrics.p99_us {
        if p99 > READ_BOUND_US {
            observe.violation(Violation { original_id: None, kind: format!("ready_read_p99/{method}/{scenario}"), observed_at_us: observe.now_us(), measured_us: Some(p99), limit_us: Some(READ_BOUND_US), detail: format!("{} verified ready samples; {} individual >100ms samples retained in method report", metrics.samples, metrics.violations.len()) });
        }
    }
}

#[path = "reads/policy_tests.rs"]
mod policy_tests;
