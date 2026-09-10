use super::*;

// ---- terminals + agent screens over runs ---------------------------------

/// A run in `state` whose WORKTREE has a live agent tab, painting steadily
/// — the shape an attaching client meets. Returns the tab's key and the
/// wire id it is addressed by.
pub(in crate::app::tests) fn insert_live_run(
    state: &Arc<Mutex<AppState>>,
    repo: &std::path::Path,
    side_root: std::path::PathBuf,
    run_id: &str,
) -> (TabKey, String) {
    let store = crate::store::Store::new(side_root.join("store")).expect("store opens");
    let side = Orchestrator::new(
        repo.to_path_buf(),
        side_root.join("wt"),
        Agent::Warm(HarnessSpec::new("true")),
        Templates::default(),
        test_bridge_exe(),
    );
    let plan = approved_side_plan(&side, &store, &format!("plan-of-{run_id}"));
    let (active, _turn) = dispatch_side_run(&side, &store, &plan, run_id);
    let root = AppState::canonical_root(&active.worktree.path);
    let (tab, rx) = Tab::spawn_agent(
        run_id.to_string(),
        crate::agent::derived_agent_id(run_id),
        test_agent_session_request(
            AgentProvider::default(),
            HarnessSpec::new("sh").arg("-c").arg(
            "printf '\\033[?2004h'; (while :; do echo agent-beat; sleep 0.05; done) & cat >/dev/null",
        ),
            root.clone(),
            terminal_size(120, 40),
        ),
    )
    .expect("the agent tab spawns");
    let key = derived_agent_key(&root, run_id);
    let wire_id = tab.wire_id();
    {
        let mut s = state.lock().unwrap();
        let project_id = s.project_at(0).id.clone();
        s.projects.bind_entity(run_id.to_string(), project_id);
        s.runs.insert(run_id.to_string(), active);
        s.session_registry.test_insert_tab(key.clone(), tab);
        let instance = s.record_agent_session_start(
            run_id,
            &crate::agent::derived_agent_id(run_id),
            &root,
            &ModelChoice::default(),
            "build",
        );
        s.session_registry
            .test_tab_mut(&key)
            .unwrap()
            .session_instance = instance.clone();
    }
    spawn_tab_pumps(state, key.clone(), rx);
    (key, wire_id)
}

/// Put an intentionally unmanaged agent-shaped process in an external
/// checkout. It has no entity or conversation lineage: these tests model
/// a raw process discovered in a worktree Build has not adopted, so it
/// must not pass through the managed delivery/session fixtures.
pub(in crate::app::tests) fn insert_unmanaged_agent_tab(
    state: &Arc<Mutex<AppState>>,
    root: &std::path::Path,
    owner: &str,
) -> Result<TabKey, String> {
    let agent_id = crate::agent::derived_agent_id(owner);
    let key = TabKey::agent(root, &agent_id);
    let (tab, output) = Tab::spawn_agent(
        owner.to_string(),
        agent_id,
        test_agent_session_request(
            AgentProvider::default(),
            warm_tui_spec(),
            root.to_path_buf(),
            terminal_size(120, 40),
        ),
    )?;
    state
        .lock()
        .unwrap()
        .session_registry
        .test_insert_tab(key.clone(), tab);
    spawn_tab_pumps(state, key.clone(), output);
    Ok(key)
}

/// A run with a worktree but no agent tab — the state every worktree is in
/// before anyone speaks to it, and the one a surface's "Start agent" button
/// acts on. Returns the run's canonical root.
pub(in crate::app::tests) fn insert_run_without_agent(
    state: &Arc<Mutex<AppState>>,
    repo: &std::path::Path,
    side_root: std::path::PathBuf,
    run_id: &str,
) -> std::path::PathBuf {
    let store = crate::store::Store::new(side_root.join("store")).expect("store opens");
    let side = Orchestrator::new(
        repo.to_path_buf(),
        side_root.join("wt"),
        Agent::Warm(HarnessSpec::new("true")),
        Templates::default(),
        test_bridge_exe(),
    );
    let plan = approved_side_plan(&side, &store, &format!("plan-of-{run_id}"));
    let (active, _turn) = dispatch_side_run(&side, &store, &plan, run_id);
    let root = AppState::canonical_root(&active.worktree.path);
    let mut s = state.lock().unwrap();
    let project_id = s.project_at(0).id.clone();
    s.projects.bind_entity(run_id.to_string(), project_id);
    s.runs.insert(run_id.to_string(), active);
    root
}

