//! Browser push follows the unread counter (#191).
//!
//! A push fires exactly when something adds to the badge the inbox wears, and
//! at no other time. The badge counts two things, and each has its test here:
//!
//! - an attention-class item on a **watched** agent's own conversation, unless
//!   its entry is muted — every agent on the roster, not only the primary;
//! - timeline news on a **watched**, unfinished task that is not the user's
//!   own doing (`tracker::inbox::counts_as_unread`, #183/#189).
//!
//! What the api sees of each push is the entity's opaque id and a generic kind
//! (`agent` or `task`). What the browser shows — the content sealed to each
//! notification key (#200) — is built here too, and its tests are at the end.

use super::project_agent::{added_project, rooted, workspace};
use super::tracker::{filed, tracked};
use super::*;
use crate::mcp::BridgeAction;
use crate::notify::content::PushContent;

const AGENT: &str = "agent";
const TASK: &str = "task";

/// A notifier whose sends go nowhere: a sync test has no runtime to spawn one
/// on, and what it would have sent is on `sent_notifies` either way.
fn listening(state: AppState) -> AppState {
    let identity = crate::transport::generate_identity_keypair();
    state.with_notifier(crate::notify::Notifier::new(
        "http://127.0.0.1:9",
        "device-under-test",
        &identity.private_key_b64,
    ))
}

fn sent(state: &mut AppState) -> Vec<(String, &'static str)> {
    state.sent_push_contents.clear();
    std::mem::take(&mut state.sent_notifies)
}

/// What each notify since the last drain would have sealed, by kind.
fn sealed(state: &mut AppState, kind: &str) -> Vec<Option<PushContent>> {
    let notifies = std::mem::take(&mut state.sent_notifies);
    let contents = std::mem::take(&mut state.sent_push_contents);
    notifies
        .into_iter()
        .zip(contents)
        .filter(|((_, sent_kind), _)| *sent_kind == kind)
        .map(|(_, content)| content)
        .collect()
}

/// A project with one workspace conversation, as `(state, project, owner)`.
fn project_with_workspace(root: &std::path::Path) -> (tempfile::TempDir, AppState, String, String) {
    let (home, state, project_id, _workspace_id, owner) = workspace_conversation(root);
    (home, state, project_id, owner)
}

/// The same, naming the workspace too:
/// `(state, project, workspace, owner)`.
fn workspace_conversation(
    root: &std::path::Path,
) -> (tempfile::TempDir, AppState, String, String, String) {
    let (home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let mut state = listening(rooted(root));
    let project_id = added_project(&mut state, &repo);
    let workspace_id = workspace(&mut state, &project_id, "pushes");
    let ensured = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace_id }),
    ));
    assert_eq!(ensured["ok"], true, "{ensured:?}");
    let owner = ensured["result"]["run_id"].as_str().unwrap().to_string();
    (home, state, project_id, workspace_id, owner)
}

/// One more agent on `owner`, watched unless `watched` says otherwise.
fn agent_on(state: &mut AppState, owner: &str, watched: bool) -> String {
    let added = state.handle(req(
        "agent.add",
        json!({ "entity_id": owner, "notify_user": watched }),
    ));
    assert_eq!(added["ok"], true, "{added:?}");
    added["result"]["agent"]["id"].as_str().unwrap().to_string()
}

/// An agent speaking in its own conversation, the way the pump records it.
fn agent_says(state: &mut AppState, owner: &str, agent_id: &str, body: &str) {
    state
        .edit_agent_conversation(owner, agent_id, |thread, _| {
            thread.post_agent(body, None, now_rfc3339());
            Ok(())
        })
        .expect("the agent's conversation takes the message");
}

/// Nothing a debounce window remembers: each case below is its own news.
fn forget_debounce(state: &mut AppState) {
    state.notify_throttle = crate::notify::NotifyThrottle::default();
}

// ---- agents -----------------------------------------------------------------

