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

const HOUR_MS: i64 = 60 * 60 * 1000;

fn now_ms() -> i64 {
    i64::try_from(crate::agent::now_ms()).unwrap()
}

/// Spin until the bridge clock has passed `ms`: a condition, not a pause.
fn after(ms: i64) {
    while now_ms() <= ms {
        std::hint::spin_loop();
    }
}

/// The user's session with its boundaries moved, as though they had last
/// acted `hours_ago`.
fn left(state: &mut AppState, started_hours_ago: i64, hours_ago: i64) {
    let now = now_ms();
    state.user_session = UserSession {
        session_started_ms: Some(now - started_hours_ago * HOUR_MS),
        last_activity_ms: Some(now - hours_ago * HOUR_MS),
        previous_session_ended_ms: None,
    };
}

/// The `issues` items one push history carries for `project_id`.
fn issues_items(pushes: &[Value], project_id: &str) -> Vec<Value> {
    pushes
        .iter()
        .filter(|push| push["type"] == "changes")
        .flat_map(|push| push["items"].as_array().cloned().unwrap_or_default())
        .filter(|item| item["entity_id"] == project_id && item.get("issues").is_some())
        .collect()
}

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
            mention_user: None,
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
            "now_ms": answer["user_session"]["now_ms"],
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

/// The reviewer's case: read marks at 09:00, 14:00 and 19:00 are one session
/// (no silence of six hours), after one that ended at 18:00 the day before. A
/// restart must not replay only its endpoints and split it at 19:00.
#[test]
fn a_restart_does_not_split_a_session_kept_alive_by_read_marks() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, _project_id) = tracked(&state_root);
    // Later than every action the set-up stored, so none of them moves it.
    let yesterday_18 = now_ms() + HOUR_MS;
    state.user_session = UserSession {
        session_started_ms: Some(yesterday_18 - 9 * HOUR_MS),
        last_activity_ms: Some(yesterday_18),
        previous_session_ended_ms: None,
    };
    for hours in [15, 20, 25] {
        state.note_user_activity(yesterday_18 + hours * HOUR_MS);
    }
    let kept = state.user_session();
    assert_eq!(kept.session_started_ms, Some(yesterday_18 + 15 * HOUR_MS));
    assert_eq!(kept.previous_session_ended_ms, Some(yesterday_18));
    drop(state);

    assert_eq!(restarted(&state_root).user_session(), kept);
}

/// Resume, Stop and a new terminal are answered outside `dispatch`, and are
/// the user acting all the same. A refused one is not.
#[test]
fn resume_stop_and_a_new_terminal_are_the_user_being_here() {
    use super::protocol::status::insert_dictated_agent_tab;
    use super::runtime::idle_sessions::insert_run;

    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let root = {
        let mut app = state.lock().unwrap();
        insert_run(&mut app, &repo, dir.path(), "run-user", RunState::Building)
    };
    let agent_id = crate::agent::derived_agent_id("run-user");
    let away = |state: &Arc<Mutex<AppState>>| {
        let mut app = state.lock().unwrap();
        left(&mut app, 10, 7);
        app.user_session()
    };
    let moved = |state: &Arc<Mutex<AppState>>, before: UserSession| {
        state.lock().unwrap().user_session() != before
    };
    let stop = json!({
        "entity_id": "run-user",
        "agent_id": agent_id,
        "conversation_id": agent_id,
    });

    // Refused: nothing is running, so the user did nothing.
    let before = away(&state);
    let refused = handler.call(
        SessionSender::detached("qa"),
        req("agent.interrupt", stop.clone()),
    );
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(!moved(&state, before), "a refused Stop is not activity");

    {
        let mut app = state.lock().unwrap();
        insert_dictated_agent_tab(
            &mut app,
            &root,
            "run-user",
            DictatedSession::reporting(AgentStatus::Working).interruptible(),
        );
    }
    let before = away(&state);
    let stopped = handler.call(SessionSender::detached("qa"), req("agent.interrupt", stop));
    assert_eq!(stopped["ok"], true, "{stopped:?}");
    assert!(moved(&state, before), "Stop is the user acting");

    let before = away(&state);
    let resumed = handler.call(
        SessionSender::detached("qa"),
        req("agent.start", json!({ "id": "run-user" })),
    );
    assert_eq!(resumed["ok"], true, "{resumed:?}");
    assert!(moved(&state, before), "Resume is the user acting");

    let project_id = state.lock().unwrap().project_at(0).id.clone();
    let before = away(&state);
    let opened = handler.call(
        SessionSender::detached("qa"),
        req("term.create", json!({ "project_id": project_id })),
    );
    assert_eq!(opened["ok"], true, "{opened:?}");
    assert!(
        moved(&state, before),
        "opening a terminal is the user acting"
    );
}

/// Arriving is the user being here: `user.present` records it on the bridge
/// clock and answers the session as `issues.list` carries it.
#[test]
fn arriving_starts_a_session_on_the_bridge_clock() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    left(&mut state, 30, 20);
    let last = state.user_session().last_activity_ms.unwrap();
    let before = now_ms();

    let present = state.handle(req("user.present", json!({})));
    assert_eq!(present["ok"], true, "{present:?}");
    let session = &present["result"]["user_session"];
    assert!(
        session["session_started_ms"].as_i64() >= Some(before),
        "{session:?}"
    );
    assert_eq!(session["previous_session_ended_ms"], last);
    assert_eq!(session["gap_ms"], USER_SESSION_GAP_MS);
    assert!(session["now_ms"].as_i64() >= session["last_activity_ms"].as_i64());

    let listed = listed(&mut state, &project_id);
    assert_eq!(
        listed["user_session"]["session_started_ms"], session["session_started_ms"],
        "a reload re-reads the same arrival"
    );
}

