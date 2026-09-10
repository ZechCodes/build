use super::*;

/// "Working" has to end when the agent hands back, and only the agent knows
/// which of its messages is a progress note and which is the handoff. A
/// posted reply hands back by default — a stuck "Working" outlives the work
/// it describes, while a progress note that clears the line early costs
/// nothing.
#[test]
fn an_agent_message_hands_back_unless_it_says_it_is_still_working() {
    let mut thread = Thread::new("run-1");
    let handoff = thread.post_agent("here is what I found", None, "2026-08-08T03:00:00Z");
    let update = thread.post_agent_progress("still digging", None, "2026-08-08T03:01:00Z");

    let message = |id: &str| {
        thread
            .items
            .iter()
            .find_map(|item| match item {
                ThreadItem::Message(m) if m.id == id => Some(m.clone()),
                _ => None,
            })
            .expect("the message is on the thread")
    };
    assert!(
        !message(&handoff).still_working,
        "an ordinary reply gives the turn back"
    );
    assert!(
        message(&update).still_working,
        "a progress note keeps the agent working"
    );
}

/// Working time is derived from the turn, not from a timer: it starts when
/// the agent READS what the human said and ends when the agent hands the
/// turn back. Same rule the MCP tool descriptions already teach agents.
#[test]
fn a_turn_runs_from_the_read_until_the_agent_hands_it_back() {
    let mut thread = Thread::new("run-working");
    assert_eq!(thread.working_since(), None, "an empty thread is idle");

    thread.post_user("do the thing", None, "2026-08-13T10:00:00Z");
    assert_eq!(
        thread.working_since(),
        None,
        "a message nobody has read yet is not work in flight"
    );

    thread.read_unread("2026-08-13T10:00:05Z");
    assert_eq!(thread.working_since(), Some("2026-08-13T10:00:05Z"));

    thread.post_agent_progress("halfway", None, "2026-08-13T10:01:00Z");
    assert_eq!(
        thread.working_since(),
        Some("2026-08-13T10:00:05Z"),
        "a progress note keeps the same turn running"
    );
    thread.push_event(
        ThreadEventKind::Committed,
        None,
        None,
        None,
        "2026-08-13T10:02:00Z",
    );
    assert_eq!(
        thread.working_since(),
        Some("2026-08-13T10:00:05Z"),
        "status is the work happening, not the work ending"
    );

    thread.post_agent("here is what I did", None, "2026-08-13T10:03:00Z");
    assert_eq!(thread.working_since(), None, "an ordinary reply hands back");
}

/// An agent that reports `done` without saying anything has still handed
/// back — the event is the record.
#[test]
fn an_attention_event_ends_the_turn_with_nothing_said() {
    let mut thread = Thread::new("run-done");
    thread.post_user("ship it", None, "2026-08-13T11:00:00Z");
    thread.read_unread("2026-08-13T11:00:01Z");
    thread.push_event(
        ThreadEventKind::Done,
        None,
        None,
        None,
        "2026-08-13T11:05:00Z",
    );
    assert_eq!(thread.working_since(), None);

    // A second message read after the handoff starts a new turn.
    thread.post_user("one more thing", None, "2026-08-13T11:06:00Z");
    thread.read_unread("2026-08-13T11:06:02Z");
    assert_eq!(thread.working_since(), Some("2026-08-13T11:06:02Z"));
}
