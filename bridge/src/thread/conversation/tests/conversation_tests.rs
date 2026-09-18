use super::*;

#[test]
fn marking_the_posted_message_complete_emits_an_incremental_update() {
    let mut thread = Thread::new("run-1");
    let message_id = thread.post_agent_offering(
        "Shipped it.",
        None,
        Vec::new(),
        Vec::new(),
        "2026-09-17T12:00:00Z",
        false,
    );
    let original_sequence = thread.items[0].latest_sequence();

    assert!(thread.mark_agent_message_outcome(
        &message_id,
        MessageOutcome::Completed,
        "Shipped it.",
        None,
    ));
    let ThreadItem::Message(message) = &thread.items[0] else {
        panic!("the outcome stays on the posted message");
    };
    assert_eq!(thread.items.len(), 1);
    assert_eq!(message.outcome, Some(MessageOutcome::Completed));
    assert!(message.updated_sequence > original_sequence);
}

#[test]
fn compaction_completion_updates_the_active_history_line() {
    let mut thread = Thread::new("plan-1");
    let sequence = thread.push_event(
        ThreadEventKind::Compaction,
        Some("Compacting".to_string()),
        None,
        None,
        "2026-09-17T12:00:00Z",
    );

    assert!(thread.resolve_compaction(None));
    assert_eq!(thread.items.len(), 1);
    let ThreadItem::Event(event) = &thread.items[0] else {
        panic!("compaction is an event");
    };
    assert_eq!(event.sequence, sequence);
    assert!(event.updated_sequence > sequence);
    assert_eq!(event.summary.as_deref(), Some("Compacted"));
    assert!(!thread.resolve_compaction(None));
    assert_eq!(thread.items.len(), 1);
}

#[test]
fn compaction_completion_does_not_close_an_older_sessions_row() {
    let mut thread = Thread::new("plan-1");
    thread.push_event(
        ThreadEventKind::Compaction,
        Some("Compacting".to_string()),
        Some("old-session".to_string()),
        None,
        "2026-09-17T12:00:00Z",
    );
    assert!(!thread.resolve_compaction(Some("new-session")));
}

fn thread_with_conversation() -> Thread {
    let mut thread = Thread::new("plan-1");
    thread.post_user("please rename the helper", None, "2026-07-24T12:00:00Z");
    thread.post_agent("Which name do you prefer?", None, "2026-07-24T12:01:00Z");
    thread.push_event(
        ThreadEventKind::Done,
        Some("Agent reported done".to_string()),
        None,
        None,
        "2026-07-24T12:02:00Z",
    );
    thread
}

/// The dismissal line ignores a hand-off, and a hand-off ignores it from
/// both post paths: the one that signs as it posts and the one that signs the
/// message it just pushed. The general message line counts them all.
#[test]
fn the_own_message_line_skips_what_another_agent_signed() {
    let mut thread = Thread::new("run-1");
    thread.post_user("please investigate", None, "2026-09-08T10:00:00Z");
    let asked = thread.last_own_message_sequence();
    assert_eq!(asked, thread.last_message_sequence());

    thread.post_user_from_agent(
        "take the retry path",
        AgentIdentity::new("project-1".to_string()),
        "2026-09-08T10:01:00Z",
    );
    assert_eq!(
        thread.last_own_message_sequence(),
        asked,
        "signed as posted"
    );
    assert!(
        thread.last_message_sequence() > asked,
        "but still a message"
    );

    thread.post_user("actually, hold on", None, "2026-09-08T10:02:00Z");
    thread.wear_sender(AgentIdentity::new("project-1".to_string()));
    assert_eq!(thread.last_own_message_sequence(), asked, "signed after");

    thread.post_agent("on it", None, "2026-09-08T10:03:00Z");
    assert_eq!(
        thread.last_own_message_sequence(),
        thread.last_message_sequence(),
        "the agent answering in its own conversation is the row speaking"
    );
}

