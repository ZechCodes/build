//! Final checks over the existing joined watch and native MCP reports.
use super::super::super::client_watch::{ClientRow, FinalOutcome, TrafficCohort};
use super::super::super::trial_complete::{Route, Traffic};
use super::*;
use std::collections::{BTreeMap, BTreeSet};
#[path = "validate_mcp.rs"]
mod mcp;

pub(super) fn validate_final_transport(
    plan: &[Slot],
    reconcile: Option<&FinalReconciliation>,
    report: Option<&WatchReport>,
    mcp: &McpShutdownReport,
    native_joined: bool,
) -> Vec<String> {
    let mut errors = Vec::new();
    let mut expected = BTreeMap::new();
    for slot in plan {
        if expected.insert(slot.original_id.as_str(), slot).is_some() {
            errors.push(format!("duplicate_planned_original: {}", slot.original_id));
        }
    }
    if let Some(reconcile) = reconcile {
        validate_clients(&expected, reconcile, &mut errors);
    } else {
        errors.push("missing_client_reconciliation".into());
    }
    validate_watch(report, &mut errors);
    mcp::validate(&expected, mcp, &mut errors);
    if !native_joined {
        errors.push("native_cleanup_unconfirmed".into());
    }
    errors
}

fn validate_clients(
    expected: &BTreeMap<&str, &Slot>,
    reconcile: &FinalReconciliation,
    errors: &mut Vec<String>,
) {
    errors.extend(
        reconcile
            .violations
            .iter()
            .map(|error| format!("client_reconciliation: {error}")),
    );
    let mut seen = BTreeSet::new();
    for row in &reconcile.rows {
        if !seen.insert(row.original_id.as_str()) {
            errors.push(format!("duplicate_client_original: {}", row.original_id));
        }
        match expected.get(row.original_id.as_str()) {
            Some(slot) if cohort(slot.traffic) != row.cohort => {
                errors.push(format!("planned_cohort_mismatch: {}", row.original_id))
            }
            None => errors.push(format!("unexpected_client_original: {}", row.original_id)),
            _ => {}
        }
        if !client_complete(row) {
            errors.push(format!(
                "client_reconciliation: {} incomplete or failed lifecycle",
                row.original_id
            ));
        }
    }
    errors.extend(
        expected
            .keys()
            .filter(|id| !seen.contains(**id))
            .map(|id| format!("missing_client_original: {id}")),
    );
}

fn cohort(traffic: Traffic) -> TrafficCohort {
    match traffic {
        Traffic::Write => TrafficCohort::SemanticWrite,
        Traffic::Read => TrafficCohort::Read,
        Traffic::Exempt => TrafficCohort::Exempt,
    }
}

fn client_complete(row: &ClientRow) -> bool {
    let semantic = row.cohort == TrafficCohort::SemanticWrite;
    let supplied = [row.offered_at, row.sent_at, row.final_at, row.finished_at];
    let semantic_supplied =
        !semantic || (row.server_registered_at.is_some() && row.admitted_at.is_some());
    supplied.iter().all(Option::is_some)
        && semantic_supplied
        && row.final_outcome == Some(FinalOutcome::Reply)
        && row.registered_at <= row.intended_at
        && !row.implicit_schedule
        && !row.cancelled
        && row.cancelled_at.is_none()
        && !row.breached
}

fn validate_watch(report: Option<&WatchReport>, errors: &mut Vec<String>) {
    let Some(report) = report else {
        errors.push("missing_watch_report".into());
        return;
    };
    for (failed, diagnostic) in [
        (report.failed, "watch_failed"),
        (report.panicked, "watch_panicked"),
        (report.still_indexed != 0, "watch_still_indexed"),
        (!report.breaches.is_empty(), "watch_breach"),
    ] {
        if failed {
            errors.push(diagnostic.into());
        }
    }
}
