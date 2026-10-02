use super::*;

#[test]
fn capture_reroute_refuses_a_task_destination_without_mutating_the_capture_or_router() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let (capture_id, agent_id) = captured(&mut state, "keep this capture intact");
    let scratch = state.router_sessions[&capture_id]
        .scratch_dir()
        .to_path_buf();
    let notes = scratch.join("routing-notes.txt");
    std::fs::write(&notes, "still deciding").unwrap();
    let before = capture_record(&mut state, &capture_id);

    let refused = state.handle(req(
        "capture.reroute",
        json!({ "capture_id": capture_id, "project_id": project_id, "kind": "task" }),
    ));

    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(
        refused["error"]
            .as_str()
            .is_some_and(|error| error.contains("\"task\" is not a destination")),
        "{refused:?}"
    );
    assert!(state.plans.is_empty());
    assert_eq!(capture_record(&mut state, &capture_id), before);
    assert_eq!(
        state
            .router_sessions
            .get(&capture_id)
            .map(|session| session.agent_id()),
        Some(agent_id.as_str()),
        "a refused destination must leave the original router in charge"
    );
    assert_eq!(std::fs::read_to_string(notes).unwrap(), "still deciding");
}

// ==== captures: durable before anything routes them ======================

pub(in crate::app::tests) fn capture_rows(state: &mut AppState) -> Vec<Value> {
    work_item_rows(state)
        .into_iter()
        .filter(|row| row["kind"] == "capture")
        .collect()
}

/// The write that has to land before anything else happens. A capture is
/// on disk by the time `capture.create` answers, so a daemon that dies the
/// instant afterwards — mid-route, pre-route, whenever — still has what the
/// user said when it comes back.
#[test]
fn a_capture_is_durable_before_anything_routes_it() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());

    let created = state.handle(req(
        "capture.create",
        json!({ "text": "fix the login redirect" }),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    let record = created["result"].clone();
    let capture_id = record["id"]
        .as_str()
        .expect("a capture has an id")
        .to_string();
    assert!(capture_id.starts_with("capture-"), "{capture_id}");
    assert_eq!(record["text"], "fix the login redirect");
    assert_eq!(
        record["state"], "routing",
        "the text was kept, and only then was anything asked to route it"
    );
    assert_eq!(record["routing"], Value::Null);
    assert_eq!(record["question"], Value::Null);

    // On disk already — read by a store this daemon never told about it.
    let on_disk = Store::new(dir.path().join("store"))
        .expect("store opens")
        .load_all_captures()
        .unwrap();
    assert_eq!(on_disk.len(), 1, "the record is written before the answer");
    assert_eq!(on_disk[0].id, capture_id);
    assert_eq!(on_disk[0].text, "fix the login redirect");

    // And a fresh daemon over the same store still has it.
    let mut rebooted = qa_state(&repo, dir.path());
    let captures = capture_rows(&mut rebooted);
    assert_eq!(captures.len(), 1);
    assert_eq!(captures[0]["capture_id"], capture_id.as_str());

    let fetched = rebooted.handle(req("capture.get", json!({ "capture_id": capture_id })));
    assert_eq!(fetched["ok"], true, "{fetched:?}");
    assert_eq!(fetched["result"]["text"], "fix the login redirect");
}

/// A capture with nothing said about it is empty text: there is nothing to
/// keep, and a record of nothing would sit on the feed forever.
#[test]
fn a_capture_of_nothing_is_refused() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());

    let blank = state.handle(req("capture.create", json!({ "text": "   \n " })));
    assert_eq!(blank["ok"], false, "{blank:?}");
    assert!(state.captures.is_empty());
    assert_eq!(
        Store::new(dir.path().join("store"))
            .expect("store opens")
            .load_all_captures()
            .unwrap(),
        Vec::new()
    );

    let missing = state.handle(req("capture.create", json!({})));
    assert_eq!(missing["ok"], false, "{missing:?}");
}

#[test]
fn capture_get_refuses_an_id_it_does_not_know() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let unknown = state.handle(req(
        "capture.get",
        json!({ "capture_id": "capture-nowhere" }),
    ));
    assert_eq!(unknown["ok"], false, "{unknown:?}");
}

/// A capture the router is still deciding is a row on the feed, in the row
/// shape every work item ships: what it says, that nothing has been
/// decided yet, and none of the branch facts it does not have.
#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: a_capture_being_routed_is_a_feed_row is at 21, threshold 15 — bring it under, then remove
fn a_capture_being_routed_is_a_feed_row() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let created = state.handle(req(
        "capture.create",
        json!({ "text": "fix the login redirect\nand the toast" }),
    ));
    let capture_id = created["result"]["id"].as_str().unwrap().to_string();

    let rows = capture_rows(&mut state);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(row["capture_id"], capture_id.as_str());
    assert_eq!(row["title"], "fix the login redirect");
    assert_eq!(row["text"], "fix the login redirect\nand the toast");
    assert_eq!(row["state"], "routing");
    assert_eq!(row["unread"], false, "nothing has asked the user anything");
    assert_eq!(row["unread_count"], 0);
    assert_eq!(row["unread_reason"], Value::Null);
    assert_eq!(row["working"], true, "the router has it");
    assert_eq!(row["branch"], Value::Null);
    assert_eq!(row["run_id"], Value::Null);
    assert_eq!(row["task_id"], Value::Null);
    assert_eq!(row["worktree_id"], Value::Null);
    assert_eq!(row["agents"].as_array().unwrap().len(), 0);
    assert_eq!(row["can_finish"], false);
    assert_eq!(row["muted"], false);
    assert_eq!(row["resume_at"], row["created_at"]);
    assert_eq!(row["routing"], Value::Null);
    assert_eq!(row["question"], Value::Null);
}

