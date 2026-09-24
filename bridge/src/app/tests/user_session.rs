//! The user's session and "Done since you left" (spec: Issues dashboard).
//!
//! Only the user's own verbs move the session; an agent working all night
//! leaves it where the user left it. `issues.list` carries the session and
//! each Done issue's `done_at`, which is everything the dashboard reads.

use super::project_agent::rooted;
use super::tracker::{filed, tracked};
use super::tracker_tools::{call, coding_agent};
use super::*;
use crate::mcp::BridgeAction;
use crate::session_summary::{UserSession, USER_SESSION_GAP_MS};

fn listed(state: &mut AppState, project_id: &str) -> Value {
    let listed = state.handle(req("issues.list", json!({ "project_id": project_id })));
    assert_eq!(listed["ok"], true, "{listed:?}");
    listed["result"].clone()
}

fn restarted(state_root: &std::path::Path) -> AppState {
    rooted(state_root)
        .with_task_store(state_root.join("store"))
        .expect("the store reopens")
}

#[test]
fn agent_work_never_starts_a_session_and_a_user_comment_does() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "night-shift");
    // Setting up the project and the agent was the user; forget it, so what
    // follows is only the agent working.
    state.user_session = UserSession::default();

    let issue = call(
        &mut state,
        &who,
        BridgeAction::TrackerCreateIssue {
            title: "Overnight fix".into(),
            body: None,
            status: None,
            labels: Vec::new(),
            priority: None,
            track: None,
            attachments: Vec::new(),
            notify_user: None,
        },
    )
    .expect("the agent files an issue")["issue"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    call(
        &mut state,
        &who,
        BridgeAction::TrackerMoveIssue {
            issue_id: issue.clone(),
            status: "done".into(),
            track: None,
        },
    )
    .expect("the agent finishes it");

    let answer = listed(&mut state, &project_id);
    assert_eq!(
        answer["user_session"],
        json!({
            "session_started_ms": null,
            "last_activity_ms": null,
            "previous_session_ended_ms": null,
            "gap_ms": USER_SESSION_GAP_MS,
        }),
        "listing is a read and the agent is not the user: {answer:?}"
    );
    let done = &answer["issues"][0];
    assert_eq!(done["status"], "done");
    assert!(done["done_at"].is_string(), "{done:?}");

    let before = crate::agent::now_ms() as i64;
    let commented = state.handle(req(
        "issues.comment",
        json!({ "issue_id": issue, "body": "Thanks" }),
    ));
    assert_eq!(commented["ok"], true, "{commented:?}");
    let session = &listed(&mut state, &project_id)["user_session"];
    let started = session["session_started_ms"].as_i64().expect("a session");
    assert!(started >= before, "{session:?}");
    assert_eq!(session["last_activity_ms"], session["session_started_ms"]);
    assert_eq!(session["previous_session_ended_ms"], Value::Null);
}

#[test]
fn a_read_mark_is_the_user_being_here() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let issue = filed(&mut state, &project_id, "read me");
    let got = state.handle(req("issues.get", json!({ "issue_id": issue["id"] })));
    let event_id = got["result"]["timeline"][0]["id"].clone();
    // Pretend the user left long ago, so a read has something to move.
    state.user_session = UserSession {
        session_started_ms: Some(0),
        last_activity_ms: Some(1),
        previous_session_ended_ms: None,
    };
    let read = state.handle(req(
        "issues.read_through",
        json!({ "issue_id": issue["id"], "event_id": event_id }),
    ));
    assert_eq!(read["ok"], true, "{read:?}");
    let session = state.user_session();
    assert_eq!(session.previous_session_ended_ms, Some(1));
    assert!(session.session_started_ms > Some(USER_SESSION_GAP_MS));
}

#[test]
fn moving_out_of_done_clears_done_at_and_back_in_stamps_it_again() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let issue = filed(&mut state, &project_id, "twice");
    assert!(issue.get("done_at").is_none(), "{issue:?}");
    let moved = |state: &mut AppState, status: &str| {
        let answer = state.handle(req(
            "issues.update",
            json!({ "issue_id": issue["id"], "status": status }),
        ));
        assert_eq!(answer["ok"], true, "{answer:?}");
        answer["result"]["issue"].clone()
    };
    let first = moved(&mut state, "done")["done_at"].clone();
    assert!(first.is_string());
    assert!(moved(&mut state, "in_review").get("done_at").is_none());
    let second = moved(&mut state, "done")["done_at"].clone();
    assert!(second.as_str() >= first.as_str(), "{first} then {second}");

    let filed_done = state.handle(req(
        "issues.create",
        json!({ "project_id": project_id, "title": "already", "status": "done" }),
    ));
    let filed_done = &filed_done["result"]["issue"];
    assert_eq!(filed_done["done_at"], filed_done["created_at"]);
}

