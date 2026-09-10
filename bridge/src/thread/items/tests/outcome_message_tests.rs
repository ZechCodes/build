use super::*;

fn report() -> CompletionReport {
    CompletionReport {
        critical_files: vec!["src/render.rs — the new draw path".to_string()],
        risk_notes: vec!["untested on the legacy screen".to_string()],
        decisions: vec!["kept the old entry point".to_string()],
        skips: vec!["no perf pass".to_string()],
    }
}

#[test]
fn a_completion_is_one_agent_message_carrying_its_outcome() {
    let mut thread = Thread::new("run-outcome");
    thread.post_outcome(
        MessageOutcome::Completed,
        "implemented the change",
        Some(&report()),
        "2026-08-24T09:00:00Z",
    );

    assert_eq!(thread.items.len(), 1, "{:?}", thread.items);
    let ThreadItem::Message(message) = &thread.items[0] else {
        panic!("the outcome is a message: {:?}", thread.items);
    };
    assert_eq!(message.role, MessageRole::Agent);
    assert_eq!(message.body, "implemented the change");
    assert_eq!(message.outcome, Some(MessageOutcome::Completed));
    assert!(
        message.done,
        "a completed outcome keeps the flag an older client reads"
    );
    assert_eq!(message.completion_report.as_deref(), Some(&report()));
    assert!(!message.still_working, "an outcome hands the turn back");
}

/// The attention job the `Done` and `Blocked` events used to do, moved onto
/// the message whole: one unread entry per outcome, naming which it was.
#[test]
fn every_outcome_needs_the_human_once_and_says_which_it_was() {
    for (outcome, reason) in [
        (MessageOutcome::Completed, "done"),
        (MessageOutcome::Blocked, "blocked"),
        (MessageOutcome::Failed, "run_failed"),
    ] {
        let mut thread = Thread::new("run-outcome");
        thread.post_user("do the thing", None, "2026-08-24T09:00:00Z");
        thread.read_unread("2026-08-24T09:00:01Z");
        let cursor = thread.last_sequence();
        thread.post_outcome(
            outcome,
            "the agent's own words",
            None,
            "2026-08-24T09:01:00Z",
        );

        let unread = thread.unread_since(cursor);
        assert_eq!(unread.count, 1, "{outcome:?}");
        assert_eq!(unread.reason, Some(reason), "{outcome:?}");
        assert_eq!(
            thread.working_since(),
            None,
            "an outcome ends the turn: {outcome:?}"
        );
    }
}

/// Additive: `outcome` is new, `done` keeps its exact meaning, and the
/// report the `Done` event carried rides the message instead.
#[test]
fn the_outcome_and_its_report_ride_the_message_on_the_wire() {
    let mut thread = Thread::new("run-outcome");
    thread.post_outcome(
        MessageOutcome::Blocked,
        "needs production credentials",
        Some(&report()),
        "2026-08-24T09:00:00Z",
    );
    thread.post_outcome(
        MessageOutcome::Completed,
        "implemented the change",
        None,
        "2026-08-24T09:02:00Z",
    );

    let wire = thread.wire_value();
    let blocked = &wire["items"][0];
    assert_eq!(blocked["type"], "message");
    assert_eq!(blocked["data"]["outcome"], "blocked");
    assert_eq!(blocked["data"]["role"], "agent");
    assert!(
        blocked["data"].get("done").is_none(),
        "only a completion sets done: {wire:?}"
    );
    assert_eq!(
        blocked["data"]["completion_report"]["risk_notes"][0],
        "untested on the legacy screen"
    );
    let completed = &wire["items"][1];
    assert_eq!(completed["data"]["outcome"], "completed");
    assert_eq!(completed["data"]["done"], true);
    assert!(
        completed["data"].get("completion_report").is_none(),
        "an outcome with no report omits the field: {wire:?}"
    );
}

#[test]
fn an_ordinary_message_carries_neither_field() {
    let mut thread = Thread::new("run-outcome");
    thread.post_agent("here is what I found", None, "2026-08-24T09:00:00Z");

    let wire = thread.wire_value();
    assert!(
        wire["items"][0]["data"].get("outcome").is_none(),
        "{wire:?}"
    );
    assert!(
        wire["items"][0]["data"].get("completion_report").is_none(),
        "{wire:?}"
    );
    assert_eq!(
        thread.items[0].attention_reason(),
        Some(AGENT_MESSAGE_REASON)
    );
}

/// The report is the densest statement of what a change touched, so a
/// search reads it with the summary — as it did off the `Done` event.
#[test]
fn a_search_reads_the_report_with_the_summary() {
    let mut thread = Thread::new("run-outcome");
    thread.post_outcome(
        MessageOutcome::Completed,
        "implemented the change",
        Some(&report()),
        "2026-08-24T09:00:00Z",
    );

    let text = thread.items[0].searchable_text();
    assert!(text.contains("implemented the change"), "{text}");
    assert!(text.contains("src/render.rs — the new draw path"), "{text}");
}

