//! What the USER watches (spec: Tasks → Watching).
//!
//! Tracking is the agents' — `trackers`, and the notices a change delivers.
//! This is the person's: which tasks are in their inbox, how far they have
//! read one, and the row the inbox draws for it.

use super::project_agent::workspace;
use super::tracker::{filed, tracked};
use super::*;
use crate::mcp::BridgeAction;

fn task_id(task: &Value) -> String {
    task["id"].as_str().unwrap().to_string()
}

/// A coding agent on a workspace of this project, as `(entity_id, agent_id)`.
fn coding_agent(state: &mut AppState, project_id: &str, name: &str) -> (String, String) {
    let workspace_id = workspace(state, project_id, name);
    let conversation = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace_id }),
    ));
    let entity_id = conversation["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let added = state.handle(req("agent.add", json!({ "entity_id": entity_id })));
    let agent_id = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    (entity_id, agent_id)
}

fn task(state: &mut AppState, task_id: &str) -> Value {
    state.handle(req("tasks.get", json!({ "task_id": task_id })))["result"]["task"].clone()
}

fn timeline(state: &mut AppState, task_id: &str) -> Vec<Value> {
    state.handle(req("tasks.get", json!({ "task_id": task_id })))["result"]["timeline"]
        .as_array()
        .unwrap()
        .clone()
}

fn event_kinds(state: &mut AppState, task_id: &str) -> Vec<String> {
    timeline(state, task_id)
        .iter()
        .filter(|entry| entry["type"] == "event")
        .map(|entry| entry["kind"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The inbox, as the board answers it.
fn rows(state: &mut AppState) -> Vec<Value> {
    state.handle(req("board.list", json!({})))["result"]["items"]
        .as_array()
        .expect("the board answers items")
        .clone()
}

fn task_rows(state: &mut AppState) -> Vec<Value> {
    rows(state)
        .into_iter()
        .filter(|row| row["kind"] == "tracker_task")
        .collect()
}

fn row_for(state: &mut AppState, task_id: &str) -> Value {
    task_rows(state)
        .into_iter()
        .find(|row| row["task_id"] == json!(task_id))
        .unwrap_or_else(|| panic!("{task_id} has an inbox row"))
}

/// An agent files a task, with whatever it asked for.
fn agent_files(
    state: &mut AppState,
    who: &(String, String),
    title: &str,
    notify_user: Option<bool>,
) -> String {
    let filed = state
        .on_agent_mcp_action(
            &who.0,
            &who.1,
            BridgeAction::TrackerCreateTask {
                title: title.into(),
                body: None,
                status: None,
                labels: Vec::new(),
                priority: None,
                track: None,
                attachments: Vec::new(),
                notify_user,
                mention_user: None,
            },
        )
        .expect("an agent may file a task");
    filed["task"]["id"].as_str().unwrap().to_string()
}

/// The user files a task, so the user watches it: it is in the inbox before
/// anybody asks. Unwatching takes the row away and watching brings it back,
/// each saying so once on the timeline.
#[test]
fn a_task_the_user_filed_is_watched_and_unwatching_takes_the_row_away() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = task_id(&filed(&mut state, &project_id, "Kanban drag"));

    assert_eq!(task(&mut state, &id)["watched"], true, "the user filed it");
    assert_eq!(row_for(&mut state, &id)["number"], 1);

    let put_down = state.handle(req("tasks.unwatch", json!({ "task_id": id })));
    assert_eq!(put_down["ok"], true, "{put_down:?}");
    assert!(
        put_down["result"]["task"]["watched"].as_bool() != Some(true),
        "{put_down:?}"
    );
    assert!(
        task_rows(&mut state).is_empty(),
        "an unwatched task has no row"
    );

    let picked_up = state.handle(req("tasks.watch", json!({ "task_id": id })));
    assert_eq!(picked_up["ok"], true, "{picked_up:?}");
    assert_eq!(picked_up["result"]["task"]["watched"], true);
    assert_eq!(row_for(&mut state, &id)["task_id"], json!(id));
    assert_eq!(
        event_kinds(&mut state, &id),
        vec!["created", "unwatched", "watched"],
        "each change said once"
    );
}

/// Watching what is already watched is not a second fact.
#[test]
fn watching_twice_writes_nothing_the_second_time() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = task_id(&filed(&mut state, &project_id, "one"));

    let again = state.handle(req("tasks.watch", json!({ "task_id": id })));
    assert_eq!(again["ok"], true, "{again:?}");
    assert_eq!(again["result"]["task"]["watched"], true);
    assert_eq!(event_kinds(&mut state, &id), vec!["created"]);
}

/// Saying something on a task is caring about it, and so is being handed it.
#[test]
fn commenting_or_being_handed_a_task_starts_the_user_watching() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");

    let spoken = agent_files(&mut state, &who, "spoken on", Some(false));
    state.handle(req("tasks.unwatch", json!({ "task_id": spoken })));
    let said = state.handle(req(
        "tasks.comment",
        json!({ "task_id": spoken, "body": "which name did you want?" }),
    ));
    assert_eq!(said["ok"], true, "{said:?}");
    assert_eq!(said["result"]["task"]["watched"], true, "{said:?}");

    let handed = agent_files(&mut state, &who, "handed over", Some(false));
    state.handle(req("tasks.unwatch", json!({ "task_id": handed })));
    let assigned = state.handle(req(
        "tasks.assign",
        json!({ "task_id": handed, "assignee": { "kind": "user" } }),
    ));
    assert_eq!(assigned["ok"], true, "{assigned:?}");
    assert_eq!(assigned["result"]["task"]["watched"], true, "{assigned:?}");
    assert_eq!(row_for(&mut state, &handed)["assigned_to_user"], true);
}

