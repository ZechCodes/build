use super::*;

#[test]
fn delivering_a_legacy_receipt_preserves_its_payload_span_before_dropping_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let record = plan_record("task-1");
    let mut receipt = queued_operation("legacy-span", 2);
    receipt.message_start_sequence = 0;
    receipt
        .delivery
        .as_mut()
        .unwrap()
        .payload
        .as_mut()
        .unwrap()
        .start_sequence = 1;
    store
        .accept_thread_post("task-1", &record.agents, &receipt)
        .unwrap();
    assert!(store
        .transition_operation(
            "legacy-span",
            OperationStatus::Queued,
            OperationStatus::Delivered,
            None
        )
        .unwrap());
    let delivered = store.operation("legacy-span").unwrap().unwrap();
    assert_eq!(delivered.message_start_sequence, 1);
    assert_eq!(delivered.posted_sequence, 2);
    assert!(delivered.delivery.is_none());
}

#[test]
fn delivered_payload_is_null_and_retry_and_conflict_need_only_the_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut record = plan_record("task-1");
    record.agents[0]
        .thread
        .post_user("keep transcript", None, NOW);
    let receipt = queued_operation("delivered", 1);
    store
        .accept_thread_post("task-1", &record.agents, &receipt)
        .unwrap();
    assert!(store
        .transition_operation(
            "delivered",
            OperationStatus::Queued,
            OperationStatus::Delivered,
            None
        )
        .unwrap());
    let raw: Option<String> = store
        .connection()
        .query_row(
            "SELECT delivery FROM operations WHERE operation_id = 'delivered'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(raw.is_none(), "delivered payload must be SQL NULL");
    let writes = store.total_changes();
    let retried = store
        .accept_thread_post("task-1", &record.agents, &receipt)
        .unwrap();
    assert_eq!(retried.status, OperationStatus::Delivered);
    assert!(retried.delivery.is_none());
    assert_eq!(store.total_changes(), writes);
    let mut conflicting = receipt;
    conflicting.request_hash = "changed".into();
    assert!(matches!(
        store.accept_thread_post("task-1", &record.agents, &conflicting),
        Err(StoreError::OperationConflict { .. })
    ));
    assert_eq!(store.thread_items(&record.agents[0].id).unwrap().len(), 1);
}

#[test]
fn recovery_retains_queued_and_uncertain_payloads_and_a_stale_transition_cannot_drop_them() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let record = plan_record("task-1");
    let queued = queued_operation("queued", 1);
    let uncertain = queued_operation("uncertain", 2);
    store
        .accept_thread_post("task-1", &record.agents, &queued)
        .unwrap();
    store
        .accept_thread_post("task-1", &record.agents, &uncertain)
        .unwrap();
    assert!(store
        .transition_operation(
            "uncertain",
            OperationStatus::Queued,
            OperationStatus::Uncertain,
            None
        )
        .unwrap());
    assert!(!store
        .transition_operation(
            "uncertain",
            OperationStatus::Claimed,
            OperationStatus::Delivered,
            None
        )
        .unwrap());
    drop(store);
    let reopened = Store::new(dir.path()).unwrap();
    let recovered = reopened.recover_operations().unwrap();
    assert_eq!(recovered.len(), 2);
    assert_eq!(
        recovered
            .iter()
            .find(|r| r.operation_id == "queued")
            .unwrap(),
        &queued
    );
    let recovered_uncertain = recovered
        .iter()
        .find(|r| r.operation_id == "uncertain")
        .unwrap();
    assert_eq!(recovered_uncertain.status, OperationStatus::Uncertain);
    assert_eq!(recovered_uncertain.delivery, uncertain.delivery);
}

#[test]
fn recovery_repairs_a_delivered_span_without_a_payload_and_keeps_other_messages() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut record = plan_record("task-1");
    record.agents[0].thread.post_user("first", None, NOW);
    record.agents[0].thread.post_user("second", None, NOW);
    record.agents[0]
        .thread
        .bind_operation_messages("delivered", 0, 2);
    record.agents[0].thread.post_user("unrelated", None, NOW);
    let mut receipt = queued_operation("delivered", 2);
    receipt.message_start_sequence = 1;
    store
        .accept_thread_post("task-1", &record.agents, &receipt)
        .unwrap();
    // Simulate a crash after the terminal status commit, before projection.
    store
        .connection()
        .execute(
            "UPDATE operations SET status = 'delivered', delivery = NULL",
            [],
        )
        .unwrap();
    drop(store);
    let reopened = Store::new(dir.path()).unwrap();
    assert!(reopened.recover_operations().unwrap().is_empty());
    let statuses: Vec<_> = reopened
        .thread_items(&record.agents[0].id)
        .unwrap()
        .into_iter()
        .filter_map(|item| match item {
            ThreadItem::Message(message) => Some(message.delivery_status),
            _ => None,
        })
        .collect();
    assert_eq!(
        statuses,
        vec![
            Some(MessageDeliveryStatus::Uncertain),
            Some(MessageDeliveryStatus::Uncertain),
            None
        ]
    );
}