/// `agent.start` is the surface's "Start agent" button: it opens the
/// worktree's one agent WITHOUT a turn to deliver. Attaching never spawns
/// (mounting a tab is a look), so before this verb the only way to get an
/// agent was to send it work — which is no help when the human just wants
/// the thing running, or wants it back after it exited.
#[tokio::test]
async fn agent_start_opens_the_worktrees_agent_and_is_idempotent() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-start");
    let key = derived_agent_key(&root, "run-start");
    assert!(
        !state.lock().unwrap().session_registry.contains(&key),
        "the worktree has no agent until someone asks for one"
    );

    let started = call(&handler, "agent.start", json!({ "id": "run-start" }));
    assert_eq!(started["ok"], true, "{started:?}");
    let wire_id = started["result"]["term_id"].as_str().unwrap().to_string();
    assert!(
        wire_id.starts_with("agent:"),
        "an agent is addressed by its worktree: {wire_id}"
    );
    wait_for_agent_tab(&state, &key).await;
    let pid = {
        let s = state.lock().unwrap();
        agent_pid(
            s.session_registry
                .test_tab(&key)
                .expect("the agent tab exists"),
        )
    };

    let again = call(&handler, "agent.start", json!({ "id": "run-start" }));
    assert_eq!(again["ok"], true, "{again:?}");
    assert_eq!(
        again["result"]["term_id"], wire_id,
        "a second start addresses the same tab"
    );
    wait_for_deliveries(&state).await;
    assert_eq!(
        agent_pid(
            state
                .lock()
                .unwrap()
                .session_registry
                .test_tab(&key)
                .unwrap()
        ),
        pid,
        "starting an agent that is already running must not spawn a second one"
    );
}

/// The restart case the human actually hits: the harness exited (codex ran
/// its self-update and quit, claude crashed), the tab retains the dead
/// screen, and the button has to bring a NEW process back on the same tab.
#[tokio::test]
async fn agent_start_restarts_an_agent_that_exited() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-restart");
    let key = derived_agent_key(&root, "run-restart");

    let first = call(&handler, "agent.start", json!({ "id": "run-restart" }));
    assert_eq!(first["ok"], true, "{first:?}");
    wait_for_agent_tab(&state, &key).await;
    let first_pid = {
        let s = state.lock().unwrap();
        agent_pid(s.session_registry.test_tab(&key).unwrap())
    };

    // The harness dies the way a real one does, and the tab is RETAINED so
    // the human can still read the last screen.
    {
        let mut s = state.lock().unwrap();
        let tab = s.session_registry.test_tab_mut(&key).unwrap();
        tab.session.end();
        tab.live = false;
    }

    let restarted = call(&handler, "agent.start", json!({ "id": "run-restart" }));
    assert_eq!(restarted["ok"], true, "{restarted:?}");
    assert_eq!(
        restarted["result"]["term_id"], first["result"]["term_id"],
        "a restart addresses the tab the agent already had: {restarted:?}"
    );
    wait_for_deliveries(&state).await;
    let s = state.lock().unwrap();
    let tab = s
        .session_registry
        .test_tab(&key)
        .expect("the tab came back");
    assert!(tab.live, "the restarted agent is live");
    assert_ne!(
        agent_pid(tab),
        first_pid,
        "restart means a NEW process, not the corpse reported as alive"
    );
}

/// The failure the human actually hits: an agent's harness exited (the TUI
/// self-updated and quit, the process died), the Agent tab still shows the
/// last screen it painted, and a message typed into the conversation lands
/// on the thread with nothing running to read it. The entity looks idle and
/// nobody is listening. A message to an agent that is not running starts it
/// again — and takes the message with it.
#[tokio::test]
async fn a_message_to_an_agent_whose_harness_exited_revives_it() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-revive");
    let key = derived_agent_key(&root, "run-revive");
    let started = call(&handler, "agent.start", json!({ "id": "run-revive" }));
    assert_eq!(started["ok"], true, "{started:?}");
    wait_for_agent_tab(&state, &key).await;
    let dead_pid = agent_pid(
        state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(&key)
            .unwrap(),
    );

    // The harness dies the way a real one does, and the tab is RETAINED so
    // the human can still read the last screen.
    {
        let mut s = state.lock().unwrap();
        let tab = s.session_registry.test_tab_mut(&key).unwrap();
        tab.session.end();
        tab.live = false;
    }
    // Let the old pump see its own EOF before the revival, so the tab it
    // closes is the corpse rather than the replacement.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-revive", "body": "are you still on this?" }),
    );

    assert_eq!(posted["ok"], true, "{posted:?}");
    wait_for_deliveries(&state).await;
    {
        let s = state.lock().unwrap();
        let tab = s
            .session_registry
            .test_tab(&key)
            .expect("the agent came back");
        assert!(
            tab.session_is_live(),
            "a message to a dead agent brings it back running"
        );
        assert_ne!(
            agent_pid(tab),
            dead_pid,
            "revival is a NEW process, not the corpse reported as alive"
        );
        assert!(
            tab.last_delivered_at.is_some(),
            "and the message that revived it was written into it"
        );
    }
    let screen = wait_for_agent_screen(&state, &root, "read_unread_messages").await;
    assert!(
        screen.contains("read_unread_messages"),
        "the revived agent is told to read what was said while it was down: {screen:?}"
    );
}

