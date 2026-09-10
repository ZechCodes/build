use super::*;

fn conversation() -> Thread {
    let mut thread = Thread::new("run-search");
    thread.post_user(
        "Rename the helper in the parser",
        None,
        "2026-08-13T09:00:00Z",
    );
    thread.post_agent_with_links(
        "Renamed it; the parser now streams tokens.",
        None,
        vec![ThreadLink::File {
            path: "src/parser.rs".to_string(),
            line_start: None,
            line_end: None,
        }],
        "2026-08-13T09:01:00Z",
    );
    thread.push_event_with_links(
        ThreadEventKind::Committed,
        Some("committed the rename".to_string()),
        None,
        None,
        vec![ThreadLink::Commit {
            sha: "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678".to_string(),
        }],
        "2026-08-13T09:02:00Z",
    );
    thread.push_event_with_links(
        ThreadEventKind::StageStarted,
        Some("Started stage Lexer".to_string()),
        None,
        None,
        vec![ThreadLink::IssueStage {
            issue_id: "issue-1".to_string(),
            stage_id: "lexer".to_string(),
            path: ".build/plan/02-lexer.md".to_string(),
        }],
        "2026-08-13T09:03:00Z",
    );
    thread
}

fn sequences(hits: &[ConversationHit]) -> Vec<u64> {
    hits.iter().map(|hit| hit.sequence).collect()
}

#[test]
fn an_empty_query_returns_the_whole_conversation_newest_first() {
    let hits = conversation().search(&ConversationQuery::default());
    assert_eq!(sequences(&hits), vec![4, 3, 2, 1]);
    assert_eq!(hits[0].role, "event");
    assert_eq!(hits[0].kind.as_deref(), Some("stage_started"));
    assert_eq!(hits[0].created_at, "2026-08-13T09:03:00Z");
    assert_eq!(hits[3].role, "user");
    assert_eq!(hits[3].kind, None);
    assert_eq!(hits[3].excerpt, "Rename the helper in the parser");
    assert_eq!(hits[3].thread_id, "thread:run-search");
}

#[test]
fn text_matching_is_case_insensitive_substring() {
    // Substring, so "RENAME" also finds "Renamed" — a cold session asking
    // about a word should not have to guess which form was written.
    let hits = conversation().search(&ConversationQuery {
        text: Some("RENAME".to_string()),
        ..ConversationQuery::default()
    });
    assert_eq!(sequences(&hits), vec![3, 2, 1]);
    assert!(conversation()
        .search(&ConversationQuery {
            text: Some("lexer".to_string()),
            ..ConversationQuery::default()
        })
        .iter()
        .all(|hit| hit.sequence == 4));
}

#[test]
fn each_metadata_filter_narrows_to_the_items_carrying_it() {
    let thread = conversation();

    let by_file = thread.search(&ConversationQuery {
        file: Some("parser.rs".to_string()),
        ..ConversationQuery::default()
    });
    assert_eq!(sequences(&by_file), vec![2]);

    // A short sha finds the full one it prefixes, and the reverse.
    let by_short_commit = thread.search(&ConversationQuery {
        commit: Some("a1b2c3d".to_string()),
        ..ConversationQuery::default()
    });
    assert_eq!(sequences(&by_short_commit), vec![3]);
    let by_full_commit = thread.search(&ConversationQuery {
        commit: Some("a1b2c3d4e5f60718293a4b5c6d7e8f9012345678".to_string()),
        ..ConversationQuery::default()
    });
    assert_eq!(sequences(&by_full_commit), vec![3]);
    assert!(thread
        .search(&ConversationQuery {
            commit: Some("f".repeat(7),),
            ..ConversationQuery::default()
        })
        .is_empty());

    let by_stage = thread.search(&ConversationQuery {
        stage: Some("lexer".to_string()),
        ..ConversationQuery::default()
    });
    assert_eq!(sequences(&by_stage), vec![4]);
}

#[test]
fn role_selects_who_said_it_and_events_are_their_own_role() {
    let thread = conversation();
    for (role, expected) in [("user", vec![1]), ("agent", vec![2]), ("event", vec![4, 3])] {
        let hits = thread.search(&ConversationQuery {
            role: Some(role.to_string()),
            ..ConversationQuery::default()
        });
        assert_eq!(sequences(&hits), expected, "role={role}");
    }
}

#[test]
fn since_sequence_and_limit_bound_the_answer() {
    let thread = conversation();
    let since = thread.search(&ConversationQuery {
        since_sequence: Some(2),
        ..ConversationQuery::default()
    });
    assert_eq!(sequences(&since), vec![4, 3]);

    let limited = thread.search(&ConversationQuery {
        limit: 2,
        ..ConversationQuery::default()
    });
    assert_eq!(sequences(&limited), vec![4, 3]);
}

#[test]
fn filters_combine_rather_than_widen() {
    let hits = conversation().search(&ConversationQuery {
        text: Some("rename".to_string()),
        role: Some("user".to_string()),
        ..ConversationQuery::default()
    });
    assert_eq!(sequences(&hits), vec![1]);
}

/// A hit is a pointer, not a transcript: the excerpt is bounded and centred
/// on what was asked for, so a cold session can decide what to read next
/// without paying for the whole conversation.
#[test]
fn a_long_body_is_excerpted_around_the_match() {
    let mut thread = Thread::new("run-long");
    let body = format!(
        "{} the decisive sentence {}",
        "x ".repeat(400),
        "y ".repeat(400)
    );
    thread.post_agent(body, None, "2026-08-13T09:00:00Z");

    let hits = thread.search(&ConversationQuery {
        text: Some("decisive".to_string()),
        ..ConversationQuery::default()
    });
    assert_eq!(hits.len(), 1);
    let excerpt = &hits[0].excerpt;
    assert!(excerpt.contains("the decisive sentence"), "{excerpt}");
    assert!(
        excerpt.chars().count() <= 210,
        "{}",
        excerpt.chars().count()
    );
}

#[test]
fn a_hit_carries_the_metadata_that_made_it_findable() {
    let hits = conversation().search(&ConversationQuery {
        file: Some("src/parser.rs".to_string()),
        ..ConversationQuery::default()
    });
    assert_eq!(hits[0].metadata.files, vec!["src/parser.rs"]);
}
