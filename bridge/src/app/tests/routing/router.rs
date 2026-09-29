use super::*;

// ==== the router: what decides where a capture goes ======================

/// Take a capture and hand back its id and the router session deciding it —
/// the fixture every routing test starts from, standing in for the harness
/// that would otherwise be driving these tools.
pub(in crate::app::tests) fn captured(state: &mut AppState, text: &str) -> (String, String) {
    let created = state.handle(req("capture.create", json!({ "text": text })));
    assert_eq!(created["ok"], true, "{created:?}");
    let capture_id = created["result"]["id"].as_str().unwrap().to_string();
    let agent_id = state.router_sessions[&capture_id].agent_id().to_string();
    (capture_id, agent_id)
}

pub(in crate::app::tests) fn capture_record(state: &mut AppState, capture_id: &str) -> Value {
    let fetched = state.handle(req("capture.get", json!({ "capture_id": capture_id })));
    assert_eq!(fetched["ok"], true, "{fetched:?}");
    fetched["result"].clone()
}

#[test]
fn router_working_message_is_visible_without_asking_or_blocking_dispatch() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let (capture_id, _) = captured(&mut state, "route this while I watch");

    state
        .router_action(
            &capture_id,
            BridgeAction::RouterMessage {
                body: "Checking active branches".into(),
                waiting: false,
            },
        )
        .unwrap();
    let progressing = capture_record(&mut state, &capture_id);
    assert_eq!(progressing["state"], "routing");
    assert_eq!(progressing["progress"], "Checking active branches");
    assert!(progressing["question"].is_null());

    let dispatched = state.router_action(
        &capture_id,
        BridgeAction::DispatchBranch {
            project_id,
            branch: None,
            name: "Branch Worker".to_string(),
            instruction: "route this while I watch".into(),
            rationale: None,
        },
    );
    assert!(dispatched.is_ok(), "progress must not require an answer");
}

/// A capture arriving puts a router on it: its own session, its own scratch
/// directory outside every repository, and a turn carrying the decision
/// rule — spawned reactively, off the record that was already written.
#[test]
fn a_capture_puts_a_router_on_it_in_a_scratch_directory_of_its_own() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (capture_id, agent_id) = captured(&mut state, "fix the login redirect");

    assert!(
        crate::router::is_router_agent(&agent_id),
        "{agent_id} must say what kind of session it is"
    );
    let session = state.router_sessions[&capture_id].clone();
    assert_eq!(
        session.scratch_dir(),
        std::fs::canonicalize(dir.path())
            .unwrap()
            .join("router-scratch")
            .join(&capture_id)
    );
    assert!(
        session.scratch_dir().is_dir(),
        "the scratch is cut before the spawn"
    );
    assert!(
        !session.scratch_dir().starts_with(&repo),
        "a router never works inside a checkout: {}",
        session.scratch_dir().display()
    );
    assert_eq!(
        session.choice().effort.as_deref(),
        Some("low"),
        "routing is cheap thinking over a lot of context"
    );

    let turn = state
        .delivery_queue
        .queued()
        .find(|turn| turn.owner == capture_id)
        .expect("the router is given a turn");
    assert_eq!(turn.agent_id, agent_id);
    assert_eq!(
        turn.root,
        AppState::canonical_root(session.scratch_dir()),
        "a queued turn's root is canonical at construction, so the key it is on its way to is a field read"
    );
    assert_eq!(turn.phase, "route");
    assert!(turn.said().cold.contains("fix the login redirect"));
    assert!(turn.said().cold.contains("dispatch_branch"));
    assert_eq!(
        turn.said().cold,
        turn.said().warm,
        "a router is one decision long: there is no conversation to continue"
    );
    assert_eq!(capture_record(&mut state, &capture_id)["state"], "routing");
}

/// One capture, one router. Everything that can ask for a route asks
/// through one door, and a capture already being decided is left to the
/// router deciding it.
#[test]
fn a_capture_being_routed_is_never_given_a_second_router() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (capture_id, agent_id) = captured(&mut state, "ship it");
    state.delivery_queue.clear_queued();

    state.begin_routing(&capture_id).unwrap();
    state.begin_routing(&capture_id).unwrap();

    assert_eq!(state.router_sessions[&capture_id].agent_id(), agent_id);
    assert!(
        state.delivery_queue.queued_is_empty(),
        "the router already deciding this capture is the one deciding it"
    );
}