/// The guard on revival: an agent whose harness is being started RIGHT NOW
/// to hear a message must not get a second one. Two harnesses in one
/// checkout both report `done` for the same owner, and the second report
/// is an illegal transition that lands on the conversation as a bogus
/// failure. The turn already mid-delivery is the one that reads this
/// message: it opens on the cold prompt, which tells it to call
/// `read_unread_messages`, and the post made the message durable before
/// the harness could ask.
#[test]
fn a_message_sent_while_the_agent_is_starting_does_not_start_a_second_one() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-in-flight");
    let root = state.entity_agent_root(&run_id).unwrap();
    let agent_id = primary_agent_id(&state, &run_id);
    state.pending_agent_turns.clear();
    let first = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "body": "the first thing" }),
    ));
    assert_eq!(first["ok"], true, "{first:?}");
    assert_eq!(
        state.pending_agent_turns.len(),
        1,
        "the first message is what brings the agent back"
    );
    // Off the queue and mid-delivery: the harness is starting to hear it.
    let mut delivering = state.take_pending_turns();

    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "body": "one more thing" }),
    ));

    assert_eq!(posted["ok"], true, "{posted:?}");
    assert!(
        state.pending_agent_turns.is_empty(),
        "the harness already starting is the one that reads this; {} turns were queued",
        state.pending_agent_turns.len()
    );

    // …and with nothing in flight, the same message is what brings the
    // agent back.
    while let Some((_, mark)) = delivering.next_turn() {
        mark.settle(&mut state);
    }
    let again = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "body": "still there?" }),
    ));
    assert_eq!(again["ok"], true, "{again:?}");
    assert_eq!(
        state.pending_agent_turns.len(),
        1,
        "one revival, addressed to the agent that was spoken to"
    );
    let queued = &state.pending_agent_turns[0];
    assert_eq!(queued.agent_id, agent_id);
    assert_eq!(queued.root, root);
    assert_eq!(queued.owner, run_id);
    // The words travel in the catch-up packet, which is composed when the
    // turn is handed over — so this is the prompt the revived agent opens
    // on, not the one the queue is holding.
    let delivered =
        state.cold_prompt_with_catch_up(&queued.owner, &queued.agent_id, &queued.said().cold);
    assert!(
        delivered.contains("still there?"),
        "the revived agent opens on what was said to it: {delivered}"
    );
}

/// History without exact persisted lineage never resumes by cwd. Both
/// revived agents start fresh and receive canonical catch-up instead of a
/// provider's newest transcript guess.
#[tokio::test]
async fn revived_agents_without_exact_lineage_never_guess_by_checkout() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let resumed_root =
        insert_run_without_agent(&state, &repo, dir.path().join("resumed"), "run-resumed");
    let fresh_root = insert_run_without_agent(&state, &repo, dir.path().join("fresh"), "run-fresh");
    let specs_built: Arc<Mutex<Vec<SpawnOptions>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let recorder = Arc::clone(&specs_built);
        let agent = Agent::WarmBuilder(Arc::new(
            move |_prompt: &str, _choice: &ModelChoice, options: &SpawnOptions| {
                recorder.lock().unwrap().push(options.clone());
                Ok(warm_tui_spec())
            },
        ));
        let mut s = state.lock().unwrap();
        let worktrees = s.worktrees_root.clone();
        s.project_at_mut(0).orch = Orchestrator::new(
            repo.clone(),
            worktrees,
            agent,
            Templates::default(),
            test_bridge_exe(),
        );
        // Both are respawns: a session of each agent's own has opened
        // before, which is what puts them in the crash window a guess is
        // for. What separates them is only what is on disk.
        for run_id in ["run-resumed", "run-fresh"] {
            primary_thread_mut(&mut s.runs.get_mut(run_id).expect("the run").agents).start_session(
                "claude",
                None,
                None,
                "implementation",
                "2026-08-29T00:00:00Z",
            );
        }
    }

    for run_id in ["run-resumed", "run-fresh"] {
        let posted = call(
            &handler,
            "thread.post",
            json!({ "entity_id": run_id, "body": "pick this up" }),
        );
        assert_eq!(posted["ok"], true, "{posted:?}");
    }

    wait_for_deliveries(&state).await;
    let built = specs_built.lock().unwrap().clone();
    let spawned_in = |root: &std::path::Path| {
        let root = AppState::canonical_root(root);
        built
            .iter()
            .find(|options| options.cwd == root)
            .unwrap_or_else(|| panic!("the message spawned an agent in {}", root.display()))
            .clone()
    };
    assert!(!spawned_in(&resumed_root).continue_session, "{built:?}");
    assert!(
        !spawned_in(&fresh_root).continue_session,
        "an agent that has never run has nothing to continue: {built:?}"
    );
}

