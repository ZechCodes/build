//! Who is watching a task, and what one agent is on (spec: Tasks →
//! Tracking).

use super::project_agent::{added_project, workspace};
use super::tracker::{filed, tracked};
use super::*;
use crate::mcp::{DoneReport, DoneStatus};

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

fn trackers(state: &mut AppState, task_id: &str) -> Vec<String> {
    state.handle(req("tasks.get", json!({ "task_id": task_id })))["result"]["task"]["trackers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_string())
        .collect()
}

fn event_kinds(state: &mut AppState, task_id: &str) -> Vec<String> {
    state.handle(req("tasks.get", json!({ "task_id": task_id })))["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["type"] == "event")
        .map(|entry| entry["kind"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Tracking and untracking move the list and say so on the timeline.
#[test]
fn tracking_adds_the_agent_and_untracking_takes_it_off() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (_entity, watcher) = coding_agent(&mut state, &project_id, "here");
    let id = task_id(&filed(&mut state, &project_id, "one"));

    assert_eq!(
        trackers(&mut state, &id),
        Vec::<String>::new(),
        "nobody yet"
    );

    let tracking = set_task_tracking(&mut state, &id, &watcher, true);
    assert_eq!(tracking["ok"], true, "{tracking:?}");
    assert_eq!(tracking["result"]["task"]["trackers"], json!([watcher]));
    assert_eq!(event_kinds(&mut state, &id), vec!["created", "tracked"]);

    let untracking = set_task_tracking(&mut state, &id, &watcher, false);
    assert_eq!(untracking["ok"], true, "{untracking:?}");
    assert_eq!(untracking["result"]["task"]["trackers"], json!([]));
    assert_eq!(
        event_kinds(&mut state, &id),
        vec!["created", "tracked", "untracked"]
    );
}

/// Saying it twice is not a second fact: the list and the timeline both stay
/// where they were.
#[test]
fn tracking_twice_and_untracking_nothing_write_no_event() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (_entity, watcher) = coding_agent(&mut state, &project_id, "here");
    let id = task_id(&filed(&mut state, &project_id, "one"));

    // Untracking one that was never tracked changes nothing.
    let never = set_task_tracking(&mut state, &id, &watcher, false);
    assert_eq!(never["ok"], true, "a no-op is legible, not a refusal");
    assert_eq!(event_kinds(&mut state, &id), vec!["created"]);

    set_task_tracking(&mut state, &id, &watcher, true);
    set_task_tracking(&mut state, &id, &watcher, true);
    assert_eq!(trackers(&mut state, &id), vec![watcher], "one entry");
    assert_eq!(
        event_kinds(&mut state, &id),
        vec!["created", "tracked"],
        "and one event"
    );
}

/// The agent a task is assigned to is subscribed by the assignment, and the
/// event says the tracking was a consequence rather than a request.
#[test]
fn assignment_tracks_the_agent_it_hands_the_work_to() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ws = workspace(&mut state, &project_id, "here");
    let id = task_id(&filed(&mut state, &project_id, "hand this over"));

    let handed = state.handle(req(
        "tasks.assign",
        json!({ "task_id": id, "assignee": { "kind": "new_agent", "workspace_id": ws } }),
    ));
    assert_eq!(handed["ok"], true, "{handed:?}");
    let assignee = handed["result"]["dispatch"]["agent_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        handed["result"]["task"]["trackers"],
        json!([assignee]),
        "the agent that gets the work hears about the task"
    );

    let tracked_event = state.handle(req("tasks.get", json!({ "task_id": id })))["result"]
        ["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == "tracked")
        .cloned()
        .expect("a tracked event");
    assert_eq!(tracked_event["payload"]["agent_id"], assignee.as_str());
    assert_eq!(
        tracked_event["payload"]["by"], "assignment",
        "a consequence, not a request"
    );

    // And unassigning leaves it watching: handing work on is exactly when the
    // previous holder still wants to know how it went.
    let unassigned = state.handle(req(
        "tasks.assign",
        json!({ "task_id": id, "assignee": Value::Null }),
    ));
    assert_eq!(unassigned["ok"], true, "{unassigned:?}");
    assert_eq!(
        unassigned["result"]["task"]["trackers"],
        json!([assignee]),
        "unassigning does not untrack"
    );
}

// ------------------------------------------------------------- notices ---

/// The notices sitting on one agent's conversation, newest last.
fn notices(state: &mut AppState, entity_id: &str, agent_id: &str) -> Vec<Value> {
    let page = state.handle(req(
        "thread.page",
        json!({ "entity_id": entity_id, "agent_id": agent_id, "limit": 50 }),
    ));
    page["result"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item["type"] == "message")
        .map(|item| item["data"].clone())
        .filter(|message| message["from_build"] == true)
        .collect()
}

/// A change reaches every tracker and never the agent that made it.
fn assert_moved_notice_identity(notice: &Value, agent_id: &str) {
    assert_eq!(notice["task_notice"]["actor"]["agent_id"], agent_id);
    assert_eq!(notice["task_notice"]["action"], "moved");
    assert_eq!(notice["task_notice"]["from"], "backlog");
    assert_eq!(notice["task_notice"]["to"], "in_review");
    let identity = &notice["task_notice"]["actor"]["identity"];
    assert_eq!(identity["agent_id"], agent_id);
    assert_eq!(identity["workspace_name"], "actor");
    assert_eq!(identity["provider"], "claude_adk");
    assert_eq!(identity["available"], true);
}

#[test]
fn a_change_reaches_every_tracker_but_the_agent_that_made_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let actor = coding_agent(&mut state, &project_id, "actor");
    let watcher = coding_agent(&mut state, &project_id, "watcher");
    let id = task_id(&filed(&mut state, &project_id, "Kanban drag"));

    for who in [&actor, &watcher] {
        set_task_tracking(&mut state, &id, &who.1, true);
    }

    // The actor moves it through its own tool, so the bridge knows who acted.
    state
        .on_agent_mcp_action(
            &actor.0,
            &actor.1,
            crate::mcp::BridgeAction::TrackerMoveTask {
                task_id: id.clone(),
                status: "in_review".into(),
                track: None,
            },
        )
        .expect("the actor moves its task");

    let told = notices(&mut state, &watcher.0, &watcher.1);
    assert_eq!(told.len(), 1, "one notice per change: {told:?}");
    let notice = &told[0];
    assert_eq!(notice["from_build"], true, "Build's own words");
    assert_eq!(notice["role"], "user", "an instruction arrives inbound");
    assert_eq!(notice["from_task"]["task_id"], id.as_str());
    assert_eq!(notice["from_task"]["number"], 1);
    assert_eq!(notice["from_task"]["title"], "Kanban drag");
    let body = notice["body"].as_str().unwrap();
    assert_eq!(
        body, "#1 moved to In review by the actor agent.",
        "one line, no title: the number is what a task is called"
    );

    // And the same thing structured, so a client draws that line with a link
    // rather than parsing it back out of prose (spec: Tasks → Tracking).
    assert_moved_notice_identity(notice, &actor.1);

    assert!(
        notices(&mut state, &actor.0, &actor.1).is_empty(),
        "nobody is told what they just did"
    );
}

#[test]
fn a_notice_names_an_actor_who_was_not_tracking_the_task() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let actor = coding_agent(&mut state, &project_id, "author workspace");
    let watcher = coding_agent(&mut state, &project_id, "reader workspace");
    let id = task_id(&filed(&mut state, &project_id, "one"));
    set_task_tracking(&mut state, &id, &watcher.1, true);
    state
        .on_agent_mcp_action(
            &actor.0,
            &actor.1,
            crate::mcp::BridgeAction::TrackerMoveTask {
                task_id: id,
                status: "in_review".into(),
                track: None,
            },
        )
        .expect("author moves the task");

    let told = notices(&mut state, &watcher.0, &watcher.1);
    let identity = &told[0]["task_notice"]["actor"]["identity"];
    assert_eq!(identity["agent_id"], actor.1);
    assert_eq!(identity["workspace_name"], "author workspace");
    assert_eq!(identity["provider"], "claude_adk");
}

#[test]
fn an_assignment_notice_names_an_unwatched_target_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let target = coding_agent(&mut state, &project_id, "target workspace");
    let watcher = coding_agent(&mut state, &project_id, "reader workspace");
    let id = task_id(&filed(&mut state, &project_id, "one"));
    let tracked = set_task_tracking(&mut state, &id, &watcher.1, true);
    assert_eq!(tracked["ok"], true, "{tracked:?}");
    let assigned = state.handle(req(
        "tasks.assign",
        json!({ "task_id": id, "assignee": { "kind": "agent", "agent_id": target.1 } }),
    ));
    assert_eq!(assigned["ok"], true, "{assigned:?}");

    let told = notices(&mut state, &watcher.0, &watcher.1);
    let identity = &told[0]["task_notice"]["assignee_identity"];
    assert_eq!(identity["agent_id"], target.1);
    assert_eq!(identity["workspace_name"], "target workspace");
    assert_eq!(identity["provider"], "claude_adk");
    assert_eq!(identity["ordinal"], 1);
}

