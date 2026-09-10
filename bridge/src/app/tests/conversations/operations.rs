use super::*;

// ---- thread.post: the non-dispatching conversation write ---------------

fn suggested(labels: &[(&str, Option<&str>)]) -> Vec<crate::thread::MessageOption> {
    let drafts: Vec<crate::thread::MessageOptionDraft> = labels
        .iter()
        .map(|(label, message)| crate::thread::MessageOptionDraft {
            label: (*label).to_string(),
            message: message.map(str::to_string),
        })
        .collect();
    crate::thread::numbered_message_options(&drafts).unwrap()
}

/// The whole of a suggested action: the agent offers it, the reviewer
/// presses it, and what comes back to the agent is an ordinary unread
/// message carrying the option's own words.
#[test]
fn pressing_a_suggested_action_answers_the_agent_and_marks_the_offer() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "the tests are red");
    let offer = state
        .on_mcp_action(
            &run_id,
            BridgeAction::PostThreadMessage {
                body: "Two ways out. Which?".into(),
                anchor: None,
                links: Vec::new(),
                still_working: false,
                options: suggested(&[
                    (
                        "Revert it",
                        Some("Revert the commit that turned the tests red."),
                    ),
                    ("Fix forward", None),
                ]),
            },
        )
        .unwrap();
    let offer_id = offer["message_id"].as_str().unwrap().to_string();

    let pressed = state.handle(req(
        "thread.post",
        json!({
            "entity_id": run_id,
            "option_reply": { "message_id": offer_id, "option_ids": ["option-1"] },
        }),
    ));
    assert_eq!(pressed["ok"], true, "{pressed:?}");

    // The choice is on the offer, which is the only place the chat shows it.
    let items = primary_thread(&state.plans[&issue_id].agents).items.clone();
    let crate::thread::ThreadItem::Message(offered) = items
        .iter()
        .find(|item| matches!(item, crate::thread::ThreadItem::Message(message) if message.id == offer_id))
        .unwrap()
    else {
        panic!("the offer is a message");
    };
    assert_eq!(offered.selected_options, vec!["option-1".to_string()]);

    // And the agent hears the option's longer text, as a message like any
    // other — the reason to write one is that this is what survives into a
    // session that no longer remembers the offer.
    let read = state
        .on_mcp_action(&run_id, BridgeAction::ReadUnreadMessages)
        .unwrap();
    let unread = read["messages"].as_array().unwrap();
    let last = unread.last().unwrap();
    assert_eq!(last["body"], "Revert the commit that turned the tests red.");
    assert_eq!(last["role"], "user");
    assert_eq!(last["answers_options_of"], offer_id.as_str());
}

/// The race the disabled chips cannot cover: something is said between the
/// render and the press. The answer is refused rather than sent stale, and
/// the conversation is left exactly as it stood.
#[test]
fn a_choice_made_after_the_conversation_moved_on_is_refused() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "the tests are red");
    let offer = state
        .on_mcp_action(
            &run_id,
            BridgeAction::PostThreadMessage {
                body: "Two ways out. Which?".into(),
                anchor: None,
                links: Vec::new(),
                still_working: false,
                options: suggested(&[("Revert it", None), ("Fix forward", None)]),
            },
        )
        .unwrap();
    let offer_id = offer["message_id"].as_str().unwrap().to_string();
    let choice = json!({
        "entity_id": run_id,
        "option_reply": { "message_id": offer_id, "option_ids": ["option-2"] },
    });
    assert_eq!(state.handle(req("thread.post", choice.clone()))["ok"], true);

    let again = state.handle(req("thread.post", choice));
    assert_eq!(again["ok"], false, "{again:?}");
    let thread = primary_thread(&state.plans[&issue_id].agents);
    assert_eq!(
        thread
            .items
            .iter()
            .filter(|item| matches!(
                item,
                crate::thread::ThreadItem::Message(message)
                    if message.answers_options_of.is_some()
            ))
            .count(),
        1
    );
}

#[test]
fn an_option_the_agent_never_offered_cannot_be_answered_with() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_issue_id, run_id) = planned_run_in_review(&mut state, "the tests are red");
    let offer = state
        .on_mcp_action(
            &run_id,
            BridgeAction::PostThreadMessage {
                body: "Which?".into(),
                anchor: None,
                links: Vec::new(),
                still_working: false,
                options: suggested(&[("Revert it", None)]),
            },
        )
        .unwrap();

    let pressed = state.handle(req(
        "thread.post",
        json!({
            "entity_id": run_id,
            "option_reply": {
                "message_id": offer["message_id"].as_str().unwrap(),
                "option_ids": ["option-9"],
            },
        }),
    ));
    assert_eq!(pressed["ok"], false, "{pressed:?}");
    // Refused before the entity was checked out of its map, so the run is
    // still there to talk to.
    assert!(state.runs.contains_key(&run_id));
}

