//! What an agent's words say about how full its context was (#68).
//!
//! A message an agent sends wears the author's last reading on `from_agent`,
//! and a comment it leaves wears it as `author_context` — both snapshotted at
//! write time and absent without a reading. The project agent, which decides
//! who takes the next issue, is also told it in a sentence: on the delivery of
//! a message, and in `read_comment`, because a comment reaches it as a notice
//! that carries only the comment's id.

use super::project_agent::{handed_over, items, sent_by, HandedOver};
use super::tracker::{filed, tracked};
use super::*;
use crate::harness::TurnContext;
use crate::mcp::BridgeAction;

const LINE: &str = "Rail scroll is at 190k of 200k (95%, compacts at 200k).";

/// A project with a project agent and one workspace agent it handed work to,
/// over a task store so issues can be filed and commented on.
fn staffed(state_root: &Path) -> (tempfile::TempDir, AppState, String, HandedOver) {
    let (home, mut state, project_id) = tracked(state_root);
    let handed = handed_over(&mut state, &project_id, "read the router");
    state.delivery_queue.take_ready(|_| false);
    (home, state, project_id, handed)
}

/// Name an agent, and give it a reading of 190k on a model whose window is
/// known, under the device's default threshold of 200k.
fn rail_scroll_at_190k(state: &mut AppState, entity_id: &str, agent_id: &str) {
    named_rail_scroll(state, entity_id, agent_id);
    state
        .set_agent_model_choice(
            entity_id,
            agent_id,
            ModelChoice {
                provider: AgentProvider::Claude,
                model: Some("claude-opus-5".to_string()),
                effort: None,
            },
        )
        .expect("the model is chosen");
    state.record_agent_turn_context(
        entity_id,
        agent_id,
        TurnContext {
            context_tokens: 190_000,
            cache_read_tokens: 0,
        },
    );
}

fn named_rail_scroll(state: &mut AppState, entity_id: &str, agent_id: &str) {
    state
        .on_agent_mcp_action(
            entity_id,
            agent_id,
            BridgeAction::SetName {
                name: "Rail scroll".to_string(),
            },
        )
        .expect("an agent names itself");
}

/// What the turns queued for one agent will say to it, warm.
fn prompts_to(state: &mut AppState, agent_id: &str) -> Vec<String> {
    state
        .delivery_queue
        .take_ready(|_| false)
        .into_iter()
        .filter(|turn| turn.agent_id == agent_id)
        .filter_map(|turn| turn.say.map(|say| say.warm))
        .collect()
}

fn worker_messages_the_project_agent(state: &mut AppState, handed: &HandedOver, body: &str) {
    state
        .agent_action(
            &handed.entity_id,
            &handed.worker,
            BridgeAction::MessageAgent {
                agent_id: handed.agent_id.clone(),
                body: body.to_string(),
            },
        )
        .expect("a workspace agent answers the project agent");
}

#[test]
fn a_message_wears_its_authors_context_and_the_project_agent_is_told_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, _project_id, handed) = staffed(&state_root);
    rail_scroll_at_190k(&mut state, &handed.entity_id, &handed.worker);

    worker_messages_the_project_agent(&mut state, &handed, "the router reads top to bottom");

    let sent = sent_by(&mut state, &handed.owner, &handed.agent_id, &handed.worker);
    let context = &sent[0]["data"]["from_agent"]["context"];
    assert_eq!(context["tokens"], 190_000, "{:?}", sent[0]);
    assert_eq!(context["window"], 1_000_000, "{:?}", sent[0]);
    assert_eq!(context["compact_at"], 200_000, "{:?}", sent[0]);
    assert!(context["at"].as_str().is_some(), "{:?}", sent[0]);

    let prompts = prompts_to(&mut state, &handed.agent_id);
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert!(prompts[0].contains(LINE), "{}", prompts[0]);
}

#[test]
fn without_a_reading_a_message_wears_no_context_and_says_none() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, _project_id, handed) = staffed(&state_root);
    named_rail_scroll(&mut state, &handed.entity_id, &handed.worker);

    worker_messages_the_project_agent(&mut state, &handed, "the router reads top to bottom");

    let sent = sent_by(&mut state, &handed.owner, &handed.agent_id, &handed.worker);
    assert!(
        sent[0]["data"]["from_agent"].get("context").is_none(),
        "{:?}",
        sent[0]
    );
    let prompts = prompts_to(&mut state, &handed.agent_id);
    assert!(!prompts[0].contains("Rail scroll is at"), "{}", prompts[0]);
}

