use super::*;

const NOW: &str = "2026-08-29T09:00:00Z";

#[test]
fn catch_up_renders_viewing_context_once_beside_the_message() {
    let mut thread = Thread::for_agent("agent-a");
    thread.post_user_with_context(
        "change this",
        None,
        Some(ViewingContext {
            version: 1,
            items: vec![ViewingContextItem::Selection {
                path: "src/lib.rs".into(),
                text: "let old = true;".into(),
                line_start: Some(4),
                line_end: Some(4),
                side: Some(SelectionSide::New),
                unsaved: false,
                truncated: false,
            }],
        }),
        NOW,
    );

    let prompt = thread.catch_up_markdown(10);
    assert_eq!(prompt.matches("viewing context:").count(), 1);
    assert!(prompt.contains("src/lib.rs"));
    assert!(prompt.contains("let old = true;"));
}

/// A conversation stored whole, and the process that booted onto the last
/// `tail` items of it — which is where an activity-heavy session leaves
/// its replacement.
fn stored_and_booted(tail: usize) -> (Vec<ThreadItem>, Thread) {
    let mut whole = Thread::new("run-restart");
    whole.post_user("please rename the helper", None, NOW);
    whole.post_agent("on it", None, NOW);
    for index in 0..8 {
        whole.push_event(
            ThreadEventKind::ToolUse,
            Some(format!("Read file-{index}.rs")),
            None,
            None,
            NOW,
        );
    }
    let stored = whole.items.clone();
    let mut booted = Thread::new("run-restart");
    booted.adopt_stored_tail(
        stored[stored.len() - tail..].to_vec(),
        (stored.len() - tail) as u64,
        whole.last_sequence(),
    );
    (stored, booted)
}

/// What the store hands the packet back: the conversation's messages,
/// oldest-first.
fn stored_messages(stored: &[ThreadItem]) -> Vec<ThreadItem> {
    stored
        .iter()
        .filter(|item| matches!(item, ThreadItem::Message(_)))
        .cloned()
        .collect()
}

/// The gate: only a starved tail pays a read. A conversation held whole,
/// and a long one whose tail still holds the packet's worth of messages,
/// are both answered out of memory.
#[test]
fn only_a_tail_short_of_its_messages_reaches_for_the_store() {
    let (_, booted) = stored_and_booted(5);
    assert!(
        booted.catch_up_reaches_stored_history(40),
        "a tail of pure activity has to read the store"
    );
    assert!(
        !booted.catch_up_reaches_stored_history(0),
        "a packet that asks for nothing needs nothing"
    );

    let mut whole = Thread::new("run-whole");
    whole.post_user("please rename the helper", None, NOW);
    assert!(
        !whole.catch_up_reaches_stored_history(40),
        "a conversation with no history under it never reads the store"
    );

    let (_, rich_tail) = stored_and_booted(10);
    assert!(
        !rich_tail.catch_up_reaches_stored_history(2),
        "a tail holding the packet's worth of messages answers from memory"
    );
}

/// The failure this exists for: the tail holds nothing but tool calls, so
/// the messages-only filter over it yields an empty packet. Read through
/// the store, the same packet carries what the human said.
#[test]
fn a_starved_tail_still_hands_over_the_conversation() {
    let (stored, booted) = stored_and_booted(5);
    assert_eq!(
        booted.catch_up_markdown(40),
        "",
        "the tail alone is the starved packet this replaces"
    );

    assert_eq!(
        booted.catch_up_markdown_including_history(&stored_messages(&stored), 40),
        "- user: please rename the helper\n- agent: on it"
    );
}

/// The merge rule, held to the precedent the forward cursor set: a stored
/// row is admitted only below what this process read, so a message the
/// tail still holds is carried once, from the tail.
#[test]
fn a_message_the_tail_still_holds_is_not_repeated() {
    let (stored, booted) = stored_and_booted(9);

    let packet = booted.catch_up_markdown_including_history(&stored_messages(&stored), 40);
    assert_eq!(packet, "- user: please rename the helper\n- agent: on it");
    assert_eq!(packet.matches("on it").count(), 1, "{packet}");
}

/// The limit still counts messages and still keeps the newest of them,
/// across the join.
#[test]
fn the_merged_packet_keeps_the_newest_messages_up_to_its_limit() {
    let (stored, booted) = stored_and_booted(5);

    assert_eq!(
        booted.catch_up_markdown_including_history(&stored_messages(&stored), 1),
        "- agent: on it"
    );
}