/// The confident destination. `dispatch_branch` is the one-call handoff, so
/// the router never owns a half-built branch.
#[test]
fn dispatch_branch_puts_an_agent_on_a_branch_and_writes_the_route_through() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let (capture_id, _) = captured(&mut state, "finish the toast on the login branch");
    state.delivery_queue.clear_queued();

    let dispatched = state
        .router_action(
            &capture_id,
            BridgeAction::DispatchBranch {
                project_id: project_id.clone(),
                branch: None,
                name: "Branch Worker".to_string(),
                instruction: "finish the toast".to_string(),
                rationale: Some("continues the login work".to_string()),
            },
        )
        .unwrap();
    let branch = dispatched["branch"].as_str().unwrap().to_string();
    assert!(dispatched["run_id"].is_string());
    assert_eq!(
        state.delivery_queue.queued_len(),
        1,
        "a dispatch is an agent already working"
    );
    assert_eq!(
        state.delivery_queue.queued_nth(0).unwrap().owner,
        dispatched["run_id"].as_str().unwrap(),
        "the only turn is the dispatch's own"
    );
    assert!(
        state.plans.is_empty(),
        "a branch route files no task, so routing adds no planning turn to what \
         the dispatch already queued"
    );

    let record = capture_record(&mut state, &capture_id);
    assert_eq!(record["state"], "routed");
    assert_eq!(record["routing"]["kind"], "branch");
    assert_eq!(record["routing"]["target_id"], branch.as_str());
    assert_eq!(record["routing"]["rationale"], "continues the login work");
}

#[test]
fn router_dispatch_tool_names_the_agent_it_creates_through_the_real_wire() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let (capture_id, router_id) = captured(&mut state, "finish the toast");
    let server = crate::mcp::DoneServer::for_owner(router_id);
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": "dispatch_branch",
            "arguments": {
                "project_id": project_id,
                "name": "  Toast   Fixer  ",
                "instruction": "finish the toast"
            }
        }
    });
    let handled = server.handle_message(&call.to_string());
    assert!(handled.reply.is_none(), "the real router tool is forwarded");
    let dispatched = state
        .router_action(&capture_id, handled.action.unwrap())
        .unwrap();
    let run_id = dispatched["run_id"].as_str().unwrap();
    let agent_id = dispatched["agent_id"].as_str().unwrap();
    let agent = state.runs[run_id].agents.by_id(agent_id).unwrap();
    assert_eq!(agent.name.as_deref(), Some("Toast Fixer"));
    assert!(agent.name_asked);
}

/// A question is not a route: the capture goes back to where the router
/// picks work up, and says what it needs from the user.
#[test]
fn ask_user_puts_the_question_on_the_captures_own_row() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (capture_id, _) = captured(&mut state, "make the thing faster");

    state
        .router_action(
            &capture_id,
            BridgeAction::AskUser {
                question: "which project is this about?".to_string(),
                options: Vec::new(),
            },
        )
        .unwrap();

    let record = capture_record(&mut state, &capture_id);
    assert_eq!(record["state"], "unrouted", "nothing was routed");
    assert_eq!(record["question"]["text"], "which project is this about?");
    assert_eq!(record["question"]["answer"], Value::Null);

    let row = capture_rows(&mut state).remove(0);
    assert_eq!(row["unread"], true);
    assert_eq!(row["unread_reason"], "router_question");
    assert_eq!(
        row["question"]["options"],
        json!([]),
        "a question with no options is still a question"
    );
}

/// The two choices a router thought of, offered beside the question — and
/// carried to every surface that shows the capture, because a choice the
/// client cannot see is a choice nobody can tap.
fn asked_with_two_options(state: &mut AppState, capture_id: &str) {
    state
        .router_action(
            capture_id,
            BridgeAction::AskUser {
                question: "which project is this about?".to_string(),
                options: vec![
                    crate::capture::CaptureOptionDraft {
                        label: "File as a task on Build".to_string(),
                        project_id: Some("proj-build".to_string()),
                        kind: Some(crate::capture::CaptureTarget::Task),
                        branch: None,
                    },
                    crate::capture::CaptureOptionDraft {
                        label: "New branch on Do".to_string(),
                        project_id: Some("proj-do".to_string()),
                        kind: Some(crate::capture::CaptureTarget::Branch),
                        branch: None,
                    },
                ],
            },
        )
        .unwrap();
}