/// Starting an agent by hand must not strand what is already waiting for
/// it. The reviewer's words are durable on the thread, and the ONLY way an
/// agent learns of them is being told to call `read_unread_messages` — a
/// fresh harness has no reason to. Without this, pressing Restart after a
/// crash brings back an agent that silently ignores every message posted
/// while it was down.
#[tokio::test]
async fn agent_start_tells_a_fresh_agent_what_is_waiting_for_it() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-waiting");
    {
        let mut s = state.lock().unwrap();
        let run = s.runs.get_mut("run-waiting").unwrap();
        primary_thread_mut(&mut run.agents).post_user(
            "look at the migration",
            None,
            "2026-07-29T12:00:00Z",
        );
    }

    let started = call(&handler, "agent.start", json!({ "id": "run-waiting" }));
    assert_eq!(started["ok"], true, "{started:?}");
    let screen = wait_for_agent_screen(&state, &root, "read_unread_messages").await;
    assert!(
        screen.contains("read_unread_messages"),
        "a started agent must be told to read what is waiting: {screen:?}"
    );
}

/// The other half: a start with nothing waiting says NOTHING. The button
/// means "give me an agent", not "go do something" — the human drives it
/// from there. An unsolicited prompt would put a fresh agent to work nobody
/// asked it to do.
#[tokio::test]
async fn agent_start_says_nothing_when_nothing_is_waiting() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-quiet");

    let started = call(&handler, "agent.start", json!({ "id": "run-quiet" }));
    assert_eq!(started["ok"], true, "{started:?}");
    // Give a prompt every chance to appear before concluding none did.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let screen = agent_screen_text(&state, &root);
    assert!(
        !screen.contains("read_unread_messages"),
        "an agent with nothing waiting must be left alone: {screen:?}"
    );
}

/// An id that owns no worktree cannot have an agent started in it — the
/// MCP `done` route is scaffolded per owner, so there is nothing to own it.
#[tokio::test]
async fn agent_start_refuses_an_id_that_owns_no_worktree() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let _ = &state;
    let refused = call(&handler, "agent.start", json!({ "id": "run-nowhere" }));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(
        refused["error"].as_str().unwrap().contains("unknown id"),
        "{refused:?}"
    );
}

/// A plan holding its workspace with no agent tab — the plan surface's idle
/// Agent tab. The dispatch turn is dropped: this fixture is about the plan,
/// not about delivering to it.
pub(in crate::app::tests) fn insert_plan_without_agent(
    state: &Arc<Mutex<AppState>>,
    repo: &std::path::Path,
    side_root: std::path::PathBuf,
    plan_id: &str,
) {
    let side = Orchestrator::new(
        repo.to_path_buf(),
        side_root.join("wt"),
        Agent::Warm(HarnessSpec::new("true")),
        Templates::default(),
        test_bridge_exe(),
    );
    let mut active = side.create_plan(
        PlanId::new(plan_id),
        "side goal",
        "main",
        Default::default(),
    );
    let store = state
        .lock()
        .unwrap()
        .require_store()
        .expect("the daemon has a store")
        .clone();
    let workspace = side.prepare_plan_workspace(plan_id, &store).unwrap();
    side.open_plan_drafting(&mut active, workspace).unwrap();
    let mut s = state.lock().unwrap();
    let project_id = s.project_at(0).id.clone();
    s.projects.bind_entity(plan_id.to_string(), project_id);
    s.plans.insert(plan_id.to_string(), active);
}