/// A comment's body does NOT ride the notice (#61).
///
/// A notice is a notification: an agent watching a task for a column move
/// used to pay for every comment anybody wrote on it. Now it pays for a line
/// naming the comment, and reads the words with `read_comment` if it decides
/// it cares.
#[test]
fn a_comment_notice_names_the_comment_and_carries_none_of_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let watcher = coding_agent(&mut state, &project_id, "watcher");
    let id = task_id(&filed(&mut state, &project_id, "one"));
    set_task_tracking(&mut state, &id, &watcher.1, true);

    state.handle(req(
        "tasks.comment",
        json!({ "task_id": id, "body": "The drop handler races the column read." }),
    ));

    let told = notices(&mut state, &watcher.0, &watcher.1);
    assert_eq!(told.len(), 1, "{told:?}");
    let body = told[0]["body"].as_str().unwrap();
    assert!(body.starts_with("New comment tc-"), "{body}");
    assert!(body.contains("on #1 from the user"), "{body}");
    assert!(body.ends_with("read_comment for their message."), "{body}");
    assert!(
        !body.contains("The drop handler races the column read."),
        "the comment's words are not in the notice: {body}"
    );
    assert_eq!(body.lines().count(), 1, "one line: {body}");

    // And the id on the line is the one `read_comment` answers to.
    let named = body
        .split_whitespace()
        .nth(2)
        .expect("the line names the comment");
    assert_eq!(told[0]["task_notice"]["comment_id"], named, "{told:?}");
}

/// The notice starts the tracker's turn, so an idle agent wakes to it.
#[test]
fn a_notice_starts_the_tracking_agents_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let watcher = coding_agent(&mut state, &project_id, "watcher");
    let id = task_id(&filed(&mut state, &project_id, "one"));
    set_task_tracking(&mut state, &id, &watcher.1, true);
    // Nothing queued yet: the tracking call itself woke nobody.
    assert!(state.delivery_queue.take_ready(|_| false).is_empty());

    state.handle(req(
        "tasks.update",
        json!({ "task_id": id, "status": "ready" }),
    ));

    // It waits out the settle window first (#67), so a burst wakes it once.
    state.delivery_queue.lapse_settle_windows();
    let queued = state.delivery_queue.take_ready(|_| false);
    assert_eq!(queued.len(), 1, "one turn for one change");
    let turn = &queued[0];
    assert_eq!(turn.agent_id, watcher.1, "the tracker's turn, not anyone's");
    assert_eq!(turn.owner, watcher.0);
    assert_eq!(turn.phase, "task_notice");
    assert!(
        turn.says_something(),
        "it tells the agent to go and read, which is what starts an idle one"
    );
    assert!(
        !turn.interrupt,
        "a notice does not cut a turn in flight short"
    );
}