/// An agent's task reaches the user because the device says so, or because
/// the agent asked — and reaches nobody when neither is true.
#[test]
fn an_agents_task_reaches_the_user_by_the_setting_or_by_asking() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");

    let by_default = agent_files(&mut state, &who, "by default", None);
    assert_ne!(task(&mut state, &by_default)["watched"], true);

    let enabled = state.handle(req(
        "settings.set",
        json!({ "watch_agent_filed_tasks": true }),
    ));
    assert_eq!(enabled["ok"], true, "{enabled:?}");
    assert_eq!(enabled["result"]["watch_agent_filed_tasks"], true);

    let watched = agent_files(&mut state, &who, "saved setting", None);
    assert_eq!(task(&mut state, &watched)["watched"], true);

    let quieted = state.handle(req(
        "settings.set",
        json!({ "watch_agent_filed_tasks": false }),
    ));
    assert_eq!(quieted["ok"], true, "{quieted:?}");

    let quiet = agent_files(&mut state, &who, "for another agent", None);
    assert!(
        task(&mut state, &quiet)["watched"].as_bool() != Some(true),
        "not the user's business until somebody says it is"
    );

    let asked = agent_files(&mut state, &who, "the user asked for this", Some(true));
    assert_eq!(
        task(&mut state, &asked)["watched"],
        true,
        "`notify_user` says so outright"
    );
}

/// `notify_user` on a write that touches a task that already exists puts
/// that task in front of the user; without it, nothing moves.
#[test]
fn notify_user_on_a_comment_or_an_assignment_puts_the_task_in_the_inbox() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");
    // With the device setting on, everything an agent files is watched
    // already; turn it off so what `notify_user` does is the only thing
    // moving.
    state.handle(req(
        "settings.set",
        json!({ "watch_agent_filed_tasks": false }),
    ));
    let quiet = agent_files(&mut state, &who, "quiet", None);
    let loud = agent_files(&mut state, &who, "loud", None);

    state
        .on_agent_mcp_action(
            &who.0,
            &who.1,
            BridgeAction::TrackerCommentTask {
                task_id: quiet.clone(),
                body: "noted".into(),
                refs: Vec::new(),
                track: None,
                attachments: Vec::new(),
                notify_user: None,
                mention_user: None,
            },
        )
        .expect("an agent comments");
    assert!(
        task(&mut state, &quiet)["watched"].as_bool() != Some(true),
        "a comment is not an ask"
    );

    let told = state
        .on_agent_mcp_action(
            &who.0,
            &who.1,
            BridgeAction::TrackerCommentTask {
                task_id: loud.clone(),
                body: "which name did you want?".into(),
                refs: Vec::new(),
                track: None,
                attachments: Vec::new(),
                notify_user: Some(true),
                mention_user: None,
            },
        )
        .expect("an agent asks the user");
    assert_eq!(told["task"]["watched"], true, "{told:?}");
    assert_eq!(row_for(&mut state, &loud)["number"], json!(2));
}