/// The Agent tab's provider picker: a start may NAME the provider it wants.
/// The picker chooses what this worktree runs on from here on — not just
/// what this one process runs on — so the entity's persisted choice follows
/// it and a restart honors it.
#[tokio::test]
async fn agent_start_with_a_provider_switches_and_persists_the_choice() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    let adopted = call(
        &handler,
        "run.adopt",
        json!({ "project_id": project_id, "primary": true }),
    );
    let run_id = run_id_of(&adopted);

    let started = call(
        &handler,
        "agent.start",
        json!({ "id": run_id, "provider": "codex" }),
    );
    assert_eq!(started["ok"], true, "{started:?}");
    wait_for_deliveries(&state).await;
    let choice = state.lock().unwrap().runs[&run_id].model_choice.clone();
    assert_eq!(choice.provider, AgentProvider::Codex);
    assert_eq!(
        choice.model, None,
        "a bare provider brings its own defaults, not the last provider's model"
    );
    assert_eq!(choice.effort, None);
    let persisted = crate::store::Store::new(dir.path().join("store"))
        .expect("store opens")
        .load_all_runs()
        .unwrap()
        .into_iter()
        .find(|run| run.id == run_id)
        .expect("the primary run is on disk");
    assert_eq!(
        persisted.provider,
        AgentProvider::Codex,
        "a restart must bring the provider the human picked back"
    );
}

/// A plan already owns its sole agent, so starting it cannot move that
/// agent onto another provider. Provider selection belongs to creation;
/// model and effort remain editable afterward through `agent.choose`.
#[tokio::test]
async fn agent_start_refuses_switching_an_existing_plans_provider() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    insert_plan_without_agent(&state, &repo, dir.path().join("side"), "plan-switch");

    let started = call(
        &handler,
        "agent.start",
        json!({ "id": "plan-switch", "provider": "codex" }),
    );
    assert_eq!(started["ok"], false, "{started:?}");
    assert!(started["error"].as_str().unwrap().contains("locked to"));
    assert_eq!(
        state.lock().unwrap().plans["plan-switch"]
            .agents
            .sole()
            .choice
            .provider,
        AgentProvider::Claude
    );
}

/// A start with no provider named is the start that has always existed: the
/// entity keeps the model AND effort chosen when it was created.
#[tokio::test]
async fn agent_start_without_a_provider_keeps_the_entitys_choice() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-kept");
    {
        let mut s = state.lock().unwrap();
        s.runs.get_mut("run-kept").unwrap().model_choice = ModelChoice {
            provider: AgentProvider::Codex,
            model: Some("gpt-5.6-terra".to_string()),
            effort: Some("high".to_string()),
        };
    }

    let started = call(&handler, "agent.start", json!({ "id": "run-kept" }));
    assert_eq!(started["ok"], true, "{started:?}");
    let choice = state.lock().unwrap().runs["run-kept"].model_choice.clone();
    assert_eq!(choice.provider, AgentProvider::Codex);
    assert_eq!(choice.model.as_deref(), Some("gpt-5.6-terra"));
    assert_eq!(
        choice.effort.as_deref(),
        Some("high"),
        "an omitted provider changes nothing about the entity"
    );
}

/// A provider the daemon cannot run is refused before anything is started —
/// the same rejection every other provider param gives.
#[tokio::test]
async fn agent_start_refuses_an_unknown_provider() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-bogus");

    let refused = call(
        &handler,
        "agent.start",
        json!({ "id": "run-bogus", "provider": "gemini" }),
    );
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("unknown agent provider"),
        "{refused:?}"
    );
    assert!(
        !state
            .lock()
            .unwrap()
            .session_registry
            .contains(&derived_agent_key(&root, "run-bogus")),
        "a refused start opens no agent"
    );
}

