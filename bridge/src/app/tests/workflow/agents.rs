use super::*;

// ==== Agents: one entity, several conversations ==========================

/// The rail needs to know whether an agent has a basement to offer, so the
/// digest says it. It is a different question from `working` — one asks
/// what the session can do, the other what it is doing — and the answer to
/// the second must not move when the first is added.
#[test]
fn the_agent_digest_says_whether_its_agent_has_a_terminal() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let terminal_provider = AgentProvider::ALL
        .into_iter()
        .find(|provider| harness_for(*provider).has_terminal())
        .expect("at least one harness exposes a terminal");
    let reporting_provider = AgentProvider::ALL
        .into_iter()
        .find(|provider| !harness_for(*provider).has_terminal())
        .expect("at least one harness reports activity");
    state.default_harness = terminal_provider;
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-basement");
    let agent_id = primary_agent_id(&state, &run_id);
    let root = state
        .entity_agent_root(&run_id)
        .expect("the adopted worktree");
    let bubble = |state: &mut AppState| -> Value {
        let listed = state.handle(req("agent.list", json!({ "entity_id": run_id })));
        listed["result"]["agents"][0].clone()
    };

    // Nothing has started yet, so the PROVIDER answers: it knows whether
    // its spawn will open a terminal, before there is a session to ask. This
    // run's provider is the one with a terminal, so the answer is yes —
    // and on a provider without one the rail stops offering a basement the
    // spawn would refuse, with no second place to fix.
    let idle = bubble(&mut state);
    let provider = state.runs[&run_id].model_choice.provider;
    assert_eq!(
        idle["has_terminal"],
        crate::harness::harness_for(provider).has_terminal(),
        "a session-less agent's answer comes from its provider: {idle:?}"
    );
    assert_eq!(idle["working"], false, "{idle:?}");

    state
        .runs
        .get_mut(&run_id)
        .expect("the run")
        .agents
        .resolve_mut(None)
        .expect("its agent")
        .choice
        .provider = reporting_provider;
    assert_eq!(
        bubble(&mut state)["has_terminal"],
        harness_for(reporting_provider).has_terminal()
    );
    state
        .runs
        .get_mut(&run_id)
        .expect("the run")
        .agents
        .resolve_mut(None)
        .expect("its agent")
        .choice
        .provider = terminal_provider;

    // A PTY session answers for itself, and answers yes: today every
    // session does.
    let (tab, _rx) = Tab::spawn_agent(
        run_id.clone(),
        agent_id.clone(),
        test_agent_session_request(
            terminal_provider,
            warm_tui_spec(),
            root.clone(),
            terminal_size(120, 40),
        ),
    )
    .expect("the agent tab spawns");
    let terminal_key = TabKey::agent(&root, &agent_id);
    let terminal_capability = tab.session.terminal().is_some();
    state
        .session_registry
        .test_insert_tab(terminal_key.clone(), tab);
    let running = bubble(&mut state);
    assert_eq!(running["has_terminal"], terminal_capability, "{running:?}");

    // And a session with no terminal answers no, while still reporting the
    // status it is in — `working` keeps its exact meaning.
    let reporting_tab = terminal_free_agent_tab(&root, &run_id, &agent_id);
    let reporting_capability = reporting_tab.session.terminal().is_some();
    state
        .session_registry
        .test_insert_tab(terminal_key, reporting_tab)
        .expect("the PTY tab it replaces")
        .session
        .end();
    let protocol = bubble(&mut state);
    assert_eq!(
        protocol["has_terminal"], reporting_capability,
        "{protocol:?}"
    );
    assert_eq!(
        protocol["working"], true,
        "a session with no terminal still says what it is doing: {protocol:?}"
    );
}

/// A branch can carry more than one agent, and each gets its own
/// conversation. Nothing an agent says lands in another agent's thread —
/// that separation is the whole point of the rail.
#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: agent_add_gives_a_branch_a_second_conversation is at 20, threshold 15 — bring it under, then remove
fn agent_add_gives_a_branch_a_second_conversation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-two-agents");
    let primary_agent = primary_agent_id(&state, &run_id);

    let added = state.handle(req(
        "agent.add",
        json!({ "entity_id": run_id, "provider": "codex", "model": "gpt-5.6-sol" }),
    ));
    assert_eq!(added["ok"], true, "{added:?}");
    let second_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    assert_ne!(second_agent, primary_agent);
    assert!(second_agent.starts_with("agent-"), "{second_agent}");
    assert_eq!(added["result"]["agent"]["ordinal"], 2);
    assert_eq!(added["result"]["agent"]["provider"], "codex");
    assert_eq!(added["result"]["agent"]["state"], "idle");

    let listed = state.handle(req("agent.list", json!({ "entity_id": run_id })));
    let agents = listed["result"]["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 2, "{listed:?}");
    assert_eq!(agents[0]["id"], primary_agent);
    assert_eq!(agents[1]["id"], second_agent);

    // The rail renders from the entity's row, so the strip has to ride the
    // polled surfaces rather than needing a call of its own.
    let board = state.handle(req("board.list", json!({})));
    let row = board["result"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["run_id"] == json!(run_id.clone()))
        .expect("the branch is on the board");
    let bubbles = row["agents"].as_array().unwrap();
    assert_eq!(bubbles.len(), 2, "{row:?}");
    assert_eq!(bubbles[1]["id"], second_agent);
    assert_eq!(bubbles[1]["provider"], "codex");
    assert_eq!(bubbles[1]["state"], "idle");
    assert_eq!(bubbles[1]["working"], false);
    assert_eq!(bubbles[1]["unread_count"], 0);

    // A message addressed to the second agent lands in the second agent's
    // conversation and nowhere else.
    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "agent_id": second_agent, "body": "only you" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let roster = &state.runs[&run_id].agents;
    assert!(
        roster.primary().unwrap().thread.items.is_empty(),
        "{roster:?}"
    );
    let mailbox = &roster.by_id(&second_agent).unwrap().thread;
    assert_eq!(mailbox.items.len(), 1);
    assert_eq!(mailbox.id, format!("thread:{second_agent}"));
}