/// The read mark is how far the user has read, and it never moves backwards —
/// a late `read_through` for an older event leaves the newer one read.
#[test]
fn the_read_mark_never_moves_backwards_and_clears_the_count() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");
    let id = agent_files(&mut state, &who, "unread", Some(true));
    for body in ["one", "two"] {
        state
            .on_agent_mcp_action(
                &who.0,
                &who.1,
                BridgeAction::TrackerCommentTask {
                    task_id: id.clone(),
                    body: body.into(),
                    refs: Vec::new(),
                    track: None,
                    attachments: Vec::new(),
                    notify_user: None,
                    mention_user: None,
                },
            )
            .expect("an agent says something");
    }
    let entries = timeline(&mut state, &id);
    let oldest = entries.first().unwrap()["id"].as_str().unwrap().to_string();
    let newest = entries.last().unwrap()["id"].as_str().unwrap().to_string();
    assert_eq!(
        row_for(&mut state, &id)["unread"],
        json!(2),
        "everything said is unread until it is read; filing is not news (#183)"
    );

    let read = state.handle(req(
        "tasks.read_through",
        json!({ "task_id": id, "event_id": newest }),
    ));
    assert_eq!(read["ok"], true, "{read:?}");
    assert_eq!(read["result"]["task"]["read_through"], json!(newest));
    assert_eq!(row_for(&mut state, &id)["unread"], json!(0));

    let stale = state.handle(req(
        "tasks.read_through",
        json!({ "task_id": id, "event_id": oldest }),
    ));
    assert_eq!(stale["ok"], true, "{stale:?}");
    assert_eq!(
        stale["result"]["task"]["read_through"],
        json!(newest),
        "reading the top of the page again does not unread the bottom"
    );
    assert_eq!(row_for(&mut state, &id)["unread"], json!(0));
}

/// The count is what is asking for the user's attention, so it leaves out
/// what the user did themselves.
#[test]
fn the_unread_count_leaves_out_the_users_own_words() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");
    let id = agent_files(&mut state, &who, "mine", None);
    let entries = timeline(&mut state, &id);
    let newest = entries.last().unwrap()["id"].as_str().unwrap().to_string();
    state.handle(req(
        "tasks.read_through",
        json!({ "task_id": id, "event_id": newest }),
    ));

    state.handle(req(
        "tasks.comment",
        json!({ "task_id": id, "body": "do this one first" }),
    ));
    assert_eq!(
        row_for(&mut state, &id)["unread"],
        json!(0),
        "the user's own comment is not news to the user"
    );

    state
        .on_agent_mcp_action(
            &who.0,
            &who.1,
            BridgeAction::TrackerCommentTask {
                task_id: id.clone(),
                body: "on it".into(),
                refs: Vec::new(),
                track: None,
                attachments: Vec::new(),
                notify_user: None,
                mention_user: None,
            },
        )
        .expect("an agent answers");
    assert_eq!(row_for(&mut state, &id)["unread"], json!(1));
}

/// The row carries what the inbox draws, and its subtitle is the reader's
/// voice of the same line an agent would be sent — no comment ids and no tool
/// names in front of a person.
#[test]
fn an_inbox_row_says_what_last_happened_in_the_readers_voice() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");
    let id = agent_files(&mut state, &who, "Kanban drag does not persist", Some(true));
    state
        .on_agent_mcp_action(
            &who.0,
            &who.1,
            BridgeAction::TrackerMoveTask {
                task_id: id.clone(),
                status: "in_review".into(),
                track: None,
            },
        )
        .expect("an agent moves what it holds");

    let row = row_for(&mut state, &id);
    assert_eq!(row["kind"], "tracker_task", "{row:?}");
    assert_eq!(row["project_id"], json!(project_id), "{row:?}");
    assert_eq!(row["title"], "Kanban drag does not persist", "{row:?}");
    assert_eq!(row["status"], "in_review", "{row:?}");
    assert_eq!(row["muted"], false, "unwatch is the only mute here");
    assert_eq!(row["anchor"], row["last_activity"], "{row:?}");
    assert_eq!(row["last_event"]["at"], row["anchor"], "{row:?}");
    let said = row["last_event"]["text"].as_str().unwrap();
    assert!(said.starts_with("Moved to In review"), "{row:?}");
    assert!(
        !said.contains("read_comment") && !said.contains("tc-"),
        "a person is reading this: {row:?}"
    );
}

/// Task rows land in the same list as the conversations and are interleaved
/// with them by `anchor`, which is what the inbox reads — so a task that
/// moved after a conversation sits after it, and the client needs no special
/// case for the kind.
#[test]
fn task_rows_interleave_with_the_conversations_by_anchor() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let _conversation = coding_agent(&mut state, &project_id, "here");
    let older = task_id(&filed(&mut state, &project_id, "older"));
    let newer = task_id(&filed(&mut state, &project_id, "newer"));
    state.handle(req(
        "tasks.comment",
        json!({ "task_id": newer, "body": "and one more thing" }),
    ));

    let all = rows(&mut state);
    let anchors: Vec<&str> = all
        .iter()
        .filter_map(|row| row["anchor"].as_str())
        .collect();
    assert!(
        anchors.windows(2).all(|pair| pair[0] <= pair[1]),
        "one list, in anchor order: {anchors:?}"
    );
    assert!(
        all.iter().any(|row| row["kind"] == "branch"),
        "a conversation is in it too: {all:?}"
    );

    let tasks: Vec<String> = task_rows(&mut state)
        .iter()
        .map(|row| row["task_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        tasks,
        vec![older, newer],
        "each task is anchored to when it last moved"
    );
}