/// The router's suggestions reach `capture.get` and the feed row, numbered
/// and whole. Anything less and the decision surface has a question with no
/// buttons under it.
#[test]
fn the_options_a_router_offers_reach_every_surface_that_shows_the_capture() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (capture_id, _) = captured(&mut state, "make the thing faster");
    asked_with_two_options(&mut state, &capture_id);

    let expected = json!([
        {
            "id": "option-1",
            "label": "File as a task on Build",
            "project_id": "proj-build",
            "kind": "task",
            "branch": Value::Null,
        },
        {
            "id": "option-2",
            "label": "New branch on Do",
            "project_id": "proj-do",
            "kind": "branch",
            "branch": Value::Null,
        },
    ]);

    let record = capture_record(&mut state, &capture_id);
    assert_eq!(record["question"]["options"], expected);
    assert_eq!(record["question"]["chosen_option_id"], Value::Null);

    let row = capture_rows(&mut state).remove(0);
    assert_eq!(row["question"]["options"], expected);

    // And a fresh daemon over the same store still has the offer.
    let mut rebooted = qa_state(&repo, dir.path());
    assert_eq!(
        capture_record(&mut rebooted, &capture_id)["question"]["options"],
        expected
    );
}

/// Tapping a choice answers the question in words the router can act on:
/// the label the user saw, and the destination it stood for.
#[test]
fn choosing_an_option_answers_the_router_in_its_own_terms() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (capture_id, _) = captured(&mut state, "make the thing faster");
    asked_with_two_options(&mut state, &capture_id);
    state.settle_router_session(&capture_id);
    state.delivery_queue.clear_queued();

    let answered = state.handle(req(
        "capture.answer",
        json!({ "capture_id": capture_id, "option_id": "option-2" }),
    ));
    assert_eq!(answered["ok"], true, "{answered:?}");
    let question = &answered["result"]["question"];
    assert_eq!(
        question["answer"],
        "New branch on Do — route this to project proj-do as a branch"
    );
    assert_eq!(question["chosen_option_id"], "option-2");
    assert_eq!(
        question["options"].as_array().unwrap().len(),
        2,
        "what the user was shown stays on the record"
    );

    let turn = state
        .delivery_queue
        .queued()
        .find(|turn| turn.owner == capture_id)
        .expect("the router is re-fired with the choice in hand");
    assert!(turn.said().cold.contains("proj-do"), "{}", turn.said().cold);
    assert!(
        turn.said().cold.contains("New branch on Do"),
        "{}",
        turn.said().cold
    );
}

/// A client that tracked positions rather than ids taps the same choice.
#[test]
fn an_option_can_be_chosen_by_position() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (capture_id, _) = captured(&mut state, "make the thing faster");
    asked_with_two_options(&mut state, &capture_id);
    state.settle_router_session(&capture_id);

    let answered = state.handle(req(
        "capture.answer",
        json!({ "capture_id": capture_id, "option_index": 0 }),
    ));
    assert_eq!(answered["ok"], true, "{answered:?}");
    assert_eq!(
        answered["result"]["question"]["chosen_option_id"], "option-1",
        "position 0 is the first option offered"
    );
}

/// The keyboard never goes away: a question that offered choices still
/// takes words, and words are not recorded as a tap.
#[test]
fn free_form_answers_a_question_that_offered_options() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (capture_id, _) = captured(&mut state, "make the thing faster");
    asked_with_two_options(&mut state, &capture_id);
    state.settle_router_session(&capture_id);

    let answered = state.handle(req(
        "capture.answer",
        json!({ "capture_id": capture_id, "text": "neither, it is the relay" }),
    ));
    assert_eq!(answered["ok"], true, "{answered:?}");
    assert_eq!(
        answered["result"]["question"]["answer"],
        "neither, it is the relay"
    );
    assert_eq!(
        answered["result"]["question"]["chosen_option_id"],
        Value::Null
    );
}

