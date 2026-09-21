//! Agents have names (spec: Agents → Names).
//!
//! An agent was "Agent 1" in the rail, in every line it appeared in and in
//! every list of agents — an ordinal that says where it sits and nothing about
//! what it is. A name is one or two meaningful words, set when the agent is
//! made or chosen by the agent itself, and it is what all of those places say
//! instead.

use super::project_agent::workspace;
use super::tracker::{filed, tracked};
use super::*;
use crate::mcp::BridgeAction;

fn issue_id(issue: &Value) -> String {
    issue["id"].as_str().unwrap().to_string()
}

/// A workspace conversation with one agent on it, and the agent's own name for
/// itself left unset.
fn agent_on(state: &mut AppState, project_id: &str, workspace_name: &str) -> (String, String) {
    let workspace_id = workspace(state, project_id, workspace_name);
    let conversation = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace_id }),
    ));
    let entity_id = conversation["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let added = state.handle(req("agent.add", json!({ "entity_id": entity_id })));
    assert_eq!(added["ok"], true, "{added:?}");
    let agent_id = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    (entity_id, agent_id)
}

/// The agent as a client reads it off `agent.list`.
fn listed(state: &mut AppState, entity_id: &str, agent_id: &str) -> Value {
    let listed = state.handle(req("agent.list", json!({ "entity_id": entity_id })));
    listed["result"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["id"] == agent_id)
        .cloned()
        .unwrap_or_else(|| panic!("no such agent: {listed:?}"))
}

/// An agent names itself, and the name is on the wire where the ordinal was.
#[test]
fn an_agent_names_itself_and_every_reader_sees_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = agent_on(&mut state, &project_id, "here");

    assert!(
        listed(&mut state, &entity_id, &agent_id)["name"].is_null(),
        "unnamed until it says otherwise"
    );

    let answered = state
        .on_agent_mcp_action(
            &entity_id,
            &agent_id,
            BridgeAction::SetName {
                name: "Rail scroll".into(),
            },
        )
        .expect("an agent may name itself");
    assert_eq!(answered, json!({ "name": "Rail scroll" }));
    assert_eq!(
        listed(&mut state, &entity_id, &agent_id)["name"],
        "Rail scroll"
    );
    assert_eq!(
        listed(&mut state, &entity_id, &agent_id)["ordinal"],
        1,
        "the ordinal is still there for a client that wants it"
    );
}

/// Two agents on one conversation cannot wear one name: the name exists to
/// tell them apart, and two of them would be worse than the ordinals.
#[test]
fn a_name_another_agent_has_is_refused_in_a_sentence() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, first) = agent_on(&mut state, &project_id, "here");
    let added = state.handle(req("agent.add", json!({ "entity_id": entity_id })));
    let second = added["result"]["agent"]["id"].as_str().unwrap().to_string();

    state
        .on_agent_mcp_action(
            &entity_id,
            &first,
            BridgeAction::SetName {
                name: "Tracker".into(),
            },
        )
        .expect("the first may take it");
    let refused = state
        .on_agent_mcp_action(
            &entity_id,
            &second,
            // Case and spacing do not make it a different name.
            BridgeAction::SetName {
                name: "tracker".into(),
            },
        )
        .expect_err("the second may not");
    assert!(
        refused.contains("already called \"tracker\""),
        "says which name: {refused}"
    );
    assert!(refused.ends_with("Pick a different name."), "{refused}");
    assert!(
        listed(&mut state, &entity_id, &second)["name"].is_null(),
        "and nothing was written"
    );

    // An agent re-stating its own name is not a clash. It asked twice.
    state
        .on_agent_mcp_action(
            &entity_id,
            &first,
            BridgeAction::SetName {
                name: "Tracker".into(),
            },
        )
        .expect("its own name is its own");
}

