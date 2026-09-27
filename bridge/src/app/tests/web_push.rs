//! Browser push follows the unread counter (#191).
//!
//! A push fires exactly when something adds to the badge the inbox wears, and
//! at no other time. The badge counts two things, and each has its test here:
//!
//! - an attention-class item on a **watched** agent's own conversation, unless
//!   its entry is muted — every agent on the roster, not only the primary;
//! - timeline news on a **watched**, unfinished issue that is not the user's
//!   own doing (`tracker::inbox::counts_as_unread`, #183/#189).
//!
//! Each push is content-free: the entity's opaque id and a generic kind
//! (`agent` or `task`), nothing else.

use super::project_agent::{added_project, rooted, workspace};
use super::tracker::{filed, tracked};
use super::*;
use crate::mcp::BridgeAction;

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
    std::mem::take(&mut state.sent_notifies)
}

/// A project with one workspace conversation, as `(state, project, owner)`.
fn project_with_workspace(root: &std::path::Path) -> (tempfile::TempDir, AppState, String, String) {
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
    (home, state, project_id, owner)
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

// ---- issues -----------------------------------------------------------------

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

fn agent_comments(state: &mut AppState, who: &(String, String), issue_id: &str) {
    act(
        state,
        who,
        BridgeAction::TrackerCommentIssue {
            issue_id: issue_id.into(),
            body: "a question for you".into(),
            attachments: Vec::new(),
            refs: Vec::new(),
            track: Some(false),
            notify_user: None,
            mention_user: None,
        },
    );
}

fn agent_moves(state: &mut AppState, who: &(String, String), issue_id: &str, status: &str) {
    act(
        state,
        who,
        BridgeAction::TrackerMoveIssue {
            issue_id: issue_id.into(),
            status: status.into(),
            track: Some(false),
        },
    );
}

/// An issue the user filed (so watched), with an agent to act on it and the
/// agents' own conversation pushes already drained.
fn watched_issue(
    root: &std::path::Path,
) -> (tempfile::TempDir, AppState, (String, String), String) {
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

/// A comment an agent leaves on a watched open issue is unread news: it
/// pushes as a `task`, by the issue's opaque id.
#[test]
fn an_agents_comment_on_a_watched_issue_pushes_a_task_notify() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, who, id) = watched_issue(&root);

    agent_comments(&mut state, &who, &id);

    let pushed: Vec<_> = sent(&mut state)
        .into_iter()
        .filter(|(_, kind)| *kind == TASK)
        .collect();
    assert_eq!(pushed, vec![(id, TASK)]);
}

/// A move and an assignment count on the badge (#183), so each pushes.
#[test]
fn a_move_and_an_assignment_on_a_watched_issue_push() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, who, id) = watched_issue(&root);

    agent_moves(&mut state, &who, &id, "in_progress");
    assert!(sent(&mut state).contains(&(id.clone(), TASK)), "a move");

    forget_debounce(&mut state);
    act(
        &mut state,
        &who,
        BridgeAction::TrackerAssignIssue {
            assignee: json!({ "kind": "user" }),
            issue_id: id.clone(),
            note: None,
            track: Some(false),
            notify_user: None,
        },
    );
    assert!(sent(&mut state).contains(&(id, TASK)), "an assignment");
}

/// An issue the user is not watching has no badge to add to.
#[test]
fn an_unwatched_issue_pushes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, who, id) = watched_issue(&root);
    let unwatched = state.handle(req("issues.unwatch", json!({ "issue_id": id })));
    assert_eq!(unwatched["ok"], true, "{unwatched:?}");
    sent(&mut state);

    agent_moves(&mut state, &who, &id, "in_progress");

    assert!(!sent(&mut state).iter().any(|(_, kind)| *kind == TASK));
}

/// A Done issue never counts in a total (#183) — neither the move that takes
/// it there nor a comment made on it afterwards.
#[test]
fn a_done_issue_pushes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, who, id) = watched_issue(&root);

    agent_moves(&mut state, &who, &id, "done");
    agent_comments(&mut state, &who, &id);

    assert!(!sent(&mut state).iter().any(|(_, kind)| *kind == TASK));
}

/// What the user does is never unread to them, and filing is bookkeeping
/// (#183): an agent filing an issue for the user to watch pushes nothing.
#[test]
fn the_users_own_change_and_a_filing_push_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, who, id) = watched_issue(&root);

    let said = state.handle(req(
        "issues.comment",
        json!({ "issue_id": id, "body": "my own words" }),
    ));
    assert_eq!(said["ok"], true, "{said:?}");
    act(
        &mut state,
        &who,
        BridgeAction::TrackerCreateIssue {
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

/// Reading an issue takes from the badge; it never adds to it. The read after
/// an agent's comment pushes nothing more, and the comment's own push is the
/// only one.
#[test]
fn reading_an_issue_pushes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, who, id) = watched_issue(&root);
    agent_comments(&mut state, &who, &id);
    assert!(sent(&mut state).contains(&(id.clone(), TASK)));
    forget_debounce(&mut state);

    let got = state.handle(req("issues.get", json!({ "issue_id": id })));
    let newest = got["result"]["timeline"]
        .as_array()
        .and_then(|timeline| timeline.last())
        .and_then(|entry| entry["id"].as_str())
        .expect("the timeline has an entry")
        .to_string();
    let read = state.handle(req(
        "issues.read_through",
        json!({ "issue_id": id, "event_id": newest }),
    ));
    assert_eq!(read["ok"], true, "{read:?}");

    assert!(!sent(&mut state).iter().any(|(_, kind)| *kind == TASK));
}