/// A tap that names nothing is refused rather than quietly read as an
/// answer of some other kind: the user believes the tap landed.
#[test]
fn an_option_nobody_offered_is_refused() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (capture_id, _) = captured(&mut state, "make the thing faster");
    asked_with_two_options(&mut state, &capture_id);
    state.settle_router_session(&capture_id);

    for chosen in [
        json!({ "capture_id": capture_id, "option_id": "option-9" }),
        json!({ "capture_id": capture_id, "option_index": 7 }),
        json!({ "capture_id": capture_id, "option_id": "option-9", "text": "the relay" }),
    ] {
        let missed = state.handle(req("capture.answer", chosen.clone()));
        assert_eq!(missed["ok"], false, "{chosen}: {missed:?}");
    }
    assert_eq!(
        capture_record(&mut state, &capture_id)["question"]["answer"],
        Value::Null
    );
}

/// Four choices is a menu. The router is told so, and the question is not
/// asked with three of them and the fourth quietly dropped.
#[test]
fn a_router_offering_more_than_three_options_is_refused_the_question() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (capture_id, _) = captured(&mut state, "make the thing faster");

    let too_many = state.router_action(
        &capture_id,
        BridgeAction::AskUser {
            question: "which project is this about?".to_string(),
            options: (1..=4)
                .map(|n| crate::capture::CaptureOptionDraft {
                    label: format!("project {n}"),
                    ..crate::capture::CaptureOptionDraft::default()
                })
                .collect(),
        },
    );
    assert!(too_many.is_err(), "{too_many:?}");
    assert_eq!(
        capture_record(&mut state, &capture_id)["question"],
        Value::Null,
        "a question Build refused is a question nobody was asked"
    );
}

/// The way out of a decision. A capture nobody wants routed stops the
/// router, leaves the feed, and takes its record with it.
#[test]
fn cancelling_a_capture_ends_the_routing_and_removes_it() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (capture_id, _) = captured(&mut state, "make the thing faster");
    asked_with_two_options(&mut state, &capture_id);
    let scratch = state.router_sessions[&capture_id]
        .scratch_dir()
        .to_path_buf();

    let cancelled = state.handle(req("capture.cancel", json!({ "capture_id": capture_id })));
    assert_eq!(cancelled["ok"], true, "{cancelled:?}");
    assert_eq!(cancelled["result"]["cancelled"], true);

    assert!(!state.router_sessions.contains_key(&capture_id));
    assert!(!scratch.exists(), "the router's scratch goes with it");
    assert_eq!(capture_rows(&mut state), Vec::<Value>::new());
    assert_eq!(
        Store::new(dir.path().join("store"))
            .expect("store opens")
            .load_all_captures()
            .unwrap(),
        Vec::new(),
        "and a reboot does not bring it back"
    );

    let again = state.handle(req("capture.cancel", json!({ "capture_id": capture_id })));
    assert_eq!(again["ok"], false, "{again:?}");
}

/// The answer comes back and the router looks again — with the answer in
/// the prompt, because that is the whole reason it asked.
#[test]
fn an_answer_re_fires_the_router_with_the_answer_in_hand() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (capture_id, primary_agent) = captured(&mut state, "make the thing faster");
    state
        .router_action(
            &capture_id,
            BridgeAction::AskUser {
                question: "which project is this about?".to_string(),
                options: Vec::new(),
            },
        )
        .unwrap();
    // The asking router reported and went away, as a router that asked does.
    state.on_router_done(
        &capture_id,
        DoneReport::new(DoneStatus::Completed, "asked which project"),
    );
    assert!(!state.router_sessions.contains_key(&capture_id));
    assert_eq!(
        capture_record(&mut state, &capture_id)["state"],
        "unrouted",
        "a router that asked is not a router that failed"
    );
    state.delivery_queue.clear_queued();

    let answered = state.handle(req(
        "capture.answer",
        json!({ "capture_id": capture_id, "text": "the bridge" }),
    ));
    assert_eq!(answered["ok"], true, "{answered:?}");
    assert_eq!(answered["result"]["state"], "routing");
    assert_eq!(answered["result"]["question"]["answer"], "the bridge");

    let session = state.router_sessions[&capture_id].clone();
    assert_ne!(
        session.agent_id(),
        primary_agent,
        "a fresh session decides again"
    );
    let turn = state
        .delivery_queue
        .queued()
        .find(|turn| turn.owner == capture_id)
        .expect("the router is re-fired");
    assert!(
        turn.said().cold.contains("the bridge"),
        "{}",
        turn.said().cold
    );
    assert!(turn.said().cold.contains("make the thing faster"));
}