/// The composer's model menu edits exactly the addressed agent's next
/// start. A live session is untouched.
#[tokio::test]
async fn agent_choose_persists_the_model_without_touching_a_live_session() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let (key, _wire_id) = insert_live_run(&state, &repo, dir.path().join("side"), "run-choose");
    let agent_id = state.lock().unwrap().runs["run-choose"]
        .agents
        .primary()
        .unwrap()
        .id
        .clone();

    let chosen = call(
        &handler,
        "agent.choose",
        json!({
            "entity_id": "run-choose",
            "agent_id": agent_id,
            "model": "claude-opus-5",
            "effort": "high",
            "expected_choice_revision": 0,
        }),
    );
    assert_eq!(chosen["ok"], true, "{chosen:?}");
    assert_eq!(chosen["result"]["model"], "claude-opus-5");
    assert_eq!(chosen["result"]["effort"], "high");
    assert_eq!(chosen["result"]["agent_id"], agent_id);
    assert_eq!(chosen["result"]["choice_revision"], 1);
    assert_eq!(
        chosen["result"]["provider"], "claude",
        "the harness the agent is locked to is what it stays on"
    );
    {
        let s = state.lock().unwrap();
        let choice = s.runs["run-choose"]
            .agents
            .by_id(&agent_id)
            .unwrap()
            .choice
            .clone();
        assert_eq!(choice.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(choice.effort.as_deref(), Some("high"));
        assert_eq!(choice.provider, AgentProvider::Claude);
        assert!(
            s.session_registry.test_tab(&key).unwrap().live,
            "the session that is running keeps running: it spends the new \
             model at its next start"
        );
    }

    let refused = call(
        &handler,
        "agent.choose",
        json!({ "entity_id": "run-choose", "provider": "codex" }),
    );
    assert_eq!(refused["ok"], false, "{refused:?}");
    let error = refused["error"].as_str().unwrap();
    assert!(
        error.contains("locked to Claude Code TUI"),
        "the refusal names the harness the agent is locked to: {error}"
    );
    assert_eq!(
        state.lock().unwrap().runs["run-choose"]
            .agents
            .by_id(&agent_id)
            .unwrap()
            .choice
            .provider,
        AgentProvider::Claude,
        "and a refusal moves nothing"
    );
}

#[test]
fn agent_choose_isolated_same_provider_siblings_and_rejects_a_stale_revision() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "choice-isolation");
    let first = primary_agent_id(&state, &run_id);
    let second = state
        .runs
        .get_mut(&run_id)
        .unwrap()
        .agents
        .add(&run_id, ModelChoice::default(), "2026-09-08T12:00:00Z")
        .id
        .clone();

    let chosen = state.handle(req(
        "agent.choose",
        json!({
            "entity_id": run_id,
            "agent_id": second,
            "model": "claude-opus-5",
            "expected_choice_revision": 0,
        }),
    ));
    assert_eq!(chosen["ok"], true, "{chosen:?}");
    assert_eq!(chosen["result"]["choice_revision"], 1);
    assert_eq!(
        state.runs[&run_id]
            .agents
            .by_id(&first)
            .unwrap()
            .choice
            .model,
        None,
        "the same-provider sibling was not changed"
    );

    let stale = state.handle(req(
        "agent.choose",
        json!({
            "entity_id": run_id,
            "agent_id": second,
            "model": "claude-sonnet-5",
            "expected_choice_revision": 0,
        }),
    ));
    assert_eq!(stale["ok"], false, "{stale:?}");
    assert!(stale["error"].as_str().unwrap().contains("stale"));
    assert_eq!(
        state.runs[&run_id]
            .agents
            .by_id(&second)
            .unwrap()
            .choice
            .model
            .as_deref(),
        Some("claude-opus-5")
    );
}

#[test]
fn agent_choose_persistence_failure_restores_memory_and_disk() {
    let (dir, repo) = init_repo();
    let run_id;
    let agent_id;
    let before;
    {
        let mut state = qa_state(&repo, dir.path());
        run_id = adopted_run(&mut state, &repo, dir.path(), "choice-write-failure");
        agent_id = primary_agent_id(&state, &run_id);
        before = state.runs[&run_id].agents.by_id(&agent_id).unwrap().clone();
        state.store.as_ref().unwrap().fail_next_write();

        let refused = state.handle(req(
            "agent.choose",
            json!({
                "entity_id": run_id,
                "agent_id": agent_id,
                "model": "claude-opus-5",
                "expected_choice_revision": 0,
            }),
        ));

        assert_eq!(refused["ok"], false, "{refused:?}");
        assert_eq!(
            state.runs[&run_id].agents.by_id(&agent_id).unwrap(),
            &before,
            "a rejected choice is not published in memory"
        );
    }

    let reloaded = qa_state(&repo, dir.path());
    assert_eq!(
        reloaded.runs[&run_id].agents.by_id(&agent_id).unwrap(),
        &before,
        "a rejected choice was not persisted either"
    );
}