#[test]
fn conversation_summary_tracks_speech_and_only_an_in_flight_stop() {
    let mut thread = Thread::new("plan-1");
    thread.post_user("please investigate", None, "2026-09-08T10:00:00Z");
    let user_sequence = thread.last_message_sequence();
    assert_eq!(
        thread.conversation_activity_at(),
        Some("2026-09-08T10:00:00Z")
    );

    thread.read_unread("2026-09-08T10:01:00Z");
    assert_eq!(thread.last_message_sequence(), user_sequence);
    assert_eq!(
        thread.conversation_activity_at(),
        Some("2026-09-08T10:00:00Z")
    );

    thread.post_agent_progress("still working", None, "2026-09-08T10:02:00Z");
    let progress_sequence = thread.last_message_sequence();
    assert!(progress_sequence > user_sequence);
    assert_eq!(
        thread.conversation_activity_at(),
        Some("2026-09-08T10:02:00Z")
    );

    thread.push_event(
        ThreadEventKind::ToolUse,
        None,
        None,
        None,
        "2026-09-08T10:03:00Z",
    );
    assert_eq!(thread.last_message_sequence(), progress_sequence);
    assert_eq!(
        thread.conversation_activity_at(),
        Some("2026-09-08T10:02:00Z")
    );

    thread.push_event(
        ThreadEventKind::Blocked,
        None,
        None,
        None,
        "2026-09-08T10:04:00Z",
    );
    assert_eq!(
        thread.conversation_activity_at(),
        Some("2026-09-08T10:04:00Z")
    );

    thread.push_event(
        ThreadEventKind::RunFailed,
        None,
        None,
        None,
        "2026-09-08T10:05:00Z",
    );
    assert_eq!(
        thread.conversation_activity_at(),
        Some("2026-09-08T10:04:00Z")
    );
    thread.normalize("plan-1");
    thread.normalize("plan-1");
    assert_eq!(
        thread.conversation_activity_at(),
        Some("2026-09-08T10:04:00Z")
    );
}

#[test]
fn digest_value_is_bounded_and_omits_bodies() {
    let mut thread = thread_with_conversation();
    thread.post_user("a second unread ask", None, "2026-07-24T12:03:00Z");
    let digest = thread.digest_value();

    assert_eq!(digest["id"], "thread:plan-1");
    assert_eq!(digest["agent"]["id"], "agent:plan-1");
    assert_eq!(digest["item_count"], 4);
    assert_eq!(digest["last_sequence"], 4);
    assert_eq!(digest["last_event"]["event"], "done");
    assert_eq!(digest["last_event"]["created_at"], "2026-07-24T12:02:00Z");
    // The bounded contract: no item array, no message bodies anywhere, and
    // no counters nothing consumes (unseen_user_messages was dead payload).
    assert!(digest.get("unseen_user_messages").is_none(), "{digest:?}");
    assert!(digest.get("items").is_none(), "{digest:?}");
    let serialized = digest.to_string();
    assert!(!serialized.contains("please rename the helper"));
    assert!(!serialized.contains("a second unread ask"));
}

#[test]
fn digest_value_of_an_empty_thread_has_zero_counters_and_no_event() {
    let digest = Thread::new("plan-empty").digest_value();
    assert_eq!(digest["item_count"], 0);
    assert_eq!(digest["last_sequence"], 0);
    assert!(digest["last_event"].is_null(), "{digest:?}");
}