/// Talking to the agent you are looking at must reach it, whatever the run
/// happens to be parked as.
///
/// The nudge used to fire only while a run was `Building`, from the era when
/// the agent EXISTED only while building — every other state meant no
/// process to talk to. A worktree's agent now outlives every phase and sits
/// right there in the Agent tab at the review gate, so gating on run state
/// meant typing into a live conversation and having it silently not arrive.
#[tokio::test]
async fn thread_post_reaches_the_live_agent_at_a_review_gate() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = "run-at-the-gate".to_string();
    let key = insert_run_with_agent_tab(
        &mut state,
        &repo,
        &dir.path().join("side"),
        &run_id,
        RunState::Review,
        warm_tui_spec(),
    );
    let pid_before = agent_pid(&state.tabs[&key]).expect("a live agent");
    let mut output = agent_terminal(&state.tabs[&key]).subscribe();
    let state = state.shared();
    let handler = AppState::handler(Arc::clone(&state));

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": run_id, "body": "why did you drop the index?" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    assert_eq!(
        posted["result"]["state"], "review",
        "the gate does not move"
    );
    assert_eq!(
        agent_pid(&state.lock().unwrap().tabs[&key]),
        Some(pid_before),
        "a post talks to the agent, it never replaces it"
    );

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut echoed = String::new();
    while std::time::Instant::now() < deadline && !echoed.contains("read_unread_messages") {
        match output.try_recv() {
            Ok(chunk) => echoed.push_str(&String::from_utf8_lossy(&chunk)),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(_) => break,
        }
    }
    assert!(
        echoed.contains("read_unread_messages"),
        "the agent at the gate must hear the message: {echoed:?}"
    );
}

/// The review surface's whole point: a message lands in the run's thread
/// as unread WITHOUT respawning the agent or moving the run's state, and
/// the agent's catch-up tool then drains it.
#[test]
fn thread_post_in_review_posts_unread_and_moves_no_state() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "post-only path");

    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "body": "just a review note" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    // The same Full-thread view shape the dispatching verbs return, so the
    // caller can render optimistically.
    assert_eq!(posted["result"]["state"], "review", "{posted:?}");
    let items = posted["result"]["thread"]["items"].as_array().unwrap();
    let message = items
        .iter()
        .find(|item| item["data"]["body"] == "just a review note")
        .unwrap_or_else(|| panic!("posted message missing: {posted:?}"));
    assert_eq!(message["data"]["role"], "user", "{message:?}");
    assert!(
        message["data"].get("seen_at").is_none(),
        "the post must land unread: {message:?}"
    );

    let active = state.runs.get(&run_id).unwrap();
    assert_eq!(active.run.state, RunState::Review, "no state transition");
    assert!(state.tabs.is_empty(), "a post starts no agent");

    let unread = state
        .on_mcp_action(&run_id, BridgeAction::ReadUnreadMessages)
        .unwrap();
    assert_eq!(unread["messages"][0]["body"], "just a review note");
}

#[tokio::test]
async fn thread_post_addressed_to_issue_nudges_its_live_implementation_agent() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "issue-addressed post");
    let root = AppState::canonical_root(&state.runs[&run_id].worktree.path);
    let (mut tab, _rx) = Tab::spawn_agent(
        run_id.clone(),
        crate::agent::derived_agent_id(&run_id),
        test_agent_session_request(
            AgentProvider::default(),
            warm_tui_spec(),
            root.clone(),
            terminal_size(120, 40),
        ),
    )
    .expect("implementation agent tab spawns");
    let mut output = agent_terminal(&tab).subscribe();
    let agent_id = crate::agent::derived_agent_id(&run_id);
    let choice = state.runs[&run_id]
        .agents
        .by_id(&agent_id)
        .unwrap()
        .choice
        .clone();
    tab.session_instance =
        state.record_agent_session_start(&run_id, &agent_id, &root, &choice, "build");
    state.tabs.insert(derived_agent_key(&root, &run_id), tab);
    // `planned_run_in_review` stages lifecycle turns for the real spawn.
    // This test installs that live session by hand, so those cold turns
    // have already happened and only the post's nudge remains deliverable.
    state.pending_agent_turns.clear();
    let state = state.shared();
    let handler = AppState::handler(Arc::clone(&state));

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": issue_id, "body": "read this in the implementation" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut echoed = String::new();
    while std::time::Instant::now() < deadline && !echoed.contains("read_unread_messages") {
        match output.try_recv() {
            Ok(chunk) => echoed.push_str(&String::from_utf8_lossy(&chunk)),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(_) => break,
        }
    }
    assert!(
        echoed.contains("read_unread_messages"),
        "the active implementation agent must be nudged: {echoed:?}"
    );
    let unread = state
        .lock()
        .unwrap()
        .on_mcp_action(&run_id, BridgeAction::ReadUnreadMessages)
        .unwrap();
    assert!(unread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["body"] == "read this in the implementation"));
}

/// A reply IS the unblock: posting to a parked plan applies the `Reply`
/// transition the state machine already defines, so the composer the user
/// is typing into resumes drafting instead of leaving the card stranded
/// at BLOCKED with no way out.
#[test]
fn thread_post_to_a_parked_plan_is_the_reply_that_resumes_drafting() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    for parked in [
        PlanState::Blocked,
        PlanState::Failed,
        PlanState::IdleUnreported,
    ] {
        let plan = state.handle(req("plan.create", json!({ "goal": "park me" })));
        let plan_id = plan_id_of(&plan);
        state.plans.get_mut(&plan_id).unwrap().plan.state = parked;

        let posted = state.handle(req(
            "thread.post",
            json!({ "entity_id": plan_id, "body": "here is your answer" }),
        ));
        assert_eq!(posted["ok"], true, "{parked:?}: {posted:?}");
        assert_eq!(
            posted["result"]["state"], "drafting",
            "{parked:?}: the reply resumes drafting: {posted:?}"
        );
        let active = state.plans.get(&plan_id).unwrap();
        assert_eq!(active.plan.state, PlanState::Drafting, "{parked:?}");
    }
}