#[test]
fn only_the_project_agent_is_told_the_line() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, _project_id, handed) = staffed(&state_root);
    rail_scroll_at_190k(&mut state, &handed.owner, &handed.agent_id);

    state
        .agent_action(
            &handed.owner,
            &handed.agent_id,
            BridgeAction::MessageAgent {
                agent_id: handed.worker.clone(),
                body: "and then the rail".to_string(),
            },
        )
        .expect("the project agent hands more work over");

    let prompts = prompts_to(&mut state, &handed.worker);
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert!(!prompts[0].contains("is at 190k"), "{}", prompts[0]);
}

fn worker_comments(state: &mut AppState, handed: &HandedOver, issue_id: &str) -> String {
    let commented = state
        .on_agent_mcp_action(
            &handed.entity_id,
            &handed.worker,
            BridgeAction::TrackerCommentIssue {
                issue_id: issue_id.to_string(),
                body: "Reproduced it.".into(),
                refs: Vec::new(),
                track: None,
                attachments: Vec::new(),
                notify_user: None,
                mention_user: None,
            },
        )
        .expect("an agent comments");
    commented["comment"]["id"].as_str().unwrap().to_string()
}

fn timeline_comment(state: &mut AppState, issue_id: &str) -> Value {
    let got = state.handle(req("issues.get", json!({ "issue_id": issue_id })));
    got["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["type"] == "comment")
        .cloned()
        .unwrap_or_else(|| panic!("a comment on the timeline: {got:?}"))
}

#[test]
fn a_comment_wears_its_authors_context_and_read_comment_says_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id, handed) = staffed(&state_root);
    rail_scroll_at_190k(&mut state, &handed.entity_id, &handed.worker);
    let issue_id = filed(&mut state, &project_id, "rail")["id"]
        .as_str()
        .unwrap()
        .to_string();
    state
        .on_agent_mcp_action(
            &handed.owner,
            &handed.agent_id,
            BridgeAction::TrackerTrackIssue {
                issue_id: issue_id.clone(),
            },
        )
        .expect("the project agent watches the issue");

    let comment_id = worker_comments(&mut state, &handed, &issue_id);

    let comment = timeline_comment(&mut state, &issue_id);
    assert_eq!(comment["author_context"]["tokens"], 190_000, "{comment:?}");
    assert_eq!(
        comment["author_context"]["window"], 1_000_000,
        "{comment:?}"
    );
    assert_eq!(
        comment["author_context"]["compact_at"], 200_000,
        "{comment:?}"
    );

    let read = state
        .on_agent_mcp_action(
            &handed.owner,
            &handed.agent_id,
            BridgeAction::TrackerReadComment { comment_id },
        )
        .expect("the project agent reads the comment");
    assert_eq!(read["author_context_line"], LINE, "{read:?}");

    let notices: Vec<Value> = items(&mut state, &handed.owner, &handed.agent_id)
        .into_iter()
        .filter(|item| {
            item["data"]["body"]
                .as_str()
                .is_some_and(|body| body.contains("New comment"))
        })
        .collect();
    assert_eq!(notices.len(), 1, "the project agent was told: {notices:?}");
    let notice = notices[0]["data"]["body"].as_str().unwrap();
    assert!(
        !notice.contains("is at"),
        "a notice stays one line: {notice}"
    );
}

#[test]
fn without_a_reading_a_comment_wears_no_context() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id, handed) = staffed(&state_root);
    let issue_id = filed(&mut state, &project_id, "rail")["id"]
        .as_str()
        .unwrap()
        .to_string();

    let comment_id = worker_comments(&mut state, &handed, &issue_id);

    let comment = timeline_comment(&mut state, &issue_id);
    assert!(comment.get("author_context").is_none(), "{comment:?}");
    let read = state
        .on_agent_mcp_action(
            &handed.owner,
            &handed.agent_id,
            BridgeAction::TrackerReadComment { comment_id },
        )
        .expect("the project agent reads the comment");
    assert!(read.get("author_context_line").is_none(), "{read:?}");
}
