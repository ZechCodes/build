//! Classification controls use real registry terminal rows. Wire literals
//! below are explicit parser inputs, never physical-run or product evidence.
use super::*;
use std::collections::BTreeMap;
use trial_complete::{Envelope, Recipe, Route, Row, Stage, Traffic, TrialName, UnionTrace};

fn pair(accepted: bool) -> (Row, trial_complete::ControlRefusal) {
    let mut slot = trial_complete::make_semantic_slots(TrialName {
        envelope: Envelope::Small,
        number: 1,
    })
    .remove(0);
    slot.due_us = 0;
    let method = "tasks.update";
    let registry = WriteWaitRegistry::new();
    let started = registry.now();
    let write = registry.begin(&slot.original_id, method, started);
    write.admitted(); // Actual carrier.classify runs before admitter Full.
    if accepted {
        write.acknowledged();
        write.complete();
    } else {
        write.cancel();
    }
    let terminal = registry.snapshot().pop().unwrap();
    assert!(terminal.completed_at.is_some());
    let native = super::super::super::native_archive::native_row(&terminal, started);
    let elapsed = evidence::us(terminal.completed_at.unwrap().duration_since(started));
    let milestones_us = [
        Stage::Offered,
        Stage::DispatchStarted,
        Stage::WireSent,
        Stage::ServerIntake,
        Stage::Admitted,
        Stage::ParsedFinalReply,
        Stage::Completed,
    ]
    .into_iter()
    .map(|stage| (stage, elapsed))
    .collect::<BTreeMap<_, _>>();
    let refusal = trial_complete::ControlRefusal {
        original_id: slot.original_id.clone(),
        method: method.into(),
        raw_response_artifact: "explicit-parser-input.json".into(),
        sent_to_decoded_final_ns: elapsed * 1000,
        intended_to_decoded_final_ns: elapsed * 1000,
        decoded_response: json!({"id":slot.original_id,"ok":false,"error_code":"busy","retryable":true}),
        native_terminal_rows: vec![native],
        acceptance_records: Some(vec![]),
        evidence_errors: vec![], // Explicit parser input, not a Store runtime claim.
        pending_pre_durable_verification: true,
    };
    let row = Row {
        slot,
        recipe: Recipe {
            method: method.into(),
            scenario: "refusal-classification-control".into(),
            recipe_key: "refusal-classification-control".into(),
            predecessor: None,
            needs_admission: true,
            needs_durable_ack: true,
            needs_registry_root: true,
            expected_read_cohort: None,
            durable_operation_id: None,
        },
        milestones_us,
        proof: None,
        error: None,
        frames: vec![],
        services: vec![],
        waits: Some(UnionTrace {
            write_ids: vec![format!("{:?}", terminal.id)],
            complete: false,
            ..UnionTrace::default()
        }),
    };
    (row, refusal)
}
#[test]
fn refusal_actual_acknowledged_terminal_never_proves_pre_durable_busy() {
    let (row, refusal) = pair(true);
    assert!(validate_candidate(&row, &refusal).is_err());
}
#[test]
fn refusal_native_cancel_is_separate_from_success_and_can_be_classified() {
    let (row, refusal) = pair(false);
    assert!(row.proof.is_none());
    validate_candidate(&row, &refusal).unwrap();
}
#[test]
fn refusal_missing_duplicate_or_wrong_native_terminal_is_rejected() {
    let (row, original) = pair(false);
    let mut missing = original.clone();
    missing.native_terminal_rows.clear();
    let mut duplicate = original.clone();
    duplicate
        .native_terminal_rows
        .push(duplicate.native_terminal_rows[0].clone());
    let mut wrong = original;
    wrong.native_terminal_rows[0]["original_id"] = json!("explicit-corruption/foreign");
    for candidate in [missing, duplicate, wrong] {
        assert!(validate_candidate(&row, &candidate).is_err());
    }
}
#[test]
fn refusal_original_due_deadline_includes_presend_delay() {
    let (row, mut candidate) = pair(false);
    candidate.sent_to_decoded_final_ns = 1;
    candidate.intended_to_decoded_final_ns = 100_000_001;
    assert!(validate_candidate(&row, &candidate).is_err());
}
#[test]
fn refusal_wrong_method_nonretryable_and_read_candidates_are_rejected() {
    let (row, candidate) = pair(false);
    let mut wrong = candidate.clone();
    wrong.method = "another.method".into();
    let mut permanent = candidate.clone();
    permanent.decoded_response["retryable"] = json!(false);
    assert!(validate_candidate(&row, &wrong).is_err());
    assert!(validate_candidate(&row, &permanent).is_err());
    let mut read = row;
    read.slot.traffic = Traffic::Read;
    read.slot.route = Route::SpaRpc;
    assert!(validate_candidate(&read, &candidate).is_err());
}
#[test]
fn refusal_executed_frame_or_missing_wait_archive_is_rejected() {
    let (mut row, candidate) = pair(false);
    row.waits = None;
    assert!(validate_candidate(&row, &candidate).is_err());
    let native = &candidate.native_terminal_rows[0];
    assert!(matches!(
        archived(
            &row.slot,
            &candidate,
            &candidate.decoded_response,
            Some(native),
            1
        ),
        BusyDisposition::Invalid { .. }
    ));
}

#[test]
fn refusal_missing_acceptance_archive_is_not_evidence_of_no_save() {
    let (row, mut candidate) = pair(false);
    candidate.acceptance_records = None;
    assert!(validate_candidate(&row, &candidate).is_err());
    candidate.acceptance_records = Some(vec![
        json!({"explicit_corruption":"unexpected accepted record"}),
    ]);
    assert!(validate_candidate(&row, &candidate).is_err());
}

#[test]
fn refusal_capture_errors_and_missing_raw_artifact_remain_failures() {
    let (row, mut candidate) = pair(false);
    candidate
        .evidence_errors
        .push("explicit retained sink failure".into());
    assert!(validate_candidate(&row, &candidate).is_err());
    candidate.evidence_errors.clear();
    candidate.raw_response_artifact.clear();
    assert!(validate_candidate(&row, &candidate).is_err());
}
