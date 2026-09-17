use super::support::*;
use super::*;
use crate::thread::MessageDeliveryStatus;

#[test]
fn thread_post_receipt_and_message_commit_together_and_retry_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut record = plan_record("issue-1");
    record.agents[0].thread.post_user("hello", None, NOW);
    let receipt = queued_operation("op-1", 1);

    let accepted = store
        .accept_thread_post("issue-1", &record.agents, &receipt)
        .unwrap();
    assert_eq!(accepted, receipt);
    let writes = store.total_changes();
    let retried = store
        .accept_thread_post("issue-1", &record.agents, &receipt)
        .unwrap();
    assert_eq!(retried, receipt);
    assert_eq!(
        store.total_changes(),
        writes,
        "retry wrote no second effect"
    );
    assert_eq!(store.thread_items(&record.agents[0].id).unwrap().len(), 1);
}

/// The agent that asked for an operation survives the write, because the reply
/// is forwarded to the conversation this names and a restart must still know
/// where that is.
#[test]
fn an_operation_an_agent_asked_for_remembers_which_agent_and_where_to_answer() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let record = plan_record("issue-1");
    let mut receipt = queued_operation("op-1", 1);
    receipt.requested_by = Some(crate::operation::OperationRequester {
        agent_id: "project-01H".to_string(),
        entity_id: "run-project".to_string(),
        conversation_id: "conversation-project".to_string(),
    });

    store
        .accept_thread_post("issue-1", &record.agents, &receipt)
        .unwrap();
    let read = store
        .operation("op-1")
        .unwrap()
        .expect("the receipt is there");
    assert_eq!(read.requested_by, receipt.requested_by);
    assert_eq!(read.requested_by.unwrap().identity().id, "project-01H");

    // The human's own operation names nobody, which is what every operation
    // written before an agent could ask for one reads as.
    let humans = queued_operation("op-2", 2);
    store
        .accept_thread_post("issue-1", &record.agents, &humans)
        .unwrap();
    assert!(store
        .operation("op-2")
        .unwrap()
        .expect("the receipt is there")
        .requested_by
        .is_none());
}

#[test]
fn operation_id_reuse_with_a_different_request_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let record = plan_record("issue-1");
    let receipt = queued_operation("op-1", 1);
    store
        .accept_thread_post("issue-1", &record.agents, &receipt)
        .unwrap();

    let mut reused = receipt.clone();
    reused.request_hash = "different-request".to_string();
    let error = store
        .accept_thread_post("issue-1", &record.agents, &reused)
        .unwrap_err();
    assert!(matches!(error, StoreError::OperationConflict { .. }));
}