/// An issue carries exactly one agent session: implementation is a handoff
/// to a new agent on a branch, never a second agent on the issue.
#[test]
fn agent_add_is_refused_on_an_issue() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "one agent only" })));
    let plan_id = plan_id_of(&plan);

    let refused = state.handle(req("agent.add", json!({ "entity_id": plan_id })));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("exactly one agent"),
        "{refused:?}"
    );
    assert_eq!(state.plans[&plan_id].agents.len(), 1);
}

#[test]
fn agent_add_refuses_an_unknown_entity_and_an_unrunnable_provider() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-bad-provider");

    let unknown = state.handle(req("agent.add", json!({ "entity_id": "run-nope" })));
    assert_eq!(unknown["ok"], false, "{unknown:?}");

    let bad = state.handle(req(
        "agent.add",
        json!({ "entity_id": run_id, "provider": "hal9000" }),
    ));
    assert_eq!(bad["ok"], false, "{bad:?}");
    assert_eq!(state.runs[&run_id].agents.len(), 1, "nothing was added");
}

#[test]
fn agent_add_retries_one_creation_operation_without_a_duplicate() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "idempotent-agent-add");
    let request = json!({
        "entity_id": run_id,
        "creation_id": "create-agent-1",
        "provider": "codex",
        "model": "gpt-5.6-sol",
    });

    let first = state.handle(req("agent.add", request.clone()));
    let retry = state.handle(req("agent.add", request));

    assert_eq!(first["ok"], true, "{first:?}");
    assert_eq!(retry["ok"], true, "{retry:?}");
    assert_eq!(first["result"]["created"], true);
    assert_eq!(retry["result"]["created"], false);
    assert_eq!(
        first["result"]["agent"]["id"],
        retry["result"]["agent"]["id"]
    );
    assert_eq!(state.runs[&run_id].agents.len(), 2);

    let before_reuse = state.runs[&run_id].agents.clone();
    let reused = state.handle(req(
        "agent.add",
        json!({
            "entity_id": run_id,
            "creation_id": "create-agent-1",
            "provider": "codex",
            "model": "gpt-5.6-terra",
        }),
    ));
    assert_eq!(reused["ok"], false, "{reused:?}");
    assert_eq!(state.runs[&run_id].agents.len(), 2);
    assert_eq!(
        state.runs[&run_id].agents, before_reuse,
        "a conflicting retry cannot remove the run or alter its history"
    );

    let valid_retry = state.handle(req(
        "agent.add",
        json!({
            "entity_id": run_id,
            "creation_id": "create-agent-1",
            "provider": "codex",
            "model": "gpt-5.6-sol",
        }),
    ));
    assert_eq!(valid_retry["ok"], true, "{valid_retry:?}");
    assert_eq!(valid_retry["result"]["created"], false);
    assert_eq!(
        valid_retry["result"]["agent"]["id"],
        first["result"]["agent"]["id"]
    );
    assert_eq!(state.runs[&run_id].agents, before_reuse);
}

#[test]
fn agent_add_persistence_failure_leaves_no_provisional_agent() {
    let (dir, repo) = init_repo();
    let run_id;
    let before;
    {
        let mut state = qa_state(&repo, dir.path());
        run_id = adopted_run(&mut state, &repo, dir.path(), "agent-add-write-failure");
        before = state.runs[&run_id].agents.clone();
        state.store.as_ref().unwrap().fail_next_write();
        let refused = state.handle(req(
            "agent.add",
            json!({
                "entity_id": run_id,
                "creation_id": "create-after-failure",
                "provider": "codex",
                "model": "gpt-5.6-sol",
            }),
        ));
        assert_eq!(refused["ok"], false, "{refused:?}");
        assert_eq!(state.runs[&run_id].agents, before);
    }

    let mut reloaded = qa_state(&repo, dir.path());
    assert_eq!(reloaded.runs[&run_id].agents, before);
    let accepted = reloaded.handle(req(
        "agent.add",
        json!({
            "entity_id": run_id,
            "creation_id": "create-after-failure",
            "provider": "codex",
            "model": "gpt-5.6-sol",
        }),
    ));
    assert_eq!(accepted["ok"], true, "{accepted:?}");
    assert_eq!(accepted["result"]["created"], true);
    assert_eq!(reloaded.runs[&run_id].agents.len(), before.len() + 1);
}

