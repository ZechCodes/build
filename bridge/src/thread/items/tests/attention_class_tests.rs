use super::*;

#[test]
fn every_event_kind_names_itself_the_way_it_serializes() {
    for kind in ThreadEventKind::ALL {
        assert_eq!(
            serde_json::to_value(kind).unwrap(),
            json!(kind.as_str()),
            "{kind:?}"
        );
    }
}

/// The split the whole inbox rests on: an agent handing back needs the
/// human, the work happening does not.
#[test]
fn handing_back_is_attention_and_working_is_status() {
    for kind in [
        ThreadEventKind::Done,
        ThreadEventKind::Blocked,
        ThreadEventKind::ReviewBlocked,
        ThreadEventKind::RunFailed,
        ThreadEventKind::StageFailed,
        ThreadEventKind::RecoveryFailed,
        ThreadEventKind::IdleUnreported,
        ThreadEventKind::Interrupted,
        ThreadEventKind::Merged,
        ThreadEventKind::Abandoned,
    ] {
        assert_eq!(kind.class(), EventClass::Attention, "{kind:?}");
    }
    for kind in [
        ThreadEventKind::RunStarted,
        ThreadEventKind::SessionStarted,
        ThreadEventKind::SessionEnded,
        ThreadEventKind::Committed,
        ThreadEventKind::Pushed,
        ThreadEventKind::RevisionCreated,
        ThreadEventKind::StageStarted,
        ThreadEventKind::StageCompleted,
        ThreadEventKind::StageApproved,
        ThreadEventKind::StageInvalidated,
        ThreadEventKind::ImplementationStarted,
        ThreadEventKind::WorktreeCreated,
        ThreadEventKind::RecoveryStarted,
    ] {
        assert_eq!(kind.class(), EventClass::Status, "{kind:?}");
    }
}

#[test]
fn an_agent_reply_needs_reading_and_its_own_words_never_do() {
    let mut thread = Thread::new("run-1");
    thread.post_user("please rename the helper", None, "2026-08-13T09:00:00Z");
    thread.post_agent_progress("still digging", None, "2026-08-13T09:01:00Z");
    thread.post_agent("renamed it, here is why", None, "2026-08-13T09:02:00Z");

    let reasons: Vec<Option<&str>> = thread
        .items
        .iter()
        .map(ThreadItem::attention_reason)
        .collect();
    assert_eq!(reasons, vec![None, None, Some(AGENT_MESSAGE_REASON)]);
}

#[test]
fn an_event_without_a_report_omits_the_field() {
    let mut thread = Thread::new("run-plain");
    thread.push_event(
        ThreadEventKind::RunStarted,
        None,
        None,
        None,
        "2026-08-13T09:01:00Z",
    );
    thread.push_event(
        ThreadEventKind::IdleUnreported,
        Some("Agent went quiet without reporting done".to_string()),
        None,
        None,
        "2026-08-13T09:02:00Z",
    );

    let wire = thread.wire_value();
    for index in 0..2 {
        assert!(
            wire["items"][index]["data"]
                .get("completion_report")
                .is_none(),
            "{wire:?}"
        );
    }
}

#[test]
fn unread_counts_only_attention_items_past_the_cursor() {
    let mut thread = Thread::new("run-1");
    thread.push_event(
        ThreadEventKind::RunStarted,
        None,
        None,
        None,
        "2026-08-13T09:00:00Z",
    );
    let cursor = thread.last_sequence();
    thread.push_event(
        ThreadEventKind::Committed,
        None,
        None,
        None,
        "2026-08-13T09:01:00Z",
    );
    thread.post_user("a note of my own", None, "2026-08-13T09:02:00Z");
    assert_eq!(thread.unread_since(cursor), UnreadSummary::default());
    assert!(!thread.unread_since(cursor).is_unread());

    thread.post_agent("here is the answer", None, "2026-08-13T09:03:00Z");
    thread.push_event(
        ThreadEventKind::Blocked,
        None,
        None,
        None,
        "2026-08-13T09:04:00Z",
    );
    let unread = thread.unread_since(cursor);
    assert_eq!(unread.count, 2);
    assert_eq!(unread.reason, Some("blocked"), "the newest one says why");
    assert!(unread.is_unread());

    // Reading through the whole conversation empties it.
    assert_eq!(
        thread.unread_since(thread.last_sequence()),
        UnreadSummary::default()
    );
}

/// The line a dismissal is measured against: where the conversation last
/// needed the human, and nowhere else. Progress the agent reports after
/// that must not move it, or a dismissed row would come back for work
/// happening quietly.
#[test]
fn the_attention_line_is_the_newest_item_that_needed_the_human() {
    let mut thread = Thread::new("run-1");
    assert_eq!(
        thread.last_attention_sequence(),
        0,
        "nothing has ever asked"
    );

    thread.push_event(
        ThreadEventKind::RunStarted,
        None,
        None,
        None,
        "2026-08-13T09:00:00Z",
    );
    assert_eq!(thread.last_attention_sequence(), 0, "status is not asking");

    thread.post_agent("here is the answer", None, "2026-08-13T09:01:00Z");
    let asked_at = thread.last_sequence();
    assert_eq!(thread.last_attention_sequence(), asked_at);

    thread.push_event(
        ThreadEventKind::Committed,
        None,
        None,
        None,
        "2026-08-13T09:02:00Z",
    );
    thread.post_user("carry on", None, "2026-08-13T09:03:00Z");
    assert_eq!(
        thread.last_attention_sequence(),
        asked_at,
        "work happening and the human talking are not the work asking"
    );

    thread.push_event(
        ThreadEventKind::Blocked,
        None,
        None,
        None,
        "2026-08-13T09:04:00Z",
    );
    assert_eq!(thread.last_attention_sequence(), thread.last_sequence());
}

/// A message marked seen bumps its `updated_sequence`; that is bookkeeping
/// for the cursored polls, not news, and must not resurrect an unread badge.
#[test]
fn marking_a_message_seen_does_not_make_the_entry_unread_again() {
    let mut thread = Thread::new("run-1");
    thread.post_agent("here is the answer", None, "2026-08-13T09:00:00Z");
    thread.post_user("thanks", None, "2026-08-13T09:01:00Z");
    let cursor = thread.last_sequence();
    thread.read_unread("2026-08-13T09:02:00Z");
    assert_eq!(thread.unread_since(cursor), UnreadSummary::default());
}

/// The inbox's two readings of a conversation: when it last did anything,
/// and when the user themselves last said something. Events and the agent's
/// own words count for the first and never for the second — the anchor rule
/// rests on the difference.
#[test]
fn a_conversation_reports_its_last_item_and_the_users_own_messages() {
    let mut thread = Thread::new("run-1");
    assert_eq!(thread.last_item_at(), None);
    assert_eq!(thread.user_message_times().count(), 0);

    thread.post_user("do the thing", None, "2026-08-13T09:00:00Z");
    thread.post_agent("on it", None, "2026-08-13T09:01:00Z");
    thread.post_user("and this too", None, "2026-08-14T22:00:00Z");
    thread.push_event(
        ThreadEventKind::Done,
        Some("finished".to_string()),
        None,
        None,
        "2026-08-14T22:05:00Z",
    );

    assert_eq!(thread.last_item_at(), Some("2026-08-14T22:05:00Z"));
    assert_eq!(
        thread.user_message_times().collect::<Vec<_>>(),
        vec!["2026-08-13T09:00:00Z", "2026-08-14T22:00:00Z"],
        "only what the user said, in the order they said it"
    );
}
