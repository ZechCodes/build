//! Unit inputs only: WatchReport/MCP structs below are not native-run evidence.
use super::super::super::client_watch::{Breach, FinalOutcome, TrafficCohort};
use super::super::super::mcp_adapter::{McpCallRow, McpEvidence, SocketEvidence};
use super::super::super::trial_complete::{Envelope, JsonArtifacts, Route, Traffic};
use super::*;
use sha2::Digest;

#[path = "unit_inputs.rs"]
mod fixture;
use fixture::unit_inputs;
use std::collections::BTreeMap;
use std::sync::Arc;

#[test]
fn final_transport_audit_accepts_exact_three_cohort_unit_lifecycles() {
    let (plan, watch, report, mcp) = unit_inputs();
    let reconcile = watch.reconcile_final();
    assert!(reconcile.violations.is_empty());
    assert!(reconcile
        .rows
        .iter()
        .filter(|row| row.cohort != TrafficCohort::SemanticWrite)
        .all(|row| row.server_registered_at.is_none() && row.admitted_at.is_none()));
    let errors =
        validate::validate_final_transport(&plan, Some(&reconcile), Some(&report), &mcp, true);
    assert!(
        errors.is_empty(),
        "complete explicit unit lifecycles must pass, without invented read/exempt origins: {errors:?}"
    );
}

#[test]
fn final_transport_audit_rejects_duplicate_missing_extra_and_wrong_cohort_unit_ids() {
    let (plan, watch, report, mcp) = unit_inputs();
    let mut reconcile = watch.reconcile_final();
    let write = reconcile
        .rows
        .iter_mut()
        .find(|row| row.original_id == "unit-write")
        .unwrap();
    write.cohort = TrafficCohort::Exempt;
    let duplicate = write.clone();
    reconcile.rows.push(duplicate);
    reconcile.rows.retain(|row| row.original_id != "unit-read");
    let mut extra = reconcile.rows[0].clone();
    extra.original_id = "unit-unplanned".into();
    reconcile.rows.push(extra);
    let errors =
        validate::validate_final_transport(&plan, Some(&reconcile), Some(&report), &mcp, true);
    for expected in [
        "duplicate_client_original",
        "missing_client_original",
        "unexpected_client_original",
        "planned_cohort_mismatch",
    ] {
        assert!(
            errors.iter().any(|error| error.contains(expected)),
            "missing {expected}: {errors:?}"
        );
    }
}

#[test]
fn final_transport_audit_rejects_actual_unfinished_or_implicit_client_reconciliation() {
    let (plan, watch, report, mcp) = unit_inputs();
    watch.register_at("unit-never-sent", Instant::now());
    watch.offer("unit-implicit");
    watch.sent("unit-implicit");
    watch.cancel("unit-implicit");
    watch.record_unmatched_reply("unit-unmatched");
    let base = watch
        .schedule_lookup("unit-write")
        .unwrap()
        .intended_at
        .unwrap();
    watch.register_cohort_at("unit-protocol-error", base, TrafficCohort::Read);
    watch.offer("unit-protocol-error");
    watch.sent("unit-protocol-error");
    watch.complete_with_outcome("unit-protocol-error", FinalOutcome::ProtocolError);
    let reconcile = watch.reconcile_final();
    assert!(reconcile
        .violations
        .iter()
        .any(|error| error.contains("actual send")));
    assert!(reconcile
        .violations
        .iter()
        .any(|error| error.contains("explicit schedule")));
    assert!(reconcile
        .violations
        .iter()
        .any(|error| error.contains("unmatched reply")));
    assert!(reconcile
        .violations
        .iter()
        .any(|error| error.contains("successful transport reply")));
    assert!(reconcile
        .violations
        .iter()
        .any(|error| error.contains("actual admission")));
    let errors =
        validate::validate_final_transport(&plan, Some(&reconcile), Some(&report), &mcp, true);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("client_reconciliation")),
        "never-sent/cancelled/implicit/unmatched are lifecycle failures even before5s: {errors:?}"
    );
}