#[test]
fn an_answer_needs_a_question_and_some_words() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (capture_id, _) = captured(&mut state, "ship it");

    let unasked = state.handle(req(
        "capture.answer",
        json!({ "capture_id": capture_id, "text": "the bridge" }),
    ));
    assert_eq!(unasked["ok"], false, "{unasked:?}");

    state
        .router_action(
            &capture_id,
            BridgeAction::AskUser {
                question: "which project?".to_string(),
                options: Vec::new(),
            },
        )
        .unwrap();
    let blank = state.handle(req(
        "capture.answer",
        json!({ "capture_id": capture_id, "text": "  " }),
    ));
    assert_eq!(blank["ok"], false, "{blank:?}");
}

/// The router's harness is killed on a thread of its own, so the wipe of
/// the directory it was writing into waits for that thread: a
/// `remove_dir_all` a child is still creating files under fails the walk,
/// and the walk itself has no business under the app mutex. The cancel
/// answers first, the place is free at once for the router a re-fire puts
/// there, and the files go once the process is reaped.
#[test]
fn cancelling_a_capture_wipes_its_scratch_once_the_router_is_reaped() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let (capture_id, agent_id) = captured(&mut app, "make the thing faster");
    // The router's first turn already reached the harness below.
    app.delivery_queue.clear_queued();
    let scratch = app.router_sessions[&capture_id].scratch_dir().to_path_buf();
    let root = AppState::canonical_root(&scratch);
    let (death, death_handle) = OffLockGate::new();
    app.session_registry.test_insert_tab(
        TabKey::agent(&root, &agent_id),
        gated_tab(
            &root,
            gated_agent_role(&agent_id),
            GatedHarness::new().refusing_to_die_until(death),
        ),
    );
    let state = app.shared();

    let cancelled = frame_on_a_thread(
        &state,
        "s-cancel",
        "capture.cancel",
        json!({ "capture_id": capture_id }),
    )
    .recv_timeout(Duration::from_secs(5))
    .expect("the cancel answers before the router is reaped");
    assert_eq!(cancelled["ok"], true, "{cancelled:?}");
    death_handle.wait_for_arrival();
    let retiring_dirs = || {
        let mark = format!("{capture_id}{}", crate::reaper::RETIRING_DIR_MARK);
        std::fs::read_dir(scratch.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&mark))
            .count()
    };
    assert!(
        !scratch.exists(),
        "the place is free the moment the cancel answers"
    );
    assert_eq!(
        retiring_dirs(),
        1,
        "the files are not removed out from under a process still writing them"
    );
    assert!(
        state.try_lock().is_ok(),
        "the wait for the router is holding the app mutex"
    );

    death_handle.release();
    assert!(
        settles(|| retiring_dirs() == 0),
        "the files go once the router is reaped"
    );
}

/// A capture cancelled while its router's first turn is still on its way
/// has no session for that turn to reach: the delivery spawns nothing, and
/// the scratch a spawn would scaffold into is not brought back.
#[test]
fn a_capture_cancelled_before_its_router_spawns_gets_no_router() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let (capture_id, agent_id) = captured(&mut app, "make the thing faster");
    let scratch = app.router_sessions[&capture_id].scratch_dir().to_path_buf();
    let cancelled = app.handle(req("capture.cancel", json!({ "capture_id": capture_id })));
    assert_eq!(cancelled["ok"], true, "{cancelled:?}");
    let state = app.shared();

    deliver_pending_agent_turns(&state);

    let s = state.lock().unwrap();
    assert!(
        !s.session_registry
            .test_tabs()
            .map(|(_, tab)| tab)
            .any(|tab| tab.role.agent().is_some_and(|(_, id)| id == agent_id)),
        "a router was spawned for a capture nobody wants routed"
    );
    assert!(!scratch.exists(), "the spawn scaffolded the scratch back");
}