/// A watcher tracking `title`, as `(watcher, task_id)`, with nothing queued.
fn watching(state: &mut AppState, project_id: &str, title: &str) -> ((String, String), String) {
    let watcher = coding_agent(state, project_id, "watcher");
    let id = task_id(&filed(state, project_id, title));
    set_task_tracking(state, &id, &watcher.1, true);
    state.delivery_queue.take_ready(|_| false);
    (watcher, id)
}

fn move_to(state: &mut AppState, task_id: &str, status: &str) {
    state.handle(req(
        "tasks.update",
        json!({ "task_id": task_id, "status": status }),
    ));
}

/// The lines the agent's next catch-up turn hands it, read off its thread.
fn unread_lines(state: &AppState, entity_id: &str, agent_id: &str) -> Vec<String> {
    state
        .legacy_delivery_payload(entity_id, agent_id)
        .expect("the thread reads")
        .map(|payload| payload.messages)
        .unwrap_or_default()
        .into_iter()
        .map(|message| message.body)
        .collect()
}

/// Every turn re-reads the agent's whole context, so a burst of changes wakes
/// a tracker once (#67): each line lands on the thread when it happens, and
/// one turn after the settle window carries all of them.
#[test]
fn changes_inside_the_settle_window_wake_the_tracker_once() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (watcher, id) = watching(&mut state, &project_id, "one");

    move_to(&mut state, &id, "ready");
    move_to(&mut state, &id, "in_progress");
    state.handle(req(
        "tasks.comment",
        json!({ "task_id": id, "body": "Half done." }),
    ));

    let told = notices(&mut state, &watcher.0, &watcher.1);
    assert_eq!(told.len(), 3, "the thread shows every notice: {told:?}");
    assert!(
        state.delivery_queue.take_ready(|_| false).is_empty(),
        "nothing wakes it inside the window"
    );

    state.delivery_queue.lapse_settle_windows();
    let woken = state.delivery_queue.take_ready(|_| false);
    assert_eq!(woken.len(), 1, "three notices, one turn");
    assert_eq!(woken[0].phase, "task_notice");
    assert!(woken[0].reads_unread_thread());
    let carried = unread_lines(&state, &watcher.0, &watcher.1);
    for notice in &told {
        let line = notice["body"].as_str().unwrap();
        assert!(
            carried.iter().any(|body| body == line),
            "{line} in {carried:?}"
        );
    }
}

/// A change after the turn went is news again, with a window of its own.
#[test]
fn a_change_after_the_notice_turn_went_wakes_the_tracker_again() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (_watcher, id) = watching(&mut state, &project_id, "one");

    move_to(&mut state, &id, "ready");
    state.delivery_queue.lapse_settle_windows();
    assert_eq!(state.delivery_queue.take_ready(|_| false).len(), 1);

    move_to(&mut state, &id, "in_progress");
    assert!(state.delivery_queue.take_ready(|_| false).is_empty());
    state.delivery_queue.lapse_settle_windows();
    assert_eq!(state.delivery_queue.take_ready(|_| false).len(), 1);
}

/// Nothing else would drain the queue when an idle tracker's window ends, so
/// the drain that leaves a notice settling asks a timer to come back for it —
/// once, however many drains pass before it fires.
#[tokio::test]
async fn a_drain_leaving_a_notice_settling_asks_for_a_wake() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (_watcher, id) = watching(&mut state, &project_id, "one");
    move_to(&mut state, &id, "ready");

    let state = state.shared();
    let clock = Arc::clone(&state.lock().unwrap().frame_clock);
    DeliveryRunner::drain(&state, &clock.frame("test"));
    assert_eq!(
        state.lock().unwrap().delivery_queue.settle_wake_due(),
        None,
        "the drain already asked for the window's end"
    );
}

/// The user speaking to a tracker does not wait on its settling notice, and
/// the notice goes with it rather than waking the agent later on its own.
#[test]
fn a_message_from_the_user_goes_now_and_takes_the_settling_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (watcher, id) = watching(&mut state, &project_id, "one");
    move_to(&mut state, &id, "ready");

    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": watcher.0, "agent_id": watcher.1, "body": "status?" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");

    let woken = state.delivery_queue.take_ready(|_| false);
    assert_eq!(woken.len(), 1, "one turn carries both: {:?}", woken.len());
    assert_eq!(woken[0].phase, "revive", "the user's turn, sent now");
    let carried = unread_lines(&state, &watcher.0, &watcher.1);
    assert!(carried.iter().any(|body| body == "status?"), "{carried:?}");
    assert!(
        carried.iter().any(|body| body.starts_with("#1 moved")),
        "{carried:?}"
    );

    state.delivery_queue.lapse_settle_windows();
    assert!(
        state.delivery_queue.take_ready(|_| false).is_empty(),
        "no second wake when the window would have ended"
    );
}