/// The mirror of `agent.add`. An agent the human put on a branch can be
/// taken back off it — off the roster, off the board row, out of the
/// attention map — and it stays off across a restart.
#[test]
fn agent_remove_takes_an_added_agent_back_off_the_branch() {
    let (dir, repo) = init_repo();
    let run_id;
    let primary_agent;
    let second_agent;
    {
        let mut state = qa_state(&repo, dir.path());
        run_id = adopted_run(&mut state, &repo, dir.path(), "feature-remove-agent");
        primary_agent = primary_agent_id(&state, &run_id);
        let added = state.handle(req("agent.add", json!({ "entity_id": run_id })));
        assert_eq!(added["ok"], true, "{added:?}");
        second_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();

        // Something was said to it and the human read it, so there is both
        // a conversation and a read cursor to take away with the agent.
        let posted = state.handle(req(
            "thread.post",
            json!({ "entity_id": run_id, "agent_id": second_agent, "body": "only you" }),
        ));
        assert_eq!(posted["ok"], true, "{posted:?}");
        state.handle(req("entity.seen", json!({ "entity_id": run_id })));
        assert!(
            state.attention[&run_id]
                .agent_read_sequences
                .contains_key(&second_agent),
            "the fixture needs a cursor to remove"
        );

        let removed = state.handle(req(
            "agent.remove",
            json!({ "entity_id": run_id, "agent_id": second_agent }),
        ));
        assert_eq!(removed["ok"], true, "{removed:?}");
        assert_eq!(removed["result"]["agent_id"], second_agent);
        // The rail repaints from the answer, so it carries what is left.
        let left = removed["result"]["agents"].as_array().unwrap();
        assert_eq!(left.len(), 1, "{removed:?}");
        assert_eq!(left[0]["id"], primary_agent);
        assert_eq!(left[0]["ordinal"], 1);

        let listed = state.handle(req("agent.list", json!({ "entity_id": run_id })));
        assert_eq!(listed["result"]["agents"].as_array().unwrap().len(), 1);
        assert!(
            state.runs[&run_id].agents.by_id(&second_agent).is_none(),
            "the conversation goes with the agent"
        );
        assert!(
            !state.attention[&run_id]
                .agent_read_sequences
                .contains_key(&second_agent),
            "and so does the cursor that tracked it"
        );

        let board = state.handle(req("board.list", json!({})));
        let row = board["result"]["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["run_id"] == json!(run_id.clone()))
            .expect("the branch is on the board")
            .clone();
        assert_eq!(row["agents"].as_array().unwrap().len(), 1, "{row:?}");
    } // daemon dies

    let mut reloaded = qa_state(&repo, dir.path());
    let listed = reloaded.handle(req("agent.list", json!({ "entity_id": run_id })));
    let agents = listed["result"]["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 1, "the removal was persisted: {listed:?}");
    assert_eq!(agents[0]["id"], primary_agent);
}

/// A request that retires an agent takes that agent's queued turn with it,
/// and the turn may have been queued before the request began — another
/// worker answered a verb and has not drained yet. When the request then
/// fails, the rule that drops what IT queued must see a queue shorter than
/// the one it measured, and answer the refusal rather than panic under the
/// app mutex.
#[test]
fn a_refused_agent_remove_that_dropped_an_earlier_turn_is_answered_not_a_panic() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-refused-remove");
    let added = state.handle(req("agent.add", json!({ "entity_id": run_id })));
    assert_eq!(added["ok"], true, "{added:?}");
    let second_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "agent_id": second_agent, "body": "only you" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    assert!(
        state
            .delivery_queue
            .queued()
            .any(|turn| turn.agent_id == second_agent),
        "the fixture needs a turn queued for the agent before the request starts"
    );
    state.store.as_ref().unwrap().fail_next_write();

    let removed = state.handle(req(
        "agent.remove",
        json!({ "entity_id": run_id, "agent_id": second_agent }),
    ));

    assert_eq!(removed["ok"], false, "{removed:?}");
    assert!(
        removed["error"]
            .as_str()
            .unwrap()
            .contains("injected store failure"),
        "{removed:?}"
    );
    assert!(
        state
            .delivery_queue
            .queued()
            .all(|turn| turn.agent_id != second_agent),
        "the retired agent's turn outlived it"
    );
}

/// A batch has already left the queue when `agent.remove` runs, so pruning
/// `pending_agent_turns` cannot reach it. Delivery must authenticate the
/// frozen owner/agent/conversation again before it reserves a spawn.
#[test]
fn a_drained_turn_cannot_spawn_an_agent_removed_before_delivery() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut app, &repo, dir.path(), "drained-agent-remove");
    let added = app.handle(req("agent.add", json!({ "entity_id": run_id })));
    let removed_agent = added["result"]["agent"]["id"]
        .as_str()
        .expect("the added agent id")
        .to_string();
    let posted = app.handle(req(
        "thread.post",
        json!({
            "entity_id": run_id,
            "agent_id": removed_agent,
            "body": "do not deliver after removal",
        }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let state = app.shared();
    let drained = state.lock().unwrap().take_pending_turns();

    let removed = state.lock().unwrap().handle(req(
        "agent.remove",
        json!({ "entity_id": run_id, "agent_id": removed_agent }),
    ));
    assert_eq!(removed["ok"], true, "{removed:?}");

    DeliveryRunner::run(&state, drained);

    let s = state.lock().unwrap();
    assert!(s.runs[&run_id].agents.by_id(&removed_agent).is_none());
    assert!(
        s.session_registry
            .test_tabs()
            .map(|(key, _)| key)
            .all(|key| key.tab_id != agent_tab_id(&removed_agent)),
        "the drained turn recreated the deleted agent's tab"
    );
    assert!(
        s.session_registry.test_token(&removed_agent).is_none(),
        "the drained turn minted a provider capability for the deleted agent"
    );
}

/// `agent.remove` refuses exactly what `agent.add` refuses: an unknown
/// agent, an unknown entity, and an issue — whose one agent IS the issue's
/// conversation. On a branch every agent may go, the primary included.
#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: agent_remove_refuses_an_issue_and_an_unknown_agent_but_never_the_primary is at 18, threshold 15 — bring it under, then remove
fn agent_remove_refuses_an_issue_and_an_unknown_agent_but_never_the_primary() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-remove-all");
    let primary_agent = primary_agent_id(&state, &run_id);
    let added = state.handle(req("agent.add", json!({ "entity_id": run_id })));
    let second_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();

    let unknown_agent = state.handle(req(
        "agent.remove",
        json!({ "entity_id": run_id, "agent_id": "agent-NOPE" }),
    ));
    assert_eq!(unknown_agent["ok"], false, "{unknown_agent:?}");
    assert!(
        unknown_agent["error"]
            .as_str()
            .unwrap()
            .contains("unknown agent_id"),
        "{unknown_agent:?}"
    );

    let unknown_entity = state.handle(req(
        "agent.remove",
        json!({ "entity_id": "run-nope", "agent_id": second_agent }),
    ));
    assert_eq!(unknown_entity["ok"], false, "{unknown_entity:?}");

    // An issue's one agent IS the issue's conversation: there is nothing to
    // remove there, only an issue to abandon.
    let plan = state.handle(req("plan.create", json!({ "goal": "one agent only" })));
    let plan_id = plan_id_of(&plan);
    let issue_agent = primary_agent_id(&state, &plan_id);
    let issue = state.handle(req(
        "agent.remove",
        json!({ "entity_id": plan_id, "agent_id": issue_agent }),
    ));
    assert_eq!(issue["ok"], false, "{issue:?}");
    assert!(
        issue["error"].as_str().unwrap().contains("issue"),
        "{issue:?}"
    );
    assert_eq!(state.plans[&plan_id].agents.len(), 1);

    // The branch's PRIMARY goes first, and the agent beside it takes its
    // place; then that one goes too, and the branch survives with none.
    let primary_gone = state.handle(req(
        "agent.remove",
        json!({ "entity_id": run_id, "agent_id": primary_agent }),
    ));
    assert_eq!(primary_gone["ok"], true, "{primary_gone:?}");
    assert_eq!(
        state.runs[&run_id].agents.primary().unwrap().id,
        second_agent,
        "the agent beside it is the primary now"
    );

    let last_gone = state.handle(req(
        "agent.remove",
        json!({ "entity_id": run_id, "agent_id": second_agent }),
    ));
    assert_eq!(last_gone["ok"], true, "{last_gone:?}");
    assert!(state.runs[&run_id].agents.is_empty());
    let listed = state.handle(req("agent.list", json!({ "entity_id": run_id })));
    assert_eq!(
        listed["result"]["agents"].as_array().unwrap().len(),
        0,
        "the branch is still on the board, with nobody on its rail: {listed:?}"
    );

    // And the next thing said to it creates an agent that hears it, on a
    // conversation of its own — nothing of the removed ones is resumed.
    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "body": "start over here" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let roster = &state.runs[&run_id].agents;
    assert_eq!(roster.len(), 1, "the post minted exactly one agent");
    let minted = roster.primary().unwrap();
    assert_ne!(minted.id, primary_agent);
    assert_ne!(minted.id, second_agent);
    assert_eq!(
        minted.resume_session_id, None,
        "a fresh agent resumes nothing"
    );
    assert!(
        minted.thread.items.iter().any(
            |item| matches!(item, crate::thread::ThreadItem::Message(message)
                if message.body == "start over here")
        ),
        "the message that created it is the first thing on its conversation: {:?}",
        minted.thread.items
    );
}

#[test]
fn removing_an_implementation_alias_never_rebinds_its_secondary_to_the_issue() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "stable conversation binding");
    let issue_agent = state.plans[&issue_id].agents.sole().id.clone();
    let primary = primary_agent_id(&state, &run_id);
    assert_eq!(
        state.runs[&run_id]
            .agents
            .by_id(&primary)
            .unwrap()
            .conversation_id(),
        issue_agent
    );
    let second = state
        .runs
        .get_mut(&run_id)
        .unwrap()
        .agents
        .add(&run_id, ModelChoice::default(), "2026-09-08T12:00:00Z")
        .id
        .clone();

    let removed = state.handle(req(
        "agent.remove",
        json!({ "entity_id": run_id, "agent_id": primary }),
    ));
    assert_eq!(removed["ok"], true, "{removed:?}");
    let remaining = state.runs[&run_id].agents.primary().unwrap();
    assert_eq!(remaining.id, second);
    assert_eq!(remaining.conversation_id(), second);
    assert_eq!(
        state
            .agent_conversation(&run_id, None)
            .expect("the remaining agent's conversation")
            .agent
            .id,
        second,
        "becoming the rail's first item did not inherit the removed alias"
    );
    assert_ne!(
        state.agent_conversation(&run_id, None).unwrap().agent.id,
        state.plans[&issue_id].agents.sole().id
    );
}