/// Same rule on the run side: a post addressed to a parked run resumes
/// building — the message is the reply the Blocked/Failed/Idle arms wait
/// for.
#[test]
fn thread_post_to_a_parked_run_is_the_reply_that_resumes_building() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    for parked in [
        RunState::Blocked,
        RunState::Failed,
        RunState::IdleUnreported,
    ] {
        let (_, run_id) = planned_run_in_review(&mut state, "park the run");
        state.runs.get_mut(&run_id).unwrap().run.state = parked;

        let posted = state.handle(req(
            "thread.post",
            json!({ "entity_id": run_id, "body": "here is your answer" }),
        ));
        assert_eq!(posted["ok"], true, "{parked:?}: {posted:?}");
        assert_eq!(
            posted["result"]["state"], "building",
            "{parked:?}: the reply resumes building: {posted:?}"
        );
        let active = state.runs.get(&run_id).unwrap();
        assert_eq!(active.run.state, RunState::Building, "{parked:?}");
    }
}

/// The Issue owns the conversation, but the reply must still unblock the
/// live implementation it wakes: posting to the Issue while its run is
/// parked resumes that run.
#[test]
fn thread_post_addressed_to_issue_unblocks_its_parked_implementation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "issue-addressed unblock");
    state.runs.get_mut(&run_id).unwrap().run.state = RunState::Blocked;

    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": issue_id, "body": "here is your answer" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let active = state.runs.get(&run_id).unwrap();
    assert_eq!(
        active.run.state,
        RunState::Building,
        "the reply resumes the implementation it nudged"
    );
}

#[test]
fn planned_run_conversation_and_mcp_alias_the_issue_thread() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (plan_id, run_id) = planned_run_in_review(&mut state, "one issue thread");
    let plan_state = state.plans[&plan_id].plan.state;
    let run_state = state.runs[&run_id].run.state;
    let issue_agent_id = state.plans[&plan_id].agents.sole().id.clone();
    let execution_agent_id = state.runs[&run_id].agents.primary().unwrap().id.clone();
    state
        .runs
        .get_mut(&run_id)
        .unwrap()
        .agents
        .primary_mut()
        .unwrap()
        .choose(ModelChoice {
            provider: AgentProvider::Codex,
            model: Some("gpt-5.6-sol".into()),
            effort: Some("high".into()),
        });
    let context = state
        .issue_execution_context(&plan_id)
        .expect("the live implementation executes the issue conversation");
    assert_eq!(context["entity_id"], run_id);
    assert_eq!(context["agent_id"], execution_agent_id);
    assert_eq!(context["conversation_id"], issue_agent_id);
    assert_eq!(context["agent"]["provider"], "codex");
    assert_eq!(context["agent"]["choice_revision"], 1);
    assert_ne!(
        context["agent"]["provider"],
        state.agent_digests(&plan_id, DigestScope::List)[0]["provider"],
        "issue and execution settings remain independently owned"
    );
    let issue_view = state.plan_view(
        &plan_id,
        &state.plans[&plan_id],
        ThreadDetail::Digest,
        DigestScope::Detail,
    );
    assert_eq!(issue_view["execution_context"], context);

    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "body": "shared implementation note" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    assert_eq!(state.plans[&plan_id].plan.state, plan_state);
    assert_eq!(state.runs[&run_id].run.state, run_state);
    assert!(primary_thread(&state.runs[&run_id].agents)
        .items
        .iter()
        .all(|item| !matches!(item, crate::thread::ThreadItem::Message(message) if message.body == "shared implementation note")));

    let run_view = state.handle(req("run.get", json!({ "run_id": run_id })));
    assert!(run_view["result"]["thread"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["data"]["body"] == "shared implementation note"));
    let issue_conversation = primary_thread(&state.plans[&plan_id].agents).id.clone();
    let unread = state
        .on_mcp_action(&run_id, BridgeAction::ReadUnreadMessages)
        .unwrap();
    assert_eq!(unread["thread_id"], issue_conversation);
    assert!(unread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["body"] == "shared implementation note"));
}

#[test]
fn explicit_issue_post_never_executes_its_implementation_alias() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "explicit issue address");
    let issue_agent = state.plans[&issue_id].agents.sole().id.clone();
    let execution_agent = state.runs[&run_id].agents.primary().unwrap().id.clone();
    state.pending_agent_turns.clear();

    let posted = state.handle(req(
        "thread.post",
        json!({
            "entity_id": issue_id,
            "agent_id": issue_agent,
            "conversation_id": issue_agent,
            "body": "answer the issue agent itself",
        }),
    ));

    assert_eq!(posted["ok"], true, "{posted:?}");
    assert!(
        state.pending_agent_turns.is_empty(),
        "the Issue workspace was handed off, so its explicitly addressed agent has no PTY; \
         the post must not silently reach implementation agent {execution_agent}"
    );
    assert!(state
        .agent_conversation(&issue_id, Some(&issue_agent))
        .unwrap()
        .items
        .iter()
        .any(|item| matches!(
            item,
            crate::thread::ThreadItem::Message(message)
                if message.body == "answer the issue agent itself"
        )));
}

fn queued_operation_turn<'a>(state: &'a AppState, operation_id: &str) -> &'a PendingAgentTurn {
    state
        .pending_agent_turns
        .iter()
        .find(|turn| turn.operation_id.as_deref() == Some(operation_id))
        .expect("the accepted operation queued its immutable turn")
}

