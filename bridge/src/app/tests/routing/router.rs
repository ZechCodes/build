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
        .pending_agent_turns
        .iter()
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
    state.pending_agent_turns.clear();

    state.begin_routing(&capture_id).unwrap();
    state.begin_routing(&capture_id).unwrap();

    assert_eq!(state.router_sessions[&capture_id].agent_id(), agent_id);
    assert!(
        state.pending_agent_turns.is_empty(),
        "the router already deciding this capture is the one deciding it"
    );
}

/// The default destination. `create_issue` files the issue AND starts its
/// planning agent: the capture text arrives on the issue's conversation as
/// a message the user sent, and a sent message nobody hears is the bug this
/// closes. The capture's record still says where it went and why.
#[test]
fn routing_a_capture_to_an_issue_starts_its_planning_agent_on_the_primary_checkout() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let (capture_id, _) = captured(&mut state, "fix the login redirect");

    let filed = state
        .router_action(
            &capture_id,
            BridgeAction::CreateIssue {
                project_id: project_id.clone(),
                goal: "fix the login redirect".to_string(),
                rationale: Some("no branch names this work".to_string()),
            },
        )
        .unwrap();
    let issue_id = filed["issue_id"].as_str().unwrap().to_string();

    assert_eq!(filed["planning"], true, "{filed:?}");
    assert_ne!(
        state.plans[&issue_id].plan.state,
        PlanState::Created,
        "the routed issue is being planned, not sitting inert"
    );
    assert_eq!(
        AppState::canonical_root(
            &state.plans[&issue_id]
                .workspace
                .as_ref()
                .expect("the planning agent has a workspace")
                .checkout
        ),
        AppState::canonical_root(&repo),
        "an issue's agent works in the primary checkout"
    );

    let turns: Vec<&PendingAgentTurn> = state
        .pending_agent_turns
        .iter()
        .filter(|turn| turn.owner == issue_id)
        .collect();
    assert_eq!(turns.len(), 1, "exactly one first turn: {}", turns.len());
    assert_eq!(turns[0].root, AppState::canonical_root(&repo));
    assert_eq!(
        turns[0].agent_id,
        state.plans[&issue_id].agents.primary().unwrap().id
    );
    assert_eq!(turns[0].phase, "plan");
    assert!(
        turns[0].said().cold.contains("fix the login redirect"),
        "the turn carries what the user said: {}",
        turns[0].said().cold
    );

    let record = capture_record(&mut state, &capture_id);
    assert_eq!(record["state"], "routed");
    assert_eq!(record["routing"]["kind"], "issue");
    assert_eq!(record["routing"]["target_id"], issue_id.as_str());
    assert_eq!(record["routing"]["project_id"], project_id.as_str());
    assert_eq!(record["routing"]["rationale"], "no branch names this work");

    // On disk, not just in this process.
    let on_disk = Store::new(dir.path().join("store"))
        .expect("store opens")
        .load_all_captures()
        .unwrap();
    assert_eq!(
        on_disk[0].routing.as_ref().unwrap().target_id,
        issue_id,
        "the route survives the daemon that made it"
    );
}

/// One route, one agent. A turn already queued for this issue's agent — or
/// a spawn already on its way to a harness — is the session that reads the
/// capture; a second would report `done` for the same issue twice.
#[test]
fn a_routed_issue_whose_agent_is_already_coming_is_not_started_twice() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let (capture_id, _) = captured(&mut state, "fix the login redirect");

    let filed = state
        .router_action(
            &capture_id,
            BridgeAction::CreateIssue {
                project_id: project_id.clone(),
                goal: "fix the login redirect".to_string(),
                rationale: None,
            },
        )
        .unwrap();
    let issue_id = filed["issue_id"].as_str().unwrap().to_string();

    // The turn from the route is still queued.
    assert!(
        routed_planning_start(&mut state, &issue_id, &capture_id).is_none(),
        "the issue already has its session"
    );
    assert_eq!(
        state
            .pending_agent_turns
            .iter()
            .filter(|turn| turn.owner == issue_id)
            .count(),
        1,
        "the turn already queued is the one that reads the capture"
    );

    // The queue drained and the turn is mid-delivery, its harness coming.
    let mut delivering = state.take_pending_turns();
    assert!(
        routed_planning_start(&mut state, &issue_id, &capture_id).is_none(),
        "a harness already coming up is the one that reads the capture"
    );
    assert!(
        state.pending_agent_turns.is_empty(),
        "a harness already coming up is the one that reads the capture"
    );

    // And once nothing is coming, the issue that already has its session
    // is still not restarted: starting is a first turn, not a nudge.
    while let Some((_, mark)) = delivering.next_turn() {
        mark.settle(&mut state);
    }
    assert!(
        routed_planning_start(&mut state, &issue_id, &capture_id).is_none(),
        "an issue with a planning session already open is not dispatched again"
    );
    assert!(
        state.pending_agent_turns.is_empty(),
        "an issue with a planning session already open is not dispatched again"
    );
}

