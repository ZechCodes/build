use super::*;

const ACTIVITY: [ThreadEventKind; 5] = [
    ThreadEventKind::Reasoning,
    ThreadEventKind::ToolUse,
    ThreadEventKind::ToolResult,
    ThreadEventKind::Narration,
    ThreadEventKind::TaskUpdate,
];

/// The fifth kind is the four's equal in every rule the roster of kinds
/// already carries: it is on `ALL`, it is `Status`, and its wire token is
/// the snake_case of its name — so the class split, the Task mirror and
/// the counted predicate cover it with no new code.
#[test]
fn background_task_updates_join_the_activity_kinds() {
    assert_eq!(ThreadEventKind::TaskUpdate.as_str(), "task_update");
    assert_eq!(ThreadEventKind::TaskUpdate.class(), EventClass::Status);
    assert!(
        ThreadEventKind::ALL.contains(&ThreadEventKind::TaskUpdate),
        "a kind off ALL is a kind every rule tested over the roster misses"
    );
    assert_eq!(
        serde_json::to_value(ThreadEventKind::TaskUpdate).unwrap(),
        serde_json::json!("task_update"),
        "the token it serializes as is the token it names"
    );
}

/// The property that makes activity safe to put in the conversation: an
/// agent thinking out loud updates the entry underneath the human and
/// never marks it unread.
#[test]
fn agent_activity_is_status_and_moves_no_unread_count() {
    let mut thread = Thread::new("run-activity");
    let cursor = thread.last_sequence();
    for kind in ACTIVITY {
        assert_eq!(kind.class(), EventClass::Status, "{kind:?}");
        thread.push_event(
            kind,
            Some(format!("{} happened", kind.as_str())),
            None,
            None,
            "2026-08-23T09:00:00Z",
        );
    }

    assert_eq!(thread.items.len(), ACTIVITY.len());
    for item in &thread.items {
        assert_eq!(item.attention_reason(), None, "{item:?}");
    }
    assert_eq!(thread.unread_since(cursor), UnreadSummary::default());
    assert_eq!(thread.last_attention_sequence(), 0);
}

/// Activity rides the wire as an ordinary thread item — no new envelope,
/// no new RPC, and a token that matches how the kind serializes.
#[test]
fn a_tool_use_rides_the_wire_as_an_ordinary_thread_item() {
    let mut thread = Thread::new("run-activity");
    thread.push_event(
        ThreadEventKind::ToolUse,
        Some("Read bridge/src/app.rs".to_string()),
        None,
        None,
        "2026-08-23T09:00:00Z",
    );

    let wire = thread.wire_value();
    assert_eq!(wire["items"][0]["type"], "event");
    assert_eq!(wire["items"][0]["data"]["event"], "tool_use");
    assert_eq!(
        wire["items"][0]["data"]["summary"],
        "Read bridge/src/app.rs"
    );
    assert_eq!(wire["items"][0]["data"]["sequence"], 1);
}

/// The packet exists to carry the conversation across a restart, so it
/// carries messages and nothing else: a session that emitted activity all
/// afternoon must still hand its replacement what the human said.
#[test]
fn the_catch_up_packet_carries_messages_and_no_events() {
    let mut thread = Thread::new("run-activity");
    thread.post_user("please rename the helper", None, "2026-08-23T09:00:00Z");
    for index in 0..3 {
        thread.push_event(
            ThreadEventKind::ToolUse,
            Some(format!("Read file-{index}.rs")),
            None,
            None,
            "2026-08-23T09:01:00Z",
        );
    }
    thread.post_agent("renamed it", None, "2026-08-23T09:02:00Z");
    // Build's own observations about the agent go the same way as activity.
    thread.push_event(
        ThreadEventKind::Blocked,
        Some("the test suite will not build".to_string()),
        None,
        None,
        "2026-08-23T09:03:00Z",
    );

    let catch_up = thread.catch_up_markdown(40);
    assert_eq!(
        catch_up, "- user: please rename the helper\n- agent: renamed it",
        "{catch_up}"
    );
}

/// The limit counts messages, not items. An agent that emitted more
/// activity than the packet holds must still be handed what the human
/// said — the case that made the packet messages-only in the first place.
#[test]
fn a_session_full_of_activity_still_hands_back_the_humans_words() {
    let mut thread = Thread::new("run-activity");
    thread.post_user("please rename the helper", None, "2026-08-23T09:00:00Z");
    for index in 0..100 {
        thread.push_event(
            ThreadEventKind::ToolUse,
            Some(format!("Read file-{index}.rs")),
            None,
            None,
            "2026-08-23T09:01:00Z",
        );
    }

    assert_eq!(
        thread.catch_up_markdown(40),
        "- user: please rename the helper"
    );
}

/// The limit still bounds the packet, and still keeps the newest.
#[test]
fn the_packet_keeps_the_newest_messages_up_to_its_limit() {
    let mut thread = Thread::new("run-activity");
    for index in 0..5 {
        thread.post_user(format!("ask {index}"), None, "2026-08-23T09:00:00Z");
        thread.push_event(
            ThreadEventKind::Reasoning,
            Some("thinking".to_string()),
            None,
            None,
            "2026-08-23T09:00:01Z",
        );
    }

    assert_eq!(thread.catch_up_markdown(2), "- user: ask 3\n- user: ask 4");
}
