//! Final-only actual transport evidence. TEST-FIRST validation remains pending.
use super::super::client_watch::{ClientWatch, FinalReconciliation, WatchReport};
use super::super::mcp_adapter::McpShutdownReport;
use super::super::trial_complete::{ArtifactSink, Slot, TrialName};
use super::overload_refusals::VerifiedRefusals;
use serde_json::{json, Value};
use std::time::Instant;

#[path = "final_transport/export.rs"]
mod export;
#[path = "final_transport/refusals.rs"]
mod refusals;
#[cfg(test)]
#[path = "final_transport/tests.rs"]
mod tests;
#[path = "final_transport/validate.rs"]
mod validate;

pub(super) struct FinalArchive {
    pub raw: Value,
    pub errors: Vec<String>,
    pub persisted: bool,
}

pub(super) struct ClientAudit<'a> {
    pub watch: Option<&'a ClientWatch>,
    pub before_cleanup: Option<&'a FinalReconciliation>,
}

pub(super) fn archive_final(
    name: TrialName,
    plan: &[Slot],
    client: ClientAudit<'_>,
    report: Option<&WatchReport>,
    mcp: &McpShutdownReport,
    native_joined: bool,
    sink: &dyn ArtifactSink,
) -> FinalArchive {
    archive_bound(name, (plan, None), client, report, mcp, native_joined, sink)
}

pub(super) fn archive_final_overload(
    name: TrialName,
    bound_plan: (&[Slot], &VerifiedRefusals),
    client: ClientAudit<'_>,
    report: Option<&WatchReport>,
    mcp: &McpShutdownReport,
    native_joined: bool,
    sink: &dyn ArtifactSink,
) -> FinalArchive {
    archive_bound(
        name,
        (bound_plan.0, Some(bound_plan.1)),
        client,
        report,
        mcp,
        native_joined,
        sink,
    )
}

fn archive_bound(
    name: TrialName,
    (plan, refused): (&[Slot], Option<&VerifiedRefusals>),
    client: ClientAudit<'_>,
    report: Option<&WatchReport>,
    mcp: &McpShutdownReport,
    native_joined: bool,
    sink: &dyn ArtifactSink,
) -> FinalArchive {
    // None explicitly means the guard was not confirmed joined. No lifecycle
    // scan occurs until its actual report exists, and never on progress polls.
    let reconcile = report.and_then(|_| client.watch.map(ClientWatch::reconcile_final));
    let violations =
        validate::validate_final_transport(plan, reconcile.as_ref(), report, mcp, native_joined);
    let anchor = reconcile
        .as_ref()
        .map(|rows| rows.sampled_at)
        .unwrap_or_else(Instant::now);
    let partition = refused.map(|verified| {
        refusals::partition(
            plan,
            reconcile.as_ref(),
            violations.clone(),
            verified,
            anchor,
        )
    });
    let effective = partition
        .as_ref()
        .map(|audit| audit.errors.as_slice())
        .unwrap_or(&violations);
    let mut record = json!({
        "schema":"build.transport-final.NoQualification.v1-testfirst", "qualification":false,
        "clock":"signed microseconds relative to this export's single actual Instant anchor",
        "scope":"final cleanup attempt only; no independent watchdog or progress snapshot",
        "native_joined":native_joined,"guard_joined":report.is_some(),
        "planned_client_slots":plan,
        "reconciliation_before_cleanup":client.before_cleanup.map(|value| export::reconciliation(value, anchor)),
        "reconciliation":reconcile.as_ref().map(|value| export::reconciliation(value, anchor)),
        "watch_report":report.map(|value| export::watch_report(value, anchor)),
        "mcp_shutdown":export::mcp_report(mcp, anchor),
        "audit_violations":effective,
        "raw_registry_reference":"native-registry-final.json",
        "raw_registry_availability":"separate artifact; not asserted by this transport export"
    });
    if let Some(partition) = &partition {
        // The original strict audit and raw ProtocolError rows stay untouched.
        record["strict_audit_violations"] = json!(violations);
        record["verified_refusal_partition"] = json!(partition.discharged);
    }
    // Persist failed audits as well, before returning any audit rejection.
    let persisted = sink.transport_archive(name, &record);
    let mut errors = partition.map(|audit| audit.errors).unwrap_or(violations);
    if let Err(error) = &persisted {
        errors.push(format!("transport archive persistence: {error}"));
    }
    FinalArchive {
        raw: record,
        errors,
        persisted: persisted.is_ok(),
    }
}
