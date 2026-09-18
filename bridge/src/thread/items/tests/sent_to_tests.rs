use super::*;

const NOW: &str = "2026-09-17T09:00:00Z";

fn recipient() -> AgentIdentity {
    AgentIdentity {
        id: "agent-2".to_string(),
        owner: Some(AgentOwnerRef {
            kind: AgentOwnerKind::Workspace,
            id: "ws-2".to_string(),
            name: "two".to_string(),
        }),
        topic: Some("Reading the router".to_string()),
    }
}

/// The record of a message this agent sent somewhere else: its own words, on
/// its own side of the conversation, naming who they went to.
#[test]
fn a_send_is_an_agent_message_naming_its_recipient() {
    let mut thread = Thread::new("run-sender");
    thread.post_agent_sent("rebase on main", recipient(), NOW);

    let ThreadItem::Message(message) = &thread.items[0] else {
        panic!("a send is a message");
    };
    assert_eq!(message.role, MessageRole::Agent);
    assert_eq!(message.body, "rebase on main");
    assert_eq!(
        message.sent_to.as_deref(),
        Some(&recipient()),
        "who it went to"
    );
    assert_eq!(message.from_agent, None, "nobody sent this one in");
}

/// It counts as a message — a page shows it, and it cuts the run of activity
/// around it — and it calls nobody: no attention, nothing unread, and the line
/// a dismissal is judged against stays where it was.
#[test]
fn a_send_counts_as_a_message_and_calls_nobody() {
    let mut thread = Thread::new("run-sender");
    thread.post_user("start on the rail", None, NOW);
    let line = thread.last_own_message_sequence();
    thread.post_agent_sent("rebase on main", recipient(), NOW);

    let item = &thread.items[1];
    assert!(item.counts_toward_page(), "a page shows it");
    assert!(!item.is_activity(), "it cuts the run of activity around it");
    assert!(!item.is_handoff(), "nobody handed this conversation work");
    assert_eq!(item.attention_reason(), None, "it calls nobody");
    assert_eq!(thread.unread_since(0).count, 0);
    assert_eq!(
        thread.last_own_message_sequence(),
        line,
        "a cleared row stays cleared"
    );
}

/// A message written before a send could be recorded reads back with no
/// recipient, and an ordinary agent message still serializes without the field.
#[test]
fn a_message_that_went_nowhere_carries_no_recipient() {
    let mut thread = Thread::new("run-sender");
    thread.post_agent("the rail reads top to bottom", None, NOW);

    let raw = serde_json::to_value(&thread.items[0]).unwrap();
    assert_eq!(raw["data"].get("sent_to"), None, "{raw:?}");
    let item: ThreadItem = serde_json::from_value(raw).unwrap();
    let ThreadItem::Message(message) = item else {
        panic!("a message round-trips as a message");
    };
    assert_eq!(message.sent_to, None);
}