/// The same verb on an idle entity, which is the ordinary case: nothing is
/// running, so there is nothing to be careful about.
#[tokio::test]
async fn agent_choose_persists_on_an_idle_entity_and_refuses_what_cannot_run() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-idle-choose");

    let chosen = call(
        &handler,
        "agent.choose",
        json!({ "entity_id": "run-idle-choose", "model": "claude-opus-5" }),
    );
    assert_eq!(chosen["ok"], true, "{chosen:?}");
    {
        let held = state.lock().unwrap();
        assert_eq!(
            held.runs["run-idle-choose"]
                .agents
                .primary()
                .unwrap()
                .choice
                .model
                .as_deref(),
            Some("claude-opus-5")
        );
        assert_eq!(
            held.runs["run-idle-choose"].model_choice.model, None,
            "the entity choice remains only a creation template"
        );
    }

    let refused = call(
        &handler,
        "agent.choose",
        json!({ "entity_id": "run-idle-choose", "model": "claude-haiku-4-5", "effort": "high" }),
    );
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("does not support effort"),
        "{refused:?}"
    );
    assert_eq!(
        state.lock().unwrap().runs["run-idle-choose"]
            .agents
            .primary()
            .unwrap()
            .choice
            .model
            .as_deref(),
        Some("claude-opus-5"),
        "a refusal moves nothing"
    );

    let unknown = call(
        &handler,
        "agent.choose",
        json!({ "entity_id": "run-nowhere", "model": "claude-opus-5" }),
    );
    assert_eq!(unknown["ok"], false, "{unknown:?}");

    let before_unknown_agent = state.lock().unwrap().runs["run-idle-choose"].agents.clone();
    let unknown_agent = call(
        &handler,
        "agent.choose",
        json!({
            "entity_id": "run-idle-choose",
            "agent_id": "agent-does-not-exist",
            "model": "claude-sonnet-5",
        }),
    );
    assert_eq!(unknown_agent["ok"], false, "{unknown_agent:?}");
    let held = state.lock().unwrap();
    assert!(held.runs.contains_key("run-idle-choose"));
    assert_eq!(held.runs["run-idle-choose"].agents, before_unknown_agent);
}

#[tokio::test]
async fn agent_choose_writes_on_the_named_agent_when_it_runs_another_harness() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let run_id = {
        let mut held = state.lock().unwrap();
        adopted_run(&mut held, &repo, dir.path(), "feature-two-harnesses")
    };
    let claude_agent = {
        let held = state.lock().unwrap();
        primary_agent_id(&held, &run_id)
    };

    let added = call(
        &handler,
        "agent.add",
        json!({ "entity_id": run_id, "provider": "codex" }),
    );
    assert_eq!(added["ok"], true, "{added:?}");
    let codex_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();

    let chosen = call(
        &handler,
        "agent.choose",
        json!({ "entity_id": run_id, "agent_id": codex_agent, "model": "gpt-5.6-sol" }),
    );
    assert_eq!(chosen["ok"], true, "{chosen:?}");

    let digests = state
        .lock()
        .unwrap()
        .agent_digests(&run_id, DigestScope::List);
    let codex_digest = digests
        .iter()
        .find(|digest| digest["id"] == json!(codex_agent.clone()))
        .expect("the codex agent is on the roster");
    assert_eq!(codex_digest["model"], "gpt-5.6-sol");
    let claude_digest = digests
        .iter()
        .find(|digest| digest["id"] == json!(claude_agent.clone()))
        .expect("the claude agent is on the roster");
    assert_eq!(
        claude_digest["model"], "",
        "the other harness's agent keeps what it had"
    );
    assert_eq!(
        state.lock().unwrap().runs[&run_id]
            .model_choice
            .model
            .as_deref(),
        None,
        "the entity's own choice is not spent on another harness's agent"
    );
}

/// Switching provider under a running harness would leave that process
/// running the old provider while the record claimed the new one — a
/// stranded agent nobody owns. The switch is refused and the running
/// session is left exactly as it was.
#[tokio::test]
async fn agent_start_refuses_a_provider_switch_while_the_agent_is_live() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let (key, _wire_id) = insert_live_run(&state, &repo, dir.path().join("side"), "run-mid");

    let refused = call(
        &handler,
        "agent.start",
        json!({ "id": "run-mid", "provider": "codex" }),
    );
    assert_eq!(refused["ok"], false, "{refused:?}");
    let error = refused["error"].as_str().unwrap();
    assert!(error.contains("locked to Claude Code TUI"), "{error}");
    assert!(
        !error.to_lowercase().contains("headless"),
        "the refusal prints provider labels, and no label names a provider \
         the way the code does: {error}"
    );
    let s = state.lock().unwrap();
    assert_eq!(
        s.runs["run-mid"].model_choice.provider,
        AgentProvider::Claude,
        "a refused switch leaves the record alone"
    );
    assert!(
        s.session_registry.test_tab(&key).unwrap().live,
        "and leaves the running harness where it was"
    );
}