/// A router that stops without deciding leaves the capture needing the
/// user, with a retry — and takes its scratch directory with it.
#[test]
fn a_router_that_reports_without_routing_marks_the_capture_failed() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (capture_id, _) = captured(&mut state, "ship it");
    let scratch = state.router_sessions[&capture_id]
        .scratch_dir()
        .to_path_buf();

    state.on_router_done(
        &capture_id,
        DoneReport::new(DoneStatus::Failed, "nothing here says which project"),
    );

    let record = capture_record(&mut state, &capture_id);
    assert_eq!(record["state"], "failed");
    assert!(!state.router_sessions.contains_key(&capture_id));
    assert!(!scratch.exists(), "the scratch goes with the session");

    let row = capture_rows(&mut state).remove(0);
    assert_eq!(row["unread"], true);
    assert_eq!(row["unread_reason"], "routing_failed");
}

/// A router process that died mid-decision told nobody, so the sweep is
/// what turns "no process" into a capture the user can act on. A session
/// still on its way to a harness has no process to have lost.
#[test]
fn the_sweep_fails_a_capture_whose_router_died_and_spares_one_still_starting() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (starting, _) = captured(&mut state, "still on its way");
    let (died, _) = captured(&mut state, "its router died");

    assert!(
        state.reap_finished_router_sessions().is_empty(),
        "nothing has started, so nothing has stopped"
    );

    // The one whose harness came up, and then went away with it.
    state.router_sessions.get_mut(&died).unwrap().mark_started();
    assert_eq!(state.reap_finished_router_sessions(), vec![died.clone()]);

    assert_eq!(capture_record(&mut state, &died)["state"], "failed");
    assert!(!state.router_sessions.contains_key(&died));
    assert_eq!(
        capture_record(&mut state, &starting)["state"],
        "routing",
        "a spawn in flight is not a dead router"
    );
    assert!(state.router_sessions.contains_key(&starting));
}

/// Rerouting to a branch takes the branch's name. The user moving a
/// misroute usually knows exactly where it should have gone, and a
/// destination picker that cannot say which branch is not a destination
/// picker. With no name the words still name it, as they do for the router.
#[test]
fn rerouting_to_a_branch_dispatches_onto_the_branch_it_names() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let (named, _) = captured(&mut state, "add the CSV export");
    let (unnamed, _) = captured(&mut state, "add the CSV export");

    let rerouted = state.handle(req(
        "capture.reroute",
        json!({
            "capture_id": named,
            "project_id": project_id,
            "kind": "branch",
            "branch": "build/csv-export",
        }),
    ));
    assert_eq!(rerouted["ok"], true, "{rerouted:?}");
    assert_eq!(rerouted["result"]["routing"]["kind"], "branch");
    assert_eq!(
        rerouted["result"]["routing"]["target_id"], "build/csv-export",
        "the capture reads as routed to the branch the user named: {rerouted:?}"
    );
    assert!(local_branch_exists(&repo, "build/csv-export").unwrap());

    let by_words = state.handle(req(
        "capture.reroute",
        json!({ "capture_id": unnamed, "project_id": project_id, "kind": "branch" }),
    ));
    assert_eq!(by_words["ok"], true, "{by_words:?}");
    assert_eq!(
        by_words["result"]["routing"]["target_id"], "build/add-the-csv-export",
        "{by_words:?}"
    );
}

/// The user's own reroute reaches a branch through the same drain: it is an
/// ordinary frame, and the checkout it cuts must not hold the daemon still
/// while it is being made.
#[test]
fn rerouting_a_capture_to_a_branch_cuts_it_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.project_at(0).id.clone();
    let (capture_id, _) = captured(&mut app, "add the CSV export");
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let rerouted = frame_on_a_thread(
        &state,
        "s-reroute",
        "capture.reroute",
        json!({ "capture_id": capture_id, "project_id": project_id, "kind": "branch" }),
    );
    gate_handle.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "the reroute is holding the app mutex through its git"
    );
    let board = frame_on_a_thread(&state, "s-board", "board.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .expect("the board answers while the reroute cuts its branch");
    assert_eq!(board["ok"], true, "{board:?}");

    gate_handle.release();
    let rerouted = rerouted
        .recv_timeout(Duration::from_secs(30))
        .expect("the reroute answers once its git is done");
    assert_eq!(rerouted["ok"], true, "{rerouted:?}");
    assert_eq!(
        rerouted["result"]["routing"]["target_id"], "build/add-the-csv-export",
        "the reroute still answers with the capture's own row: {rerouted:?}"
    );
}

