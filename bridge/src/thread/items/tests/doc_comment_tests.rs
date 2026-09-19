use super::*;

const NOW: &str = "2026-08-13T09:00:00Z";

const STAGE_PATH: &str = ".build/plan/01-database-schema.md";

fn passage() -> DocAnchor {
    DocAnchor {
        heading_path: vec!["Database schema".to_string(), "Tables".to_string()],
        snippet: "users table gets a soft-delete column".to_string(),
        line_start: Some(12),
        line_end: Some(14),
    }
}

fn commented_stage() -> Thread {
    let mut thread = Thread::for_agent("agent-1");
    thread.post_doc_comment(
        "issue-1",
        "database-schema",
        STAGE_PATH,
        Some(passage()),
        "use a deleted_at timestamp, not a boolean",
        NOW,
    );
    thread
}

/// A plan-doc comment is a post like any other: an anchored user message on
/// the conversation, not a record beside it.
#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: a_doc_comment_is_an_anchored_post_on_the_conversation is at 19, threshold 15 — bring it under, then remove
fn a_doc_comment_is_an_anchored_post_on_the_conversation() {
    let thread = commented_stage();
    assert_eq!(thread.items.len(), 1, "{:?}", thread.items);
    let ThreadItem::Message(message) = &thread.items[0] else {
        panic!("a comment is a message: {:?}", thread.items);
    };
    assert_eq!(message.role, MessageRole::User);
    let anchor = message.anchor.as_ref().expect("an anchored comment");
    assert_eq!(anchor.artifact, ArtifactKind::Doc);
    assert_eq!(anchor.path.as_deref(), Some(STAGE_PATH));
    assert_eq!(anchor.line_start, Some(12));
    assert_eq!(anchor.line_end, Some(14));
    assert_eq!(anchor.snippet, passage().snippet);
    assert!(message.links.contains(&ThreadLink::IssueStage {
        issue_id: "issue-1".to_string(),
        stage_id: "database-schema".to_string(),
        path: STAGE_PATH.to_string(),
    }));
    // Posting it derives the same findability as any other item.
    assert_eq!(message.metadata.stages, vec!["database-schema".to_string()]);
    assert_eq!(message.metadata.files, vec![STAGE_PATH.to_string()]);

    let comments = thread.doc_comments();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].id, message.id);
    assert_eq!(comments[0].stage_id, "database-schema");
    assert_eq!(comments[0].path, STAGE_PATH);
    assert_eq!(
        comments[0].body,
        "use a deleted_at timestamp, not a boolean"
    );
    assert_eq!(comments[0].state, DocCommentState::Open);
    assert_eq!(comments[0].anchor.as_ref(), Some(&passage()));
    assert_eq!(comments[0].created_at, NOW);
}

/// A comment on the stage as a whole anchors to the document, not a passage
/// — and it is still that stage's comment.
#[test]
fn a_general_comment_points_at_the_document_rather_than_a_passage() {
    let mut thread = Thread::for_agent("agent-1");
    thread.post_doc_comment(
        "issue-1",
        "database-schema",
        STAGE_PATH,
        None,
        "this stage is too big",
        NOW,
    );
    let comments = thread.doc_comments();
    assert_eq!(comments.len(), 1);
    assert!(comments[0].anchor.is_none(), "{:?}", comments[0]);
    assert_eq!(comments[0].path, STAGE_PATH);
    assert_eq!(
        thread.open_doc_comments_for("database-schema").len(),
        1,
        "a general comment is still open work on the stage"
    );
}

/// Ordinary conversation is not a comment, and one stage's comments are not
/// another's.
#[test]
fn open_comments_are_scoped_to_their_stage() {
    let mut thread = commented_stage();
    thread.post_doc_comment(
        "issue-1",
        "api-surface",
        ".build/plan/02-api-surface.md",
        None,
        "name the endpoint after the resource",
        NOW,
    );
    thread.post_user("unrelated direction", None, NOW);
    thread.post_agent("and an answer", None, NOW);

    assert_eq!(thread.doc_comments().len(), 2);
    let open = thread.open_doc_comments_for("api-surface");
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(open[0].stage_id, "api-surface");
}

/// Deleting a comment deletes the post: there is nowhere else it lives.
#[test]
fn removing_an_open_comment_takes_the_post_with_it() {
    let mut thread = commented_stage();
    let id = thread.doc_comments()[0].id.clone();

    let removed = thread.remove_doc_comment(&id).expect("an open comment");
    assert_eq!(removed.id, id);
    assert!(thread.items.is_empty(), "{:?}", thread.items);
    assert!(thread.remove_doc_comment(&id).is_none(), "already gone");

    let mut thread = commented_stage();
    let id = thread.doc_comments()[0].id.clone();
    // A comment answered before replies were retired still reads as answered.
    let ThreadItem::Message(message) = &mut thread.items[0] else {
        panic!("a comment is a message");
    };
    message.agent_reply = Some("done".into());
    assert_eq!(
        thread.doc_comments()[0].state,
        DocCommentState::Addressed,
        "stored agent_reply data still reads as addressed"
    );
    assert!(
        thread.remove_doc_comment(&id).is_none(),
        "an answered comment is history, not a draft"
    );
}

#[test]
fn a_doc_anchor_says_doc_on_the_wire() {
    assert_eq!(ArtifactKind::Doc.as_str(), "doc");
    assert_eq!(
        serde_json::to_value(ArtifactKind::Doc).unwrap(),
        json!("doc")
    );
    let thread = commented_stage();
    let wire = thread.wire_value();
    assert_eq!(wire["items"][0]["data"]["anchor"]["artifact"], "doc");
    assert_eq!(wire["items"][0]["data"]["anchor"]["line_start"], 12);
    let reloaded: Thread = serde_json::from_value(json!({
        "id": thread.id,
        "agent": thread.agent,
        "items": thread.items,
    }))
    .expect("a conversation carrying a doc comment reloads");
    assert_eq!(reloaded.doc_comments(), thread.doc_comments());
}

/// Every message written before doc comments existed loads, and none of
/// them pays for the reply field.
#[test]
fn a_message_written_before_replies_existed_carries_none() {
    let mut thread = Thread::for_agent("agent-1");
    thread.post_user("plain direction", None, NOW);
    let wire = thread.wire_value();
    assert!(
        wire["items"][0]["data"].get("agent_reply").is_none(),
        "{wire:?}"
    );
    assert!(thread.doc_comments().is_empty());
}