/// The agent that answers is what adds to the badge, so its message pushes —
/// generic kind, the owner's opaque id.
#[test]
fn a_watched_agents_message_pushes_an_agent_notify() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut state, _project, owner) = project_with_workspace(tmp.path());
    let agent = agent_on(&mut state, &owner, true);
    sent(&mut state);

    agent_says(&mut state, &owner, &agent, "the fix is in");

    assert_eq!(sent(&mut state), vec![(owner, AGENT)]);
}

/// A workspace's badge is the sum of its watched agents' unread, so a second
/// agent on the roster pushes too — not only the one whose conversation is the
/// entity's own.
#[test]
fn a_second_agent_on_the_roster_pushes_as_well() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut state, _project, owner) = project_with_workspace(tmp.path());
    let _first = agent_on(&mut state, &owner, true);
    let second = agent_on(&mut state, &owner, true);
    sent(&mut state);

    agent_says(&mut state, &owner, &second, "the second one speaks");

    assert_eq!(sent(&mut state), vec![(owner, AGENT)]);
}

/// An unwatched agent is not on the badge (`watchedUnreadCount`), so the phone
/// stays dark when it speaks.
#[test]
fn an_unwatched_agent_pushes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut state, _project, owner) = project_with_workspace(tmp.path());
    let quiet = agent_on(&mut state, &owner, false);
    sent(&mut state);

    agent_says(&mut state, &owner, &quiet, "nobody is listening");

    assert_eq!(sent(&mut state), vec![]);
}

/// A muted entry's badge says nothing, and nor does its push.
#[test]
fn a_muted_entry_pushes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut state, _project, owner) = project_with_workspace(tmp.path());
    let agent = agent_on(&mut state, &owner, true);
    let muted = state.handle(req(
        "entity.mute",
        json!({ "entity_id": owner, "muted": true }),
    ));
    assert_eq!(muted["ok"], true, "{muted:?}");
    sent(&mut state);

    agent_says(&mut state, &owner, &agent, "muted news");

    assert_eq!(sent(&mut state), vec![]);
}

/// What the user says is not news to them, and a burst of agent news inside
/// the debounce window is one push.
#[test]
fn the_users_own_message_is_quiet_and_a_burst_pushes_once() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut state, _project, owner) = project_with_workspace(tmp.path());
    let agent = agent_on(&mut state, &owner, true);
    sent(&mut state);

    state
        .edit_agent_conversation(&owner, &agent, |thread, _| {
            thread.post_user("please look at the rail", None, now_rfc3339());
            Ok(())
        })
        .unwrap();
    assert_eq!(sent(&mut state), vec![], "the user's own words");

    agent_says(&mut state, &owner, &agent, "looking");
    agent_says(&mut state, &owner, &agent, "found it");
    assert_eq!(
        sent(&mut state),
        vec![(owner, AGENT)],
        "one burst, one push"
    );
}

/// An agent whose process dies mid-turn leaves `Interrupted` on its
/// conversation, which is attention and adds to the badge — so it pushes. The
/// live death is the pump's (`record_agent_session_end`), with nobody at the
/// machine to see it.
#[test]
fn an_agent_dying_mid_turn_pushes() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut state, _project, owner) = project_with_workspace(tmp.path());
    let agent = agent_on(&mut state, &owner, true);
    state.record_agent_working_since(&owner, &agent, Some(now_rfc3339()));
    sent(&mut state);

    state.close_turn_of_dead_agent(&owner, &agent);

    let thread = state.agent_conversation(&owner, Some(&agent)).unwrap();
    assert_eq!(
        thread.unread_since(0).reason,
        Some(crate::thread::ThreadEventKind::Interrupted.as_str()),
        "the death is on the conversation as attention"
    );
    assert_eq!(sent(&mut state), vec![(owner, AGENT)]);
}