/// An assignment is work handed over: it goes at once, whatever is settling.
#[test]
fn an_assignment_goes_now_and_takes_the_settling_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (watcher, id) = watching(&mut state, &project_id, "one");
    move_to(&mut state, &id, "ready");

    let work = task_id(&filed(&mut state, &project_id, "two"));
    let handed = state.handle(req(
        "tasks.assign",
        json!({ "task_id": work, "assignee": { "kind": "agent", "agent_id": watcher.1 } }),
    ));
    assert_eq!(handed["ok"], true, "{handed:?}");

    let woken = state.delivery_queue.take_ready(|_| false);
    assert!(
        woken.iter().any(|turn| turn.operation_id.is_some()),
        "the assignment is not held by the window"
    );
    assert!(
        woken.iter().any(|turn| turn.phase == "task_notice"),
        "the settling notice goes with it, not thirty seconds later"
    );
    state.delivery_queue.lapse_settle_windows();
    assert!(state.delivery_queue.take_ready(|_| false).is_empty());
}

/// A write that changes nothing delivers nothing: it is not news, for the same
/// reason it writes no event.
#[test]
fn a_change_that_changes_nothing_wakes_nobody() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let watcher = coding_agent(&mut state, &project_id, "watcher");
    let id = task_id(&filed(&mut state, &project_id, "one"));
    set_task_tracking(&mut state, &id, &watcher.1, true);

    // It is already in Backlog.
    state.handle(req(
        "tasks.update",
        json!({ "task_id": id, "status": "backlog" }),
    ));
    assert!(
        notices(&mut state, &watcher.0, &watcher.1).is_empty(),
        "moving a card where it already is is not news"
    );

    // And somebody else starting to watch is not a change to the task.
    let other = coding_agent(&mut state, &project_id, "other");
    set_task_tracking(&mut state, &id, &other.1, true);
    assert!(
        notices(&mut state, &watcher.0, &watcher.1).is_empty(),
        "who else is watching is not a change to the task"
    );
}

// --------------------------------------------------------------- tools ---

/// The tool subscribes the CALLER. There is no agent id on it to get wrong,
/// and an agent cannot decide what another agent is woken for.
#[test]
fn the_track_tool_subscribes_whoever_called_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let caller = coding_agent(&mut state, &project_id, "caller");
    let id = task_id(&filed(&mut state, &project_id, "one"));

    let tracked_by_tool = state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerTrackTask {
                task_id: id.clone(),
            },
        )
        .expect("an agent tracks a task of its own project");
    assert_eq!(
        tracked_by_tool["task"]["trackers"],
        json!([caller.1]),
        "the caller, and nobody it might have named"
    );

    state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerUntrackTask {
                task_id: id.clone(),
            },
        )
        .expect("and stops");
    assert_eq!(trackers(&mut state, &id), Vec::<String>::new());
}

/// A task of another project is unknown to the tool, the way every other
/// tracker tool reads it.
#[test]
fn the_track_tool_refuses_a_task_outside_the_callers_project() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let caller = coding_agent(&mut state, &project_id, "caller");

    let elsewhere = tempfile::tempdir().unwrap();
    let other_repo = init_repo_named(elsewhere.path(), "other");
    let other_repo = std::fs::canonicalize(&other_repo).unwrap();
    let other_project = added_project(&mut state, &other_repo);
    let theirs = task_id(&filed(&mut state, &other_project, "not yours"));

    let refused = state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerTrackTask {
                task_id: theirs.clone(),
            },
        )
        .expect_err("another project's task");
    assert!(refused.contains("unknown task_id"), "{refused}");
    assert_eq!(trackers(&mut state, &theirs), Vec::<String>::new());
}

/// The prompt tells an agent the tool exists and the two things it would
/// otherwise get wrong: assignment tracks for you, and unassignment does not
/// untrack.
#[test]
fn the_prompt_says_what_tracking_does_and_does_not_do() {
    let templates = crate::templates::Templates::default();
    // Collapsed, so the assertions read the wording rather than the wrapping.
    let flat = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    for (name, text) in [
        ("build", &templates.build),
        ("plan", &templates.plan),
        ("project_agent", &templates.project_agent),
    ] {
        let text = flat(text);
        assert!(text.contains("`track_task`"), "{name} does not offer it");
        assert!(
            text.contains("tracked automatically on anything assigned to you"),
            "{name} does not say assignment tracks for you"
        );
        assert!(
            text.contains("being unassigned does not"),
            "{name} does not say unassignment leaves you watching"
        );
    }
}

// ------------------------------------------------- the Complete reminder ---

/// A reminder is a `from_build` message naming what is still open.
fn reminders(state: &mut AppState, entity_id: &str, agent_id: &str) -> Vec<String> {
    notices(state, entity_id, agent_id)
        .into_iter()
        .map(|message| message["body"].as_str().unwrap_or_default().to_string())
        .filter(|body| body.starts_with("You reported Complete"))
        .collect()
}

#[test]
fn internal_wake_delivers_new_task_notices_after_clear() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut state, project_id) = tracked(tmp.path());
    let (owner, watcher) = coding_agent(&mut state, &project_id, "watcher");
    let task = task_id(&filed(&mut state, &project_id, "Still tracked"));
    assert_eq!(
        set_task_tracking(&mut state, &task, &watcher, true)["ok"],
        true
    );
    super::resume::clear_conversation(&mut state, &owner, &watcher);
    let moved = state.handle(req(
        "tasks.update",
        json!({"task_id": task, "status": "in_review"}),
    ));
    assert_eq!(moved["ok"], true, "{moved:?}");
    let told = notices(&mut state, &owner, &watcher);
    assert_eq!(
        told.len(),
        1,
        "a new change reaches its retained tracker: {told:?}"
    );
    assert_eq!(told[0]["from_task"]["task_id"], task);
    assert!(
        state.delivery_queue.settle_wake_due().is_some(),
        "the notice queues a wake"
    );
}