/// A planning workspace that cannot be written never fails the route: the
/// capture is recorded and the Issue holds the text, so the route answers
/// with an inert Issue that says no agent is reading it — and once the
/// disk is fixed, the same Issue starts. The refusal travels the whole way
/// through `PlanWorkspaceRefused` and `RoutedIssueDrafting::refused`,
/// which is the one override of the trait's `Err`.
#[test]
fn routing_to_an_issue_whose_workspace_cannot_be_written_keeps_the_route() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let (capture_id, _) = captured(&mut state, "fix the login redirect");
    // Every Issue's scratch docs dir is cut under this root; a plain file
    // standing there fails `create_dir_all` for any Issue.
    let docs_root = dir.path().join("wt").join(&project_id).join(".issue-docs");
    std::fs::create_dir_all(docs_root.parent().unwrap()).unwrap();
    std::fs::write(&docs_root, "not a directory").unwrap();

    let filed = state
        .router_action(
            &capture_id,
            BridgeAction::CreateIssue {
                project_id: project_id.clone(),
                goal: "fix the login redirect".to_string(),
                rationale: None,
            },
        )
        .expect("an unwritable workspace never fails the route");
    let issue_id = filed["issue_id"].as_str().unwrap().to_string();

    assert_eq!(filed["planning"], false, "{filed:?}");
    assert_eq!(
        state.plans[&issue_id].plan.state,
        PlanState::Created,
        "the Issue is inert, not half-started"
    );
    assert!(
        state.plans[&issue_id].workspace.is_none(),
        "no workspace was written"
    );
    assert!(
        !state
            .pending_agent_turns
            .iter()
            .any(|turn| turn.owner == issue_id),
        "no turn was queued for an agent that has nowhere to work"
    );
    assert!(
        state.pending_rows.is_empty(),
        "the refused workspace left its row on the board"
    );
    let record = capture_record(&mut state, &capture_id);
    assert_eq!(record["state"], "routed");
    assert_eq!(record["routing"]["kind"], "issue");
    assert_eq!(record["routing"]["target_id"], issue_id.as_str());

    // Re-startable: with the disk fixed, the same Issue's session opens.
    std::fs::remove_file(&docs_root).unwrap();
    let job = routed_planning_start(&mut state, &issue_id, &capture_id)
        .expect("an inert Issue has a session to start");
    state
        .run_lifecycle_here(job)
        .expect("the session opens once the disk is fixed");
    assert_ne!(state.plans[&issue_id].plan.state, PlanState::Created);
    assert!(state.plans[&issue_id].workspace.is_some());
    assert_eq!(
        state
            .pending_agent_turns
            .iter()
            .filter(|turn| turn.owner == issue_id)
            .count(),
        1
    );
}

/// What a route finds when it asks for a planning session a second time.
/// `None` is "nothing to start", which is the whole answer this is asked
/// for: a job would mean a second harness on the same issue.
fn routed_planning_start(
    state: &mut AppState,
    issue_id: &str,
    capture_id: &str,
) -> Option<WorktreeLifecycleJob> {
    let project_id = state.project_of(issue_id).expect("the issue has a project");
    state
        .reserve_plan_drafting(
            issue_id,
            Box::new(RoutedIssueDrafting {
                issue_id: issue_id.to_string(),
                project_id,
                capture_id: capture_id.to_string(),
                answer: capture_after_routing,
            }),
        )
        .expect("the issue is on the board")
}