/// A new session reaches every client, not just the one the user acted on: a
/// laptop holding the old one must not infer an absence from it. More
/// activity inside the same session pushes nothing.
#[tokio::test]
async fn a_new_session_on_one_client_is_pushed_to_another() {
    let (dir, repo) = init_repo();
    let (state, handler, laptop, mut rx, key) =
        super::push::greeted_push_session(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    let subscribed = handler.call(
        laptop.clone(),
        req(
            "changes.subscribe",
            json!({
                "subscription_id": "s-issues",
                "scope": { "kind": "entity", "id": project_id },
                "kinds": ["issues"],
            }),
        ),
    );
    assert_eq!(subscribed["ok"], true, "{subscribed:?}");
    left(&mut state.lock().unwrap(), 10, 7);
    super::push::settled_pushes(&mut rx, &key).await;

    let phone = SessionSender::detached("phone");
    let present = handler.call(phone.clone(), req("user.present", json!({})));
    assert_eq!(present["ok"], true, "{present:?}");
    let pushed = super::push::pushes_until(&mut rx, &key, |pushes| {
        !issues_items(pushes, &project_id).is_empty()
    })
    .await;
    assert_eq!(issues_items(&pushed, &project_id).len(), 1, "{pushed:?}");

    let again = handler.call(phone, req("user.present", json!({})));
    assert_eq!(again["ok"], true, "{again:?}");
    let quiet = super::push::settled_pushes(&mut rx, &key).await;
    assert_eq!(issues_items(&quiet, &project_id), Vec::<Value>::new());
}

/// The answers the dashboard reads around an absence, end to end over two
/// clients. The user left eight hours before an issue was finished; a laptop
/// holds that. The user comes back on a phone, and the laptop is pushed the
/// new session and reads it. Then they comment on the finished issue.
///
/// With `BUILD_PRINT_DONE_SINCE_LEFT` set it prints the three `issues.list`
/// answers and the push item, which `spa/test/doneSinceLeftWiring.test.js`
/// paints.
#[tokio::test]
async fn the_answers_the_dashboard_reads_around_an_absence() {
    let (dir, repo) = init_repo();
    let (state, handler, laptop, mut rx, key) =
        super::push::greeted_push_session(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    let phone = SessionSender::detached("phone");
    let list = |who: &SessionSender| {
        let listed = handler.call(
            who.clone(),
            req("issues.list", json!({ "project_id": project_id })),
        );
        assert_eq!(listed["ok"], true, "{listed:?}");
        listed["result"].clone()
    };
    let subscribed = handler.call(
        laptop.clone(),
        req(
            "changes.subscribe",
            json!({
                "subscription_id": "s-issues",
                "scope": { "kind": "entity", "id": project_id },
                "kinds": ["issues"],
            }),
        ),
    );
    assert_eq!(subscribed["ok"], true, "{subscribed:?}");
    let filed = handler.call(
        phone.clone(),
        req(
            "issues.create",
            json!({ "project_id": project_id, "title": "Finished while you were out", "status": "done" }),
        ),
    );
    assert_eq!(filed["ok"], true, "{filed:?}");
    let issue = filed["result"]["issue"].clone();
    let done_ms = crate::session_summary::message_millis(issue["done_at"].as_str().unwrap())
        .expect("done_at is a timestamp");
    // Filing it stands in for an agent: the user had left eight hours before.
    left(&mut state.lock().unwrap(), 10, 8);
    let away = list(&laptop);
    super::push::settled_pushes(&mut rx, &key).await;

    after(done_ms);
    let present = handler.call(phone.clone(), req("user.present", json!({})));
    assert_eq!(present["ok"], true, "{present:?}");
    let pushed = super::push::pushes_until(&mut rx, &key, |pushes| {
        !issues_items(pushes, &project_id).is_empty()
    })
    .await;
    let back = list(&laptop);
    assert!(back["user_session"]["session_started_ms"].as_i64() > Some(done_ms));
    assert_eq!(
        back["user_session"]["previous_session_ended_ms"],
        away["user_session"]["last_activity_ms"]
    );

    let said = handler.call(
        phone,
        req(
            "issues.comment",
            json!({ "issue_id": issue["id"], "body": "Seen it" }),
        ),
    );
    assert_eq!(said["ok"], true, "{said:?}");
    let commented = list(&laptop);
    assert_eq!(
        commented["user_session"]["session_started_ms"],
        back["user_session"]["session_started_ms"]
    );
    assert!(commented["user_session"]["last_activity_ms"].as_i64() > Some(done_ms));

    if std::env::var_os("BUILD_PRINT_DONE_SINCE_LEFT").is_some() {
        println!(
            "BUILD_DONE_SINCE_LEFT={}",
            json!({
                "away": away,
                "back": back,
                "commented": commented,
                "pushed": issues_items(&pushed, &project_id)[0],
            })
        );
    }
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