#[test]
fn operation_acknowledgement_updates_a_message_below_the_resident_tail() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut record = plan_record("issue-1");
    let thread = &mut record.agents[0].thread;
    let before = thread.last_sequence();
    thread.post_user("managed message", None, NOW);
    let sequence = thread.last_sequence();
    let messages = thread.bind_operation_messages("old-op", before, sequence);
    let mut receipt = queued_operation("old-op", sequence);
    receipt.message_start_sequence = sequence;
    receipt.delivery.as_mut().unwrap().payload = Some(OperationPayload {
        start_sequence: sequence,
        end_sequence: sequence,
        messages,
        prior_context: String::new(),
    });
    store
        .accept_thread_post("issue-1", &record.agents, &receipt)
        .unwrap();
    for index in 0..250 {
        record.agents[0]
            .thread
            .post_user(format!("later {index}"), None, NOW);
    }
    store.save_issue_plan(&record).unwrap();
    drop(store);

    let reopened = Store::new(dir.path()).unwrap();
    let loaded = reopened.load_all_issues().unwrap().remove(0);
    assert!(
        loaded.issue.agents[0]
            .thread
            .items
            .iter()
            .all(|item| item.sequence() != sequence),
        "the managed message is below the bounded resident tail"
    );
    let previous_last = loaded.issue.agents[0].thread.last_sequence();
    let acknowledged_sequence = reopened
        .acknowledge_operation_messages(
            &record.agents[0].id,
            "old-op",
            sequence,
            sequence,
            "2026-08-21T10:01:00Z",
        )
        .unwrap();
    assert!(acknowledged_sequence > previous_last);
    assert!(reopened
        .thread_items_after(&record.agents[0].id, previous_last)
        .unwrap()
        .iter()
        .any(|item| item.sequence() == sequence));
    let raw: String = reopened
        .connection()
        .query_row(
            "SELECT item FROM thread_items WHERE agent_id = ?1 AND sequence = ?2",
            rusqlite::params![record.agents[0].id, sequence as i64],
            |row| row.get(0),
        )
        .unwrap();
    let item: ThreadItem = serde_json::from_str(&raw).unwrap();
    let ThreadItem::Message(message) = item else {
        panic!("the accepted item is a message");
    };
    assert_eq!(message.operation_id.as_deref(), Some("old-op"));
    assert_eq!(message.seen_at.as_deref(), Some("2026-08-21T10:01:00Z"));
    assert_eq!(message.updated_sequence, acknowledged_sequence);
    let mut after_ack = reopened.load_all_issues().unwrap().remove(0);
    after_ack.issue.agents[0]
        .thread
        .post_user("after acknowledgement", None, NOW);
    assert!(
        after_ack.issue.agents[0].thread.last_sequence() > acknowledged_sequence,
        "the store-side acknowledgement sequence cannot be reused"
    );
}

#[test]
fn delivery_status_persists_below_the_tail_without_touching_another_operation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut record = plan_record("issue-1");
    let thread = &mut record.agents[0].thread;
    thread.post_user("managed one", None, NOW);
    thread.bind_operation_messages("op-one", 0, 1);
    thread.post_user("managed two", None, NOW);
    thread.bind_operation_messages("op-two", 1, 2);
    store.save_issue_plan(&record).unwrap();

    let bumped = store
        .set_operation_delivery_status(
            &record.agents[0].id,
            "op-one",
            1,
            1,
            MessageDeliveryStatus::Sent,
        )
        .unwrap();
    let items = store.thread_items(&record.agents[0].id).unwrap();
    let statuses: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            ThreadItem::Message(message) => Some(message.delivery_status),
            _ => None,
        })
        .collect();
    assert_eq!(
        statuses,
        vec![
            Some(MessageDeliveryStatus::Sent),
            Some(MessageDeliveryStatus::Queued)
        ]
    );
    assert!(items[0].latest_sequence() == bumped);

    drop(store);
    let reopened = Store::new(dir.path()).unwrap();
    let loaded = reopened.load_all_issues().unwrap().remove(0);
    let ThreadItem::Message(message) = &loaded.issue.agents[0].thread.items[0] else {
        panic!()
    };
    assert_eq!(message.delivery_status, Some(MessageDeliveryStatus::Sent));
}

#[test]
fn recovery_marks_only_submitted_legacy_messages_uncertain() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut record = plan_record("issue-1");
    let thread = &mut record.agents[0].thread;
    thread.post_user("submitted", None, NOW);
    thread.post_user("sent", None, NOW);
    thread.post_user("seen", None, NOW);
    store.save_issue_plan(&record).unwrap();
    store
        .set_legacy_delivery_status(&record.agents[0].id, 1, 1, MessageDeliveryStatus::Submitted)
        .unwrap();
    store
        .set_legacy_delivery_status(&record.agents[0].id, 2, 2, MessageDeliveryStatus::Sent)
        .unwrap();
    store
        .set_legacy_delivery_status(&record.agents[0].id, 3, 3, MessageDeliveryStatus::Seen)
        .unwrap();

    store.recover_operations().unwrap();
    let statuses: Vec<_> = store
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
            Some(MessageDeliveryStatus::Sent),
            Some(MessageDeliveryStatus::Seen),
        ]
    );
}