/// The confident destination. `dispatch_branch` is the one-call handoff, so
/// the router never owns a half-built branch.
#[test]
fn dispatch_branch_puts_an_agent_on_a_branch_and_writes_the_route_through() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let (capture_id, _) = captured(&mut state, "finish the toast on the login branch");
    state.pending_agent_turns.clear();

    let dispatched = state
        .router_action(
            &capture_id,
            BridgeAction::DispatchBranch {
                project_id: project_id.clone(),
                branch: None,
                instruction: "finish the toast".to_string(),
                rationale: Some("continues the login work".to_string()),
            },
        )
        .unwrap();
    let branch = dispatched["branch"].as_str().unwrap().to_string();
    assert!(dispatched["run_id"].is_string());
    assert_eq!(
        state.pending_agent_turns.len(),
        1,
        "a dispatch is an agent already working"
    );
    assert_eq!(
        state.pending_agent_turns[0].owner,
        dispatched["run_id"].as_str().unwrap(),
        "the only turn is the dispatch's own"
    );
    assert!(
        state.plans.is_empty(),
        "a branch route files no issue, so routing adds no planning turn to what \
         the dispatch already queued"
    );

    let record = capture_record(&mut state, &capture_id);
    assert_eq!(record["state"], "routed");
    assert_eq!(record["routing"]["kind"], "branch");
    assert_eq!(record["routing"]["target_id"], branch.as_str());
    assert_eq!(record["routing"]["rationale"], "continues the login work");
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
                        label: "File as an issue on Build".to_string(),
                        project_id: Some("proj-build".to_string()),
                        kind: Some(crate::capture::CaptureTarget::Issue),
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

/// The router's suggestions reach `capture.get`, `capture.list` and the
/// feed row, numbered and whole. Anything less and the decision surface has
/// a question with no buttons under it.
#[test]
fn the_options_a_router_offers_reach_every_surface_that_shows_the_capture() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (capture_id, _) = captured(&mut state, "make the thing faster");
    asked_with_two_options(&mut state, &capture_id);

    let expected = json!([
        {
            "id": "option-1",
            "label": "File as an issue on Build",
            "project_id": "proj-build",
            "kind": "issue",
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

    let listed = state.handle(req("capture.list", json!({})));
    assert_eq!(
        listed["result"]["captures"][0]["question"]["options"],
        expected
    );

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
    state.pending_agent_turns.clear();

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
        .pending_agent_turns
        .iter()
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
        state.handle(req("capture.list", json!({})))["result"]["captures"],
        json!([])
    );
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

/// Once a capture became work, that work is what there is to cancel. A
/// cancel here would drop the record that says where it went and leave the
/// issue behind it unexplained.
#[test]
fn a_capture_that_became_work_is_not_cancelled_from_here() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (capture_id, _) = captured(&mut state, "ship it");
    let project_id = state.project_at(0).id.clone();
    state
        .router_action(
            &capture_id,
            BridgeAction::CreateIssue {
                project_id,
                goal: "ship it".to_string(),
                rationale: None,
            },
        )
        .unwrap();

    let refused = state.handle(req("capture.cancel", json!({ "capture_id": capture_id })));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(state.captures.contains_key(&capture_id));
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
        DoneReport {
            phase: DonePhase::Route,
            status: DoneStatus::Completed,
            summary: "asked which project".to_string(),
            outputs: crate::mcp::DoneOutputs::default(),
        },
    );
    assert!(!state.router_sessions.contains_key(&capture_id));
    assert_eq!(
        capture_record(&mut state, &capture_id)["state"],
        "unrouted",
        "a router that asked is not a router that failed"
    );
    state.pending_agent_turns.clear();

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
        .pending_agent_turns
        .iter()
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
    app.pending_agent_turns.clear();
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
        DoneReport {
            phase: DonePhase::Route,
            status: DoneStatus::Failed,
            summary: "nothing here says which project".to_string(),
            outputs: crate::mcp::DoneOutputs::default(),
        },
    );

    let record = capture_record(&mut state, &capture_id);
    assert_eq!(record["state"], "failed");
    assert!(!state.router_sessions.contains_key(&capture_id));
    assert!(!scratch.exists(), "the scratch goes with the session");

    let row = capture_rows(&mut state).remove(0);
    assert_eq!(row["unread"], true);
    assert_eq!(row["unread_reason"], "routing_failed");
}

/// A router that reported a route is a router that finished: the same
/// settle leaves the decision alone and only tidies up after the process.
#[test]
fn a_router_that_routed_keeps_its_route_when_it_reports() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let (capture_id, _) = captured(&mut state, "fix the login redirect");
    let scratch = state.router_sessions[&capture_id]
        .scratch_dir()
        .to_path_buf();
    state
        .router_action(
            &capture_id,
            BridgeAction::CreateIssue {
                project_id,
                goal: "fix the login redirect".to_string(),
                rationale: None,
            },
        )
        .unwrap();

    state.on_router_done(
        &capture_id,
        DoneReport {
            phase: DonePhase::Route,
            status: DoneStatus::Completed,
            summary: "filed an issue".to_string(),
            outputs: crate::mcp::DoneOutputs::default(),
        },
    );

    assert_eq!(capture_record(&mut state, &capture_id)["state"], "routed");
    assert!(!scratch.exists());
    assert!(
        capture_rows(&mut state).is_empty(),
        "what it became is the presence"
    );
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