/// A removed agent's harness must not outlive it: it would keep working in
/// the branch's checkout and report `done` for an agent nothing can route
/// to. So the tab goes through the same kill-AND-reap teardown the verbs
/// that remove an owner use, and its MCP capability goes with it — while
/// the agent beside it, sharing the same checkout, keeps running.
#[tokio::test]
async fn agent_remove_kills_and_reaps_the_agents_live_session() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-retire");
    let primary_agent = state.lock().unwrap().runs["run-retire"]
        .agents
        .primary()
        .unwrap()
        .id
        .clone();
    let added = call(&handler, "agent.add", json!({ "entity_id": "run-retire" }));
    assert_eq!(added["ok"], true, "{added:?}");
    let second_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    for agent_id in [&primary_agent, &second_agent] {
        let started = call(
            &handler,
            "agent.start",
            json!({ "id": "run-retire", "agent_id": agent_id }),
        );
        assert_eq!(started["ok"], true, "{started:?}");
    }
    wait_for_deliveries(&state).await;
    let pid = agent_pid(
        state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(&TabKey::agent(&root, &second_agent))
            .unwrap(),
    )
    .unwrap();

    let removed = call(
        &handler,
        "agent.remove",
        json!({ "entity_id": "run-retire", "agent_id": second_agent }),
    );
    assert_eq!(removed["ok"], true, "{removed:?}");

    {
        let s = state.lock().unwrap();
        let tokens: HashMap<_, _> = s.session_registry.test_tokens().collect();
        assert!(
            !s.session_registry
                .contains(&TabKey::agent(&root, &second_agent)),
            "the removed agent's PTY is gone"
        );
        assert!(
            s.session_registry
                .contains(&TabKey::agent(&root, &primary_agent)),
            "the agent beside it kept running"
        );
        assert!(
            s.session_registry.test_token(&second_agent).is_none(),
            "and its capability with it: {:?}",
            tokens
        );
        assert!(s.session_registry.test_token(&primary_agent).is_some());
        assert_eq!(s.runs["run-retire"].agents.len(), 1);
    }
    assert!(process_reaped(pid), "the harness must be killed AND reaped");
}