fn assert_scoped_operation_reads(state: &mut AppState, run_id: &str, agent_id: &str) {
    let generic = state
        .on_agent_mcp_action(run_id, agent_id, BridgeAction::ReadUnreadMessages)
        .unwrap();
    assert_eq!(generic["messages"], json!([]));

    let other_agent = state
        .runs
        .get_mut(run_id)
        .unwrap()
        .agents
        .add(run_id, ModelChoice::default(), &now_rfc3339())
        .id
        .clone();
    let unauthorized = state.on_agent_mcp_action(
        run_id,
        &other_agent,
        BridgeAction::ReadOperationMessages {
            operation_id: "operation-second".into(),
        },
    );
    assert!(unauthorized.unwrap_err().contains("does not belong"));

    let read_second = state
        .on_agent_mcp_action(
            run_id,
            agent_id,
            BridgeAction::ReadOperationMessages {
                operation_id: "operation-second".into(),
            },
        )
        .unwrap();
    assert_eq!(
        read_second["messages"][0]["body"],
        "only model B may consume this"
    );
    let repeated = state
        .on_agent_mcp_action(
            run_id,
            agent_id,
            BridgeAction::ReadOperationMessages {
                operation_id: "operation-second".into(),
            },
        )
        .unwrap();
    assert_eq!(repeated["messages"], read_second["messages"]);
    let read_first = state
        .on_agent_mcp_action(
            run_id,
            agent_id,
            BridgeAction::ReadOperationMessages {
                operation_id: "operation-first".into(),
            },
        )
        .unwrap();
    assert_eq!(
        read_first["messages"][0]["body"],
        "only model A may consume this"
    );
}

fn assert_operation_payload_snapshots(state: &AppState) {
    let first_receipt = state.operation_receipt("operation-first").unwrap().unwrap();
    let second_receipt = state
        .operation_receipt("operation-second")
        .unwrap()
        .unwrap();
    let first_payload = first_receipt.delivery.unwrap().payload.unwrap();
    let second_payload = second_receipt.delivery.unwrap().payload.unwrap();
    assert_eq!(first_payload.messages.len(), 1);
    assert_eq!(
        first_payload.messages[0].body,
        "only model A may consume this"
    );
    assert_eq!(second_payload.messages.len(), 1);
    assert_eq!(
        second_payload.messages[0].body,
        "only model B may consume this"
    );
    assert!(
        !second_payload
            .prior_context
            .contains("only model A may consume this"),
        "a later operation cannot inherit another unresolved operation as context"
    );
}

#[test]
fn operation_reads_are_bounded_to_the_exact_agent_and_payload() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "bounded-operation-read");
    let agent_id = primary_agent_id(&state, &run_id);
    state
        .runs
        .get_mut(&run_id)
        .unwrap()
        .agents
        .primary_mut()
        .unwrap()
        .choice
        .model = Some("claude-sonnet-5".into());

    let first_post = state.handle(req(
        "thread.post",
        json!({
            "entity_id": run_id,
            "agent_id": agent_id,
            "conversation_id": agent_id,
            "operation_id": "operation-first",
            "body": "only model A may consume this",
            "choice_revision": 0,
        }),
    ));
    assert_eq!(first_post["ok"], true, "{first_post:?}");
    let chose_second_model = state.handle(req(
        "agent.choose",
        json!({
            "entity_id": run_id,
            "agent_id": agent_id,
            "conversation_id": agent_id,
            "model": "claude-opus-5",
            "expected_choice_revision": 0,
        }),
    ));
    assert_eq!(chose_second_model["ok"], true, "{chose_second_model:?}");
    let second_post = state.handle(req(
        "thread.post",
        json!({
            "entity_id": run_id,
            "agent_id": agent_id,
            "conversation_id": agent_id,
            "operation_id": "operation-second",
            "body": "only model B may consume this",
            "choice_revision": 1,
        }),
    ));
    assert_eq!(second_post["ok"], true, "{second_post:?}");
    assert!(first_post["result"]["message_start_sequence"].is_number());
    assert!(second_post["result"]["message_start_sequence"].is_number());
    let first_turn = queued_operation_turn(&state, "operation-first");
    let second_turn = queued_operation_turn(&state, "operation-second");
    assert_eq!(first_turn.agent_id, agent_id);
    assert_eq!(first_turn.choice_revision, 0);
    assert_eq!(
        first_turn.model_choice.model.as_deref(),
        Some("claude-sonnet-5")
    );
    assert_eq!(second_turn.agent_id, agent_id);
    assert_eq!(second_turn.choice_revision, 1);
    assert_eq!(
        second_turn.model_choice.model.as_deref(),
        Some("claude-opus-5")
    );
    assert_operation_payload_snapshots(&state);

    assert_scoped_operation_reads(&mut state, &run_id, &agent_id);
}

