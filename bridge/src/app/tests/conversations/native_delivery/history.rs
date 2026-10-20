use super::*;

struct HistoryReceipt {
    _directory: tempfile::TempDir,
    state: AppState,
    owner: String,
    agent: String,
    sequence: u64,
    cursor: u64,
}

impl HistoryReceipt {
    fn new(managed: bool) -> Self {
        let (directory, repo) = init_repo();
        let mut state = qa_state(&repo, directory.path());
        let owner = adopted_run(&mut state, &repo, directory.path(), "historical-receipt");
        let agent = primary_agent_id(&state, &owner);
        if managed {
            post_operation(
                &mut state,
                &owner,
                &agent,
                "historical-operation",
                "old message",
            );
        } else {
            state
                .edit_agent_conversation(&owner, &agent, |thread, _| {
                    thread.post_user("old message", None, "2026-10-19T12:00:00Z");
                    Ok(())
                })
                .unwrap();
        }
        let sequence = message_by_body(&state, &owner, &agent, "old message").sequence;
        state
            .edit_agent_conversation(&owner, &agent, |thread, _| {
                for _ in 0..300 {
                    thread.push_event(
                        crate::thread::ThreadEventKind::ToolUse,
                        None,
                        None,
                        None,
                        "2026-10-19T12:01:00Z",
                    );
                }
                let cursor = thread.last_sequence();
                let total = thread.items.len();
                thread.adopt_stored_tail(
                    thread.items[total - 2..].to_vec(),
                    (total - 2) as u64,
                    cursor,
                );
                Ok(())
            })
            .unwrap();
        let cursor = state
            .agent_conversation(&owner, Some(&agent))
            .unwrap()
            .last_sequence();
        state.initialize_snapshot_projection();
        Self {
            _directory: directory,
            state,
            owner,
            agent,
            sequence,
            cursor,
        }
    }

    fn assert_status(&self, status: &str) {
        let snapshot = self.state.read_view.load();
        let page = snapshot
            .history(&self.agent)
            .unwrap()
            .render(&json!({"thread_after_sequence":self.cursor}));
        let rows = page["items"].as_array().unwrap();
        assert_eq!(
            rows.len(),
            1,
            "the old message must advance the immutable cursor"
        );
        assert_eq!(rows[0]["data"]["sequence"], self.sequence);
        assert_eq!(rows[0]["data"]["delivery_status"], status);
        assert_eq!(rows[0]["data"]["seen_at"].is_string(), status == "seen");
        assert!(page["thread_last_sequence"].as_u64().unwrap() > self.cursor);
    }
}

#[test]
fn native_historical_receipt_updates_warmed_snapshot_below_resident_tail() {
    let mut fixture = HistoryReceipt::new(true);
    fixture
        .state
        .record_native_operation_seen(&fixture.owner, &fixture.agent, "historical-operation")
        .unwrap();
    fixture.assert_status("seen");
}

#[test]
fn legacy_historical_receipt_updates_warmed_snapshot_below_resident_tail() {
    let mut fixture = HistoryReceipt::new(false);
    let token = format!("@legacy/{}", fixture.sequence);
    fixture
        .state
        .record_legacy_native_receipt(
            &fixture.owner,
            &fixture.agent,
            &token,
            MessageDeliveryStatus::Seen,
        )
        .unwrap();
    fixture.assert_status("seen");
}

#[test]
fn committed_historical_receipt_survives_a_later_roster_save_failure() {
    let mut fixture = HistoryReceipt::new(false);
    let connection =
        rusqlite::Connection::open(fixture._directory.path().join("store/build.db")).unwrap();
    connection.execute_batch("CREATE TABLE receipt_updates (count INTEGER); INSERT INTO receipt_updates VALUES (0);
        CREATE TRIGGER fail_second_receipt_update BEFORE UPDATE ON agents BEGIN
          UPDATE receipt_updates SET count = count + 1;
          SELECT CASE WHEN (SELECT count FROM receipt_updates) > 1 THEN RAISE(FAIL, 'later roster save fails') END;
        END;").unwrap();
    let token = format!("@legacy/{}", fixture.sequence);
    assert!(fixture
        .state
        .record_legacy_native_receipt(
            &fixture.owner,
            &fixture.agent,
            &token,
            MessageDeliveryStatus::Seen
        )
        .is_err());
    fixture.assert_status("seen");
    let committed_sequence = fixture
        .state
        .read_view
        .load()
        .history(&fixture.agent)
        .unwrap()
        .render(&json!({"thread_after_sequence":fixture.cursor}))["thread_last_sequence"]
        .as_u64()
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER fail_second_receipt_update")
        .unwrap();
    fixture
        .state
        .edit_agent_conversation(&fixture.owner, &fixture.agent, |thread, _| {
            thread.post_user("after committed receipt", None, "2026-10-19T12:02:00Z");
            Ok(())
        })
        .unwrap();
    let appended = message_by_body(
        &fixture.state,
        &fixture.owner,
        &fixture.agent,
        "after committed receipt",
    );
    assert!(
        appended.sequence > committed_sequence,
        "a restored owner reused a separately committed SQL mutation counter"
    );
}

#[test]
fn native_historical_handoff_updates_warmed_snapshot_below_resident_tail() {
    let mut fixture = HistoryReceipt::new(true);
    fixture
        .state
        .record_operation_delivery_status("historical-operation", MessageDeliveryStatus::Sent)
        .unwrap();
    fixture.assert_status("sent");
}

#[test]
fn legacy_historical_handoff_updates_warmed_snapshot_below_resident_tail() {
    let mut fixture = HistoryReceipt::new(false);
    let payload = fixture
        .state
        .legacy_delivery_payload(&fixture.owner, &fixture.agent)
        .unwrap()
        .unwrap();
    fixture
        .state
        .record_legacy_delivery_status(
            &fixture.owner,
            &fixture.agent,
            &payload,
            MessageDeliveryStatus::Sent,
        )
        .unwrap();
    fixture.assert_status("sent");
}