#[test]
fn internal_wake_delivers_open_task_reminders_after_clear() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut state, project_id) = tracked(tmp.path());
    let (owner, agent) = reported(
        &mut state,
        &project_id,
        DoneStatus::Completed,
        &[("unfinished", "in_progress"), ("finished", "in_progress")],
    );
    super::resume::clear_conversation(&mut state, &owner, &agent);
    state.remind_of_open_tasks(&owner, &agent);
    let told = reminders(&mut state, &owner, &agent);
    assert_eq!(
        told.len(),
        1,
        "retained assignments can still remind the fresh agent: {told:?}"
    );
    assert!(told[0].contains("unfinished"));
}

/// An agent, holding `titles`, that has just reported `status`.
fn reported(
    state: &mut AppState,
    project_id: &str,
    status: DoneStatus,
    holding: &[(&str, &str)],
) -> (String, String) {
    let ws = workspace(state, project_id, "here");
    let first = task_id(&filed(state, project_id, holding[0].0));
    let handed = state.handle(req(
        "tasks.assign",
        json!({ "task_id": first, "assignee": { "kind": "new_agent", "workspace_id": ws } }),
    ));
    let dispatch = &handed["result"]["dispatch"];
    let entity_id = dispatch["entity_id"].as_str().unwrap().to_string();
    let agent_id = dispatch["agent_id"].as_str().unwrap().to_string();
    state.handle(req(
        "tasks.update",
        json!({ "task_id": first, "status": holding[0].1 }),
    ));
    for (title, column) in &holding[1..] {
        let id = task_id(&filed(state, project_id, title));
        state.handle(req(
            "tasks.assign",
            json!({ "task_id": id, "assignee": { "kind": "agent", "agent_id": agent_id } }),
        ));
        state.handle(req(
            "tasks.update",
            json!({ "task_id": id, "status": column }),
        ));
    }
    state.done_deferring_for_agent(
        &entity_id,
        &agent_id,
        DoneReport {
            status,
            summary: "Did the thing.".into(),
            message_id: None,
        },
    );
    (entity_id, agent_id)
}

/// Complete with open assigned tasks delivers the list, and names every one.
#[test]
fn complete_with_open_tasks_lists_every_one_of_them() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = reported(
        &mut state,
        &project_id,
        DoneStatus::Completed,
        // The report moves the NEWEST task the agent holds to In review, so
        // the last one listed is the one this Complete hands over.
        &[
            ("first", "in_progress"),
            ("second", "ready"),
            ("just finished", "in_progress"),
        ],
    );

    let told = reminders(&mut state, &entity_id, &agent_id);
    assert_eq!(told.len(), 1, "one reminder per Complete: {told:?}");
    let body = &told[0];
    assert!(body.contains("#1 first (In progress)"), "{body}");
    assert!(body.contains("#2 second (Ready)"), "{body}");
    // The case that made every Complete nag about the work it had just handed
    // over: the report's own move put #3 in In review, and In review is the
    // agent saying it is finished.
    assert!(
        !body.contains("#3 just finished"),
        "In review is not held open: {body}"
    );
    assert!(
        body.contains("2 Build tasks assigned to you are still open"),
        "{body}"
    );
}

/// The reminder does not start a turn of its own (#67): the agent just ended
/// one, and waking it again only to read a list costs its whole context. The
/// list waits on the thread and rides the agent's next delivery.
#[test]
fn the_reminder_rides_the_agents_next_delivery() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = reported(
        &mut state,
        &project_id,
        DoneStatus::Completed,
        &[("first", "in_progress"), ("just finished", "in_progress")],
    );
    assert_eq!(reminders(&mut state, &entity_id, &agent_id).len(), 1);

    state.delivery_queue.lapse_settle_windows();
    let woken = state.delivery_queue.take_ready(|_| false);
    assert!(
        woken.iter().all(|turn| turn.phase != "task_reminder"),
        "the reminder wakes nobody: {:?}",
        woken.iter().map(|turn| turn.phase).collect::<Vec<_>>()
    );

    state.handle(req(
        "thread.post",
        json!({ "entity_id": entity_id, "agent_id": agent_id, "body": "one more thing" }),
    ));
    let next = state.delivery_queue.take_ready(|_| false);
    assert_eq!(next.len(), 1, "one delivery");
    let carried = unread_lines(&state, &entity_id, &agent_id);
    assert!(
        carried
            .iter()
            .any(|body| body.starts_with("You reported Complete")),
        "the reminder rides it: {carried:?}"
    );
    assert!(state.delivery_queue.take_ready(|_| false).is_empty());
}

/// In review means the agent has said the work is ready to be looked at, and
/// deciding it is done is somebody else's. So it is not held, and an agent
/// that reports Complete holding nothing else hears nothing at all.
#[test]
fn a_task_in_review_is_not_held_open_by_the_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = reported(
        &mut state,
        &project_id,
        DoneStatus::Completed,
        &[("handed over", "in_review")],
    );
    assert!(
        reminders(&mut state, &entity_id, &agent_id).is_empty(),
        "nothing is waiting on the agent, so nothing is said"
    );
}

/// Blocked says nothing: the agent has already told us it cannot finish.
#[test]
fn blocked_delivers_no_reminder() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = reported(
        &mut state,
        &project_id,
        DoneStatus::Blocked,
        &[("first", "in_progress")],
    );
    assert!(reminders(&mut state, &entity_id, &agent_id).is_empty());
}

/// An agent holding nothing open hears nothing.
#[test]
fn complete_holding_nothing_open_delivers_no_reminder() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ws = workspace(&mut state, &project_id, "here");
    let conversation = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": ws }),
    ));
    let entity_id = conversation["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let added = state.handle(req("agent.add", json!({ "entity_id": entity_id })));
    let agent_id = added["result"]["agent"]["id"].as_str().unwrap().to_string();

    state.done_deferring_for_agent(
        &entity_id,
        &agent_id,
        DoneReport {
            status: DoneStatus::Completed,
            summary: "Nothing assigned to me.".into(),
            message_id: None,
        },
    );
    assert!(reminders(&mut state, &entity_id, &agent_id).is_empty());
}