#[test]
fn operation_payload_preserves_batches_attachments_and_normalized_options() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "rich-operation-payload");
    let agent_id = primary_agent_id(&state, &run_id);
    let attached = state.handle(req(
        "thread.attach",
        json!({
            "entity_id": run_id,
            "agent_id": agent_id,
            "filename": "review.png",
            "content_b64": b64encode(ONE_PIXEL_PNG),
        }),
    ));
    assert_eq!(attached["ok"], true, "{attached:?}");

    let batched = state.handle(req(
        "thread.post",
        json!({
            "entity_id": run_id,
            "agent_id": agent_id,
            "conversation_id": agent_id,
            "operation_id": "operation-batch",
            "messages": [
                { "body": "first accepted message", "anchor": null },
                { "body": "second accepted message", "anchor": null },
            ],
            "attachments": [attached["result"].clone()],
        }),
    ));
    assert_eq!(batched["ok"], true, "{batched:?}");
    let batch_receipt = state.operation_receipt("operation-batch").unwrap().unwrap();
    let batch = batch_receipt.delivery.unwrap().payload.unwrap().messages;
    assert_eq!(batch.len(), 2);
    assert_eq!(batch[0].body, "first accepted message");
    assert_eq!(batch[1].body, "second accepted message");
    assert!(batch[0].sequence < batch[1].sequence);
    assert!(batch[0].attachments.is_empty());
    assert_eq!(batch[1].attachments.len(), 1);
    assert_eq!(batch[1].attachments[0].name, "review.png");

    let offered = state
        .on_agent_mcp_action(
            &run_id,
            &agent_id,
            BridgeAction::PostThreadMessage {
                body: "Choose a repair".into(),
                anchor: None,
                links: Vec::new(),
                still_working: false,
                options: suggested(&[(
                    "Fix forward",
                    Some("Apply the forward repair and retain the migration."),
                )]),
            },
        )
        .unwrap();
    let offer_id = offered["message_id"].as_str().unwrap().to_string();
    let answered = state.handle(req(
        "thread.post",
        json!({
            "entity_id": run_id,
            "agent_id": agent_id,
            "conversation_id": agent_id,
            "operation_id": "operation-option",
            "option_reply": {
                "message_id": offer_id,
                "option_ids": ["option-1"],
            },
        }),
    ));
    assert_eq!(answered["ok"], true, "{answered:?}");
    let option_receipt = state
        .operation_receipt("operation-option")
        .unwrap()
        .unwrap();
    let option = option_receipt.delivery.unwrap().payload.unwrap().messages;
    assert_eq!(option.len(), 1);
    assert_eq!(
        option[0].body,
        "Apply the forward repair and retain the migration."
    );
    assert_eq!(
        option[0].answers_options_of.as_deref(),
        Some(offer_id.as_str())
    );
}

#[test]
fn failed_operation_acceptance_rolls_back_history_and_can_retry_once() {
    let (dir, repo) = init_repo();
    let run_id;
    let agent_id;
    let request;
    {
        let mut state = qa_state(&repo, dir.path());
        run_id = adopted_run(&mut state, &repo, dir.path(), "operation-write-failure");
        agent_id = primary_agent_id(&state, &run_id);
        request = json!({
            "entity_id": run_id,
            "agent_id": agent_id,
            "conversation_id": agent_id,
            "operation_id": "retry-after-write-failure",
            "body": "append me exactly once",
        });
        let before = state
            .agent_conversation(&run_id, Some(&agent_id))
            .unwrap()
            .clone();
        state.store.as_ref().unwrap().fail_next_write();

        let refused = state.handle(req("thread.post", request.clone()));

        assert_eq!(refused["ok"], false, "{refused:?}");
        assert_eq!(
            state.agent_conversation(&run_id, Some(&agent_id)).unwrap(),
            &before,
            "failed acceptance cannot remain visible in memory"
        );
        assert!(state
            .operation_receipt("retry-after-write-failure")
            .unwrap()
            .is_none());

        let unrelated = state.handle(req(
            "agent.choose",
            json!({
                "entity_id": run_id,
                "agent_id": agent_id,
                "model": "claude-opus-5",
            }),
        ));
        assert_eq!(unrelated["ok"], true, "{unrelated:?}");
    }

    let mut reloaded = qa_state(&repo, dir.path());
    assert!(!reloaded
        .agent_conversation(&run_id, Some(&agent_id))
        .unwrap()
        .items
        .iter()
        .any(|item| matches!(
            item,
            crate::thread::ThreadItem::Message(message)
                if message.body == "append me exactly once"
        )));
    assert!(reloaded
        .operation_receipt("retry-after-write-failure")
        .unwrap()
        .is_none());

    let accepted = reloaded.handle(req("thread.post", request));
    assert_eq!(accepted["ok"], true, "{accepted:?}");
    assert_eq!(accepted["result"]["operation_status"], "queued");
    assert_eq!(
        reloaded
            .agent_conversation(&run_id, Some(&agent_id))
            .unwrap()
            .items
            .iter()
            .filter(|item| matches!(
                item,
                crate::thread::ThreadItem::Message(message)
                    if message.body == "append me exactly once"
            ))
            .count(),
        1
    );
}

/// A mid-build post must leave the worktree's agent running (same process,
/// same tab) and nudge it in place through its PTY — the PTY echoes written
/// input back to its reader, so the nudge is observable on the tab's output
/// stream.
#[test]
fn thread_post_in_building_nudges_the_live_session_without_ending_it() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = "run-nudge".to_string();
    let key = insert_run_with_agent_tab(
        &mut state,
        &repo,
        &dir.path().join("side"),
        &run_id,
        RunState::Building,
        warm_tui_spec(),
    );
    let pid_before = agent_pid(&state.tabs[&key]).expect("a live agent");
    let mut output = agent_terminal(&state.tabs[&key]).subscribe();
    let state = state.shared();
    let handler = AppState::handler(Arc::clone(&state));

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": run_id, "body": "while you build" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");

    let held = state.lock().unwrap();
    let active = held.runs.get(&run_id).unwrap();
    assert_eq!(active.run.state, RunState::Building, "no state transition");
    assert_eq!(
        agent_pid(&held.tabs[&key]),
        Some(pid_before),
        "the worktree's agent must not be respawned"
    );
    assert!(
        held.tabs[&key].session_is_live(),
        "the worktree's agent must not be ended"
    );
    drop(held);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut echoed = String::new();
    while std::time::Instant::now() < deadline && !echoed.contains("read_unread_messages") {
        match output.try_recv() {
            Ok(chunk) => echoed.push_str(&String::from_utf8_lossy(&chunk)),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(_) => break,
        }
    }
    assert!(
        echoed.contains("read_unread_messages"),
        "the live PTY must hear the nudge: {echoed:?}"
    );
}