/// Unread is per agent, and the entry's badge is the union: a message
/// waiting on the second agent makes the branch unread, and reading that
/// agent through clears exactly that much.
#[test]
fn unread_counts_per_agent_and_the_entry_is_their_union() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-unread");
    let added = state.handle(req("agent.add", json!({ "entity_id": run_id })));
    let second_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    let primary_agent = primary_agent_id(&state, &run_id);

    // The human has read everything that exists so far.
    state.handle(req("entity.seen", json!({ "entity_id": run_id })));
    let view = state.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(view["result"]["unread_count"], 0, "{view:?}");

    // Each agent says something; both are attention-class.
    for agent_id in [&primary_agent, &second_agent] {
        let mut run = state.take_run(&run_id).unwrap();
        run.agents
            .resolve_mut(Some(agent_id))
            .unwrap()
            .thread
            .post_agent("here is what I found", None, now_rfc3339());
        state.finish_run_mutation(run_id.clone(), run).unwrap();
    }

    let view = state.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(view["result"]["unread_count"], 2, "{view:?}");
    let digests = view["result"]["agents"].as_array().unwrap();
    assert_eq!(digests.len(), 2, "{digests:?}");
    for digest in digests {
        assert_eq!(digest["unread_count"], 1, "{digest:?}");
    }

    // Reading one agent through clears its badge, and only its badge.
    let seen = state.handle(req(
        "entity.seen",
        json!({ "entity_id": run_id, "agent_id": second_agent }),
    ));
    assert_eq!(seen["ok"], true, "{seen:?}");
    let view = state.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(view["result"]["unread_count"], 1, "{view:?}");
    let digests = view["result"]["agents"].as_array().unwrap();
    let unread_of = |agent_id: &str| {
        digests
            .iter()
            .find(|digest| digest["id"] == agent_id)
            .map(|digest| digest["unread_count"].as_u64().unwrap())
            .unwrap()
    };
    assert_eq!(unread_of(&primary_agent), 1);
    assert_eq!(unread_of(&second_agent), 0);
}

/// Reaching the end of a WINDOW is not reading the conversation. A long
/// conversation reaches the client as a page of its newest items, so a
/// `seen` that names where that page starts has to leave the badge up for
/// an agent message waiting below the floor: the reader was never shipped
/// it, let alone shown it. Scrolling back far enough to hold it is what
/// clears it.
#[test]
fn seeing_a_window_leaves_the_message_below_its_floor_unread() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-window");
    let agent_id = primary_agent_id(&state, &run_id);
    state.handle(req("entity.seen", json!({ "entity_id": run_id })));

    // One message that calls the human, then enough conversation after it
    // to push it out of any window the client opens on the newest items.
    let mut run = state.take_run(&run_id).unwrap();
    {
        let thread = &mut run.agents.resolve_mut(Some(&agent_id)).unwrap().thread;
        thread.post_agent("the question nobody has answered", None, now_rfc3339());
        for turn in 0..40 {
            thread.post_user(format!("turn {turn}"), None, now_rfc3339());
        }
    }
    state.finish_run_mutation(run_id.clone(), run).unwrap();

    let window = state.handle(req(
        "run.get",
        json!({ "run_id": run_id, "thread_limit": 10 }),
    ));
    assert_eq!(window["result"]["unread_count"], 1, "{window:?}");
    let floor = window["result"]["thread"]["oldest_sequence"]
        .as_u64()
        .expect("a page names where it starts");

    let seen = state.handle(req(
        "entity.seen",
        json!({
            "entity_id": run_id,
            "agent_id": agent_id,
            "read_from_sequence": floor,
        }),
    ));
    assert_eq!(seen["ok"], true, "{seen:?}");
    let view = state.handle(req(
        "run.get",
        json!({ "run_id": run_id, "thread_limit": 10 }),
    ));
    assert_eq!(
        view["result"]["unread_count"], 1,
        "a window that never held the message cannot have read it: {view:?}"
    );

    // Scrolled back to the start of the conversation: the same report on a
    // window that does hold the message clears it.
    let seen = state.handle(req(
        "entity.seen",
        json!({
            "entity_id": run_id,
            "agent_id": agent_id,
            "read_from_sequence": 1,
        }),
    ));
    assert_eq!(seen["ok"], true, "{seen:?}");
    let view = state.handle(req(
        "run.get",
        json!({ "run_id": run_id, "thread_limit": 10 }),
    ));
    assert_eq!(view["result"]["unread_count"], 0, "{view:?}");
}