/// Naming the provider the entity already runs on is not a switch: nothing
/// is stranded, so the idempotent "give me the tab" start still works while
/// a session is live.
#[tokio::test]
async fn agent_start_naming_the_live_agents_own_provider_is_idempotent() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let (key, wire_id) = insert_live_run(&state, &repo, dir.path().join("side"), "run-same");
    let live_pid = agent_pid(
        state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(&key)
            .unwrap(),
    );
    // The live run is on the TUI carrier, which is what "claude" names.
    let again = call(
        &handler,
        "agent.start",
        json!({ "id": "run-same", "provider": "claude" }),
    );
    assert_eq!(again["ok"], true, "{again:?}");
    assert_eq!(again["result"]["term_id"], wire_id, "{again:?}");
    wait_for_deliveries(&state).await;
    assert_eq!(
        agent_pid(
            state
                .lock()
                .unwrap()
                .session_registry
                .test_tab(&key)
                .unwrap()
        ),
        live_pid,
        "the live session is the one the start hands back, not a replacement"
    );
}

/// Attaching to an entity's agent finds the tab of the WORKTREE it works
/// in, streams it, and — when that agent's process ends — retains the last
/// screen with `live: false` rather than erroring or going blank.
/// The QA suite's last check, in-process: an Issue is planned, its first
/// stage implemented by the QA agent, and its run merged. Attaching to the
/// merged run must answer `live: false` — the agent's session is over with
/// its work, whatever the harness process is still doing.
#[tokio::test]
async fn agent_attach_on_a_merged_run_answers_live_false() {
    let (dir, repo) = init_repo();
    let canonical_dir = dir.path().canonicalize().unwrap();
    let repo = repo.canonicalize().unwrap();
    let (state, handler) = shared_qa_state_and_handler(&repo, &canonical_dir);
    let created = call(
        &handler,
        "issue.create",
        json!({ "goal": "Add a greeting banner", "provider": "claude" }),
    );
    assert_eq!(created["ok"], true, "{created:?}");
    let issue_id = created["result"]["issue_id"].as_str().unwrap().to_string();
    wait_for_deliveries(&state).await;
    let approved = call(&handler, "issue.approve", json!({ "issue_id": issue_id }));
    assert_eq!(approved["ok"], true, "{approved:?}");
    let stages = call(&handler, "issue.stages", json!({ "issue_id": issue_id }));
    let first = stages["result"]["stages"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let gate = call(
        &handler,
        "issue.stage_approve",
        json!({ "issue_id": issue_id, "stage_id": first }),
    );
    assert_eq!(gate["ok"], true, "{gate:?}");
    let implemented = call(
        &handler,
        "issue.implement_stage",
        json!({ "issue_id": issue_id, "stage_id": first }),
    );
    assert_eq!(implemented["ok"], true, "{implemented:?}");
    let run_id = implemented["result"]["current_implementation_id"]
        .as_str()
        .unwrap()
        .to_string();
    wait_for_deliveries(&state).await;
    let second = stages["result"]["stages"][1]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let gate = call(
        &handler,
        "issue.stage_approve",
        json!({ "issue_id": issue_id, "stage_id": second }),
    );
    assert_eq!(gate["ok"], true, "{gate:?}");
    let implemented = call(
        &handler,
        "issue.implement_stage",
        json!({ "issue_id": issue_id, "stage_id": second }),
    );
    assert_eq!(implemented["ok"], true, "{implemented:?}");
    let merged = call(
        &handler,
        "issue.git_action",
        json!({ "issue_id": issue_id, "action": "merge" }),
    );
    assert_eq!(merged["ok"], true, "{merged:?}");
    assert_eq!(
        merged["result"]["current_implementation"]["state"], "merged",
        "{merged:?}"
    );
    let attached = handler.call(
        SessionSender::detached("s-merged"),
        req("agent.attach", json!({ "id": run_id })),
    );
    assert_eq!(attached["ok"], true, "{attached:?}");
    assert_eq!(attached["result"]["live"], false, "{attached:?}");
}