#[test]
fn a_done_issue_from_before_done_at_reads_it_from_its_timeline() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let issue = filed(&mut state, &project_id, "old");
    let id = issue["id"].as_str().unwrap().to_string();
    let answer = state.handle(req(
        "issues.update",
        json!({ "issue_id": id, "status": "done" }),
    ));
    let done_at = answer["result"]["issue"]["done_at"].clone();
    // Written by a bridge that had no such field.
    let (_, mut record) = state.tracker_issue(&id).unwrap();
    record.done_at = None;
    state
        .tracker_store()
        .unwrap()
        .save_tracker_issue_activity(&record, &[], &[])
        .unwrap();

    let answer = listed(&mut state, &project_id);
    assert_eq!(answer["issues"][0]["done_at"], done_at);
    let (_, kept) = state.tracker_issue(&id).unwrap();
    assert_eq!(
        kept.done_at.as_deref(),
        done_at.as_str(),
        "the backfill is kept"
    );
}

#[test]
fn a_restart_keeps_the_session_and_rebuilds_it_from_stored_actions() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    filed(&mut state, &project_id, "by the user");
    let session = state.user_session();
    assert!(session.last_activity_ms.is_some());
    drop(state);

    let state = restarted(&state_root);
    assert_eq!(state.user_session(), session);

    // A store with no saved summary (a bridge from before this shipped)
    // replays the user's stored actions: filing the issue was one.
    state
        .store
        .as_ref()
        .unwrap()
        .save_user_session(&UserSession::default())
        .unwrap();
    drop(state);
    let state = restarted(&state_root);
    let rebuilt = state.user_session();
    assert!(rebuilt.last_activity_ms.is_some(), "{rebuilt:?}");
    assert!(rebuilt.last_activity_ms <= session.last_activity_ms);
}

/// Every verb the session counts is one this bridge serves, so a rename
/// cannot quietly stop counting it.
#[test]
fn every_user_activity_verb_is_served() {
    let served = crate::api::capabilities(false);
    for verb in crate::app::rpc::USER_ACTIVITY_VERBS {
        assert!(served.contains(verb), "{verb} is not a served verb");
    }
}

/// The chain the dashboard reads, end to end: an agent finishes an issue, then
/// the user acts. Before the action the finished issue is news; after it the
/// user has been here since, and the bridge's answer says so.
///
/// With `BUILD_PRINT_DONE_SINCE_LEFT` set it prints both `issues.list`
/// answers, which `spa/test/doneSinceLeftWiring.test.js` paints.
#[test]
fn a_user_action_after_work_finished_moves_the_answer_the_dashboard_reads() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "finisher");
    // Setting the agent up was the user; the finish comes a millisecond on.
    let set_up = state
        .user_session()
        .last_activity_ms
        .expect("the user set up");
    while (crate::agent::now_ms() as i64) <= set_up {
        std::hint::spin_loop();
    }
    let issue = call(
        &mut state,
        &who,
        BridgeAction::TrackerCreateIssue {
            title: "Finished while you were out".into(),
            body: None,
            status: Some("done".into()),
            labels: Vec::new(),
            priority: None,
            track: None,
            attachments: Vec::new(),
            notify_user: None,
        },
    )
    .expect("the agent files a finished issue")["issue"]
        .clone();
    let done_ms = crate::session_summary::message_millis(issue["done_at"].as_str().unwrap())
        .expect("done_at is a timestamp");
    let before = listed(&mut state, &project_id);
    assert_eq!(before["user_session"]["last_activity_ms"], set_up);

    // Condition, not a pause: the user's action must land in a later
    // millisecond than the finish it follows.
    while (crate::agent::now_ms() as i64) <= done_ms {
        std::hint::spin_loop();
    }
    let commented = state.handle(req(
        "issues.comment",
        json!({ "issue_id": issue["id"], "body": "Seen it" }),
    ));
    assert_eq!(commented["ok"], true, "{commented:?}");
    let after = listed(&mut state, &project_id);
    assert!(after["user_session"]["last_activity_ms"].as_i64() > Some(done_ms));
    assert_eq!(after["issues"][0]["done_at"], issue["done_at"]);

    if std::env::var_os("BUILD_PRINT_DONE_SINCE_LEFT").is_some() {
        println!(
            "BUILD_DONE_SINCE_LEFT={}",
            json!({ "before": before, "after": after })
        );
    }
}