#[test]
fn recovery_repairs_uncertain_receipts_and_preserves_definitive_failure() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut record = plan_record("issue-1");

    record.agents[0].thread.post_user("repair me", None, NOW);
    let repair_messages = record.agents[0]
        .thread
        .bind_operation_messages("repair-op", 0, 1);
    let mut repair = queued_operation("repair-op", 1);
    repair.delivery.as_mut().unwrap().payload = Some(OperationPayload {
        start_sequence: 1,
        end_sequence: 1,
        messages: repair_messages,
        prior_context: String::new(),
    });
    store
        .accept_thread_post("issue-1", &record.agents, &repair)
        .unwrap();
    assert!(store
        .transition_operation(
            "repair-op",
            OperationStatus::Queued,
            OperationStatus::Uncertain,
            Some("crashed before message projection"),
        )
        .unwrap());

    record.agents[0].thread.post_user("failed", None, NOW);
    let failed_messages = record.agents[0]
        .thread
        .bind_operation_messages("failed-op", 1, 2);
    let mut failed = queued_operation("failed-op", 2);
    failed.delivery.as_mut().unwrap().payload = Some(OperationPayload {
        start_sequence: 2,
        end_sequence: 2,
        messages: failed_messages,
        prior_context: String::new(),
    });
    store
        .accept_thread_post("issue-1", &record.agents, &failed)
        .unwrap();
    store
        .set_operation_delivery_status(
            &record.agents[0].id,
            "failed-op",
            2,
            2,
            MessageDeliveryStatus::Failed,
        )
        .unwrap();
    assert!(store
        .transition_operation(
            "failed-op",
            OperationStatus::Queued,
            OperationStatus::Delivered,
            Some("provider declined delivery"),
        )
        .unwrap());

    store.recover_operations().unwrap();
    let statuses: Vec<_> = store
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
            Some(MessageDeliveryStatus::Failed),
        ]
    );
}

#[test]
fn recovery_ignores_an_orphaned_receipt_without_deleting_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut record = plan_record("issue-1");
    record.agents[0].thread.post_user("managed", None, NOW);
    let messages = record.agents[0]
        .thread
        .bind_operation_messages("orphan-op", 0, 1);
    let mut receipt = queued_operation("orphan-op", 1);
    receipt.delivery.as_mut().unwrap().payload = Some(OperationPayload {
        start_sequence: 1,
        end_sequence: 1,
        messages,
        prior_context: String::new(),
    });
    store
        .accept_thread_post("issue-1", &record.agents, &receipt)
        .unwrap();
    assert!(store
        .transition_operation(
            "orphan-op",
            OperationStatus::Queued,
            OperationStatus::Delivered,
            None,
        )
        .unwrap());
    store
        .connection()
        .execute("DELETE FROM agents WHERE id = ?1", [&record.agents[0].id])
        .unwrap();

    store.recover_operations().unwrap();
    assert_eq!(
        store.operation("orphan-op").unwrap().unwrap().status,
        OperationStatus::Delivered,
        "recovery leaves the historical receipt intact"
    );
}

#[test]
fn v5_database_gains_operation_receipts_without_touching_conversations() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut record = plan_record("issue-1");
    record.agents[0].thread.post_user("keep me", None, NOW);
    store.save_issue_plan(&record).unwrap();
    store.pretend_to_be_v5();
    drop(store);

    let migrated = Store::new(dir.path()).unwrap();
    assert_eq!(
        migrated.thread_items(&record.agents[0].id).unwrap().len(),
        1
    );
    let receipt = queued_operation("after-upgrade", 1);
    migrated
        .accept_thread_post("issue-1", &record.agents, &receipt)
        .unwrap();
    assert_eq!(migrated.operation("after-upgrade").unwrap(), Some(receipt));
}

/// An in-place mutation — marking a message seen — reaches the store even
/// though the item's creation sequence has not moved. The cursor column is
/// what makes that visible, so a save that compared creation sequences
/// alone would silently drop it.
#[test]
fn a_message_mutated_in_place_is_written_back() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    record.agents[0].thread.post_user("read me", None, NOW);
    store.save_run(&record).expect("the run saves");

    let read = record.agents[0].thread.read_unread(NOW);
    assert!(!read.is_empty(), "there is an unread message to mark seen");
    store.save_run(&record).expect("the mutation saves");

    let reloaded = reload_run(&store, "run-1");
    assert_eq!(
        reloaded.agents[0].thread.items, record.agents[0].thread.items,
        "the in-place mutation reached the store"
    );
}

