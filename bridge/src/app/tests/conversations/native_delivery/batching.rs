use super::*;
use crate::app::conversations::FrozenLegacyDelivery;
use crate::timing::FrameClock;

struct LegacyBatch {
    directory: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    clock: Arc<FrameClock>,
    owner: String,
    agent: String,
    frozen: FrozenLegacyDelivery,
    admitted_thread: String,
    admitted_generation: u64,
}

impl LegacyBatch {
    fn new() -> Self {
        let (directory, repo) = init_repo();
        let mut app = qa_state(&repo, directory.path());
        let owner = adopted_run(&mut app, &repo, directory.path(), "legacy-batch");
        let agent = primary_agent_id(&app, &owner);
        app.edit_agent_conversation(&owner, &agent, |thread, _| {
            for index in 0..8 {
                thread.post_user(format!("selected {index}"), None, "2026-10-20T12:00:00Z");
                if index == 3 {
                    thread.post_user("unlisted middle", None, "2026-10-20T12:00:00Z");
                    thread.post_user("managed middle", None, "2026-10-20T12:00:00Z");
                    let sequence = thread.last_sequence();
                    thread.bind_operation_messages("managed-middle", sequence - 1, sequence);
                }
            }
            Ok(())
        })
        .unwrap();
        let mut frozen = app.freeze_legacy_delivery(&owner, &agent).unwrap().unwrap();
        frozen
            .payload
            .messages
            .retain(|message| message.body.starts_with("selected "));
        assert_eq!(frozen.payload.messages.len(), 8);
        let admitted_thread = frozen.thread_id.clone();
        let admitted_generation = frozen.generation_revision;
        app.initialize_snapshot_projection();
        let clock = Arc::clone(&app.frame_clock);
        Self {
            directory,
            state: Arc::new(Mutex::new(app)),
            clock,
            owner,
            agent,
            frozen,
            admitted_thread,
            admitted_generation,
        }
    }

    fn record(&self, status: MessageDeliveryStatus) -> (Result<(), String>, Value) {
        let samples = self.clock.capture_phases();
        let timer = self.clock.frame("agent.deliver");
        let result = timer.lock(&self.state).record_legacy_delivery_status(
            &self.owner,
            &self.agent,
            &self.frozen,
            status,
        );
        drop(timer);
        (result, samples.snapshot())
    }

    fn messages(&self) -> Vec<ThreadMessage> {
        self.state
            .lock()
            .unwrap()
            .store
            .as_ref()
            .unwrap()
            .thread_items(&self.agent)
            .unwrap()
            .into_iter()
            .filter_map(|item| match item {
                ThreadItem::Message(message) => Some(message),
                _ => None,
            })
            .collect()
    }

    fn assert_selected_status(&self, status: MessageDeliveryStatus) {
        let messages = self.messages();
        for expected in &self.frozen.payload.messages {
            let actual = messages
                .iter()
                .find(|message| message.id == expected.id)
                .unwrap();
            assert_eq!(actual.delivery_status, Some(status), "{}", actual.body);
        }
        assert_eq!(
            messages
                .iter()
                .find(|message| message.body == "unlisted middle")
                .unwrap()
                .delivery_status,
            None
        );
        assert_eq!(
            messages
                .iter()
                .find(|message| message.body == "managed middle")
                .unwrap()
                .delivery_status,
            Some(MessageDeliveryStatus::Queued)
        );
    }

    fn make_historical(&self) -> u64 {
        let mut app = self.state.lock().unwrap();
        app.edit_agent_conversation(&self.owner, &self.agent, |thread, _| {
            for _ in 0..300 {
                thread.push_event(
                    crate::thread::ThreadEventKind::ToolUse,
                    None,
                    None,
                    None,
                    "2026-10-20T12:01:00Z",
                );
            }
            let cursor = thread.last_sequence();
            let count = thread.items.len();
            thread.adopt_stored_tail(
                thread.items[count - 2..].to_vec(),
                (count - 2) as u64,
                cursor,
            );
            Ok(cursor)
        })
        .unwrap()
    }

    fn assert_historical_delta(&self, cursor: u64, status: &str) -> u64 {
        let app = self.state.lock().unwrap();
        let snapshot = app.read_view.load();
        let page = snapshot
            .history(&self.agent)
            .unwrap()
            .render(&json!({"thread_after_sequence":cursor}));
        let items = page["items"].as_array().unwrap();
        assert_eq!(items.len(), self.frozen.payload.messages.len(), "{page}");
        for expected in &self.frozen.payload.messages {
            let row = items
                .iter()
                .find(|item| item["data"]["id"] == expected.id)
                .unwrap();
            assert_eq!(row["data"]["delivery_status"], status);
        }
        assert_eq!(
            app.agent_conversation(&self.owner, Some(&self.agent))
                .unwrap()
                .items
                .len(),
            2
        );
        page["thread_last_sequence"].as_u64().unwrap()
    }
}

#[test]
fn legacy_delivery_batch_finishes_once_per_status_and_preserves_sparse_selection() {
    let fixture = LegacyBatch::new();
    let mut captured = Vec::new();
    for status in [
        MessageDeliveryStatus::Submitted,
        MessageDeliveryStatus::Sent,
    ] {
        let (result, samples) = fixture.record(status);
        result.unwrap();
        fixture.assert_selected_status(status);
        captured.push(samples);
    }
    let before = fixture.messages();
    fixture.record(MessageDeliveryStatus::Sent).0.unwrap();
    fixture.record(MessageDeliveryStatus::Uncertain).0.unwrap();
    assert_eq!(
        fixture.messages(),
        before,
        "duplicates and delayed uncertainty must not mint new mutation counters"
    );
    for samples in captured {
        let method = &samples["agent.deliver"];
        assert_eq!(method["entered.finisher.save"]["count"], 1, "{samples}");
        assert_eq!(
            method["entered.publication.global"]["count"], 1,
            "{samples}"
        );
        assert_eq!(
            method["entered.sql.write"]["count"], 2,
            "one status transaction and one owner save: {samples}"
        );
    }
}