/// A conversation is in the inbox until the user puts it down, and the run
/// whose every agent has been put down is not a row at all.
#[test]
fn unwatching_every_agent_takes_the_conversation_off_the_board() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = coding_agent(&mut state, &project_id, "here");
    assert!(
        rows(&mut state)
            .iter()
            .any(|row| row["run_id"] == json!(entity_id)),
        "a conversation the user made is theirs"
    );

    let put_down = state.handle(req(
        "conversation.unwatch",
        json!({ "entity_id": entity_id, "agent_id": agent_id }),
    ));
    assert_eq!(put_down["ok"], true, "{put_down:?}");
    assert_eq!(put_down["result"]["watched"], false, "{put_down:?}");
    assert!(
        !rows(&mut state)
            .iter()
            .any(|row| row["run_id"] == json!(entity_id)),
        "nobody is watching it"
    );

    let picked_up = state.handle(req(
        "conversation.watch",
        json!({ "entity_id": entity_id, "agent_id": agent_id }),
    ));
    assert_eq!(picked_up["result"]["watched"], true, "{picked_up:?}");
    assert!(
        rows(&mut state)
            .iter()
            .any(|row| row["run_id"] == json!(entity_id)),
        "and back"
    );
}

/// An agent an agent made is its own business until it asks for the user.
#[test]
fn an_agent_made_agent_is_not_in_the_inbox_unless_it_asks() {
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

    let made = state.handle(req(
        "agent.add",
        json!({ "entity_id": entity_id, "made_by_agent": true }),
    ));
    assert_eq!(made["ok"], true, "{made:?}");
    assert!(
        !rows(&mut state)
            .iter()
            .any(|row| row["run_id"] == json!(entity_id)),
        "an agent's own helper is not the user's inbox"
    );

    let asked = state.handle(req(
        "agent.add",
        json!({ "entity_id": entity_id, "made_by_agent": true, "notify_user": true }),
    ));
    assert_eq!(asked["ok"], true, "{asked:?}");
    assert!(
        rows(&mut state)
            .iter()
            .any(|row| row["run_id"] == json!(entity_id)),
        "one that asks for the user is seen"
    );
}

/// `tasks.list` and `tasks.get` say how much of a watched task is unread
/// (#104), by the same count the task's inbox row carries: what the list holds is what the
/// Tasks tab and the rail badges read, and an `tasks` push re-reads it.
/// An unwatched task says nothing at all, not zero.
#[test]
fn the_list_carries_each_watched_tasks_unread_count() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");
    let id = agent_files(&mut state, &who, "watched", Some(true));
    let quiet = task_id(&filed(&mut state, &project_id, "put down"));
    state.handle(req("tasks.unwatch", json!({ "task_id": quiet })));
    let listed = |state: &mut AppState, task_id: &str| -> Value {
        let list = state.handle(req("tasks.list", json!({ "project_id": project_id })));
        list["result"]["tasks"]
            .as_array()
            .expect("the list answers tasks")
            .iter()
            .find(|task| task["id"] == json!(task_id))
            .cloned()
            .unwrap_or_else(|| panic!("{task_id} is listed"))
    };

    let unread = row_for(&mut state, &id)["unread"].clone();
    assert_eq!(
        unread,
        json!(0),
        "filing and watching are bookkeeping, not news (#183)"
    );
    assert_eq!(listed(&mut state, &id)["unread_count"], unread);
    assert!(
        listed(&mut state, &quiet).get("unread_count").is_none(),
        "an unwatched task carries no count"
    );

    let newest = timeline(&mut state, &id).last().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    state.handle(req(
        "tasks.read_through",
        json!({ "task_id": id, "event_id": newest }),
    ));
    assert_eq!(listed(&mut state, &id)["unread_count"], json!(0));

    state.handle(req(
        "tasks.comment",
        json!({ "task_id": id, "body": "my own words" }),
    ));
    assert_eq!(listed(&mut state, &id)["unread_count"], json!(0));

    state
        .on_agent_mcp_action(
            &who.0,
            &who.1,
            BridgeAction::TrackerCommentTask {
                task_id: id.clone(),
                body: "on it".into(),
                refs: Vec::new(),
                track: None,
                attachments: Vec::new(),
                notify_user: None,
                mention_user: None,
            },
        )
        .expect("an agent answers");
    assert_eq!(listed(&mut state, &id)["unread_count"], json!(1));
    assert_eq!(
        task(&mut state, &id)["unread_count"],
        json!(1),
        "reading the task alone says the same"
    );
    assert!(task(&mut state, &quiet).get("unread_count").is_none());
    let paged = state.handle(req(
        "tasks.list",
        json!({ "project_id": project_id, "limit": 10 }),
    ));
    let paged_row = paged["result"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == json!(id))
        .cloned()
        .unwrap();
    assert_eq!(paged_row["unread_count"], json!(1), "a page says it too");
}