/// The only test that can prove prompt delivery actually works.
///
/// Everything else in this suite runs against a scripted harness, which by
/// construction cannot tell a delivered prompt from one eaten by a startup
/// dialog or shredded into per-line turns — that blind spot is exactly how
/// three rounds of green suites hid a dispatch that delivered nothing. This
/// spawns the REAL `claude` binary through the REAL adapter, in a fresh
/// worktree-like directory (so the workspace-trust dialog is armed), with a
/// deliberately MULTI-LINE prompt (so bracketed-paste framing is exercised),
/// and asserts the agent acted on the whole prompt.
///
/// Ignored by default: it needs `claude` installed, authenticated, and a
/// network round trip, none of which belong in `cargo test`. Run it by hand
/// after touching anything in the spawn path:
///
/// ```text
/// cargo test --lib real_claude -- --ignored --nocapture
/// BUILD_E2E_TIMING=1     # timestamp every chunk, flag paste-mode/alt-screen
/// BUILD_E2E_TRANSCRIPT=/tmp/e2e.txt   # dump the full raw stream
/// BUILD_E2E_WAIT=45      # shorten the wait while iterating
/// ```
///
/// STATUS: currently FAILS against claude 2.1.219, and that failure is real
/// — warm-TUI dispatch does not deliver. What it has already established:
///   - Workspace trust is fixed. The dialog no longer appears in a brand-new
///     directory, so `pre_trust_worktree_for_claude` works.
///   - Readiness and settle are necessary but not sufficient. With
///     REAL_TUI_SETTLE the write now lands ~750ms after the final startup
///     paint (measured: last paint 2716ms, write 3466ms) instead of into the
///     alternate-screen clear.
///   - The prompt text never appears on screen at all, and the TUI emits
///     ZERO output for the following two minutes. A composer receiving
///     keystrokes would redraw, so the remaining fault is below paste
///     framing and below the submit key — the bytes are not reaching
///     claude's input reader. Cause not yet identified.
///
/// Do not treat the warm-TUI path as working until this passes.
///
/// Shortened via BUILD_E2E_WAIT while iterating on the spawn path.
fn e2e_wait() -> Duration {
    Duration::from_secs(
        std::env::var("BUILD_E2E_WAIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(180),
    )
}

#[test]
#[ignore = "spawns the real claude binary; needs auth + network"]
fn real_claude_session_receives_the_whole_multiline_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("fresh-worktree");
    std::fs::create_dir_all(&workspace).unwrap();
    // The adapter passes --mcp-config .build/mcp.json --strict-mcp-config,
    // so the file must exist or claude exits before reading a byte of the
    // prompt. Real dispatch scaffolds this (Orchestrator::scaffold_build_dir);
    // mirror the shape here. The server is never called — this test asserts
    // prompt DELIVERY, not the done round trip.
    let build_dir = workspace.join(".build");
    std::fs::create_dir_all(&build_dir).unwrap();
    std::fs::write(
        build_dir.join("mcp.json"),
        serde_json::to_vec_pretty(&json!({ "mcpServers": {} })).unwrap(),
    )
    .unwrap();

    // The marker is split across prompt LINES on purpose: only a prompt that
    // arrived as one turn can reassemble it. A prompt submitted line-by-line
    // leaves the agent acting on a fragment, which is the exact production
    // failure this guards.
    let prompt = "You are being driven by an automated test.\n\
         Do exactly this and nothing else, then stop.\n\
         \n\
         Create a file named `handshake.txt` in the current directory.\n\
         Its only contents must be these two words joined by a hyphen:\n\
         first word: BUILD\n\
         second word: DELIVERED\n\
         \n\
         So the file contains exactly: BUILD-DELIVERED\n";

    let Agent::WarmBuilder(build) = test_build_agent("/tmp/unused-e2e.sock") else {
        panic!("real agent should be a provider-aware warm TUI");
    };
    let choice = ModelChoice {
        provider: AgentProvider::Claude,
        model: Some("haiku".into()),
        effort: None,
    };
    let options = SpawnOptions {
        continue_session: false,
        resume_session_id: None,
        owner_id: "e2e".into(),
        mcp_session_token: "e2e-session-token".into(),
        cwd: workspace.clone(),
    };
    // Building the spec is what pre-trusts the workspace — the dialog this
    // guards against fires precisely because the directory is brand new.
    let spec = build(prompt, &choice, &options).unwrap();

    // The three lines under test, mirroring what `open_session` waits out
    // and what `deliver` then hands over — spelled out against the concrete
    // PTY, because what is under test here is the terminal mechanics.
    let session = PtySession::spawn(
        &spec,
        Some(workspace.clone()),
        PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        },
    )
    .expect("claude should spawn — is it installed and on PATH?");
    // Capture the session so a failure reports what the harness actually did
    // — a trust dialog, an argv rejection and an unsubmitted prompt all look
    // identical from the filesystem alone.
    let mut output = session.subscribe();
    let transcript = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    {
        let transcript = std::sync::Arc::clone(&transcript);
        let started = std::time::Instant::now();
        std::thread::spawn(move || {
            while let Ok(chunk) = output.blocking_recv() {
                let text = String::from_utf8_lossy(&chunk).into_owned();
                if std::env::var("BUILD_E2E_TIMING").is_ok() {
                    eprintln!(
                        "[{:>6}ms] {:>5}B{}{}",
                        started.elapsed().as_millis(),
                        chunk.len(),
                        if text.contains("\u{1b}[?2004h") {
                            " PASTE-MODE"
                        } else {
                            ""
                        },
                        if text.contains("\u{1b}[?1049h") {
                            " ALT-SCREEN"
                        } else {
                            ""
                        },
                    );
                }
                transcript.lock().unwrap().push_str(&text);
            }
        });
    }

    let ready = session.ready_within(Duration::from_secs(30));
    let written = session.write_prompt(prompt);

    let handshake = workspace.join("handshake.txt");
    let deadline = std::time::Instant::now() + e2e_wait();
    while std::time::Instant::now() < deadline {
        if std::fs::read_to_string(&handshake).is_ok_and(|body| body.contains("BUILD-DELIVERED")) {
            session.kill_and_reap();
            return;
        }
        if session.has_exited() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    let observed = std::fs::read_to_string(&handshake).unwrap_or_default();
    session.kill_and_reap();
    let seen = transcript.lock().unwrap().clone();
    if let Ok(dump) = std::env::var("BUILD_E2E_TRANSCRIPT") {
        let _ = std::fs::write(&dump, &seen);
    }
    panic!(
        "the agent never acted on the delivered prompt.\n\
         ready={ready} write={written:?} handshake={observed:?}\n\
         Either the prompt landed in a startup dialog, was never submitted, \
         or arrived as fragmented turns.\n\
         ---- harness output ----\n{}\n---- end ----",
        &seen[seen.len().saturating_sub(4000)..]
    );
}