/// A restart interrupts every turn that was in flight, and each is unread; but
/// the restart is the operator's own doing, at the machine, and the history a
/// boot recovers was never news a tail announced — so a reboot rings nothing.
/// (The user stopping a turn from the app writes no event at all: see
/// `protocol::session`'s interrupt tests.)
#[test]
fn a_restart_with_a_turn_in_flight_pushes_nothing() {
    let (dir, repo) = init_repo();
    let checkout = add_external_worktree(&repo, dir.path(), "mid-turn", "mid-turn");
    let store = crate::store::Store::new(dir.path().join("store")).expect("store opens");
    store
        .save_run(&super::workflow::recovery::building_run(
            "run-mid-turn",
            &repo,
            &checkout,
        ))
        .unwrap();
    drop(store);
    let context =
        HarnessContext::resolved(dir.path().join("test-mcp.sock"), dir.path().to_path_buf())
            .unwrap();
    let mut state = listening(AppState::new_configured(
        repo.clone(),
        dir.path().join("wt"),
        "main",
        true,
        context,
    ))
    .with_task_store(dir.path().join("store"))
    .unwrap();

    let thread = state.agent_conversation("run-mid-turn", None).unwrap();
    assert!(
        thread.items.iter().any(|item| matches!(
            item,
            crate::thread::ThreadItem::Event(event)
                if event.event == crate::thread::ThreadEventKind::Interrupted
        )),
        "boot interrupted the turn: {:?}",
        thread.items
    );
    assert_eq!(sent(&mut state), vec![], "the boot itself");

    // Nor does the first change after it announce what boot wrote.
    let agent = state.runs["run-mid-turn"]
        .agents
        .iter()
        .next()
        .unwrap()
        .id
        .clone();
    state
        .edit_agent_conversation("run-mid-turn", &agent, |thread, _| {
            thread.post_user("carry on", None, now_rfc3339());
            Ok(())
        })
        .unwrap();
    assert_eq!(sent(&mut state), vec![], "the first change after boot");
}

// ---- tasks -----------------------------------------------------------------

/// A coding agent on a fresh workspace of `project_id`, as `(owner, agent)`.
fn coding_agent(state: &mut AppState, project_id: &str, name: &str) -> (String, String) {
    let workspace_id = workspace(state, project_id, name);
    let ensured = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace_id }),
    ));
    let owner = ensured["result"]["run_id"].as_str().unwrap().to_string();
    let agent = agent_on(state, &owner, true);
    (owner, agent)
}

fn act(state: &mut AppState, who: &(String, String), action: BridgeAction) -> Value {
    state
        .on_agent_mcp_action(&who.0, &who.1, action)
        .unwrap_or_else(|error| panic!("the agent's tool call lands: {error}"))
}

fn agent_comments(state: &mut AppState, who: &(String, String), task_id: &str) {
    act(
        state,
        who,
        BridgeAction::TrackerCommentTask {
            task_id: task_id.into(),
            body: "a question for you".into(),
            attachments: Vec::new(),
            refs: Vec::new(),
            track: Some(false),
            notify_user: None,
            mention_user: None,
        },
    );
}

fn agent_moves(state: &mut AppState, who: &(String, String), task_id: &str, status: &str) {
    act(
        state,
        who,
        BridgeAction::TrackerMoveTask {
            task_id: task_id.into(),
            status: status.into(),
            track: Some(false),
        },
    );
}

/// A task the user filed (so watched), with an agent to act on it and the
/// agents' own conversation pushes already drained.
fn watched_task(root: &std::path::Path) -> (tempfile::TempDir, AppState, (String, String), String) {
    let (home, state, project_id) = tracked(root);
    let mut state = listening(state);
    let id = filed(&mut state, &project_id, "push me")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let who = coding_agent(&mut state, &project_id, "actor");
    sent(&mut state);
    (home, state, who, id)
}

