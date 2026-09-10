use super::*;

/// The two readings of the rule, held equal over every kind there is: the
/// Rust one here, and the `message = 1 OR attention = 1` the store filters
/// with. A kind added later cannot make them disagree without failing
/// here.
#[test]
fn a_counted_item_is_a_message_or_a_call_for_the_human() {
    let mut thread = Thread::new("run-counted");
    for kind in ThreadEventKind::ALL {
        thread.push_event(
            kind,
            Some(format!("{} happened", kind.as_str())),
            None,
            None,
            "2026-08-29T09:00:00Z",
        );
    }
    thread.post_user("please rename the helper", None, "2026-08-29T09:01:00Z");
    thread.post_agent("renamed it", None, "2026-08-29T09:02:00Z");
    thread.post_agent_progress("still going", None, "2026-08-29T09:03:00Z");
    thread.post_outcome(
        MessageOutcome::Blocked,
        "needs production credentials",
        None,
        "2026-08-29T09:04:00Z",
    );

    for item in &thread.items {
        let expected = matches!(item, ThreadItem::Message(_)) || item.attention_reason().is_some();
        assert_eq!(item.counted(), expected, "{item:?}");
    }
}

/// Both halves of the rule, said out loud rather than only as an
/// equivalence: a progress note is conversation even though it asks
/// nothing, and activity is not even though it is the agent talking.
#[test]
fn every_message_counts_and_no_activity_does() {
    let mut thread = Thread::new("run-counted");
    thread.post_agent_progress("still going", None, "2026-08-29T09:00:00Z");
    for kind in [
        ThreadEventKind::Reasoning,
        ThreadEventKind::ToolUse,
        ThreadEventKind::ToolResult,
        ThreadEventKind::Narration,
        ThreadEventKind::TaskUpdate,
        ThreadEventKind::Triaged,
    ] {
        thread.push_event(kind, None, None, None, "2026-08-29T09:01:00Z");
    }
    thread.push_event(
        ThreadEventKind::Interrupted,
        None,
        None,
        None,
        "2026-08-29T09:02:00Z",
    );

    let counted: Vec<bool> = thread.items.iter().map(ThreadItem::counted).collect();
    assert_eq!(
        counted,
        vec![true, false, false, false, false, false, false, true],
        "{:?}",
        thread.items
    );
}