#[test]
fn final_transport_audit_rejects_watch_failure_pending_index_and_unjoined_overlimit_mcp() {
    let (plan, watch, mut report, mut mcp) = unit_inputs();
    let reconcile = watch.reconcile_final();
    report.failed = true;
    report.panicked = true;
    report.still_indexed = 1;
    let now = Instant::now();
    report.breaches.push(Breach {
        original_id: "unit-write".into(),
        cohort: TrafficCohort::SemanticWrite,
        intended_at: now,
        observed_at: now,
        settled_at: None,
    });
    mcp.evidence.sockets[0].maximum_active = 3;
    let call = mcp.evidence.calls.get_mut("unit-write").unwrap();
    call.server_joined_at = None;
    call.worker_reaped_at = None;
    let errors =
        validate::validate_final_transport(&plan, Some(&reconcile), Some(&report), &mcp, true);
    for expected in [
        "watch_failed",
        "watch_panicked",
        "watch_still_indexed",
        "watch_breach",
        "mcp_socket_limit",
        "mcp_worker_join",
    ] {
        assert!(
            errors.iter().any(|error| error.contains(expected)),
            "missing {expected}: {errors:?}"
        );
    }
}

#[test]
fn final_transport_export_retains_actual_unit_rows_and_reports_even_when_audit_rejects() {
    let (plan, watch, report, mcp) = unit_inputs();
    let root = tempfile::tempdir().unwrap();
    let sink = JsonArtifacts::new(root.path().join("unit-artifacts")).unwrap();
    let name = TrialName {
        envelope: Envelope::Small,
        number: 1,
    };
    let result = archive_final(name, &plan, Some(&watch), Some(&report), &mcp, true, &sink);
    assert!(result.persisted);
    let path = root
        .path()
        .join("unit-artifacts/small_trial_1/transport-final.json");
    let raw: Value = serde_json::from_reader(std::fs::File::open(path).unwrap()).unwrap();
    assert_eq!(raw["qualification"], false);
    assert_eq!(raw["reconciliation"]["sampled_at_us"], 0);
    assert_eq!(raw["reconciliation"]["rows"].as_array().unwrap().len(), 3);
    assert_eq!(raw["watch_report"]["still_indexed"], 0);
    assert_eq!(raw["mcp_shutdown"]["calls"][0]["original_id"], "unit-write");
    assert_eq!(
        raw["mcp_shutdown"]["calls"][0]["actual_wire_id"],
        Value::Null
    );
    assert!(raw["mcp_shutdown"]["calls"][0]["server_joined_at_us"].is_i64());
    assert_eq!(raw["mcp_shutdown"]["sockets"][0]["maximum_active"], 1);
    assert_eq!(raw["mcp_shutdown"]["sockets"][0]["limit"], 2);
    assert_eq!(raw["audit_violations"], result.raw["audit_violations"]);
}

#[test]
fn final_transport_audit_rejects_missing_extra_or_wrong_cohort_actual_mcp_calls() {
    let (plan, watch, report, mcp) = unit_inputs();
    let reconcile = watch.reconcile_final();
    let mut missing = mcp.clone();
    missing.evidence.calls.clear();
    let errors =
        validate::validate_final_transport(&plan, Some(&reconcile), Some(&report), &missing, true);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("missing_mcp_original")),
        "{errors:?}"
    );
    let mut extra = mcp.clone();
    let mut call = extra.evidence.calls["unit-write"].clone();
    call.original_id = "unit-unplanned".into();
    extra.evidence.calls.insert(call.original_id.clone(), call);
    let errors =
        validate::validate_final_transport(&plan, Some(&reconcile), Some(&report), &extra, true);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("unexpected_mcp_original")),
        "{errors:?}"
    );
    let mut wrong = mcp.clone();
    wrong.evidence.calls.get_mut("unit-write").unwrap().read = true;
    let errors =
        validate::validate_final_transport(&plan, Some(&reconcile), Some(&report), &wrong, true);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("mcp_cohort_mismatch")),
        "{errors:?}"
    );
}

#[test]
fn final_transport_audit_rejects_absent_actual_guard_or_native_join_receipt() {
    let (plan, _, _, mcp) = unit_inputs();
    let errors = validate::validate_final_transport(&plan, None, None, &mcp, false);
    for expected in [
        "missing_client_reconciliation",
        "missing_watch_report",
        "native_cleanup_unconfirmed",
    ] {
        assert!(
            errors.iter().any(|error| error.contains(expected)),
            "missing {expected}: {errors:?}"
        );
    }
}
