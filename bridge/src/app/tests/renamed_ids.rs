//! #190: an id an agent or a client still holds from before issues were
//! renamed tasks finds the same task, comment and parent under the new
//! prefixes — through an agent's tools and through the client's verbs.

use super::tracker::tracked;
use super::tracker_tools::{call, coding_agent};
use super::*;
use crate::mcp::{BridgeAction, DoneServer};

/// The id as a v9 bridge minted it.
fn old(id: &str) -> String {
    [("task-", "issue-"), ("tc-", "ic-")]
        .iter()
        .find_map(|(new, old)| id.strip_prefix(new).map(|rest| format!("{old}{rest}")))
        .expect("a minted id")
}

fn filed(state: &mut AppState, who: &(String, String), title: &str) -> String {
    let created = call(
        state,
        who,
        BridgeAction::TrackerCreateTask {
            title: title.into(),
            body: None,
            status: None,
            labels: Vec::new(),
            priority: None,
            attachments: Vec::new(),
            track: None,
            notify_user: None,
            mention_user: None,
        },
    )
    .unwrap();
    created["task"]["id"].as_str().unwrap().to_string()
}

fn tool(who: &(String, String), name: &str, arguments: Value) -> BridgeAction {
    let frame = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    });
    DoneServer::new(&who.1)
        .handle_message(&frame.to_string())
        .action
        .unwrap_or_else(|| panic!("{name} emits an action"))
}

#[test]
fn an_old_issue_id_still_resolves_through_get_task() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");
    let task_id = filed(&mut state, &who, "Renamed");

    let action = tool(&who, "get_task", json!({ "task_id": old(&task_id) }));
    let read = call(&mut state, &who, action).unwrap();
    assert_eq!(read["task"]["id"], task_id);

    let answered = state.handle(req("tasks.get", json!({ "task_id": old(&task_id) })));
    assert_eq!(answered["result"]["task"]["id"], task_id, "{answered}");
}

#[test]
fn an_old_comment_id_still_reads_and_an_old_parent_still_links() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");
    let parent = filed(&mut state, &who, "Parent");
    let child = filed(&mut state, &who, "Child");

    let action = tool(
        &who,
        "comment_task",
        json!({ "task_id": old(&child), "body": "old id" }),
    );
    let commented = call(&mut state, &who, action).unwrap();
    let comment_id = commented["comment"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        commented["comment"]["task_id"], child,
        "stored under the new id"
    );

    let action = tool(
        &who,
        "read_comment",
        json!({ "comment_id": old(&comment_id) }),
    );
    let read = call(&mut state, &who, action).unwrap();
    assert_eq!(read["body"], "old id", "{read}");

    let action = tool(
        &who,
        "link_task",
        json!({ "task_id": old(&child), "parent_task_id": old(&parent) }),
    );
    let linked = call(&mut state, &who, action).unwrap();
    assert_eq!(
        linked["task"]["links"]["parent_task_id"], parent,
        "{linked}"
    );
}