#[test]
fn legacy_delivery_batch_updates_all_selected_messages_below_the_resident_tail() {
    let fixture = LegacyBatch::new();
    let cursor = fixture.make_historical();
    fixture.record(MessageDeliveryStatus::Sent).0.unwrap();
    fixture.assert_selected_status(MessageDeliveryStatus::Sent);
    assert!(fixture.assert_historical_delta(cursor, "sent") > cursor);
}

#[test]
fn legacy_delivery_batch_rolls_back_all_selected_rows_when_one_status_write_fails() {
    let fixture = LegacyBatch::new();
    let before = fixture.messages();
    let connection =
        rusqlite::Connection::open(fixture.directory.path().join("store/build.db")).unwrap();
    let second = fixture.frozen.payload.messages[1].sequence;
    connection.execute_batch(&format!("CREATE TRIGGER refuse_second_batch_message BEFORE UPDATE ON thread_items WHEN NEW.sequence = {second} AND json_extract(NEW.item, '$.data.delivery_status') = 'submitted' BEGIN SELECT RAISE(ABORT, 'injected batch failure'); END;")).unwrap();
    assert!(fixture.record(MessageDeliveryStatus::Submitted).0.is_err());
    assert_eq!(
        fixture.messages(),
        before,
        "a failed exact batch must not leave its first message committed"
    );
}

#[test]
fn legacy_delivery_batch_commit_survives_a_later_owner_save_failure() {
    let fixture = LegacyBatch::new();
    let cursor = fixture.make_historical();
    let connection =
        rusqlite::Connection::open(fixture.directory.path().join("store/build.db")).unwrap();
    connection.execute_batch("CREATE TABLE batch_agent_updates (count INTEGER); INSERT INTO batch_agent_updates VALUES (0); CREATE TRIGGER refuse_later_batch_owner_save BEFORE UPDATE ON agents BEGIN UPDATE batch_agent_updates SET count = count + 1; SELECT CASE WHEN (SELECT count FROM batch_agent_updates) > 1 THEN RAISE(FAIL, 'later owner save fails') END; END;").unwrap();
    assert!(fixture.record(MessageDeliveryStatus::Sent).0.is_err());
    fixture.assert_selected_status(MessageDeliveryStatus::Sent);
    let committed = fixture.assert_historical_delta(cursor, "sent");
    connection
        .execute_batch("DROP TRIGGER refuse_later_batch_owner_save")
        .unwrap();
    let mut app = fixture.state.lock().unwrap();
    let sequence = app
        .edit_agent_conversation(&fixture.owner, &fixture.agent, |thread, _| {
            thread.post_user(
                "after failed batch owner save",
                None,
                "2026-10-20T12:02:00Z",
            );
            Ok(thread.last_sequence())
        })
        .unwrap();
    assert!(
        sequence > committed,
        "restoring an owner reused a committed batch counter"
    );
}

#[test]
fn frozen_legacy_delivery_batch_cannot_change_messages_from_a_reset_generation() {
    let fixture = LegacyBatch::new();
    let params = {
        let app = fixture.state.lock().unwrap();
        let thread = app
            .agent_conversation(&fixture.owner, Some(&fixture.agent))
            .unwrap();
        json!({"project_id":app.projects.project_id_of(&fixture.owner).unwrap(), "entity_id":fixture.owner, "agent_id":fixture.agent, "conversation_id":fixture.agent, "expected_thread_id":thread.id})
    };
    let handler = AppState::handler(Arc::clone(&fixture.state));
    let reset = handler.call(
        SessionSender::detached("legacy-batch-reset"),
        req("conversation.reset", params),
    );
    assert_eq!(reset["ok"], true, "{reset}");
    assert_ne!(reset["result"]["thread_id"], fixture.admitted_thread);
    assert_eq!(
        reset["result"]["thread_generation_revision"],
        fixture.admitted_generation + 1
    );
    fixture
        .state
        .lock()
        .unwrap()
        .edit_agent_conversation(&fixture.owner, &fixture.agent, |thread, _| {
            for sequence in 0..=fixture.frozen.payload.end_sequence {
                thread.post_user(
                    format!("replacement {sequence}"),
                    None,
                    "2026-10-20T12:03:00Z",
                );
            }
            Ok(())
        })
        .unwrap();
    let before = fixture.messages();
    assert!(before
        .iter()
        .any(|message| message.sequence == fixture.frozen.payload.messages[0].sequence));
    assert!(
        before.iter().any(|message| {
            message.sequence == fixture.frozen.payload.messages[0].sequence
                && message.id == fixture.frozen.payload.messages[0].id
        }),
        "message IDs also repeat across reset generations"
    );
    let (result, samples) = fixture.record(MessageDeliveryStatus::Sent);
    result.unwrap();
    assert!(samples["agent.deliver"].get("entered.sql.write").is_none());
    assert!(samples["agent.deliver"]
        .get("entered.finisher.save")
        .is_none());
    assert_eq!(
        fixture.messages(),
        before,
        "the generation frozen with admission must fence reused IDs and sequences"
    );
}