/// A task parked in Done is one the agent is finished with, so it is not
/// named — a reminder that is noise is one an agent answers without reading.
#[test]
fn a_task_in_the_done_column_is_not_reminded_about() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = reported(
        &mut state,
        &project_id,
        DoneStatus::Completed,
        &[
            ("finished", "done"),
            ("still going", "in_progress"),
            ("just finished", "in_progress"),
        ],
    );

    let told = reminders(&mut state, &entity_id, &agent_id);
    assert_eq!(told.len(), 1, "{told:?}");
    let body = &told[0];
    assert!(body.contains("#2 still going"), "{body}");
    assert!(
        !body.contains("#1 finished"),
        "Done is finished with: {body}"
    );
    assert!(
        !body.contains("#3 just finished"),
        "and so is In review: {body}"
    );
    assert!(
        body.contains("1 Build task assigned to you is still open"),
        "{body}"
    );
}

/// Answering a reminder with another Complete while holding the SAME tasks
/// says nothing the second time.
///
/// The reminder is delivered as a turn, so an agent that answers it reports
/// Complete again — which is another reminder, which is another answer. That
/// loop ran five times on #27 before the agent stopped replying. The nudge is
/// worth sending when the set changes and worth nothing when it has not.
#[test]
fn a_second_complete_holding_the_same_tasks_says_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = reported(
        &mut state,
        &project_id,
        DoneStatus::Completed,
        &[("second", "in_progress"), ("first", "in_progress")],
    );
    assert_eq!(reminders(&mut state, &entity_id, &agent_id).len(), 1);

    state.done_deferring_for_agent(
        &entity_id,
        &agent_id,
        DoneReport {
            status: DoneStatus::Completed,
            summary: "Still done.".into(),
            message_id: None,
        },
    );
    assert_eq!(
        reminders(&mut state, &entity_id, &agent_id).len(),
        1,
        "the same set is not worth saying twice"
    );
}

/// But a set that CHANGED is news again: an agent handed a second task after
/// being reminded about the first is told about both.
#[test]
fn a_complete_holding_a_different_set_reminds_again() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = reported(
        &mut state,
        &project_id,
        DoneStatus::Completed,
        &[
            ("a", "in_progress"),
            ("b", "in_progress"),
            ("just finished", "in_progress"),
        ],
    );
    assert_eq!(reminders(&mut state, &entity_id, &agent_id).len(), 1);

    // One of the two it was told about is finished with, so the set has moved.
    let listed = state.handle(req("tasks.list", json!({ "project_id": project_id })));
    let done = listed["result"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["title"] == "b")
        .and_then(|task| task["id"].as_str())
        .unwrap()
        .to_string();
    state.handle(req(
        "tasks.update",
        json!({ "task_id": done, "status": "done" }),
    ));

    state.done_deferring_for_agent(
        &entity_id,
        &agent_id,
        DoneReport {
            status: DoneStatus::Completed,
            summary: "And that one is finished with.".into(),
            message_id: None,
        },
    );
    let told = reminders(&mut state, &entity_id, &agent_id);
    assert_eq!(told.len(), 2, "the set moved, so it is said again");
    assert!(told[1].contains("#1 a"), "{told:?}");
    assert!(!told[1].contains("#2 b"), "{told:?}");
}

// ------------------------------------------ the agent says what it did ---

/// The `task_action` messages on one agent's own conversation.
fn said(state: &mut AppState, entity_id: &str, agent_id: &str) -> Vec<Value> {
    let page = state.handle(req(
        "thread.page",
        json!({ "entity_id": entity_id, "agent_id": agent_id, "limit": 50 }),
    ));
    page["result"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item["type"] == "message")
        .map(|item| item["data"].clone())
        .filter(|message| message["task_action"].is_object())
        .collect()
}

/// Every tool an agent writes with posts exactly one message saying what it
/// did, authored by the agent and not marked as Build's.
#[test]
fn each_tool_posts_one_message_saying_what_the_agent_did() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");

    let filed_by_agent = state
        .on_agent_mcp_action(
            &who.0,
            &who.1,
            crate::mcp::BridgeAction::TrackerCreateTask {
                title: "Kanban drag".into(),
                body: None,
                status: None,
                labels: Vec::new(),
                priority: None,
                track: None,
                attachments: Vec::new(),
                notify_user: None,
                mention_user: None,
            },
        )
        .expect("an agent files a task");
    let id = filed_by_agent["task"]["id"].as_str().unwrap().to_string();

    for (action, expected) in [
        (
            crate::mcp::BridgeAction::TrackerMoveTask {
                task_id: id.clone(),
                status: "in_review".into(),
                track: None,
            },
            "moved",
        ),
        (
            crate::mcp::BridgeAction::TrackerCommentTask {
                task_id: id.clone(),
                body: "Reproduced it.".into(),
                refs: Vec::new(),
                track: None,
                attachments: Vec::new(),
                notify_user: None,
                mention_user: None,
            },
            "commented_on",
        ),
        (
            crate::mcp::BridgeAction::TrackerCloseTask {
                task_id: id.clone(),
                reason: None,
                track: None,
            },
            "closed",
        ),
    ] {
        state
            .on_agent_mcp_action(&who.0, &who.1, action)
            .unwrap_or_else(|why| panic!("{expected}: {why}"));
    }

    let messages = said(&mut state, &who.0, &who.1);
    let actions: Vec<&str> = messages
        .iter()
        .map(|message| message["task_action"]["action"].as_str().unwrap())
        .collect();
    assert_eq!(
        actions,
        vec!["created", "moved", "commented_on", "closed"],
        "one message per write, in the order they happened"
    );
    assert_eq!(messages[1]["task_action"]["to"], "in_review");

    let first = &messages[0];
    assert_eq!(first["role"], "agent", "the agent's own words");
    assert!(
        first["from_build"] != true,
        "not Build's sentence: {first:?}"
    );
    assert_eq!(first["task_action"]["task_id"], id.as_str());
    assert_eq!(first["task_action"]["number"], 1);
    assert_eq!(first["task_action"]["title"], "Kanban drag");
    assert_eq!(first["body"], "Created #1 Kanban drag");
}