/// A post at a review gate reaches the agent, and still moves nothing.
///
/// This used to assert the opposite — that a parked harness was left alone,
/// because waking it produced work whose `done` is an illegal transition
/// from `review`. That reasoning died twice over: an out-of-phase `done` is
/// now RECORDED rather than rejected, and the agent no longer parks at all —
/// it is live in the Agent tab the human is typing into. Withholding the
/// message made the conversation lie about itself, which is worse than a
/// report that moves nothing. What must still hold is everything else: no
/// respawn, no state change, and durability either way.
#[test]
fn thread_post_at_a_review_gate_reaches_the_agent_without_moving_the_run() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = "run-parked".to_string();
    let key = insert_run_with_agent_tab(
        &mut state,
        &repo,
        &dir.path().join("side"),
        &run_id,
        RunState::Review,
        warm_tui_spec(),
    );
    assert!(
        state.tabs[&key].session_is_live(),
        "precondition: the agent is still live at the gate"
    );
    let mut output = agent_terminal(&state.tabs[&key]).subscribe();
    let state = state.shared();
    let handler = AppState::handler(Arc::clone(&state));

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": run_id, "body": "a note for later" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut echoed = String::new();
    while std::time::Instant::now() < deadline && !echoed.contains("read_unread_messages") {
        match output.try_recv() {
            Ok(chunk) => echoed.push_str(&String::from_utf8_lossy(&chunk)),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(_) => break,
        }
    }
    assert!(
        echoed.contains("read_unread_messages"),
        "the agent the human is looking at must hear them: {echoed:?}"
    );
    assert_eq!(
        state.lock().unwrap().runs[&run_id].run.state,
        RunState::Review,
        "hearing a message is not a state transition"
    );
    assert!(
        state.lock().unwrap().tabs[&key].session_is_live(),
        "the agent is talked to, never replaced"
    );
    // Durable regardless: the next session's catch-up carries it.
    let unread = state
        .lock()
        .unwrap()
        .on_mcp_action(&run_id, BridgeAction::ReadUnreadMessages)
        .unwrap();
    assert_eq!(unread["messages"][0]["body"], "a note for later");
}

/// Abandon must leave no agent behind. Worktree removal is best-effort by
/// contract — a leftover worktree is logged, never a reason to fail the
/// abandon — so when it fails the worktree stays on disk and the orphan
/// reaper (which only sweeps tabs whose root is GONE) never fires. The kill
/// has to be the abandon's own, or the human is left paying for an agent
/// working on something they abandoned.
#[test]
fn abandon_closes_the_agent_even_when_the_worktree_survives() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "abandon me");
    let worktree = state.runs[&run_id].worktree.path.clone();
    let root = AppState::canonical_root(&worktree);
    let (tab, _rx) = Tab::spawn_agent(
        run_id.clone(),
        crate::agent::derived_agent_id(&run_id),
        test_agent_session_request(
            AgentProvider::default(),
            warm_tui_spec(),
            root.clone(),
            terminal_size(120, 40),
        ),
    )
    .expect("the agent tab spawns");
    let agent_pid = agent_pid(&tab).expect("the agent has a pid");
    state.tabs.insert(derived_agent_key(&root, &run_id), tab);
    primary_thread_mut(&mut state.runs.get_mut(&run_id).unwrap().agents).start_session(
        "claude",
        None,
        None,
        "build",
        &now_rfc3339(),
    );

    // Cleanup will fail before it touches the worktree: the orchestrator's
    // repo is not a repo, so `remove` errors on the very first step and the
    // worktree survives the abandon.
    state.projects[0].orch = Orchestrator::new(
        dir.path().join("not-a-repo"),
        dir.path().join("wt"),
        Agent::Warm(HarnessSpec::new("true")),
        Templates::default(),
        test_bridge_exe(),
    );

    let abandoned = state.handle(req("run.abandon", json!({ "run_id": run_id })));
    assert_eq!(abandoned["result"]["state"], "abandoned", "{abandoned:?}");
    assert!(
        worktree.exists(),
        "this test is only meaningful while the failed cleanup leaves the worktree behind"
    );
    assert!(
        !state.tabs.contains_key(&derived_agent_key(&root, &run_id)),
        "an abandoned run's agent is gone from the registry"
    );
    assert!(
        process_reaped(agent_pid),
        "an abandoned run's agent process is killed and reaped"
    );
    let session = primary_thread(&state.runs[&run_id].agents)
        .sessions
        .last()
        .expect("the run had a session");
    assert!(
        session.ended_at.is_some(),
        "abandon ends the session it just killed: {session:?}"
    );
}

