use build_bridge::thread::{
    ArtifactKind, MessageAnchor, Thread, ThreadEventDraft, ThreadEventKind, ThreadItem,
};

#[test]
fn unread_messages_are_scoped_seen_and_not_returned_twice() {
    let mut thread = Thread::new("run-7");
    thread.post_user("Please rename it", None, "2026-07-22T10:00:00Z");
    thread.post_agent(
        "Which public name should I use?",
        None,
        "2026-07-22T10:01:00Z",
    );

    let unread = thread.read_unread("2026-07-22T10:02:00Z");
    assert_eq!(unread.len(), 1);
    assert_eq!(unread[0].body, "Please rename it");
    assert!(thread.read_unread("2026-07-22T10:03:00Z").is_empty());
}

#[test]
fn a_revision_resolves_an_anchored_comment_when_its_passage_changes() {
    let mut thread = Thread::new("plan-4");
    let first = thread.add_revision(
        ArtifactKind::Plan,
        "Before\nold sentence\nAfter",
        "2026-07-22T10:00:00Z",
    );
    thread.post_user(
        "Make this concrete",
        Some(MessageAnchor {
            artifact: ArtifactKind::Plan,
            revision_id: Some(first.id),
            path: Some(".build/plan.md".into()),
            side: None,
            line_start: None,
            line_end: None,
            heading_path: vec!["Implementation".into()],
            snippet: "old sentence".into(),
        }),
        "2026-07-22T10:01:00Z",
    );

    let second = thread.add_revision(
        ArtifactKind::Plan,
        "Before\nnew sentence\nAfter",
        "2026-07-22T10:02:00Z",
    );
    let message = thread
        .items
        .iter()
        .find_map(|item| match item {
            ThreadItem::Message(message) if message.role.as_str() == "user" => Some(message),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        message.resolved_by_revision.as_deref(),
        Some(second.id.as_str())
    );
    assert_eq!(
        second.snapshot.as_deref(),
        Some("Before\nnew sentence\nAfter")
    );
    let wire = thread.wire_value();
    assert!(wire["revisions"][1].get("snapshot").is_none());
}

#[test]
fn session_lineage_and_completion_report_survive_json_round_trip() {
    let mut thread = Thread::new("run-9");
    let parent = thread.start_session(
        "codex",
        Some("gpt-5.3-codex"),
        Some("high"),
        "build",
        "2026-07-22T10:00:00Z",
    );
    thread.finish_session(&parent, "2026-07-22T10:05:00Z");
    let child = thread.start_session("codex", None, None, "revise", "2026-07-22T10:06:00Z");

    let restored: Thread = serde_json::from_str(&serde_json::to_string(&thread).unwrap()).unwrap();
    assert_eq!(restored.sessions[1].id, child);
    assert_eq!(
        restored.sessions[1].parent_session_id.as_deref(),
        Some(parent.as_str())
    );
    assert_eq!(restored.agent.id, "agent:run-9");
}

#[test]
fn a_drafted_event_carries_its_parent_sequence_and_an_unparented_one_carries_no_key() {
    let mut thread = Thread::new("run-11");
    let spawning_call = thread.push_event(
        ThreadEventKind::ToolUse,
        Some("Agent read the README".into()),
        None,
        None,
        "2026-09-01T10:00:00Z",
    );
    let folded = thread.push_drafted_event(
        ThreadEventDraft {
            event: ThreadEventKind::Reasoning,
            summary: Some("counting the characters".into()),
            session_id: None,
            revision_id: None,
            links: Vec::new(),
            parent_sequence: Some(spawning_call),
        },
        "2026-09-01T10:00:01Z",
    );

    let wire = thread.wire_value();
    let items = wire["items"].as_array().unwrap();
    let spawning = items
        .iter()
        .find(|item| item["data"]["sequence"] == spawning_call)
        .unwrap();
    let child = items
        .iter()
        .find(|item| item["data"]["sequence"] == folded)
        .unwrap();
    assert_eq!(child["data"]["parent_sequence"], spawning_call);
    assert!(
        spawning["data"].get("parent_sequence").is_none(),
        "a row with no parent carries no key at all: {spawning}"
    );
}