#[test]
fn wire_value_after_ships_only_newer_items_with_totals() {
    let thread = thread_with_conversation();
    let delta = thread.wire_value_after(1);

    let items = delta["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0]["data"]["sequence"], 2);
    assert_eq!(items[1]["data"]["sequence"], 3);
    assert_eq!(delta["thread_total"], 3);
    assert_eq!(delta["thread_last_sequence"], 3);
    // The small bounded companions still ship in full.
    assert!(delta["sessions"].is_array());
    assert!(delta["revisions"].is_array());
}

#[test]
fn wire_value_after_past_the_end_is_an_empty_delta_not_an_error() {
    let thread = thread_with_conversation();
    let delta = thread.wire_value_after(9_999);
    assert_eq!(delta["items"].as_array().unwrap().len(), 0);
    assert_eq!(delta["thread_total"], 3);
    assert_eq!(delta["thread_last_sequence"], 3);
}

#[test]
fn wire_value_after_reships_a_message_marked_seen_after_the_cursor() {
    let mut thread = Thread::new("plan-1");
    thread.post_user("please rename the helper", None, "2026-07-24T12:00:00Z");
    let cursor = thread.last_sequence();
    thread.read_unread("2026-07-24T12:05:00Z");

    let delta = thread.wire_value_after(cursor);
    let items = delta["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["data"]["seen_at"], "2026-07-24T12:05:00Z");
    // The mutation advances the high-water mark so the client's next
    // cursor moves past it instead of re-requesting the item forever.
    let bumped = delta["thread_last_sequence"].as_u64().unwrap();
    assert!(bumped > cursor, "{delta:?}");
    let drained = thread.wire_value_after(bumped);
    assert_eq!(drained["items"].as_array().unwrap().len(), 0, "{drained:?}");
}

#[test]
fn managed_message_delivery_status_is_scoped_and_seen_is_terminal() {
    let mut thread = Thread::new("plan-1");
    thread.post_user("first", None, "2026-07-24T12:00:00Z");
    thread.bind_operation_messages("op-1", 0, 1);
    thread.post_user("second", None, "2026-07-24T12:01:00Z");
    thread.bind_operation_messages("op-2", 1, 2);

    let queued = thread.wire_value();
    assert_eq!(queued["items"][0]["data"]["delivery_status"], "queued");
    let before = thread.last_sequence();
    thread.set_operation_delivery_status("op-1", 1, 1, MessageDeliveryStatus::Sent);
    let sent = thread.last_sequence();
    assert!(sent > before);
    thread.set_operation_delivery_status("op-1", 1, 1, MessageDeliveryStatus::Sent);
    assert_eq!(
        thread.last_sequence(),
        sent,
        "an equal status does not bump"
    );
    thread.read_operation_messages("op-1", 1, 1, "2026-07-24T12:02:00Z");
    thread.set_operation_delivery_status("op-1", 1, 1, MessageDeliveryStatus::Queued);

    let messages: Vec<_> = thread
        .items
        .iter()
        .filter_map(|item| match item {
            ThreadItem::Message(message) => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(
        messages[0].delivery_status,
        Some(MessageDeliveryStatus::Seen)
    );
    assert_eq!(
        messages[1].delivery_status,
        Some(MessageDeliveryStatus::Queued)
    );
}

#[test]
fn legacy_message_without_delivery_status_still_loads() {
    let mut thread = Thread::new("plan-1");
    thread.post_user("legacy", None, "2026-07-24T12:00:00Z");
    let mut value = serde_json::to_value(&thread).unwrap();
    value["items"][0]["data"]
        .as_object_mut()
        .unwrap()
        .remove("delivery_status");
    let loaded: Thread = serde_json::from_value(value).unwrap();
    let ThreadItem::Message(message) = &loaded.items[0] else {
        panic!()
    };
    assert_eq!(message.delivery_status, None);
}

#[test]
fn legacy_mailbox_read_marks_a_tracked_message_seen() {
    let mut thread = Thread::new("plan-1");
    thread.post_user("legacy delivery", None, "2026-07-24T12:00:00Z");
    thread.set_legacy_delivery_status(1, 1, MessageDeliveryStatus::Sent);
    let read = thread.read_unread("2026-07-24T12:01:00Z");
    assert_eq!(read[0].delivery_status, Some(MessageDeliveryStatus::Seen));
    assert_eq!(read[0].seen_at.as_deref(), Some("2026-07-24T12:01:00Z"));
}

#[test]
fn wire_value_after_reships_a_message_resolved_by_a_later_revision() {
    let mut thread = Thread::new("plan-1");
    let anchor = MessageAnchor {
        artifact: ArtifactKind::Plan,
        revision_id: None,
        path: None,
        side: None,
        line_start: None,
        line_end: None,
        heading_path: Vec::new(),
        snippet: "old wording".to_string(),
    };
    thread.post_user("tighten this", Some(anchor), "2026-07-24T12:00:00Z");
    let cursor = thread.last_sequence();
    let revision = thread.add_revision(ArtifactKind::Plan, "rewritten", "2026-07-24T12:06:00Z");

    let delta = thread.wire_value_after(cursor);
    let items = delta["items"].as_array().unwrap();
    assert!(
        items
            .iter()
            .any(|item| item["data"]["resolved_by_revision"] == json!(revision.id)),
        "{items:?}"
    );
    let bumped = delta["thread_last_sequence"].as_u64().unwrap();
    assert!(bumped > cursor, "{delta:?}");
    let drained = thread.wire_value_after(bumped);
    assert_eq!(drained["items"].as_array().unwrap().len(), 0, "{drained:?}");
}

/// A conversation holding one open tool call, and the sequence that call
/// was minted at — the handle the pump keeps and the answer comes back on.
fn thread_with_an_open_tool_call() -> (Thread, u64) {
    let mut thread = Thread::new("run-1");
    let sequence = thread.push_event(
        ThreadEventKind::ToolUse,
        Some("Read bridge/src/app.rs".to_string()),
        None,
        None,
        "2026-08-30T09:00:00Z",
    );
    (thread, sequence)
}

fn event_at(thread: &Thread, sequence: u64) -> &ThreadEvent {
    thread
        .items
        .iter()
        .find_map(|item| match item {
            ThreadItem::Event(event) if event.sequence == sequence => Some(event),
            _ => None,
        })
        .expect("the row the call minted")
}

/// The call and its answer are ONE row: the answer lands on the row the
/// call minted, as a suffix line and an outcome, and mints nothing beside
/// it.
#[test]
fn a_tool_calls_answer_updates_the_row_the_call_minted() {
    let (mut thread, call) = thread_with_an_open_tool_call();
    assert_eq!(thread.items.len(), 1);

    assert!(thread.resolve_tool_call(call, ToolCallOutcome::Ok, "fn main() {}"));

    assert_eq!(thread.items.len(), 1, "no second row: {:?}", thread.items);
    let row = event_at(&thread, call);
    assert_eq!(
        row.summary.as_deref(),
        Some("Read bridge/src/app.rs\n→ fn main() {}")
    );
    assert_eq!(row.outcome, Some(ToolCallOutcome::Ok));
    assert!(row.updated_sequence > row.sequence, "{row:?}");
    assert_eq!(
        ThreadItem::Event(row.clone()).latest_sequence(),
        row.updated_sequence,
        "the bump is what every cursor path reads"
    );
}

/// An answer with nothing in it still closes the row: the outcome carries
/// the state, and an empty suffix line would say nothing.
#[test]
fn an_empty_answer_closes_the_row_without_a_suffix() {
    let (mut thread, call) = thread_with_an_open_tool_call();

    assert!(thread.resolve_tool_call(call, ToolCallOutcome::Unanswered, ""));

    let row = event_at(&thread, call);
    assert_eq!(row.summary.as_deref(), Some("Read bridge/src/app.rs"));
    assert_eq!(row.outcome, Some(ToolCallOutcome::Unanswered));
    assert!(row.updated_sequence > row.sequence, "{row:?}");
}

/// A sequence that names no resident tool call is refused rather than
/// guessed at, which is what sends the caller back to minting a row of its
/// own.
#[test]
fn resolving_a_call_no_resident_row_holds_is_refused() {
    let (mut thread, call) = thread_with_an_open_tool_call();
    thread.push_event(
        ThreadEventKind::Narration,
        Some("dropped the index".to_string()),
        None,
        None,
        "2026-08-30T09:00:01Z",
    );
    let before = thread.last_sequence();

    assert!(!thread.resolve_tool_call(call + 1, ToolCallOutcome::Ok, "answer"));
    assert!(!thread.resolve_tool_call(9_999, ToolCallOutcome::Ok, "answer"));

    assert_eq!(
        thread.last_sequence(),
        before,
        "a refusal spends no counter value"
    );
}

/// The event mirror of
/// [`wire_value_after_reships_a_message_marked_seen_after_the_cursor`]: a
/// client whose cursor sits past the call's creation is still owed the
/// answer, and the same bump that ships it moves the high-water mark so the
/// row is shipped once.
#[test]
fn wire_value_after_reships_a_tool_call_its_answer_completed() {
    let (mut thread, call) = thread_with_an_open_tool_call();
    let cursor = thread.last_sequence();

    assert!(thread.resolve_tool_call(call, ToolCallOutcome::Ok, "fn main() {}"));

    let delta = thread.wire_value_after(cursor);
    let items = delta["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["data"]["sequence"], json!(call));
    assert_eq!(items[0]["data"]["outcome"], "ok");
    assert_eq!(
        items[0]["data"]["summary"],
        "Read bridge/src/app.rs\n→ fn main() {}"
    );
    let bumped = delta["thread_last_sequence"].as_u64().unwrap();
    assert!(bumped > cursor, "{delta:?}");
    let drained = thread.wire_value_after(bumped);
    assert_eq!(drained["items"].as_array().unwrap().len(), 0, "{drained:?}");
}

/// §6.3 invariance. An answer arriving is the agent working, never the
/// agent addressing anyone — so the two hoisted columns, the counted
/// predicate and the unread rule read exactly as they did before it landed.
#[test]
fn an_answer_landing_moves_neither_the_counted_predicate_nor_attention() {
    let (mut thread, call) = thread_with_an_open_tool_call();
    thread.post_user("drop the index", None, "2026-08-30T09:00:01Z");
    let read_to = thread.last_sequence();
    let before = event_at(&thread, call).clone();
    let attention_line = thread.last_attention_sequence();

    assert!(thread.resolve_tool_call(call, ToolCallOutcome::Error, "no such file"));

    let after = ThreadItem::Event(event_at(&thread, call).clone());
    let before = ThreadItem::Event(before);
    assert_eq!(before.counted(), after.counted());
    assert_eq!(before.attention_reason(), after.attention_reason());
    assert_eq!(after.attention_reason(), None, "activity asks for nothing");
    assert!(!after.counted(), "and buys no slot against either bound");
    assert_eq!(thread.unread_since(read_to).count, 0);
    assert_eq!(thread.last_attention_sequence(), attention_line);
}

/// A row written before events could mutate loads with the machinery it
/// predates absent, and goes back to the store byte for byte as it came.
#[test]
fn an_event_written_before_events_mutated_loads_and_round_trips_unchanged() {
    let stored = r#"{"type":"event","data":{"id":"event-7","sequence":7,"event":"tool_use","created_at":"2026-08-01T09:00:00Z","summary":"Read bridge/src/app.rs"}}"#;

    let item: ThreadItem = serde_json::from_str(stored).expect("an old row still loads");
    let ThreadItem::Event(event) = &item else {
        panic!("{item:?}");
    };
    assert_eq!(event.updated_sequence, 0, "never mutated");
    assert_eq!(event.outcome, None, "and never answered");
    assert_eq!(item.latest_sequence(), 7, "so the max is its creation");
    assert_eq!(
        serde_json::to_string(&item).unwrap(),
        stored,
        "old rows are untouched by machinery they predate"
    );
}

fn thread_with_long_conversation(item_count: usize) -> Thread {
    let mut thread = Thread::new("plan-long");
    for turn in 0..item_count {
        thread.post_user(format!("ask number {turn}"), None, "2026-08-20T09:00:00Z");
    }
    thread
}

fn page_sequences(page: &Value) -> Vec<u64> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["data"]["sequence"].as_u64().unwrap())
        .collect()
}

#[test]
fn wire_value_page_ships_only_its_limit_but_reports_the_whole_conversation() {
    let thread = thread_with_long_conversation(200);
    let page = thread.wire_value_page(None, 25);

    let sequences = page_sequences(&page);
    assert_eq!(sequences.len(), 25, "{sequences:?}");
    // The tail of the conversation, ascending, so the client renders it in
    // the order it happened.
    assert_eq!(sequences, (176..=200).collect::<Vec<u64>>());
    assert_eq!(page["oldest_sequence"], 176);
    assert_eq!(page["has_more"], true);
    assert_eq!(page["thread_total"], 200);
    assert_eq!(page["thread_last_sequence"], 200);
    // The small bounded companions still ship in full, exactly as the
    // forward cursor ships them.
    assert_eq!(page["id"], "thread:plan-long");
    assert_eq!(page["agent"]["id"], "agent:plan-long");
    assert!(page["sessions"].is_array());
    assert!(page["revisions"].is_array());
    assert!(page.get("last_completion").is_some(), "{page:?}");
}

#[test]
fn wire_value_page_names_the_conversations_newest_counter_value_not_the_pages() {
    // The idle state a reviewer opens a long conversation in: the last
    // thing the counter moved for was an in-place bump on an old item — a
    // long-queued question marked seen, a plan comment resolved — with
    // nothing posted after it. The bump lands far below the newest page.
    let mut thread = Thread::new("plan-long");
    thread.post_user("please rename the helper", None, "2026-08-20T09:00:00Z");
    for turn in 0..200 {
        thread.post_agent(format!("progress {turn}"), None, "2026-08-20T09:01:00Z");
    }
    thread.read_unread("2026-08-20T10:00:00Z");

    let page = thread.wire_value_page(None, DEFAULT_THREAD_PAGE);
    let sequences = page_sequences(&page);
    assert_eq!(sequences.len(), DEFAULT_THREAD_PAGE, "{sequences:?}");
    assert!(!sequences.contains(&1), "the bumped item is below the page");
    // One meaning for one field: the newest counter value in the whole
    // conversation, exactly as the forward cursor reports it. A page that
    // named its own top instead would leave the client asking for a
    // cursor the daemon has already moved past, so the bump would re-ship
    // on every poll for the life of the view. The client knows a page
    // delivers only its own window and reads its cursor off the items.
    assert_eq!(page["thread_last_sequence"], 202);
    assert_eq!(page["thread_total"], 201);
    assert_eq!(*sequences.last().unwrap(), 201);
}

#[test]
fn wire_value_page_has_more_only_while_older_items_remain() {
    let thread = thread_with_long_conversation(30);

    let oldest_page = thread.wire_value_page(Some(11), 10);
    assert_eq!(page_sequences(&oldest_page), (1..=10).collect::<Vec<u64>>());
    assert_eq!(oldest_page["oldest_sequence"], 1);
    assert_eq!(oldest_page["has_more"], false);

    let middle_page = thread.wire_value_page(Some(21), 10);
    assert_eq!(
        page_sequences(&middle_page),
        (11..=20).collect::<Vec<u64>>()
    );
    assert_eq!(middle_page["has_more"], true);
}

#[test]
fn a_conversation_shorter_than_the_page_ships_whole_and_says_so() {
    let thread = thread_with_conversation();
    let page = thread.wire_value_page(None, DEFAULT_THREAD_PAGE);

    assert_eq!(page_sequences(&page), vec![1, 2, 3]);
    assert_eq!(page["oldest_sequence"], 1);
    assert_eq!(page["has_more"], false);
    assert_eq!(page["thread_total"], 3);
}

#[test]
fn paging_backward_from_oldest_sequence_walks_the_whole_conversation() {
    let thread = thread_with_long_conversation(97);

    let mut walked: Vec<u64> = Vec::new();
    let mut before = None;
    loop {
        let page = thread.wire_value_page(before, 20);
        let sequences = page_sequences(&page);
        assert!(!sequences.is_empty(), "{page:?}");
        // Prepending keeps the walk in conversation order, which is how the
        // client grows its cache upward.
        walked.splice(0..0, sequences);
        if !page["has_more"].as_bool().unwrap() {
            break;
        }
        before = Some(page["oldest_sequence"].as_u64().unwrap());
    }

    assert_eq!(walked, (1..=97).collect::<Vec<u64>>());
}

#[test]
fn wire_value_page_of_an_empty_conversation_is_empty_and_final() {
    let page = Thread::new("plan-empty").wire_value_page(None, DEFAULT_THREAD_PAGE);

    assert_eq!(page["items"].as_array().unwrap().len(), 0, "{page:?}");
    assert!(page["oldest_sequence"].is_null(), "{page:?}");
    assert_eq!(page["has_more"], false);
    assert_eq!(page["thread_total"], 0);
    assert_eq!(page["thread_last_sequence"], 0);
}

#[test]
fn paging_before_the_oldest_item_is_an_empty_final_page() {
    let thread = thread_with_conversation();
    let page = thread.wire_value_page(Some(1), DEFAULT_THREAD_PAGE);

    assert_eq!(page["items"].as_array().unwrap().len(), 0, "{page:?}");
    assert!(page["oldest_sequence"].is_null(), "{page:?}");
    assert_eq!(page["has_more"], false);
    assert_eq!(page["thread_total"], 3);
}

#[test]
fn the_default_page_bounds_a_first_load_without_hiding_a_sitting() {
    // A sitting of the default's worth of messages opens whole, so the
    // default is not a bound the reviewer feels.
    let one_sitting = thread_with_long_conversation(DEFAULT_THREAD_PAGE);
    let sitting_page = one_sitting.wire_value_page(None, DEFAULT_THREAD_PAGE);
    assert_eq!(sitting_page["has_more"], false, "{sitting_page:?}");
    assert_eq!(page_sequences(&sitting_page).len(), DEFAULT_THREAD_PAGE);

    // Everything past it is paged, not shipped.
    let long = thread_with_long_conversation(DEFAULT_THREAD_PAGE * 4);
    let page = long.wire_value_page(None, DEFAULT_THREAD_PAGE);
    assert_eq!(page_sequences(&page).len(), DEFAULT_THREAD_PAGE);
    assert_eq!(page["has_more"], true, "{page:?}");
}

#[test]
fn the_outcome_message_is_the_whole_wire_record_of_a_completion() {
    let mut thread = Thread::new("run-done");
    thread.post_agent("here is what I found", None, "2026-07-24T11:00:00Z");
    thread.post_outcome(
        MessageOutcome::Completed,
        "Implemented the change",
        None,
        "2026-07-24T12:00:00Z",
    );

    let wire = thread.wire_value();
    assert_eq!(wire["items"][0]["type"], "message");
    assert!(wire["items"][0]["data"].get("source").is_none(), "{wire:?}");
    assert_eq!(wire["items"][1]["type"], "message");
    assert_eq!(wire["items"][1]["data"]["outcome"], "completed");
    assert_eq!(wire["items"][1]["data"]["done"], true);
    assert_eq!(wire["items"][1]["data"]["body"], "Implemented the change");
    assert_eq!(wire["items"].as_array().unwrap().len(), 2, "{wire:?}");
}

#[test]
fn message_and_event_links_round_trip_on_the_wire() {
    let mut thread = Thread::new("run-linked");
    thread.post_agent_with_links(
        "The parser and its tests changed.",
        None,
        vec![ThreadLink::File {
            path: "src/parser.rs".to_string(),
            line_start: Some(12),
            line_end: Some(24),
        }],
        "2026-07-24T12:00:00Z",
    );
    thread.push_event_with_links(
        ThreadEventKind::StageStarted,
        Some("Started stage Parser".to_string()),
        None,
        None,
        vec![ThreadLink::PlanStage {
            plan_id: "plan-1".to_string(),
            stage_id: "parser".to_string(),
            path: ".build/plan/01-parser.md".to_string(),
        }],
        "2026-07-24T12:01:00Z",
    );

    let wire = thread.wire_value();
    assert_eq!(wire["items"][0]["data"]["links"][0]["kind"], "file");
    assert_eq!(
        wire["items"][0]["data"]["links"][0]["path"],
        "src/parser.rs"
    );
    assert_eq!(wire["items"][1]["data"]["links"][0]["kind"], "plan_stage");
    assert_eq!(wire["items"][1]["data"]["links"][0]["stage_id"], "parser");

    let canonical = vec![
        ThreadLink::IssueStage {
            issue_id: "issue-1".into(),
            stage_id: "parser".into(),
            path: ".build/plan/01-parser.md".into(),
        },
        ThreadLink::Implementation {
            issue_id: "issue-1".into(),
            implementation_id: "run-1".into(),
        },
        ThreadLink::Worktree {
            worktree_id: "wt-0123456789ab".into(),
        },
        ThreadLink::Commit {
            sha: "a".repeat(40),
        },
        ThreadLink::Recovery {
            recovery_id: "recovery-1".into(),
        },
    ];
    let value = serde_json::to_value(&canonical).unwrap();
    assert_eq!(value[0]["kind"], "issue_stage");
    assert_eq!(value[1]["kind"], "implementation");
    assert_eq!(value[2]["kind"], "worktree");
    assert_eq!(value[3]["kind"], "commit");
    assert_eq!(value[4]["kind"], "recovery");
    assert_eq!(
        serde_json::from_value::<Vec<ThreadLink>>(value).unwrap(),
        canonical
    );
}