/// Only entities with no meaningful conversation left refuse a post:
/// terminal states and unknown ids.
#[test]
fn thread_post_refuses_terminal_and_unknown_entities() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());

    let (_, run_id) = planned_run_in_review(&mut state, "goes away");
    state.handle(req("run.abandon", json!({ "run_id": run_id })));
    let refused = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "body": "anyone home?" }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(
        refused["error"].as_str().unwrap().contains("abandoned"),
        "{refused:?}"
    );

    let (_, archived_id) = planned_run_in_review(&mut state, "swept away");
    state.runs.get_mut(&archived_id).unwrap().run.state = RunState::Archived;
    let refused_archived = state.handle(req(
        "thread.post",
        json!({ "entity_id": archived_id, "body": "anyone home?" }),
    ));
    assert_eq!(refused_archived["ok"], false, "{refused_archived:?}");
    assert!(
        refused_archived["error"]
            .as_str()
            .unwrap()
            .contains("archived"),
        "{refused_archived:?}"
    );

    let plan = state.handle(req("plan.create", json!({ "goal": "dropped plan" })));
    let plan_id = plan_id_of(&plan);
    state.handle(req("plan.abandon", json!({ "plan_id": plan_id })));
    let refused_plan = state.handle(req(
        "thread.post",
        json!({ "entity_id": plan_id, "body": "anyone home?" }),
    ));
    assert_eq!(refused_plan["ok"], false, "{refused_plan:?}");
    assert!(
        refused_plan["error"]
            .as_str()
            .unwrap()
            .contains("abandoned"),
        "{refused_plan:?}"
    );

    let unknown = state.handle(req(
        "thread.post",
        json!({ "entity_id": "nope", "body": "hi" }),
    ));
    assert_eq!(unknown["ok"], false, "{unknown:?}");
    assert!(
        unknown["error"].as_str().unwrap().contains("unknown"),
        "{unknown:?}"
    );
}

/// A stage awaiting its validation verdict refuses the dispatching verb
/// (`run.message`) but must NOT block a post-only write.
#[test]
fn thread_post_is_not_blocked_by_a_stage_awaiting_validation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "stage gate wait");
    let active = state.runs.get_mut(&run_id).unwrap();
    active.run.state = RunState::Building;
    active.current_stage_id = Some("stage-1".into());
    active.stages = vec![StageProgress {
        stage_id: "stage-1".into(),
        state: StageProgressState::Built,
        start_sha: None,
        built_sha: None,
        completion_sha: None,
        publication: crate::run::StagePublication::Local,
        invalidation_reason: None,
        validation: None,
    }];

    let dispatching = state.handle(req(
        "run.message",
        json!({ "run_id": run_id, "message": "hurry it up" }),
    ));
    assert_eq!(dispatching["ok"], false, "{dispatching:?}");
    assert!(
        dispatching["error"]
            .as_str()
            .unwrap()
            .contains("awaiting validation"),
        "{dispatching:?}"
    );

    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "body": "for the record" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    assert!(
        posted["result"]["thread"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["data"]["body"] == "for the record"),
        "{posted:?}"
    );
}

/// The plan review gate refuses `plan.message` (the dispatching verb) but
/// accepts a post; a mismatched anchor artifact is rejected by the shared
/// validator.
#[test]
fn thread_post_reaches_a_plan_at_its_review_gate() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "gate keeping" })));
    let plan_id = plan_id_of(&plan);
    assert_eq!(plan["result"]["state"], "plan_review", "{plan:?}");

    let dispatching = state.handle(req(
        "plan.message",
        json!({ "plan_id": plan_id, "message": "psst" }),
    ));
    assert_eq!(dispatching["ok"], false, "{dispatching:?}");

    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": plan_id, "body": "a note at the gate" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    assert_eq!(posted["result"]["state"], "plan_review", "{posted:?}");
    let items = posted["result"]["thread"]["items"].as_array().unwrap();
    let message = items
        .iter()
        .find(|item| item["data"]["body"] == "a note at the gate")
        .unwrap_or_else(|| panic!("posted message missing: {posted:?}"));
    assert_eq!(message["data"]["role"], "user", "{message:?}");
    assert!(message["data"].get("seen_at").is_none(), "{message:?}");

    let bad_anchor = state.handle(req(
        "thread.post",
        json!({
            "entity_id": plan_id,
            "body": "anchored wrong",
            "anchor": { "artifact": "diff", "heading_path": ["A"], "snippet": "x" }
        }),
    ));
    assert_eq!(bad_anchor["ok"], false, "{bad_anchor:?}");
    assert!(
        bad_anchor["error"]
            .as_str()
            .unwrap()
            .contains("anchor artifact must be plan"),
        "{bad_anchor:?}"
    );
}
