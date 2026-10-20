use super::*;
use crate::operation::OperationStatus;

fn accept_in_memory(state: &mut AppState, record: &PersistedRun) {
    state.store = None;
    state
        .operation_ledger
        .stage_acceptance(record.id.clone(), read_receipt(record));
    state
        .operation_ledger
        .remember_in_memory_acceptance(&record.id);
    state.initialize_read_context();
}

fn claim_operation(state: &mut AppState) {
    assert!(state
        .transition_delivery_operation(
            "operation-read",
            OperationStatus::Queued,
            OperationStatus::Claimed,
            None,
        )
        .unwrap());
}

#[test]
fn receipt_publication_preserves_temporarily_checked_out_owner() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let record = stored_read_run(&mut state, "run-receipt-held", 0);
    accept_in_memory(&mut state, &record);
    let reads = state.read_service();
    let params = json!({"entity_id": record.id});
    let before = reads.call(req("thread.page", params.clone()), None);
    assert_eq!(before["ok"], true, "{before}");
    let active = state.runs.remove(&record.id).unwrap();

    claim_operation(&mut state);

    let mut claimed = read_receipt(&record);
    claimed.status = OperationStatus::Claimed;
    let operation = reads.call(
        req(
            "thread.operation",
            json!({"entity_id": record.id, "operation_id": claimed.operation_id}),
        ),
        None,
    );
    assert_eq!(operation["ok"], true, "{operation}");
    assert_eq!(operation["result"], claimed.wire_value());
    assert_eq!(reads.call(req("thread.page", params), None), before);
    state.runs.insert(record.id, active);
}

#[test]
fn receipt_publication_does_not_admit_failed_roster_candidates() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let record = stored_read_run(&mut state, "run-receipt-failed", 0);
    accept_in_memory(&mut state, &record);
    let reads = state.read_service();
    let params = json!({"entity_id": record.id});
    let before = reads.call(req("thread.page", params.clone()), None);
    assert_eq!(before["ok"], true, "{before}");
    let roster = &mut state.runs.get_mut(&record.id).unwrap().agents;
    let added = roster
        .add(
            record.id.as_str(),
            ModelChoice::default(),
            "2026-10-19T20:03:00Z",
        )
        .id
        .clone();
    roster
        .primary_mut()
        .unwrap()
        .thread
        .post_user("unsaved", None, "2026-10-19T20:03:00Z");
    state.publish_read_owner(&record.id, false);

    claim_operation(&mut state);

    assert_eq!(reads.call(req("thread.page", params), None), before);
    let refused = reads.call(
        req(
            "thread.page",
            json!({"entity_id": record.id, "agent_id": added}),
        ),
        None,
    );
    assert_eq!(refused["ok"], false, "{refused}");
}