/// While the router has it, the capture reads as working; when the router
/// gives up, it reads as needing the user.
#[test]
fn a_capture_says_whether_the_router_has_it_or_gave_up() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let created = state.handle(req("capture.create", json!({ "text": "ship it" })));
    let capture_id = created["result"]["id"].as_str().unwrap().to_string();

    state.captures.get_mut(&capture_id).unwrap().state = crate::capture::CaptureState::Routing;
    let routing = capture_rows(&mut state).remove(0);
    assert_eq!(routing["state"], "routing");
    assert_eq!(routing["working"], true);
    assert_eq!(routing["unread"], false);

    state.captures.get_mut(&capture_id).unwrap().state = crate::capture::CaptureState::Failed;
    let failed = capture_rows(&mut state).remove(0);
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["working"], false);
    assert_eq!(failed["unread"], true);
    assert_eq!(failed["unread_count"], 1);
    assert_eq!(failed["unread_reason"], "routing_failed");
}

/// Once a capture is routed and quiet, the task or branch it became is its
/// presence — a second row for one piece of work is a lie. An unanswered
/// question is the exception: that is the capture itself asking.
#[test]
fn a_routed_and_quiet_capture_leaves_the_feed() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let created = state.handle(req("capture.create", json!({ "text": "ship it" })));
    let capture_id = created["result"]["id"].as_str().unwrap().to_string();

    {
        let capture = state.captures.get_mut(&capture_id).unwrap();
        capture.state = crate::capture::CaptureState::Routed;
        capture.routing = Some(crate::capture::CaptureRouting {
            project_id: project_id.clone(),
            kind: crate::capture::CaptureTarget::Task,
            target_id: "plan-7".to_string(),
            routed_at: now_rfc3339(),
            rationale: Some("no branch names this work".to_string()),
        });
    }
    assert_eq!(capture_rows(&mut state), Vec::<Value>::new());

    state.captures.get_mut(&capture_id).unwrap().question = Some(
        crate::capture::CaptureQuestion::new("which project is this?", now_rfc3339()),
    );
    let asking = capture_rows(&mut state).remove(0);
    assert_eq!(asking["unread"], true);
    assert_eq!(asking["unread_reason"], "router_question");
    assert_eq!(asking["question"]["text"], "which project is this?");
    assert_eq!(asking["project_id"], project_id.as_str());
    assert_eq!(asking["task_id"], "plan-7");
    assert_eq!(asking["routing"]["kind"], "task");

    state
        .captures
        .get_mut(&capture_id)
        .unwrap()
        .question
        .as_mut()
        .unwrap()
        .answer = Some("the bridge".to_string());
    assert_eq!(capture_rows(&mut state), Vec::<Value>::new());
}

/// The router process died with the daemon, so a route that was in flight
/// is re-fired at boot from the recovered record: the daemon owes the user
/// a decision, not a row waiting for one. A question still waiting on the
/// user, and a decision already made, are left alone.
#[test]
fn boot_refires_an_interrupted_route_and_leaves_the_rest_alone() {
    let (dir, repo) = init_repo();
    let store = Store::new(dir.path().join("store")).expect("store opens");
    let mut mid_route =
        crate::capture::Capture::new("capture-mid-route", "fix the login redirect", now_rfc3339());
    mid_route.state = crate::capture::CaptureState::Routing;
    store.save_capture(&mid_route).unwrap();
    let mut routed = crate::capture::Capture::new("capture-routed", "ship it", now_rfc3339());
    routed.state = crate::capture::CaptureState::Routed;
    store.save_capture(&routed).unwrap();
    let mut asking = crate::capture::Capture::new("capture-asking", "do the thing", now_rfc3339());
    asking.question = Some(crate::capture::CaptureQuestion::new(
        "which project?",
        now_rfc3339(),
    ));
    store.save_capture(&asking).unwrap();

    let mut state = qa_state(&repo, dir.path());
    let refired = state.handle(req(
        "capture.get",
        json!({ "capture_id": "capture-mid-route" }),
    ));
    assert_eq!(
        refired["result"]["state"], "routing",
        "the interrupted route is the router's again: {refired:?}"
    );
    assert!(
        state.router_sessions.contains_key("capture-mid-route"),
        "a fresh router session decides the recovered capture"
    );

    let waiting = state.handle(req(
        "capture.get",
        json!({ "capture_id": "capture-asking" }),
    ));
    assert_eq!(
        waiting["result"]["state"], "unrouted",
        "an unanswered question waits for the user, not a router: {waiting:?}"
    );
    assert!(!state.router_sessions.contains_key("capture-asking"));

    let settled = state.handle(req(
        "capture.get",
        json!({ "capture_id": "capture-routed" }),
    ));
    assert_eq!(
        settled["result"]["state"], "routed",
        "a decision already made is not a session to recover"
    );
    assert!(!state.router_sessions.contains_key("capture-routed"));
}
