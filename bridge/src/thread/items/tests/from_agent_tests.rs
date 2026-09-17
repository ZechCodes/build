use super::*;

const NOW: &str = "2026-09-17T09:00:00Z";

fn message_at(thread: &Thread, index: usize) -> &ThreadMessage {
    match &thread.items[index] {
        ThreadItem::Message(message) => message,
        other => panic!("item {index} is not a message: {other:?}"),
    }
}

/// One agent's words in another agent's conversation stay on the inbound side
/// of it — the side the human's words land on — and say who wrote them.
#[test]
fn a_message_an_agent_sent_is_a_user_message_naming_its_sender() {
    let mut thread = Thread::new("run-dispatched");
    thread.post_user_from_agent(
        "finish the toast",
        AgentIdentity {
            id: "router-7".to_string(),
        },
        NOW,
    );

    let message = message_at(&thread, 0);
    assert_eq!(
        message.role,
        MessageRole::User,
        "the role is the direction, not the author"
    );
    assert_eq!(
        message.from_agent.as_ref().map(|from| from.id.as_str()),
        Some("router-7")
    );
}

/// The human's own words claim no sender, so a message they wrote serializes
/// byte-for-byte as it always has.
#[test]
fn the_humans_own_message_carries_no_sender() {
    let mut thread = Thread::new("run-dispatched");
    thread.post_user("finish the toast", None, NOW);
    thread.post_user_from_agent(
        "and rebase it",
        AgentIdentity {
            id: "router-7".to_string(),
        },
        NOW,
    );

    let human = serde_json::to_value(message_at(&thread, 0)).unwrap();
    assert_eq!(human.get("from_agent"), None, "{human:?}");
    let sent = serde_json::to_value(message_at(&thread, 1)).unwrap();
    assert_eq!(sent["from_agent"], json!({ "id": "router-7" }), "{sent:?}");
}

/// A message an agent sent is not the conversation calling the human: they
/// were not spoken to, so nothing about it counts as unread.
#[test]
fn a_message_an_agent_sent_never_needs_the_human() {
    let mut thread = Thread::new("run-dispatched");
    thread.post_user_from_agent(
        "finish the toast",
        AgentIdentity {
            id: "router-7".to_string(),
        },
        NOW,
    );

    assert_eq!(thread.items[0].attention_reason(), None);
    assert_eq!(thread.unread_since(0).count, 0);
}

/// A record written before agents could speak to each other reads back with no
/// sender at all, and gains nothing.
#[test]
fn a_record_written_before_senders_reads_back_anonymous() {
    let mut thread = Thread::new("run-dispatched");
    thread.post_user("finish the toast", None, NOW);
    let mut raw = serde_json::to_value(&thread.items[0]).unwrap();
    raw["data"].as_object_mut().unwrap().remove("from_agent");

    let item: ThreadItem = serde_json::from_value(raw).unwrap();
    let ThreadItem::Message(message) = item else {
        panic!("a message round-trips as a message");
    };
    assert_eq!(message.from_agent, None);
}