/// Two agents on one checkout are two PTYs, and each reports as itself:
/// the MCP capability is minted per agent, so the second spawn cannot take
/// the first agent's identity.
#[tokio::test]
async fn two_agents_on_one_worktree_get_their_own_ptys_and_tokens() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-pair");
    let primary_agent = state.lock().unwrap().runs["run-pair"]
        .agents
        .primary()
        .unwrap()
        .id
        .clone();
    let added = call(&handler, "agent.add", json!({ "entity_id": "run-pair" }));
    assert_eq!(added["ok"], true, "{added:?}");
    let second_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();

    let one = call(&handler, "agent.start", json!({ "id": "run-pair" }));
    assert_eq!(one["ok"], true, "{one:?}");
    assert_eq!(
        one["result"]["agent_id"], primary_agent,
        "the default is the first agent"
    );
    let two = call(
        &handler,
        "agent.start",
        json!({ "id": "run-pair", "agent_id": second_agent }),
    );
    assert_eq!(two["ok"], true, "{two:?}");
    assert_ne!(two["result"]["term_id"], one["result"]["term_id"]);
    wait_for_deliveries(&state).await;

    let s = state.lock().unwrap();
    let tokens: HashMap<_, _> = s.session_registry.test_tokens().collect();
    assert!(s
        .session_registry
        .contains(&TabKey::agent(&root, &primary_agent)));
    assert!(s
        .session_registry
        .contains(&TabKey::agent(&root, &second_agent)));
    assert_eq!(
        s.session_registry
            .test_tabs()
            .map(|(key, _)| key)
            .filter(|key| key.is_agent())
            .count(),
        2,
        "the one-agent-per-worktree rule is gone for branches"
    );
    assert!(tokens.contains_key(primary_agent.as_str()), "{tokens:?}");
    assert!(tokens.contains_key(second_agent.as_str()), "{tokens:?}");
    assert_ne!(
        tokens[primary_agent.as_str()],
        tokens[second_agent.as_str()]
    );
    // Each agent's harness reports through its own config file.
    assert!(root
        .join(crate::orchestrator::mcp_config_path(&second_agent))
        .is_file());
}

/// The bodies of a detail poll's conversation, in wire order.
fn thread_bodies(view: &Value) -> Vec<String> {
    view["items"]
        .as_array()
        .unwrap_or_else(|| panic!("a thread ships items: {view:?}"))
        .iter()
        .filter_map(|item| item["data"]["body"].as_str().map(str::to_string))
        .collect()
}

/// Whether a conversation holds a message with exactly this body.
fn thread_holds(thread: &crate::thread::Thread, body: &str) -> bool {
    thread.items.iter().any(|item| match item {
        crate::thread::ThreadItem::Message(message) => message.body == body,
        crate::thread::ThreadItem::Event(_) => false,
    })
}

/// A branch with two agents, each with one thing said to it — the setup
/// every per-agent selection test starts from.
fn branch_with_two_conversations(
    state: &mut AppState,
    repo: &std::path::Path,
    dir: &std::path::Path,
    branch: &str,
) -> (String, String, String) {
    let run_id = adopted_run(state, repo, dir, branch);
    let primary_agent = primary_agent_id(state, &run_id);
    let added = state.handle(req("agent.add", json!({ "entity_id": run_id })));
    assert_eq!(added["ok"], true, "{added:?}");
    let second_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    for (agent_id, body) in [
        (&primary_agent, "first-agent-marker"),
        (&second_agent, "second-agent-marker"),
    ] {
        let posted = state.handle(req(
            "thread.post",
            json!({ "entity_id": run_id, "agent_id": agent_id, "body": body }),
        ));
        assert_eq!(posted["ok"], true, "{posted:?}");
    }
    (run_id, primary_agent, second_agent)
}

/// A detail poll answers with the conversation of the agent it named. The
/// rail's bubble is the selector, so `run.get`/`branch.get` have to be able
/// to say WHICH conversation — and an id that names no agent on this entity
/// is an error, never a silent fall back to the first one's.
#[test]
fn a_detail_poll_answers_with_the_named_agents_conversation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (run_id, primary_agent, second_agent) =
        branch_with_two_conversations(&mut state, &repo, dir.path(), "feature-two-threads");

    // Named nothing: the conversation every surface before the rail asked
    // for — the entity's first agent's.
    let default_view = state.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(
        thread_bodies(&default_view["result"]["thread"]),
        vec!["first-agent-marker".to_string()],
        "{default_view:?}"
    );

    let first_view = state.handle(req(
        "run.get",
        json!({ "run_id": run_id, "agent_id": primary_agent }),
    ));
    assert_eq!(
        thread_bodies(&first_view["result"]["thread"]),
        vec!["first-agent-marker".to_string()],
        "{first_view:?}"
    );

    let second_view = state.handle(req(
        "run.get",
        json!({ "run_id": run_id, "agent_id": second_agent }),
    ));
    assert_eq!(
        thread_bodies(&second_view["result"]["thread"]),
        vec!["second-agent-marker".to_string()],
        "{second_view:?}"
    );

    let unknown = state.handle(req(
        "run.get",
        json!({ "run_id": run_id, "agent_id": "agent-NOSUCHTHING" }),
    ));
    assert_eq!(unknown["ok"], false, "{unknown:?}");
    assert!(
        unknown["error"]
            .as_str()
            .unwrap()
            .contains("unknown agent_id"),
        "{unknown:?}"
    );
}