/// The gap this exists to close: a replacement agent is told why its
/// predecessor blocked, out of the packet that carries the human's words.
#[test]
fn the_catch_up_packet_carries_the_outcome_a_predecessor_reported() {
    let mut thread = Thread::new("run-outcome");
    thread.post_user("please rename the helper", None, "2026-08-24T09:00:00Z");
    thread.post_outcome(
        MessageOutcome::Blocked,
        "needs production credentials",
        None,
        "2026-08-24T09:01:00Z",
    );
    thread.push_event(
        ThreadEventKind::ToolUse,
        Some("Read src/app.rs".to_string()),
        None,
        None,
        "2026-08-24T09:02:00Z",
    );

    assert_eq!(
        thread.catch_up_markdown(40),
        "- user: please rename the helper\n- agent [blocked]: needs production credentials",
    );
}

/// Every outcome is in the packet, each prefixed with which it was — and no
/// event line is re-admitted with them.
#[test]
fn the_packet_names_each_outcome_and_still_carries_no_events() {
    let mut thread = Thread::new("run-outcome");
    for (outcome, summary) in [
        (MessageOutcome::Completed, "implemented the change"),
        (MessageOutcome::Blocked, "needs production credentials"),
        (MessageOutcome::Failed, "the migration will not run"),
    ] {
        thread.post_outcome(outcome, summary, None, "2026-08-24T09:00:00Z");
    }
    thread.push_event(
        ThreadEventKind::IdleUnreported,
        Some("Agent went quiet without reporting done".to_string()),
        None,
        None,
        "2026-08-24T09:03:00Z",
    );

    assert_eq!(
        thread.catch_up_markdown(40),
        "- agent [completed]: implemented the change\n\
             - agent [blocked]: needs production credentials\n\
             - agent [failed]: the migration will not run",
    );
}

/// A thread persisted before outcomes existed: a `Done` event carrying the
/// report, and the companion completion message an older bridge wrote
/// beside it. It loads, it still needs the human where it did, and the
/// event still carries what it always carried — no migration.
fn pre_step_7_thread() -> Thread {
    let raw = serde_json::json!({
        "id": "thread:run-old",
        "agent": { "id": "agent:run-old" },
        "items": [
            { "type": "message", "data": {
                "id": "message-1", "sequence": 1, "role": "user",
                "body": "please rename the helper",
                "created_at": "2026-07-24T12:00:00Z", "seen_at": "2026-07-24T12:00:30Z" } },
            { "type": "message", "data": {
                "id": "message-2", "sequence": 2, "role": "agent", "done": true,
                "source": "completion", "body": "Implemented the change",
                "created_at": "2026-07-24T12:01:00Z" } },
            { "type": "event", "data": {
                "id": "event-3", "sequence": 3, "event": "done",
                "created_at": "2026-07-24T12:01:00Z",
                "summary": "Implemented the change",
                "completion_report": { "critical_files": ["src/render.rs"] } } },
            { "type": "event", "data": {
                "id": "event-4", "sequence": 4, "event": "blocked",
                "created_at": "2026-07-24T12:02:00Z",
                "summary": "needs production credentials" } }
        ],
        "next_sequence": 4
    });
    serde_json::from_value(raw).expect("a pre-outcome thread loads")
}

#[test]
fn a_thread_written_before_outcomes_loads_and_reads_as_it_did() {
    let thread = pre_step_7_thread();

    let reasons: Vec<Option<&str>> = thread
        .items
        .iter()
        .map(ThreadItem::attention_reason)
        .collect();
    assert_eq!(
        reasons,
        vec![None, Some("done"), Some("done"), Some("blocked")],
        "{:?}",
        thread.items
    );
    let ThreadItem::Message(completion) = &thread.items[1] else {
        panic!("{:?}", thread.items);
    };
    assert_eq!(
        completion.outcome, None,
        "an old record carries no outcome field"
    );
    assert_eq!(completion.source, MessageSource::Completion);
    let ThreadItem::Event(done) = &thread.items[2] else {
        panic!("{:?}", thread.items);
    };
    assert_eq!(
        done.completion_report
            .as_ref()
            .map(|report| report.critical_files.clone()),
        Some(vec!["src/render.rs".to_string()]),
        "the old event still carries the report it was written with"
    );
    assert_eq!(thread.unread_since(0).count, 3);
}

/// The packet reads an old completion message as the completion it was:
/// `done` without an `outcome` is a completed outcome.
#[test]
fn an_old_completion_message_reads_as_a_completed_outcome() {
    let thread = pre_step_7_thread();

    assert_eq!(
        thread.catch_up_markdown(40),
        "- user: please rename the helper\n- agent [completed]: Implemented the change",
    );
}