/// The user moves a misroute off an issue whose planning agent the route
/// itself started. The route made the issue AND the session, so the reroute
/// takes both back: the agent is retired and the issue archived. Leaving it
/// would put a second row on the feed for one piece of work — and, worse,
/// leave a planning agent working an issue nobody is going to read.
#[test]
fn rerouting_off_an_issue_only_its_own_agent_touched_stops_the_agent_and_archives_it() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let (capture_id, _) = captured(&mut state, "fix the login redirect");
    let filed = state
        .router_action(
            &capture_id,
            BridgeAction::CreateIssue {
                project_id: project_id.clone(),
                goal: "fix the login redirect".to_string(),
                rationale: None,
            },
        )
        .unwrap();
    let guessed = filed["issue_id"].as_str().unwrap().to_string();
    let agent_id = primary_agent_id(&state, &guessed);
    assert!(
        state
            .pending_agent_turns
            .iter()
            .any(|turn| turn.owner == guessed),
        "the route started the planning agent"
    );

    let rerouted = state.handle(req(
        "capture.reroute",
        json!({ "capture_id": capture_id, "project_id": project_id, "kind": "branch" }),
    ));
    assert_eq!(rerouted["ok"], true, "{rerouted:?}");
    assert_eq!(rerouted["result"]["routing"]["kind"], "branch");
    assert_eq!(
        rerouted["result"]["rerouted_from"][0]["target_id"],
        guessed.as_str(),
        "where it has been stays on the record"
    );
    assert!(
        state.plans[&guessed].plan.archived_at.is_some(),
        "a guess only Build's own agent touched is taken back"
    );
    assert!(
        !state
            .pending_agent_turns
            .iter()
            .any(|turn| turn.agent_id == agent_id),
        "the planning agent goes with the issue it was planning"
    );
    assert!(
        !state
            .session_registry
            .contains(&TabKey::agent(&AppState::canonical_root(&repo), &agent_id)),
        "and its session in the primary checkout is closed"
    );
}

/// An issue somebody has already spoken to is not a guess any more, and a
/// branch an agent worked is work. Both are kept, and stay reachable from
/// the capture rather than orphaned beside it.
#[test]
fn rerouting_keeps_a_destination_that_has_been_worked() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();

    let (touched_capture, _) = captured(&mut state, "fix the login redirect");
    let filed = state
        .router_action(
            &touched_capture,
            BridgeAction::CreateIssue {
                project_id: project_id.clone(),
                goal: "fix the login redirect".to_string(),
                rationale: None,
            },
        )
        .unwrap();
    let issue_id = filed["issue_id"].as_str().unwrap().to_string();
    let agent_id = primary_agent_id(&state, &issue_id);
    // The user opened it and said something: no longer a guess nobody read.
    let spoken_to = state.handle(req(
        "thread.post",
        json!({ "entity_id": issue_id, "body": "start with the redirect loop" }),
    ));
    assert_eq!(spoken_to["ok"], true, "{spoken_to:?}");

    let rerouted = state.handle(req(
        "capture.reroute",
        json!({ "capture_id": touched_capture, "project_id": project_id, "kind": "issue" }),
    ));
    assert_eq!(rerouted["ok"], true, "{rerouted:?}");
    assert!(
        state.plans[&issue_id].plan.archived_at.is_none(),
        "an issue with something said to it is nobody's to archive"
    );
    assert!(
        state
            .pending_agent_turns
            .iter()
            .any(|turn| turn.agent_id == agent_id),
        "and its agent is nobody's to stop either"
    );
    assert_eq!(
        rerouted["result"]["rerouted_from"][0]["target_id"],
        issue_id.as_str()
    );

    let (branch_capture, _) = captured(&mut state, "finish the toast");
    state
        .router_action(
            &branch_capture,
            BridgeAction::DispatchBranch {
                project_id: project_id.clone(),
                branch: None,
                instruction: "finish the toast".to_string(),
                rationale: None,
            },
        )
        .unwrap();
    let dispatched_branch = capture_record(&mut state, &branch_capture)["routing"]["target_id"]
        .as_str()
        .unwrap()
        .to_string();
    let runs_before = state.runs.len();

    let moved = state.handle(req(
        "capture.reroute",
        json!({ "capture_id": branch_capture, "project_id": project_id, "kind": "issue" }),
    ));
    assert_eq!(moved["ok"], true, "{moved:?}");
    assert_eq!(
        state.runs.len(),
        runs_before,
        "the branch's work is untouched"
    );
    assert_eq!(
        moved["result"]["rerouted_from"][0]["target_id"],
        dispatched_branch.as_str()
    );
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
            .pending_agent_turns
            .iter()
            .all(|turn| !turn.owner.starts_with("run-")),
        "the refused dispatch left a turn queued for a run that does not exist"
    );
    assert!(
        state.pending_rows.is_empty(),
        "the refused dispatch left its row on the board"
    );
    assert!(
        state
            .project_at(0)
            .external_scan
            .as_ref()
            .is_some_and(|cache| {
                cache
                    .worktrees
                    .iter()
                    .any(|worktree| worktree.branch.as_deref() == Some("build/add-the-csv-export"))
            }),
        "the checkout the git cut is not on the board: {:?}",
        state
            .project_at(0)
            .external_scan
            .as_ref()
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
        DoneReport {
            phase: DonePhase::Route,
            status: DoneStatus::Failed,
            summary: "could not decide".to_string(),
            outputs: crate::mcp::DoneOutputs::default(),
        },
    );
    assert_eq!(capture_record(&mut state, &capture_id)["state"], "failed");
    state.pending_agent_turns.clear();

    let retried = state.handle(req("capture.reroute", json!({ "capture_id": capture_id })));
    assert_eq!(retried["ok"], true, "{retried:?}");
    assert_eq!(retried["result"]["state"], "routing");
    assert_ne!(state.router_sessions[&capture_id].agent_id(), primary_agent);
    assert!(state
        .pending_agent_turns
        .iter()
        .any(|turn| turn.owner == capture_id));
}