/// A comment an agent leaves on a watched open task is unread news: it
/// pushes as a `task`, by the task's opaque id.
#[test]
fn an_agents_comment_on_a_watched_task_pushes_a_task_notify() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, who, id) = watched_task(&root);

    agent_comments(&mut state, &who, &id);

    let pushed: Vec<_> = sent(&mut state)
        .into_iter()
        .filter(|(_, kind)| *kind == TASK)
        .collect();
    assert_eq!(pushed, vec![(id, TASK)]);
}

/// A move and an assignment count on the badge (#183), so each pushes.
#[test]
fn a_move_and_an_assignment_on_a_watched_task_push() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, who, id) = watched_task(&root);

    agent_moves(&mut state, &who, &id, "in_progress");
    assert!(sent(&mut state).contains(&(id.clone(), TASK)), "a move");

    forget_debounce(&mut state);
    act(
        &mut state,
        &who,
        BridgeAction::TrackerAssignTask {
            assignee: json!({ "kind": "user" }),
            task_id: id.clone(),
            note: None,
            track: Some(false),
            notify_user: None,
        },
    );
    assert!(sent(&mut state).contains(&(id, TASK)), "an assignment");
}

/// A task the user is not watching has no badge to add to.
#[test]
fn an_unwatched_task_pushes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, who, id) = watched_task(&root);
    let unwatched = state.handle(req("tasks.unwatch", json!({ "task_id": id })));
    assert_eq!(unwatched["ok"], true, "{unwatched:?}");
    sent(&mut state);

    agent_moves(&mut state, &who, &id, "in_progress");

    assert!(!sent(&mut state).iter().any(|(_, kind)| *kind == TASK));
}

/// A Done task never counts in a total (#183) — neither the move that takes
/// it there nor a comment made on it afterwards.
#[test]
fn a_done_task_pushes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, who, id) = watched_task(&root);

    agent_moves(&mut state, &who, &id, "done");
    agent_comments(&mut state, &who, &id);

    assert!(!sent(&mut state).iter().any(|(_, kind)| *kind == TASK));
}

/// What the user does is never unread to them, and filing is bookkeeping
/// (#183): an agent filing a task for the user to watch pushes nothing.
#[test]
fn the_users_own_change_and_a_filing_push_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, who, id) = watched_task(&root);

    let said = state.handle(req(
        "tasks.comment",
        json!({ "task_id": id, "body": "my own words" }),
    ));
    assert_eq!(said["ok"], true, "{said:?}");
    act(
        &mut state,
        &who,
        BridgeAction::TrackerCreateTask {
            title: "filed for the user".into(),
            body: None,
            status: None,
            labels: Vec::new(),
            priority: None,
            track: None,
            attachments: Vec::new(),
            notify_user: Some(true),
            mention_user: None,
        },
    );

    assert!(!sent(&mut state).iter().any(|(_, kind)| *kind == TASK));
}

/// An agent filing a task that asks the user to read it is unread news
/// (#189), so the new task pushes as a `task`, by its own id.
#[test]
fn an_agent_filing_a_task_that_asks_the_user_pushes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, who, _id) = watched_task(&root);

    let filed = act(
        &mut state,
        &who,
        BridgeAction::TrackerCreateTask {
            title: "which route should we take?".into(),
            body: None,
            status: None,
            labels: Vec::new(),
            priority: None,
            track: None,
            attachments: Vec::new(),
            notify_user: None,
            mention_user: Some(true),
        },
    );
    let asked = filed["task"]["id"].as_str().unwrap().to_string();

    let pushed: Vec<_> = sent(&mut state)
        .into_iter()
        .filter(|(_, kind)| *kind == TASK)
        .collect();
    assert_eq!(pushed, vec![(asked, TASK)]);
}

