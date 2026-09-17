use super::*;

fn receipt(id: &str, owner: &str, status: OperationStatus) -> OperationReceipt {
    OperationReceipt {
        operation_id: id.into(),
        method: crate::operation::THREAD_POST_METHOD.into(),
        entity_id: owner.into(),
        agent_id: "agent".into(),
        conversation_id: "conversation".into(),
        choice_revision: 7,
        posted_sequence: 12,
        message_start_sequence: 10,
        status,
        execution_error: None,
        request_hash: "request-hash".into(),
        delivery: None,
        requested_by: None,
    }
}

#[test]
fn acceptance_is_consumed_only_by_its_canonical_owner() {
    let mut ledger = OperationLedger::default();
    ledger.stage_acceptance(
        "owner".into(),
        receipt("operation", "owner", OperationStatus::Queued),
    );
    assert!(ledger.consume_acceptance_for("alias").is_none());
    assert!(ledger.consume_acceptance_for("owner").is_some());
    assert!(ledger.cached("operation").is_none());
}

#[test]
fn only_no_store_acceptance_hydrates_the_cache() {
    let mut ledger = OperationLedger::default();
    ledger.stage_acceptance(
        "owner".into(),
        receipt("operation", "owner", OperationStatus::Queued),
    );
    ledger.remember_in_memory_acceptance("owner");
    assert_eq!(
        ledger.cached("operation").unwrap().status,
        OperationStatus::Queued
    );
}

#[test]
fn no_store_compare_and_set_checks_the_cached_expected_status() {
    let mut ledger = OperationLedger::default();
    ledger.stage_acceptance(
        "owner".into(),
        receipt("operation", "owner", OperationStatus::Queued),
    );
    ledger.remember_in_memory_acceptance("owner");
    assert!(!ledger.cached_has_status("operation", OperationStatus::Claimed));
    assert!(ledger.cached_has_status("operation", OperationStatus::Queued));
}

#[test]
fn authoritative_store_success_overwrites_a_stale_cached_status() {
    let mut ledger = OperationLedger::default();
    ledger.stage_acceptance(
        "owner".into(),
        receipt("operation", "owner", OperationStatus::Delivered),
    );
    ledger.remember_in_memory_acceptance("owner");
    let changed_owner = ledger.record_transition_if_cached(
        "operation",
        OperationStatus::Claimed,
        Some("authoritative transition"),
    );
    assert_eq!(changed_owner.as_deref(), Some("owner"));
    let updated = ledger.cached("operation").unwrap();
    assert_eq!(updated.status, OperationStatus::Claimed);
    assert_eq!(
        updated.execution_error.as_deref(),
        Some("authoritative transition")
    );
    assert_eq!(updated.posted_sequence, 12);
    assert_eq!(updated.message_start_sequence, 10);
}

#[test]
fn authoritative_transition_succeeds_without_a_cached_receipt() {
    let mut ledger = OperationLedger::default();
    assert!(ledger
        .record_transition_if_cached("store-only", OperationStatus::Claimed, None)
        .is_none());
    // The adapter still returns Store's `changed == true`; cache absence is not
    // a second CAS and only suppresses the entity-change notification.
}

// Boot recovery remains adapter code: queued receipts become delivery turns in
// input order, but none are inserted into OperationLedger.receipts.