/// `branch.get` is the branch surface's read, and it carries the run's view
/// whole — including which agent's conversation the caller asked for.
#[test]
fn branch_get_carries_the_named_agents_conversation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, _, second_agent) =
        branch_with_two_conversations(&mut state, &repo, dir.path(), "feature-branch-threads");
    let project_id = state.project_at(0).id.clone();

    let default_row = state.handle(req(
        "branch.get",
        json!({ "project_id": project_id, "branch": "feature-branch-threads" }),
    ));
    assert_eq!(
        thread_bodies(&default_row["result"]["run"]["thread"]),
        vec!["first-agent-marker".to_string()],
        "{default_row:?}"
    );

    let selected = state.handle(req(
        "branch.get",
        json!({
            "project_id": project_id,
            "branch": "feature-branch-threads",
            "agent_id": second_agent
        }),
    ));
    assert_eq!(
        thread_bodies(&selected["result"]["run"]["thread"]),
        vec!["second-agent-marker".to_string()],
        "{selected:?}"
    );

    let unknown = state.handle(req(
        "branch.get",
        json!({
            "project_id": project_id,
            "branch": "feature-branch-threads",
            "agent_id": "agent-NOSUCHTHING"
        }),
    ));
    assert_eq!(unknown["ok"], false, "{unknown:?}");
    assert!(
        unknown["error"]
            .as_str()
            .unwrap()
            .contains("unknown agent_id"),
        "{unknown:?}"
    );
}

/// `branch.get` is the branch surface's read, and that surface paints no
/// conversation — the rail beside it does, off its own paged read of this
/// same RPC. So the surface asks for the smallest page there is, and the
/// bound has to hold on both roads through the run view: the poll that
/// named an agent (the rail's bubble is open) and the poll that named none.
#[test]
fn branch_get_ships_the_page_the_branch_surface_asked_for() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-bounded-branch");
    let primary_agent = primary_agent_id(&state, &run_id);
    let held = {
        let active = state.runs.get_mut(&run_id).unwrap();
        for turn in 0..250 {
            primary_thread_mut(&mut active.agents).post_user(
                format!("turn {turn}"),
                None,
                now_rfc3339(),
            );
        }
        primary_thread(&active.agents).items.len()
    };
    let project_id = state.project_at(0).id.clone();

    for scope in [json!({}), json!({ "agent_id": primary_agent })] {
        let mut params = json!({
            "project_id": project_id,
            "branch": "feature-bounded-branch",
            "thread_limit": 1,
        });
        for (key, value) in scope.as_object().unwrap() {
            params[key] = value.clone();
        }
        let read = state.handle(req("branch.get", params));
        assert_eq!(read["ok"], true, "{read:?}");
        let thread = &read["result"]["run"]["thread"];
        assert_eq!(thread["items"].as_array().unwrap().len(), 1, "{read:?}");
        // Bounded, and still honest about the conversation behind the
        // window: the count is the whole of it, and there is more above.
        assert_eq!(thread["thread_total"], held as u64, "{read:?}");
        assert_eq!(thread["has_more"], true, "{read:?}");
        assert!(!read.to_string().contains("turn 0\""), "{read:?}");
    }
}

/// The delta cursor is per conversation: a sequence held for one agent's
/// thread must be applied to THAT thread, and the totals it is checked
/// against must be that thread's too. Cursoring one agent can never drain
/// another's.
#[test]
fn the_thread_cursor_is_read_against_the_named_agents_conversation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (run_id, _, second_agent) =
        branch_with_two_conversations(&mut state, &repo, dir.path(), "feature-cursor-threads");
    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "agent_id": second_agent, "body": "and one more" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");

    let full = state.handle(req(
        "run.get",
        json!({ "run_id": run_id, "agent_id": second_agent }),
    ));
    let items = full["result"]["thread"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{full:?}");
    let cursor = items[0]["data"]["sequence"].as_u64().unwrap();

    let delta = state.handle(req(
        "run.get",
        json!({
            "run_id": run_id,
            "agent_id": second_agent,
            "thread_after_sequence": cursor
        }),
    ));
    assert_eq!(
        thread_bodies(&delta["result"]["thread"]),
        vec!["and one more".to_string()],
        "{delta:?}"
    );
    assert_eq!(delta["result"]["thread"]["thread_total"], 2, "{delta:?}");

    // The same cursor against the FIRST agent's conversation reads its own
    // sequences: its one message is older, so it is already held.
    let other = state.handle(req(
        "run.get",
        json!({ "run_id": run_id, "thread_after_sequence": cursor }),
    ));
    assert!(
        thread_bodies(&other["result"]["thread"]).is_empty(),
        "{other:?}"
    );
    assert_eq!(other["result"]["thread"]["thread_total"], 1, "{other:?}");
}