/// Reading a task takes from the badge; it never adds to it. The read after
/// an agent's comment pushes nothing more, and the comment's own push is the
/// only one.
#[test]
fn reading_a_task_pushes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, who, id) = watched_task(&root);
    agent_comments(&mut state, &who, &id);
    assert!(sent(&mut state).contains(&(id.clone(), TASK)));
    forget_debounce(&mut state);

    let got = state.handle(req("tasks.get", json!({ "task_id": id })));
    let newest = got["result"]["timeline"]
        .as_array()
        .and_then(|timeline| timeline.last())
        .and_then(|entry| entry["id"].as_str())
        .expect("the timeline has an entry")
        .to_string();
    let read = state.handle(req(
        "tasks.read_through",
        json!({ "task_id": id, "event_id": newest }),
    ));
    assert_eq!(read["ok"], true, "{read:?}");

    assert!(!sent(&mut state).iter().any(|(_, kind)| *kind == TASK));
}

// ---- sealed content (#200) ----------------------------------------------

const DEVICE: &str = "device-under-test";

fn named(state: &mut AppState, owner: &str, agent: &str, name: &str) {
    state
        .set_agent_name(owner, agent, name)
        .expect("the agent takes the name");
}

/// An agent's push says who spoke and the first line of what it said, and
/// links the workspace's Changes with the rail on that agent, written the
/// way the SPA's router writes it.
#[test]
fn a_workspace_agents_push_carries_its_name_first_line_and_deep_link() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut state, project, workspace, owner) = workspace_conversation(tmp.path());
    let agent = agent_on(&mut state, &owner, true);
    named(&mut state, &owner, &agent, "Banner fade fixer");
    sent(&mut state);

    agent_says(
        &mut state,
        &owner,
        &agent,
        "\n  Fixed on   build/banner-fade\nand more",
    );

    let content = sealed(&mut state, AGENT).pop().flatten().expect("content");
    assert_eq!(content.title, "Banner fade fixer");
    assert_eq!(content.body, "Fixed on build/banner-fade");
    assert_eq!(
        content.url,
        format!(
            "/app/#/device/{DEVICE}/project/{project}/workspace/{workspace}/changes?agent={agent}"
        )
    );
}

/// An agent nobody named goes by its conversation's name, the way the rail
/// says it.
#[test]
fn an_unnamed_agent_goes_by_its_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut state, _project, _workspace, owner) = workspace_conversation(tmp.path());
    let agent = agent_on(&mut state, &owner, true);
    sent(&mut state);

    agent_says(&mut state, &owner, &agent, "done");

    let content = sealed(&mut state, AGENT).pop().flatten().expect("content");
    assert_eq!(content.title, "pushes");
}

/// The project agent's conversation is the project page with the rail on it.
#[test]
fn a_project_agents_push_links_the_project_page() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let mut state = listening(rooted(tmp.path()));
    let project = added_project(&mut state, &repo);
    let ensured = state.handle(req(
        "project.ensure_conversation",
        json!({ "project_id": project }),
    ));
    assert_eq!(ensured["ok"], true, "{ensured:?}");
    let owner = ensured["result"]["run_id"].as_str().unwrap().to_string();
    let agent = agent_on(&mut state, &owner, true);
    named(&mut state, &owner, &agent, "Planner");
    sent(&mut state);

    agent_says(&mut state, &owner, &agent, "the plan is ready");

    let content = sealed(&mut state, AGENT).pop().flatten().expect("content");
    assert_eq!(content.title, "Planner");
    assert_eq!(content.body, "the plan is ready");
    assert_eq!(
        content.url,
        format!("/app/#/device/{DEVICE}/project/{project}?agent={agent}")
    );
}

/// An attention event says what happened: its own summary's first line.
#[test]
fn an_agent_dying_mid_turn_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, mut state, _project, owner) = project_with_workspace(tmp.path());
    let agent = agent_on(&mut state, &owner, true);
    state.record_agent_working_since(&owner, &agent, Some(now_rfc3339()));
    sent(&mut state);

    state.close_turn_of_dead_agent(&owner, &agent);

    let content = sealed(&mut state, AGENT).pop().flatten().expect("content");
    assert_eq!(
        content.body,
        "The agent's session ended without reporting back"
    );
}