/// The app mutex is free while a dispatch cuts its branch, so the capture
/// it was routed from can be cancelled meanwhile. A route that cannot be
/// written down refuses BEFORE the run is durable: nothing is ever both
/// persisted and reported as a failure, and the checkout the git made
/// stays on the board as the unowned card it is.
#[test]
fn a_capture_cancelled_while_its_dispatch_cuts_the_branch_refuses_before_the_run_is_durable() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.project_at(0).id.clone();
    app.scan_external_worktrees_now(&project_id).unwrap();
    let (capture_id, _) = captured(&mut app, "add the CSV export");
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();

    let rerouted = frame_on_a_thread(
        &state,
        "s-reroute",
        "capture.reroute",
        json!({ "capture_id": capture_id, "project_id": project_id, "kind": "branch" }),
    );
    gate_handle.wait_for_arrival();
    let cancelled = frame_on_a_thread(
        &state,
        "s-cancel",
        "capture.cancel",
        json!({ "capture_id": capture_id }),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("the cancel answers while the reroute cuts its branch");
    assert_eq!(cancelled["ok"], true, "{cancelled:?}");

    gate_handle.release();
    let rerouted = rerouted
        .recv_timeout(Duration::from_secs(30))
        .expect("the reroute answers once its git is done");
    assert_eq!(rerouted["ok"], false, "{rerouted:?}");
    assert!(
        rerouted["error"]
            .as_str()
            .unwrap()
            .contains("unknown capture_id"),
        "{rerouted:?}"
    );

    let state = state.lock().unwrap();
    assert!(
        state.runs.is_empty(),
        "the refused dispatch left a run in memory"
    );
    let durable = state.store.as_ref().unwrap().load_all_runs().unwrap();
    assert!(
        durable.is_empty(),
        "the refused dispatch left {} run(s) in the store",
        durable.len()
    );
    assert!(
        state
            .delivery_queue
            .queued()
            .all(|turn| !turn.owner.starts_with("run-")),
        "the refused dispatch left a turn queued for a run that does not exist"
    );
    assert!(
        state.pending_rows.is_empty(),
        "the refused dispatch left its row on the board"
    );
    assert!(
        state.external_scan_of(&project_id).is_some_and(|cache| {
            cache
                .worktrees
                .iter()
                .any(|worktree| worktree.branch.as_deref() == Some("build/add-the-csv-export"))
        }),
        "the checkout the git cut is not on the board: {:?}",
        state
            .external_scan_of(&project_id)
            .map(|cache| &cache.worktrees)
    );
}

/// A failed route's retry is the same door: no destination named, so the
/// router decides again.
#[test]
fn a_reroute_with_no_destination_re_fires_the_router() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (capture_id, primary_agent) = captured(&mut state, "ship it");
    state.on_router_done(
        &capture_id,
        DoneReport::new(DoneStatus::Failed, "could not decide"),
    );
    assert_eq!(capture_record(&mut state, &capture_id)["state"], "failed");
    state.delivery_queue.clear_queued();

    let retried = state.handle(req("capture.reroute", json!({ "capture_id": capture_id })));
    assert_eq!(retried["ok"], true, "{retried:?}");
    assert_eq!(retried["result"]["state"], "routing");
    assert_ne!(state.router_sessions[&capture_id].agent_id(), primary_agent);
    assert!(state
        .delivery_queue
        .queued()
        .any(|turn| turn.owner == capture_id));
}

/// The two surfaces are enforced where the frames arrive, not only in the
/// tool list a session is shown: a harness that writes its own frames still
/// only reaches the surface it was spawned on.
#[test]
fn neither_session_kind_can_call_the_others_tools() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let goal = "add a greeting";
    let task_id = file_legacy_task(&mut state, goal);
    let (capture_id, _) = captured(&mut state, "ship it");

    let coding_reaching_out = state
        .on_mcp_action(&task_id, BridgeAction::ListProjects)
        .unwrap_err();
    assert!(
        coding_reaching_out.contains("list_projects")
            && coding_reaching_out.contains("router tool"),
        "{coding_reaching_out}"
    );

    let router_reaching_in = state
        .router_action(
            &capture_id,
            BridgeAction::SetTopic {
                topic: "routing".to_string(),
            },
        )
        .unwrap_err();
    assert!(
        router_reaching_in.contains("set_topic") && router_reaching_in.contains("coding tool"),
        "{router_reaching_in}"
    );
}