/// The whole point of paging: a client can walk a long conversation
/// backward a page at a time and see every item exactly once, in order,
/// without ever asking for the conversation whole.
#[test]
fn paging_backward_reaches_every_item_exactly_once_and_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let (store, agent_id) = store_with_conversation(&dir.path().join("tasks"), 60);

    let mut walked: Vec<u64> = Vec::new();
    let mut before = None;
    loop {
        let page = store
            .thread_page(&agent_id, before, 17)
            .expect("a page reads");
        if page.is_empty() {
            break;
        }
        let page_sequences = sequences(&page);
        assert!(
            page_sequences.windows(2).all(|pair| pair[0] < pair[1]),
            "a page came back out of order: {page_sequences:?}"
        );
        before = page_sequences.first().copied();
        // Pages arrive newest-first, so the walk builds the conversation
        // from the front.
        walked.splice(0..0, page_sequences);
    }

    let every_sequence: Vec<u64> = (1..=60).collect();
    assert_eq!(
        walked, every_sequence,
        "the backward walk missed, repeated or reordered items"
    );
}

/// A backup is one self-contained file taken from a consistent read, so a
/// copy made while the daemon is writing is a database rather than a torn
/// one — the thing a file-at-a-time tool can no longer do for itself.
#[test]
fn a_backup_is_a_whole_store_and_never_silently_replaces_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    record.agents[0].thread.post_user("keep me", None, NOW);
    store.save_run(&record).expect("the run saves");

    let backup = dir.path().join("backups").join("store.db");
    store.backup_to(&backup).expect("the backup is written");
    assert!(backup.is_file(), "the backup created its parent directory");

    // The copy stands on its own: opened as a store, it holds the work.
    let restored = Store::new(dir.path().join("restored")).expect("store opens");
    drop(restored);
    std::fs::copy(&backup, dir.path().join("restored").join("build.db")).unwrap();
    let restored = Store::new(dir.path().join("restored")).expect("the backup opens");
    let runs = restored.load_all_runs().expect("runs load");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].agents[0].thread.items.len(), 1);

    // Overwriting is refused: a backup that replaced the previous one
    // silently is one that can be lost twice.
    let refused = store.backup_to(&backup).expect_err("the second is refused");
    assert!(refused.to_string().contains("already exists"), "{refused}");
}

/// A reply is forwarded once. Reading the debts a conversation owes clears
/// them, so the next terminal message that conversation produces answers
/// nobody — the receipt itself stays exactly where it was.
#[test]
fn the_answer_a_conversation_owes_is_taken_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let record = plan_record("issue-1");
    let conversation_id = record.agents[0].id.clone();
    let requester = crate::operation::OperationRequester {
        agent_id: "project-01H".to_string(),
        entity_id: "run-project".to_string(),
        conversation_id: "conversation-project".to_string(),
    };
    let mut asked = queued_operation("op-1", 1);
    asked.requested_by = Some(requester.clone());
    let humans = queued_operation("op-2", 2);
    store
        .accept_thread_post("issue-1", &record.agents, &asked)
        .unwrap();
    store
        .accept_thread_post("issue-1", &record.agents, &humans)
        .unwrap();

    let owed = store.take_operation_requesters(&conversation_id).unwrap();
    assert_eq!(
        owed,
        vec![requester],
        "the human's own operation owes nobody"
    );
    assert!(
        store
            .take_operation_requesters(&conversation_id)
            .unwrap()
            .is_empty(),
        "a debt is paid once"
    );
    assert!(store
        .operation("op-1")
        .unwrap()
        .expect("the receipt stays")
        .requested_by
        .is_none());
    assert!(store
        .take_operation_requesters("conversation-nobody")
        .unwrap()
        .is_empty());
}