#[test]
fn per_message_read_reports_preserve_explicit_agent_isolation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (run_id, first_agent, second_agent) =
        branch_with_two_conversations(&mut state, &repo, dir.path(), "scoped-read-report");
    let first_cursor = state.read_cursor(&run_id, &first_agent);
    let reached = state
        .edit_agent_conversation(&run_id, &second_agent, |thread, _| {
            thread.post_agent("first question", None, now_rfc3339());
            let reached = thread.last_sequence();
            thread.post_agent("still unread", None, now_rfc3339());
            Ok(reached)
        })
        .unwrap();

    let read = state.handle(req(
        "entity.seen",
        json!({
            "entity_id": run_id,
            "agent_id": second_agent,
            "conversation_id": second_agent,
            "read_through_sequence": reached,
        }),
    ));
    assert_eq!(read["ok"], true, "{read:?}");
    assert_eq!(state.read_cursor(&run_id, &second_agent), reached);
    assert_eq!(state.read_cursor(&run_id, &first_agent), first_cursor);

    let refused = state.handle(req(
        "entity.seen",
        json!({ "entity_id": run_id, "agent_id": "", "read_through_sequence": u64::MAX }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(state.read_cursor(&run_id, &second_agent), reached);
    assert_eq!(state.read_cursor(&run_id, &first_agent), first_cursor);
}

/// An issue carries exactly one agent session, so naming it is a check
/// rather than a choice — but the check has to hold: an id that is not this
/// issue's agent is refused instead of answering with the issue's own.
#[test]
fn issue_get_honors_the_agent_it_was_addressed_to() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue = state.handle(req("plan.create", json!({ "goal": "one conversation" })));
    let issue_id = plan_id_of(&issue);
    let agent_id = primary_agent_id(&state, &issue_id);

    let named = state.handle(req(
        "issue.get",
        json!({ "issue_id": issue_id, "agent_id": agent_id }),
    ));
    assert_eq!(named["ok"], true, "{named:?}");
    let default_view = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
    assert_eq!(
        named["result"]["thread"]["items"], default_view["result"]["thread"]["items"],
        "the issue's one agent IS the issue's conversation"
    );

    let unknown = state.handle(req(
        "issue.get",
        json!({ "issue_id": issue_id, "agent_id": "agent-NOSUCHTHING" }),
    ));
    assert_eq!(unknown["ok"], false, "{unknown:?}");
    assert!(
        unknown["error"]
            .as_str()
            .unwrap()
            .contains("unknown agent_id"),
        "{unknown:?}"
    );
}

/// Review comments land in the conversation the reviewer was reading. The
/// Changes surface sits under the rail, so the agent whose bubble is open
/// is the agent the comments are addressed to — and the turn they queue
/// goes to that agent's PTY, not to the branch's first.
#[test]
fn request_changes_lands_on_the_named_agents_conversation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "review with two agents");
    let primary_agent = primary_agent_id(&state, &run_id);
    let added = state.handle(req("agent.add", json!({ "entity_id": run_id })));
    let second_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();

    let addressed = state.handle(req(
        "run.request_changes",
        json!({
            "run_id": run_id,
            "agent_id": second_agent,
            "messages": [{ "body": "second-agent-comment", "anchor": null }]
        }),
    ));
    assert_eq!(addressed["ok"], true, "{addressed:?}");

    // The comments are on the addressed agent's own thread, and nowhere
    // else: not on the branch's first agent, not on the Issue the first
    // agent speaks in.
    let second_thread = &state.runs[&run_id]
        .agents
        .by_id(&second_agent)
        .expect("the added agent is on the roster")
        .thread;
    assert!(
        thread_holds(second_thread, "second-agent-comment"),
        "{second_thread:?}"
    );
    assert!(
        !thread_holds(
            state.plans[&issue_id].agents.sole_thread(),
            "second-agent-comment"
        ),
        "the Issue's conversation belongs to the first agent"
    );

    let queued = state
        .delivery_queue
        .queued_last()
        .expect("a change request is a turn");
    assert_eq!(queued.agent_id, second_agent);
    assert_ne!(queued.agent_id, primary_agent);
    let delivered =
        state.cold_prompt_with_catch_up(&queued.owner, &queued.agent_id, &queued.said().cold);
    assert!(
        delivered.contains("second-agent-comment"),
        "a cold spawn catches up on ITS conversation: {delivered}"
    );

    // Named nothing, the comments still land where every surface before the
    // rail put them: the first agent's conversation, which for a planned
    // implementation is the Issue's.
    let defaulted = state.handle(req(
        "run.request_changes",
        json!({
            "run_id": run_id,
            "messages": [{ "body": "first-agent-comment", "anchor": null }]
        }),
    ));
    assert_eq!(defaulted["ok"], true, "{defaulted:?}");
    assert!(
        thread_holds(
            state.plans[&issue_id].agents.sole_thread(),
            "first-agent-comment"
        ),
        "{:?}",
        primary_thread(&state.plans[&issue_id].agents).items
    );
    assert_eq!(
        state.delivery_queue.queued_last().unwrap().agent_id,
        primary_agent
    );

    let unknown = state.handle(req(
        "run.request_changes",
        json!({
            "run_id": run_id,
            "agent_id": "agent-NOSUCHTHING",
            "messages": [{ "body": "nowhere", "anchor": null }]
        }),
    ));
    assert_eq!(unknown["ok"], false, "{unknown:?}");
    assert!(
        unknown["error"]
            .as_str()
            .unwrap()
            .contains("unknown agent_id"),
        "{unknown:?}"
    );
}