/// The read half of the router's surface: every project on the device, the
/// work in flight across all of them, and one work item's conversation.
#[test]
fn the_router_reads_across_every_project_and_writes_to_none_of_them() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "add-a-greeting");
    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "body": "add a greeting" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let (capture_id, _) = captured(&mut state, "ship it");

    let projects = state
        .router_action(&capture_id, BridgeAction::ListProjects)
        .unwrap();
    assert_eq!(projects["projects"].as_array().unwrap().len(), 1);

    let work = state
        .router_action(&capture_id, BridgeAction::ListWork)
        .unwrap();
    let rows = work["work"].as_array().unwrap();
    assert!(
        rows.iter().any(|row| row["entity_id"] == run_id.as_str()),
        "the adopted run in flight is what the router checks a capture against: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| row["kind"] != "capture"),
        "a capture is not work yet: {rows:?}"
    );

    let conversation = state
        .router_action(
            &capture_id,
            BridgeAction::ReadConversation {
                entity_id: run_id.clone(),
                agent_id: None,
                limit: 10,
            },
        )
        .unwrap();
    assert_eq!(conversation["entity_id"], run_id.as_str());
    assert!(conversation["transcript"]
        .as_str()
        .unwrap()
        .contains("add a greeting"));

    let unknown = state.router_action(
        &capture_id,
        BridgeAction::ReadConversation {
            entity_id: "run-nowhere".to_string(),
            agent_id: None,
            limit: 10,
        },
    );
    assert!(unknown.is_err(), "{unknown:?}");
}

// ==== attribution: whose words the dispatched agent is holding ============

/// The instruction a dispatch left on its agent's thread, and the agent that
/// sent it — `None` when the words are the human's own.
fn dispatched_from(state: &AppState, run_id: &str, agent_id: &str) -> Option<String> {
    let agent = state.runs[run_id]
        .agents
        .by_id(agent_id)
        .unwrap_or_else(|| panic!("{agent_id} is on {run_id}'s roster"));
    match agent.thread.items.first().expect("the instruction") {
        crate::thread::ThreadItem::Message(message) => {
            assert_eq!(message.role, crate::thread::MessageRole::User);
            message.from_agent.as_ref().map(|from| from.id.clone())
        }
        other => panic!("the first item is the instruction, not {other:?}"),
    }
}

/// When the human last acted on a run, as the inbox records it.
fn last_interaction(state: &AppState, run_id: &str) -> Option<String> {
    state
        .board
        .attention()
        .attention(run_id)
        .and_then(|attention| attention.last_interaction_at.clone())
}

/// The router hands work over, and the agent it hands it to is told who is
/// speaking: the message is on the user's side of the conversation because
/// that is the side an instruction arrives on, and it names the router.
#[test]
fn a_router_dispatch_says_which_agent_sent_the_instruction() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let (capture_id, router_id) = captured(&mut state, "finish the toast on the login branch");

    let dispatched = state
        .router_action(
            &capture_id,
            BridgeAction::DispatchBranch {
                project_id,
                branch: None,
                name: "Branch Worker".to_string(),
                instruction: "finish the toast".to_string(),
                rationale: None,
            },
        )
        .unwrap();

    let run_id = dispatched["run_id"].as_str().unwrap().to_string();
    let agent_id = dispatched["agent_id"].as_str().unwrap().to_string();
    assert_eq!(
        dispatched_from(&state, &run_id, &agent_id),
        Some(router_id),
        "the words came from the router, not from the user"
    );
    assert_eq!(
        last_interaction(&state, &run_id),
        None,
        "one agent telling another is the work happening, not the human acting"
    );
}

/// The same dispatch from the browser is the human's own: nothing claims to
/// have sent it for them, and the inbox records that they acted.
#[test]
fn a_dispatch_the_human_made_carries_no_sender() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();

    let dispatched = state.handle(req(
        "branch.dispatch",
        json!({ "project_id": project_id, "instruction": "finish the toast" }),
    ));
    assert_eq!(dispatched["ok"], true, "{dispatched:?}");

    let run_id = dispatched["result"]["run_id"].as_str().unwrap().to_string();
    let agent_id = dispatched["result"]["agent_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(dispatched_from(&state, &run_id, &agent_id), None);
    assert!(
        last_interaction(&state, &run_id).is_some(),
        "the human dispatched this one themselves"
    );
}