/// One capture, one destination. A router that already routed cannot route
/// again — that would leave two artifacts and a record naming one.
#[test]
fn a_routed_capture_refuses_a_second_route_from_the_router() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let (capture_id, _) = captured(&mut state, "ship it");
    let file = |state: &mut AppState| {
        state.router_action(
            &capture_id,
            BridgeAction::CreateIssue {
                project_id: project_id.clone(),
                goal: "ship it".to_string(),
                rationale: None,
            },
        )
    };
    file(&mut state).unwrap();
    let again = file(&mut state).unwrap_err();
    assert!(again.contains("already has a destination"), "{again}");
}

/// The two surfaces are enforced where the frames arrive, not only in the
/// tool list a session is shown: a harness that writes its own frames still
/// only reaches the surface it was spawned on.
#[test]
fn neither_session_kind_can_call_the_others_tools() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let goal = "add a greeting";
    let plan = state.handle(req("plan.create", json!({ "goal": goal })));
    let plan_id = plan["result"]["issue_id"].as_str().unwrap().to_string();
    let (capture_id, _) = captured(&mut state, "ship it");

    let coding_reaching_out = state
        .on_mcp_action(&plan_id, BridgeAction::ListProjects)
        .unwrap_err();
    assert!(
        coding_reaching_out.contains("list_projects")
            && coding_reaching_out.contains("router tool"),
        "{coding_reaching_out}"
    );

    let router_reaching_in = state
        .router_action(&capture_id, BridgeAction::ReadUnreadMessages)
        .unwrap_err();
    assert!(
        router_reaching_in.contains("read_unread_messages")
            && router_reaching_in.contains("coding agent's tool"),
        "{router_reaching_in}"
    );
}

/// The read half of the router's surface: every project on the device, the
/// work in flight across all of them, and one work item's conversation.
#[test]
fn the_router_reads_across_every_project_and_writes_to_none_of_them() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "add a greeting" })));
    let issue_id = plan["result"]["issue_id"].as_str().unwrap().to_string();
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
        rows.iter().any(|row| row["entity_id"] == issue_id.as_str()),
        "the issue in flight is what the router checks a capture against: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| row["kind"] != "capture"),
        "a capture is not work yet: {rows:?}"
    );

    let conversation = state
        .router_action(
            &capture_id,
            BridgeAction::ReadConversation {
                entity_id: issue_id.clone(),
                agent_id: None,
                limit: 10,
            },
        )
        .unwrap();
    assert_eq!(conversation["entity_id"], issue_id.as_str());
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