/// A comment's message carries the id that deep-links the comment itself.
#[test]
fn a_comment_message_carries_its_comment_id() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");
    let id = task_id(&filed(&mut state, &project_id, "one"));

    let commented = state
        .on_agent_mcp_action(
            &who.0,
            &who.1,
            crate::mcp::BridgeAction::TrackerCommentTask {
                task_id: id.clone(),
                body: "Reproduced it.".into(),
                refs: Vec::new(),
                track: None,
                attachments: Vec::new(),
                notify_user: None,
                mention_user: None,
            },
        )
        .expect("an agent comments");
    let comment_id = commented["comment"]["id"].as_str().unwrap().to_string();

    let messages = said(&mut state, &who.0, &who.1);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(
        messages[0]["task_action"]["comment_id"],
        comment_id.as_str(),
        "the id that links the comment rather than the task"
    );
    assert_eq!(messages[0]["body"], "Commented on #1 one");
}

/// The api path posts nothing: a human moving a card is already looking at
/// the board.
#[test]
fn the_api_path_posts_no_action_message() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");
    let id = task_id(&filed(&mut state, &project_id, "one"));

    state.handle(req(
        "tasks.update",
        json!({ "task_id": id, "status": "in_review" }),
    ));
    state.handle(req(
        "tasks.comment",
        json!({ "task_id": id, "body": "from the board" }),
    ));

    assert!(
        said(&mut state, &who.0, &who.1).is_empty(),
        "the human is already looking at the board"
    );
}

/// The message is in the ACTING agent's conversation, not the assignee's.
#[test]
fn the_message_lands_on_the_actor_and_not_on_the_assignee() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let actor = coding_agent(&mut state, &project_id, "actor");
    let target = coding_agent(&mut state, &project_id, "target");
    let id = task_id(&filed(&mut state, &project_id, "hand this over"));

    state
        .on_agent_mcp_action(
            &actor.0,
            &actor.1,
            crate::mcp::BridgeAction::TrackerAssignTask {
                task_id: id.clone(),
                assignee: json!({ "kind": "agent", "agent_id": target.1 }),
                note: None,
                track: None,
                notify_user: None,
            },
        )
        .expect("an agent hands work over");

    let by_actor = said(&mut state, &actor.0, &actor.1);
    assert_eq!(by_actor.len(), 1, "{by_actor:?}");
    assert_eq!(by_actor[0]["task_action"]["action"], "assigned");
    assert_eq!(by_actor[0]["body"], "Assigned #1 hand this over");

    assert!(
        said(&mut state, &target.0, &target.1).is_empty(),
        "the assignee gets the TASK, not a note about somebody assigning it"
    );
}

// ------------------------------------------- track in the same call ---

/// A task an agent FILES tracks it, without being asked.
///
/// The default that matters (spec: Tasks → Tracking). An agent that files a
/// task almost always wants to know how it goes, and the one that filed and
/// assigned twelve in an afternoon heard nothing about any of them.
#[test]
fn a_task_an_agent_files_tracks_it_unless_it_says_otherwise() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let caller = coding_agent(&mut state, &project_id, "filer");

    let filed = state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerCreateTask {
                title: "Kanban drag".into(),
                body: None,
                status: None,
                labels: Vec::new(),
                priority: None,
                track: None,
                attachments: Vec::new(),
                notify_user: None,
                mention_user: None,
            },
        )
        .expect("an agent may file a task");
    let id = filed["task"]["id"].as_str().unwrap().to_string();
    assert_eq!(trackers(&mut state, &id), vec![caller.1.clone()]);
    assert_eq!(
        filed["task"]["trackers"],
        json!([caller.1]),
        "and the answer says so, rather than making the caller read it back"
    );

    // Said otherwise, it does not.
    let quiet = state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerCreateTask {
                title: "not my problem".into(),
                body: None,
                status: None,
                labels: Vec::new(),
                priority: None,
                track: Some(false),
                attachments: Vec::new(),
                notify_user: None,
                mention_user: None,
            },
        )
        .expect("an agent may file one it does not want to hear about");
    let quiet_id = quiet["task"]["id"].as_str().unwrap().to_string();
    assert_eq!(trackers(&mut state, &quiet_id), Vec::<String>::new());
}

/// Every other write takes `track: true` and defaults to false: an agent that
/// moves somebody else's card in passing has not asked to follow it.
#[test]
fn any_other_write_can_start_tracking_in_the_same_call() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let caller = coding_agent(&mut state, &project_id, "caller");
    let id = task_id(&filed(&mut state, &project_id, "Kanban drag"));

    // Without the flag, nothing is followed.
    state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerMoveTask {
                task_id: id.clone(),
                status: "ready".into(),
                track: None,
            },
        )
        .expect("a move");
    assert_eq!(trackers(&mut state, &id), Vec::<String>::new());

    // With it, once — and the answer carries the trackers as they now stand.
    let moved = state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerMoveTask {
                task_id: id.clone(),
                status: "in_progress".into(),
                track: Some(true),
            },
        )
        .expect("a move that follows");
    assert_eq!(moved["task"]["trackers"], json!([caller.1]));
    assert_eq!(trackers(&mut state, &id), vec![caller.1.clone()]);

    // Idempotent, the way track_task is: asking twice is not two trackers,
    // and writes no second `tracked` event.
    let before = event_kinds(&mut state, &id).len();
    state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerCommentTask {
                task_id: id.clone(),
                body: "still on it".into(),
                refs: Vec::new(),
                track: Some(true),
                attachments: Vec::new(),
                notify_user: None,
                mention_user: None,
            },
        )
        .expect("a comment that follows");
    assert_eq!(trackers(&mut state, &id), vec![caller.1.clone()]);
    assert_eq!(
        event_kinds(&mut state, &id).len(),
        before,
        "a set does not record being told twice"
    );
}