/// A task's push is its number and title, and who said what: the newest
/// comment's first line, cut to the cap.
#[test]
fn a_comment_pushes_the_task_title_and_the_first_line_said() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, who, id) = watched_task(&root);
    named(&mut state, &who.0, &who.1, "Rail scroll");
    sent(&mut state);
    let long = "word ".repeat(60);

    act(
        &mut state,
        &who,
        BridgeAction::TrackerCommentTask {
            task_id: id.clone(),
            body: format!("\n{long}\nsecond line"),
            attachments: Vec::new(),
            refs: Vec::new(),
            track: Some(false),
            notify_user: None,
            mention_user: None,
        },
    );

    let content = sealed(&mut state, TASK).pop().flatten().expect("content");
    let number = state.handle(req("tasks.get", json!({ "task_id": id })))["result"]["task"]
        ["number"]
        .as_u64()
        .unwrap();
    assert_eq!(content.title, format!("#{number} push me"));
    assert!(
        content.body.starts_with("Rail scroll: word word"),
        "{}",
        content.body
    );
    assert_eq!(content.body.chars().count(), 160);
    assert!(content.body.ends_with('…'));
    assert_eq!(content.url, format!("/app/#/tasks/{id}"));
}

/// No comment, so the news is the event, in a phrase.
#[test]
fn a_move_and_an_assignment_push_a_phrase() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, who, id) = watched_task(&root);
    named(&mut state, &who.0, &who.1, "Rail scroll");
    sent(&mut state);

    agent_moves(&mut state, &who, &id, "in_review");
    let moved = sealed(&mut state, TASK).pop().flatten().expect("content");
    assert_eq!(moved.body, "Rail scroll moved it to In review");

    forget_debounce(&mut state);
    act(
        &mut state,
        &who,
        BridgeAction::TrackerAssignTask {
            assignee: json!({ "kind": "user" }),
            task_id: id,
            note: None,
            track: Some(false),
            notify_user: None,
        },
    );
    let assigned = sealed(&mut state, TASK).pop().flatten().expect("content");
    assert_eq!(assigned.body, "Rail scroll assigned it to you");
}

/// The phrase table: every event that counts as news has words, and one
/// without any sends no content, so that push goes out generic.
#[test]
fn the_news_phrases() {
    use crate::tracker::{Actor, TaskEvent, TaskEventKind as Kind};
    let phrase = |kind, payload: Value| {
        crate::app::tracker::news_phrase(&TaskEvent::new(
            "task-1",
            Actor::Build,
            kind,
            payload,
            "2026-09-27T00:00:00Z",
        ))
    };
    assert_eq!(
        phrase(Kind::Moved, json!({ "to": "done" })).as_deref(),
        Some("moved it to Done")
    );
    assert_eq!(
        phrase(Kind::Moved, json!({ "to": "someday" })).as_deref(),
        Some("moved it")
    );
    assert_eq!(
        phrase(
            Kind::Assigned,
            json!({ "assignee": { "kind": "agent", "agent_id": "a" } })
        )
        .as_deref(),
        Some("assigned it")
    );
    assert_eq!(
        phrase(Kind::Unassigned, json!({})).as_deref(),
        Some("unassigned it")
    );
    assert_eq!(
        phrase(Kind::Closed, json!({})).as_deref(),
        Some("closed it")
    );
    assert_eq!(
        phrase(Kind::Reopened, json!({})).as_deref(),
        Some("reopened it")
    );
    assert_eq!(
        phrase(Kind::Created, json!({})).as_deref(),
        Some("filed it for you")
    );
    assert_eq!(phrase(Kind::Labelled, json!({})), None);
    assert_eq!(phrase(Kind::WorkspaceIdle, json!({})), None);
}