/// A name may be given when the agent is made, which is the usual case: the
/// caller cutting an agent for a piece of work knows what that work is.
#[test]
fn an_agent_can_be_made_with_a_name() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let workspace_id = workspace(&mut state, &project_id, "here");
    let conversation = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace_id }),
    ));
    let entity_id = conversation["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();

    let added = state.handle(req(
        "agent.add",
        json!({ "entity_id": entity_id, "name": "  Transport  " }),
    ));
    assert_eq!(added["ok"], true, "{added:?}");
    assert_eq!(
        added["result"]["agent"]["name"], "Transport",
        "stored as it will be read: {added:?}"
    );

    // The same name twice is refused before anything is made.
    let refused = state.handle(req(
        "agent.add",
        json!({ "entity_id": entity_id, "name": "transport" }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("already called"),
        "{refused:?}"
    );

    // And a name that cannot fit is refused by the same words the tool uses.
    let long = state.handle(req(
        "agent.add",
        json!({ "entity_id": entity_id, "name": "one two three four" }),
    ));
    assert_eq!(long["ok"], false, "{long:?}");
    assert!(
        long["error"].as_str().unwrap().contains("one or two words"),
        "{long:?}"
    );
}

/// The first thing the user says to an unnamed agent carries the ask, and
/// nothing after it does.
#[test]
fn the_user_s_first_message_asks_an_unnamed_agent_to_name_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = agent_on(&mut state, &project_id, "here");

    // With an operation id, so the turn carries the native delivery payload —
    // which is where the envelope's asks are written.
    let mut operation = 0;
    let mut said = |state: &mut AppState, body: &str| {
        operation += 1;
        let posted = state.handle(req(
            "thread.post",
            json!({
                "entity_id": entity_id,
                "agent_id": agent_id,
                "body": body,
                "operation_id": format!("op-name-{operation}"),
            }),
        ));
        assert_eq!(posted["ok"], true, "{posted:?}");
        state
            .delivery_queue
            .take_ready(|_| false)
            .into_iter()
            .filter_map(|turn| turn.say.map(|say| say.warm))
            .collect::<Vec<_>>()
    };

    let first = said(&mut state, "have a look at the retry path");
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(first[0].contains("You have no name yet"), "{}", first[0]);
    assert!(first[0].contains("call set_name"), "{}", first[0]);
    assert!(
        first[0].contains("have a look at the retry path"),
        "and the user's own words are still what it answers: {}",
        first[0]
    );

    let second = said(&mut state, "and the rail too");
    assert_eq!(second.len(), 1, "{second:?}");
    assert!(
        !second[0].contains("You have no name yet"),
        "asked once: {}",
        second[0]
    );
}

/// An agent that HAS a name is never asked, and neither is one woken by
/// another agent rather than by the user.
#[test]
fn a_named_agent_and_an_agent_to_agent_hand_off_are_not_asked() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, named) = agent_on(&mut state, &project_id, "named");
    state
        .on_agent_mcp_action(
            &entity_id,
            &named,
            BridgeAction::SetName {
                name: "Tracker".into(),
            },
        )
        .expect("it names itself");
    state.delivery_queue.take_ready(|_| false);

    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": entity_id, "agent_id": named, "body": "carry on" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let turns = state.delivery_queue.take_ready(|_| false);
    for turn in &turns {
        let warm = turn.say.as_ref().map(|say| say.warm.as_str()).unwrap_or("");
        assert!(!warm.contains("You have no name yet"), "{warm}");
    }

    // An unnamed agent written to by ANOTHER agent is being given work, not
    // greeted. The ask waits for the user.
    let (other_entity, unnamed) = agent_on(&mut state, &project_id, "unnamed");
    state
        .on_agent_mcp_action(
            &entity_id,
            &named,
            BridgeAction::MessageAgent {
                agent_id: unnamed.clone(),
                body: "take the retry path".into(),
            },
        )
        .expect("an agent may write to a colleague");
    let handed = state.delivery_queue.take_ready(|_| false);
    for turn in &handed {
        let warm = turn.say.as_ref().map(|say| say.warm.as_str()).unwrap_or("");
        assert!(!warm.contains("You have no name yet"), "{warm}");
    }
    assert!(
        !other_entity.is_empty(),
        "the second conversation was made for this"
    );
}

/// A name rides every place an agent is named: the message it sent, and the
/// notice about what it did.
#[test]
fn the_name_travels_with_everything_the_agent_is_named_on() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = agent_on(&mut state, &project_id, "here");
    let (watcher_entity, watcher) = agent_on(&mut state, &project_id, "watching");
    state
        .on_agent_mcp_action(
            &entity_id,
            &agent_id,
            BridgeAction::SetName {
                name: "Rail scroll".into(),
            },
        )
        .expect("it names itself");

    // On a message it sends to a colleague.
    state
        .on_agent_mcp_action(
            &entity_id,
            &agent_id,
            BridgeAction::MessageAgent {
                agent_id: watcher.clone(),
                body: "take the retry path".into(),
            },
        )
        .expect("an agent may write to a colleague");
    let page = state.handle(req(
        "thread.page",
        json!({ "entity_id": watcher_entity, "agent_id": watcher, "limit": 20 }),
    ));
    let sent = page["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["data"]["from_agent"].as_object())
        .next()
        .cloned()
        .unwrap_or_else(|| panic!("no message wearing a sender: {page:?}"));
    assert_eq!(sent["name"], "Rail scroll", "{sent:?}");
    assert_eq!(sent["id"], agent_id.as_str(), "and the id is still there");

    // And on the notice about an issue it changed.
    let id = issue_id(&filed(&mut state, &project_id, "Kanban drag"));
    state.handle(req(
        "issues.track",
        json!({ "issue_id": id, "agent_id": watcher }),
    ));
    state
        .on_agent_mcp_action(
            &entity_id,
            &agent_id,
            BridgeAction::TrackerMoveIssue {
                issue_id: id.clone(),
                status: "in_review".into(),
                track: None,
            },
        )
        .expect("the actor moves it");
    let page = state.handle(req(
        "thread.page",
        json!({ "entity_id": watcher_entity, "agent_id": watcher, "limit": 20 }),
    ));
    let notice = page["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["data"].clone())
        .find(|data| data["issue_notice"].is_object())
        .unwrap_or_else(|| panic!("no notice: {page:?}"));
    assert_eq!(notice["issue_notice"]["actor"]["name"], "Rail scroll");
    assert_eq!(
        notice["issue_notice"]["actor"]["agent_id"],
        agent_id.as_str(),
        "beside the id, not instead of it"
    );
    assert_eq!(
        notice["body"], "#1 moved to In review by Rail scroll.",
        "and the one line a harness reads names it too"
    );
}