/// The api path is a human on the board, who is not an agent and cannot be
/// put on a task's trackers. It carries no such field: asking for one is
/// refused by name, and nothing is filed.
#[test]
fn the_api_path_tracks_nobody() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let refused = state.handle(req(
        "tasks.create",
        json!({ "project_id": project_id, "title": "filed by the user", "track": true }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(refused["error"], "unknown param: track", "{refused:?}");
    let listed = state.handle(req("tasks.list", json!({ "project_id": project_id })));
    assert_eq!(listed["result"]["tasks"], json!([]), "{listed:?}");

    let made = state.handle(req(
        "tasks.create",
        json!({ "project_id": project_id, "title": "filed by the user" }),
    ));
    assert_eq!(made["ok"], true, "{made:?}");
    assert_eq!(
        made["result"]["task"]["trackers"],
        json!([]),
        "the user is not a tracker"
    );
}

/// An agent's own line says who it handed the task TO, not just that it
/// assigned it.
#[test]
fn an_assignment_the_agent_made_records_who_got_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ws = workspace(&mut state, &project_id, "here");
    let caller = coding_agent(&mut state, &project_id, "caller");
    let id = task_id(&filed(&mut state, &project_id, "Kanban drag"));

    // Handed to a new agent on an existing workspace: a creating kind, which
    // resolves to the agent it makes.
    state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerAssignTask {
                task_id: id.clone(),
                assignee: json!({ "kind": "new_agent", "workspace_id": ws }),
                note: None,
                track: None,
                notify_user: None,
            },
        )
        .expect("an agent may assign");

    let lines = said(&mut state, &caller.0, &caller.1);
    let assigned = lines
        .iter()
        .find(|line| line["task_action"]["action"] == "assigned")
        .unwrap_or_else(|| panic!("no assignment line: {lines:?}"));
    let got = assigned["task_action"]["assignee"].clone();
    assert_eq!(got["kind"], "agent", "{assigned:?}");
    assert!(
        got["agent_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("agent-")),
        "the agent the dispatch made, not the kind that asked for it: {assigned:?}"
    );

    // Handing it back is its own word, and goes to nobody.
    state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerAssignTask {
                task_id: id.clone(),
                assignee: Value::Null,
                note: None,
                track: None,
                notify_user: None,
            },
        )
        .expect("an agent may hand it back");
    let lines = said(&mut state, &caller.0, &caller.1);
    let handed_back = lines
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("no line: {lines:?}"));
    assert_eq!(handed_back["task_action"]["action"], "unassigned");
    assert!(
        handed_back["task_action"]["assignee"].is_null(),
        "nobody has it: {handed_back:?}"
    );
    assert_eq!(handed_back["body"], "Unassigned #1 Kanban drag");
}

/// `read_comment` is what a notice does not carry: the words, on request.
#[test]
fn read_comment_answers_the_one_comment_a_notice_named() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let caller = coding_agent(&mut state, &project_id, "caller");
    let id = task_id(&filed(&mut state, &project_id, "Kanban drag"));

    let said = state.handle(req(
        "tasks.comment",
        json!({ "task_id": id, "body": "The drop handler races the column read." }),
    ));
    assert_eq!(said["ok"], true, "{said:?}");
    let comment_id = said["result"]["comment"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let read = state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerReadComment {
                comment_id: comment_id.clone(),
            },
        )
        .expect("an agent may read a comment of its own project");
    assert_eq!(read["comment_id"], comment_id.as_str());
    assert_eq!(read["body"], "The drop handler races the column read.");
    assert_eq!(read["author"], json!({ "kind": "user" }));
    assert!(read["created_at"].is_string(), "{read:?}");
    // Enough of the task to answer about it: the notice gave a number.
    assert_eq!(read["number"], 1);
    assert_eq!(read["title"], "Kanban drag");
    assert_eq!(read["task_id"], id.as_str());

    // An id nobody answers to is refused in a sentence.
    let refused = state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerReadComment {
                comment_id: "tc-nobody".into(),
            },
        )
        .expect_err("no such comment");
    assert_eq!(
        refused,
        "There is no comment tc-nobody on this project's tasks."
    );
}

/// The prompt says what a notice is, and where a question on your own task
/// is answered.
#[test]
fn the_prompt_says_a_notice_is_one_line_and_where_to_answer() {
    let templates = crate::templates::Templates::default();
    let flat = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    for (name, text) in [
        ("build", &templates.build),
        ("plan", &templates.plan),
        ("project_agent", &templates.project_agent),
    ] {
        let text = flat(text);
        assert!(
            text.contains("arrives here as ONE LINE"),
            "{name} does not say what arrives"
        );
        assert!(
            text.contains("Read the words with `read_comment` when you care"),
            "{name} does not say how to read it"
        );
        assert!(
            text.contains("ignore the line entirely when it is not about what you are waiting for"),
            "{name} does not say it may be ignored"
        );
        assert!(
            text.contains("answered on the task with `comment_task`, not in this conversation"),
            "{name} does not say where to answer"
        );
    }
}
