//! Boundary controls: genuine cancelled registry/accepted-collector tokens from
//! the shared verifier fixture; MCP report below is unit input, not 400/s proof.
use super::super::super::super::client_watch::FinalOutcome;
use super::super::super::super::mcp_adapter::McpEvidence;
use super::super::super::super::trial_complete::{Envelope, JsonArtifacts, Route, Traffic};
use super::super::super::overload_refusals::test_support::{
    verified_spa_full, verified_zero_refusals, Bundle,
};
use super::*;
use std::time::Duration;

fn quiet_mcp_unit_input() -> McpShutdownReport {
    McpShutdownReport {
        quiet: true,
        unfinished: Vec::new(),
        evidence: McpEvidence::default(),
    }
}
fn run_audit(bundle: &Bundle, plan: &[Slot], rows: &FinalReconciliation) -> (Vec<String>, Audit) {
    let strict = super::super::validate::validate_final_transport(
        plan,
        Some(rows),
        Some(&bundle.watch_report),
        &quiet_mcp_unit_input(),
        true,
    );
    let audit = partition(
        plan,
        Some(rows),
        strict.clone(),
        &bundle.token,
        rows.sampled_at,
    );
    (strict, audit)
}
fn contains(errors: &[String], expected: &str) {
    assert!(
        errors.iter().any(|error| error.contains(expected)),
        "missing {expected}: {errors:?}"
    );
}

#[test]
fn final_transport_refusal_partitions_only_exact_verified_spa_protocol_reply() {
    crate::git_fixture::environment::isolated_git_test!();
    let bundle = verified_spa_full();
    let rows = bundle.watch.reconcile_final();
    let raw_violations = rows.violations.clone();
    let (strict, audit) = run_audit(&bundle, &bundle.plan, &rows);
    contains(&strict, "successful transport reply");
    contains(&strict, "incomplete or failed lifecycle");
    assert!(
        audit.errors.is_empty(),
        "verified refusal partition must pass: {:?}",
        audit.errors
    );
    assert_eq!(audit.discharged.len(), 1);
    assert_eq!(
        rows.violations, raw_violations,
        "raw strict reconciliation remains intact"
    );
    assert_eq!(
        rows.rows[0].final_outcome,
        Some(FinalOutcome::ProtocolError)
    );
    let proof = bundle.token.get(&bundle.plan[0].original_id).unwrap();
    assert_eq!(audit.discharged[0]["original_id"], proof.original_id());
    assert_eq!(audit.discharged[0]["method"], proof.method());
    assert_eq!(audit.discharged[0]["raw_reply"], *proof.raw_reply());
    assert_eq!(
        audit.discharged[0]["raw_response_artifact"],
        proof.raw_response_artifact()
    );
}

#[test]
fn final_transport_zero_refusals_preserves_exact_normal_audit() {
    crate::git_fixture::environment::isolated_git_test!();
    let bundle = verified_zero_refusals();
    let mut rows = bundle.watch.reconcile_final();
    let (strict, audit) = run_audit(&bundle, &bundle.plan, &rows);
    assert!(
        strict.is_empty(),
        "empty native-quiet fixture prerequisite: {strict:?}"
    );
    assert_eq!(
        audit.errors, strict,
        "busy is optional; zero verified refusals is valid"
    );
    assert!(audit.discharged.is_empty());
    rows.violations
        .push("unmatched reply identity: unrelated-zero-busy".into());
    let (strict, audit) = run_audit(&bundle, &bundle.plan, &rows);
    assert_eq!(
        audit.errors, strict,
        "zero-refusal mode cannot relax any normal failure"
    );
}

#[test]
fn final_transport_refusal_rejects_changed_missing_duplicate_or_mcp_plan_binding() {
    crate::git_fixture::environment::isolated_git_test!();
    let bundle = verified_spa_full();
    let rows = bundle.watch.reconcile_final();
    for case in [
        "missing",
        "duplicate",
        "mcp",
        "read",
        "method",
        "owner",
        "original",
    ] {
        let mut plan = bundle.plan.clone();
        match case {
            "missing" => plan.clear(),
            "duplicate" => plan.push(plan[0].clone()),
            "mcp" => plan[0].route = Route::AgentMcp,
            "read" => plan[0].traffic = Traffic::Read,
            "method" => plan[0].method = Some("unrelated.method".into()),
            "owner" => plan[0].owner += 1,
            "original" => plan[0].original_id.push_str("/different"),
            _ => unreachable!(),
        }
        let (_, audit) = run_audit(&bundle, &plan, &rows);
        contains(&audit.errors, "refusal_plan_binding");
        assert!(
            audit.discharged.is_empty(),
            "cannot partition changed immutable {case} plan"
        );
    }
}

