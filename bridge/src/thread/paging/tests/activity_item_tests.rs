use super::*;

/// The two readings of the rule, held equal over every kind there is: the
/// Rust one here, and the set the web client folds with. A kind added
/// later cannot make them disagree without failing here.
#[test]
fn activity_is_the_five_kinds_the_client_folds() {
    const FOLDED_BY_THE_CLIENT: [&str; 5] = [
        "reasoning",
        "tool_use",
        "tool_result",
        "narration",
        "task_update",
    ];

    for kind in ThreadEventKind::ALL {
        assert_eq!(
            kind.is_activity(),
            FOLDED_BY_THE_CLIENT.contains(&kind.as_str()),
            "{kind:?}"
        );
    }
}

/// A message is never activity, whoever wrote it and whatever it reports —
/// a run of work ends the moment somebody says something.
#[test]
fn no_message_is_activity_and_every_activity_event_is() {
    let mut thread = Thread::new("run-activity");
    thread.post_user("please rename the helper", None, "2026-08-29T09:00:00Z");
    thread.push_event(
        ThreadEventKind::ToolUse,
        Some("Read src/thread.rs".to_string()),
        None,
        None,
        "2026-08-29T09:01:00Z",
    );
    thread.push_event(
        ThreadEventKind::Committed,
        None,
        None,
        None,
        "2026-08-29T09:02:00Z",
    );
    thread.post_agent_progress("still going", None, "2026-08-29T09:03:00Z");

    let folded: Vec<bool> = thread.items.iter().map(ThreadItem::is_activity).collect();
    assert_eq!(
        folded,
        vec![false, true, false, false],
        "{:?}",
        thread.items
    );
}