#[test]
fn final_transport_refusal_rejects_missing_duplicate_or_mismatched_client_evidence() {
    crate::git_fixture::environment::isolated_git_test!();
    let bundle = verified_spa_full();
    for case in [
        "missing",
        "duplicate",
        "intake",
        "classified",
        "sent",
        "decoded",
        "late",
        "cancelled",
        "transport",
        "implicit",
    ] {
        let mut rows = bundle.watch.reconcile_final();
        let proof = bundle.token.get(&bundle.plan[0].original_id).unwrap();
        match case {
            "missing" => rows.rows.clear(),
            "duplicate" => rows.rows.push(rows.rows[0].clone()),
            "intake" => {
                rows.rows[0].server_registered_at =
                    Some(proof.intake_at() + Duration::from_nanos(1))
            }
            "classified" => {
                rows.rows[0].admitted_at = Some(proof.classified_at() + Duration::from_nanos(1))
            }
            "sent" => rows.rows[0].sent_at = proof.sent_at().checked_sub(Duration::from_nanos(1)),
            "decoded" => {
                rows.rows[0].final_at = proof.decoded_at().checked_sub(Duration::from_nanos(1))
            }
            "late" => {
                rows.rows[0].final_at =
                    Some(proof.intended_at() + Duration::from_millis(100) + Duration::from_nanos(1))
            }
            "cancelled" => rows.rows[0].cancelled = true,
            "transport" => rows.rows[0].final_outcome = Some(FinalOutcome::TransportError),
            "implicit" => rows.rows[0].implicit_schedule = true,
            _ => unreachable!(),
        }
        let (_, audit) = run_audit(&bundle, &bundle.plan, &rows);
        contains(&audit.errors, "refusal_client_binding");
        assert!(
            audit.discharged.is_empty(),
            "cannot partition mismatched {case} client evidence"
        );
    }
}

#[test]
fn final_transport_refusal_retains_unknown_and_unrelated_failures_verbatim() {
    crate::git_fixture::environment::isolated_git_test!();
    let bundle = verified_spa_full();
    let mut rows = bundle.watch.reconcile_final();
    let original = bundle.plan[0].original_id.clone();
    let unknown = "unmatched reply identity: unknown-wire-original";
    let same_original_unknown = format!("{original}: unrelated native audit failure");
    rows.violations.push(unknown.into());
    rows.violations.push(same_original_unknown.clone());
    let mut unrelated = rows.rows[0].clone();
    unrelated.original_id = "unplanned-protocol-error".into();
    rows.rows.push(unrelated);
    let (_, audit) = run_audit(&bundle, &bundle.plan, &rows);
    contains(&audit.errors, unknown);
    contains(&audit.errors, &same_original_unknown);
    contains(
        &audit.errors,
        "unexpected_client_original: unplanned-protocol-error",
    );
    contains(
        &audit.errors,
        "unplanned-protocol-error incomplete or failed lifecycle",
    );
    assert_eq!(
        audit.discharged.len(),
        1,
        "only the exact verified row is discharged"
    );

    // An indistinguishable extra diagnostic is not silently eaten twice.
    rows.violations
        .push(format!("{original}: successful transport reply"));
    let (_, audit) = run_audit(&bundle, &bundle.plan, &rows);
    contains(&audit.errors, "refusal_partition_diagnostic");
    assert!(audit.discharged.is_empty());
}

#[test]
fn final_transport_refusal_export_retains_raw_partition_after_sink_failure() {
    crate::git_fixture::environment::isolated_git_test!();
    let bundle = verified_spa_full();
    let directory = tempfile::tempdir().unwrap();
    let sink = JsonArtifacts::new(directory.path().join("transport")).unwrap();
    let name = TrialName {
        envelope: Envelope::Small,
        number: 1,
    };
    let mcp = quiet_mcp_unit_input();
    let before = bundle.watch.reconcile_final();
    let first = super::super::archive_final_overload(
        name,
        (&bundle.plan, &bundle.token),
        super::super::ClientAudit {
            watch: Some(&bundle.watch),
            before_cleanup: Some(&before),
        },
        Some(&bundle.watch_report),
        &mcp,
        true,
        &sink,
    );
    assert!(
        first.persisted && first.errors.is_empty(),
        "{:?}",
        first.errors
    );
    let path = directory
        .path()
        .join("transport/small_trial_1/transport-final.json");
    let saved = std::fs::read(&path).unwrap();
    let raw: Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(
        raw["reconciliation_before_cleanup"]["rows"][0]["final_outcome"],
        "ProtocolError"
    );
    assert_eq!(
        raw["reconciliation"]["rows"][0]["final_outcome"],
        "ProtocolError"
    );
    assert!(!raw["strict_audit_violations"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(raw["audit_violations"].as_array().unwrap().is_empty());
    assert_eq!(
        raw["verified_refusal_partition"].as_array().unwrap().len(),
        1
    );
    let second = super::super::archive_final_overload(
        name,
        (&bundle.plan, &bundle.token),
        super::super::ClientAudit {
            watch: Some(&bundle.watch),
            before_cleanup: Some(&before),
        },
        Some(&bundle.watch_report),
        &mcp,
        true,
        &sink,
    );
    assert!(
        !second.persisted,
        "create_new must reject an existing final archive"
    );
    contains(&second.errors, "transport archive persistence");
    assert_eq!(
        second.raw["verified_refusal_partition"][0]["raw_reply"],
        raw["verified_refusal_partition"][0]["raw_reply"]
    );
    assert_eq!(
        second.raw["reconciliation"]["rows"][0]["final_outcome"],
        "ProtocolError"
    );
    assert_eq!(
        std::fs::read(path).unwrap(),
        saved,
        "failed sink cannot replace prior raw evidence"
    );
}
