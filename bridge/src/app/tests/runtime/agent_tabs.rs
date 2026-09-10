use super::*;

// ---- the agent tab primitive: a path-keyed, tab-backed PTY --------------

/// A shared QA state, a worktree-like root to run an agent in, and an owner
/// id bound to the state's project — the three things an agent tab needs
/// (the owner resolves the project whose orchestrator builds the harness).
pub(in crate::app::tests) fn agent_tab_fixture(
    repo: &std::path::Path,
    dir: &std::path::Path,
    owner: &str,
) -> (Arc<Mutex<AppState>>, FrameHandler, PathBuf) {
    let root = dir.join("agent-root");
    std::fs::create_dir_all(&root).unwrap();
    agent_tab_fixture_at(repo, dir, owner, root)
}

/// The same fixture with the agent rooted at a checkout the test already
/// owns (notably the primary checkout mounted by project-scoped attach).
pub(in crate::app::tests) fn agent_tab_fixture_at(
    repo: &std::path::Path,
    dir: &std::path::Path,
    owner: &str,
    root: PathBuf,
) -> (Arc<Mutex<AppState>>, FrameHandler, PathBuf) {
    let (state, handler) = shared_state_and_handler(repo, dir);
    {
        let mut s = state.lock().unwrap();
        let project_id = s.projects[0].id.clone();
        s.entity_project.insert(owner.to_string(), project_id);
        let mut record = fake_run_record(owner);
        record.worktree_path = root.display().to_string();
        record.project_path = repo.display().to_string();
        s.runs.insert(
            owner.to_string(),
            ActiveRun::reattach(&record, ".build/plan.md".to_string()),
        );
    }
    (state, handler, root)
}

/// The agent tab's screen, rendered — what an attaching client would see.
pub(in crate::app::tests) fn agent_screen_text(
    state: &Arc<Mutex<AppState>>,
    root: &std::path::Path,
) -> String {
    let root = AppState::canonical_root(root);
    let s = state.lock().unwrap();
    let Some(tab) = s
        .tabs
        .values()
        .find(|tab| tab.root == root && matches!(tab.role, TabRole::Agent { .. }))
    else {
        return String::new();
    };
    String::from_utf8_lossy(&b64decode(&screen_of(tab).snapshot().snapshot).unwrap()).into_owned()
}

/// Poll the agent tab's screen until it shows `needle` (the pump feeds it),
/// returning what was on screen at the end either way.
pub(in crate::app::tests) async fn wait_for_agent_screen(
    state: &Arc<Mutex<AppState>>,
    root: &std::path::Path,
    needle: &str,
) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let text = agent_screen_text(state, root);
        if text.contains(needle) || std::time::Instant::now() >= deadline {
            return text;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// One worktree, one agent: find-or-create keyed by the canonical root, so
/// a second call hands back the SAME tab (warm) rather than a second
/// harness in the same directory.
#[tokio::test]
async fn ensure_agent_tab_is_idempotent_for_one_root() {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-one-agent");
    let choice = ModelChoice::default();

    let (first_id, first) = ensure_agent_tab(
        &state,
        &root,
        "run-one-agent",
        &crate::agent::derived_agent_id("run-one-agent"),
        &choice,
        "start",
    )
    .expect("the agent spawns");
    let (second_id, second) = ensure_agent_tab(
        &state,
        &root,
        "run-one-agent",
        &crate::agent::derived_agent_id("run-one-agent"),
        &choice,
        "start",
    )
    .expect("the agent is found");

    assert_eq!(first, Spawned::Fresh, "the first call creates the tab");
    assert_eq!(second, Spawned::Warm, "the second call finds it");
    assert_eq!(first_id, second_id, "both calls address one tab");
    assert_eq!(
        first_id,
        agent_tab_id(&crate::agent::derived_agent_id("run-one-agent")),
        "an agent tab is addressed by the agent, not by the entity that owns it"
    );

    let s = state.lock().unwrap();
    assert_eq!(s.tabs.len(), 1, "exactly one tab in the registry");
    assert!(
        s.agent_spawns_in_flight.is_empty(),
        "the spawn reservation is released"
    );
    // Under --strict-mcp-config a missing config kills the harness before it
    // reads a byte of the prompt, so the scaffold is part of the spawn.
    assert!(root
        .join(crate::orchestrator::mcp_config_path(
            &crate::agent::derived_agent_id("run-one-agent")
        ))
        .exists());
}

/// Two deliveries racing on one worktree must produce ONE harness: the find
/// and the in-flight reservation are taken under the same lock, so the
/// loser waits for the winner's tab instead of spawning a second agent
/// (two agents in one worktree both report `done` for the same owner, and
/// the second report lands as a bogus failure on the thread).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_ensure_agent_tab_spawns_one_agent() {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-race");

    let mut racers = Vec::new();
    for _ in 0..4 {
        let state = Arc::clone(&state);
        let root = root.clone();
        racers.push(tokio::task::spawn_blocking(move || {
            ensure_agent_tab(
                &state,
                &root,
                "run-race",
                &crate::agent::derived_agent_id("run-race"),
                &ModelChoice::default(),
                "start",
            )
        }));
    }
    let mut outcomes = Vec::new();
    for racer in racers {
        outcomes.push(racer.await.unwrap().expect("every racer gets the tab"));
    }

    let fresh = outcomes
        .iter()
        .filter(|(_, spawned)| *spawned == Spawned::Fresh)
        .count();
    assert_eq!(fresh, 1, "exactly one caller spawned: {outcomes:?}");
    assert!(
        outcomes.iter().all(|(id, _)| id == &outcomes[0].0),
        "every caller addresses the same tab: {outcomes:?}"
    );
    let s = state.lock().unwrap();
    assert_eq!(
        s.tabs.len(),
        1,
        "one worktree, one agent {:?}",
        s.tabs.keys().collect::<Vec<_>>()
    );
    assert!(s.agent_spawns_in_flight.is_empty());
}

#[test]
fn harness_spec_construction_does_not_hold_the_app_state_lock() {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-slow-spec");
    let (arrived_tx, arrived_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    {
        let release_rx = Arc::clone(&release_rx);
        let agent = Agent::WarmBuilder(Arc::new(move |_, _, _| {
            arrived_tx.send(()).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
            Ok(HarnessSpec::new("sh")
                .arg("-c")
                .arg("printf '\\033[?2004h'; cat >/dev/null"))
        }));
        let mut app = state.lock().unwrap();
        let worktrees = app.worktrees_root.clone();
        app.projects[0].orch = Orchestrator::new(
            repo.clone(),
            worktrees,
            agent,
            Templates::default(),
            test_bridge_exe(),
        );
    }
    let spawning_state = Arc::clone(&state);
    let spawn = std::thread::spawn(move || {
        ensure_agent_tab(
            &spawning_state,
            &root,
            "run-slow-spec",
            &crate::agent::derived_agent_id("run-slow-spec"),
            &ModelChoice::default(),
            "start",
        )
    });
    arrived_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the harness builder started");

    assert!(
        state.try_lock().is_ok(),
        "unrelated app state remains available while a harness spec is built"
    );

    release_tx.send(()).unwrap();
    spawn.join().unwrap().unwrap();
}

/// The cold/warm rule: a tab that had to be spawned gets the full prompt (a
/// cold agent has no context to read messages into), and a tab that was
/// already alive gets the short nudge — the messages are already durable in
/// the thread. The PTY echoes what is written to it, so the tab's screen is
/// the proof of which one travelled.
#[tokio::test]
async fn deliver_sends_the_cold_prompt_on_a_fresh_tab_and_the_nudge_on_a_warm_one() {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-deliver");
    let choice = ModelChoice::default();

    let (cold_id, cold_spawned) = deliver(
        &state,
        &root,
        "run-deliver",
        &crate::agent::derived_agent_id("run-deliver"),
        &choice,
        "build",
        ["COLD-CONTEXT-PROMPT", "WARM-NUDGE-PROMPT"],
    )
    .expect("a cold delivery spawns and submits");
    assert_eq!(cold_spawned, Spawned::Fresh);
    let cold_screen = wait_for_agent_screen(&state, &root, "COLD-CONTEXT-PROMPT").await;
    assert!(
        cold_screen.contains("COLD-CONTEXT-PROMPT"),
        "a fresh tab hears the cold prompt: {cold_screen:?}"
    );
    assert!(
        !cold_screen.contains("WARM-NUDGE-PROMPT"),
        "a fresh tab must NOT hear the nudge: {cold_screen:?}"
    );

    let (warm_id, warm_spawned) = deliver(
        &state,
        &root,
        "run-deliver",
        &crate::agent::derived_agent_id("run-deliver"),
        &choice,
        "build",
        ["COLD-CONTEXT-PROMPT", "WARM-NUDGE-PROMPT"],
    )
    .expect("a warm delivery reuses the tab");
    assert_eq!(warm_spawned, Spawned::Warm);
    assert_eq!(warm_id, cold_id, "both deliveries address one tab");
    let warm_screen = wait_for_agent_screen(&state, &root, "WARM-NUDGE-PROMPT").await;
    assert!(
        warm_screen.contains("WARM-NUDGE-PROMPT"),
        "a warm tab hears the nudge: {warm_screen:?}"
    );
    assert_eq!(state.lock().unwrap().tabs.len(), 1);
}

/// A session that answers what the app-wide state lock was doing at the
/// moment its turn arrived.
///
/// It cannot own the state — the state owns the tab that owns the session —
/// so it holds a `Weak` and upgrades it for the one question it asks.
struct LockProbingSession {
    state: std::sync::Weak<Mutex<AppState>>,
    state_was_free: Arc<std::sync::atomic::AtomicBool>,
}

impl AgentSession for LockProbingSession {
    fn send_turn(&self, _turn: &Turn) -> Result<(), HarnessError> {
        let state = self.state.upgrade().expect("the daemon outlives the turn");
        // `try_lock` on a std mutex fails for the thread that already holds
        // it, so this reads the DELIVERY's own lock, not a race with some
        // other caller's.
        self.state_was_free.store(
            state.try_lock().is_ok(),
            std::sync::atomic::Ordering::Relaxed,
        );
        Ok(())
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Waiting
    }
    fn quiet_for(&self) -> Duration {
        Duration::ZERO
    }
    fn exited_within(&self, _timeout: Duration) -> bool {
        false
    }
    fn end(&self) {}
    fn backdate_last_output(&self, _ago: Duration) {}
}

/// The turn travels with the app-wide state lock RELEASED.
///
/// Every RPC, every terminal pump and the idle sweep wait on that lock, so
/// a session that takes its time accepting a turn — a protocol write to a
/// full pipe, an ack the harness answers late — would stall the whole
/// daemon if the turn were handed over under it. `AgentSession::send_turn`
/// promises callers they may take that time; this is where the promise is
/// kept, and it is kept for the exit-race wait on the failure path too.
#[test]
fn a_turn_travels_with_the_state_lock_released() {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-lock");
    let agent_id = crate::agent::derived_agent_id("run-lock");
    let canonical = AppState::canonical_root(&root);
    let state_was_free = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let mut tab = terminal_free_agent_tab(&canonical, "run-lock", &agent_id);
        tab.session = Arc::new(LockProbingSession {
            state: Arc::downgrade(&state),
            state_was_free: Arc::clone(&state_was_free),
        });
        let mut s = state.lock().unwrap();
        tab.session_instance = s.record_agent_session_start(
            "run-lock",
            &agent_id,
            &canonical,
            &ModelChoice::default(),
            "build",
        );
        s.tabs.insert(TabKey::agent(&canonical, &agent_id), tab);
    }

    let (_, spawned) = deliver(
        &state,
        &root,
        "run-lock",
        &agent_id,
        &ModelChoice::default(),
        "build",
        ["COLD-CONTEXT-PROMPT", "WARM-NUDGE-PROMPT"],
    )
    .expect("the live tab takes the turn");

    assert_eq!(spawned, Spawned::Warm, "the tab was already alive");
    assert!(
        state_was_free.load(std::sync::atomic::Ordering::Relaxed),
        "a delivery must not hold the app-wide state lock across send_turn"
    );
    let s = state.lock().unwrap();
    assert!(
        s.tabs[&TabKey::agent(&canonical, &agent_id)]
            .last_delivered_at
            .is_some(),
        "the quiescence clock still restarts on the delivered turn"
    );
}

/// The conversation's open session for `owner`, if it has one.
pub(in crate::app::tests) fn open_session_count(
    state: &Arc<Mutex<AppState>>,
    owner: &str,
) -> usize {
    primary_thread(&state.lock().unwrap().runs[owner].agents)
        .sessions
        .iter()
        .filter(|session| session.ended_at.is_none())
        .count()
}

/// A session belongs to the agent PROCESS, not to a phase. `done` is the
/// agent finishing a turn at its prompt — it is still there, still in the
/// same session — so the thread must not record a session end, and the warm
/// turn that follows must not read back as a turn taken outside any
/// session.
#[tokio::test]
async fn done_then_a_warm_turn_stays_in_one_open_session() {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-lineage");
    let queue_turn = || {
        state
            .lock()
            .unwrap()
            .pending_agent_turns
            .push(PendingAgentTurn {
                operation_id: None,
                root: AppState::canonical_root(&root),
                owner: "run-lineage".into(),
                agent_id: crate::agent::derived_agent_id("run-lineage"),
                conversation_id: crate::agent::derived_agent_id("run-lineage"),
                model_choice: ModelChoice::default(),
                choice_revision: 0,
                interrupt: false,
                say: Some(TurnText {
                    cold: "COLD-TURN".into(),
                    warm: "WARM-TURN".into(),
                }),
                phase: "build",
                wants_catch_up: false,
                survives_refusal: false,
            });
    };

    queue_turn();
    deliver_pending_agent_turns(&state);
    assert_eq!(
        open_session_count(&state, "run-lineage"),
        1,
        "a cold delivery opens the session"
    );

    state.lock().unwrap().on_agent_done(
        "run-lineage",
        DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Completed,
            summary: "built".into(),
            outputs: DoneOutputs::default(),
        },
    );
    queue_turn();
    deliver_pending_agent_turns(&state);

    let s = state.lock().unwrap();
    let thread = primary_thread(&s.runs["run-lineage"].agents);
    assert_eq!(
        thread.sessions.len(),
        1,
        "a warm turn continues the one session: {:?}",
        thread.sessions
    );
    assert!(
        thread.sessions[0].ended_at.is_none(),
        "the agent is still at its prompt: {:?}",
        thread.sessions
    );
    assert!(
        !thread.items.iter().any(|item| matches!(
            item,
            crate::thread::ThreadItem::Event(event)
                if event.event == crate::thread::ThreadEventKind::SessionEnded
        )),
        "no session ended, so the thread must not say one did: {:?}",
        thread.items
    );
}

/// A session ends where it really ends: when the agent's process does. The
/// pump's EOF is the only place that knows, so that is where the thread
/// learns it — otherwise a run whose agent died reads back as forever in
/// session.
#[tokio::test]
async fn the_session_closes_when_the_agent_process_exits() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let (tab_key, _wire_id) = insert_live_run(&state, &repo, dir.path().join("side"), "run-eof");
    assert_eq!(open_session_count(&state, "run-eof"), 1);

    state.lock().unwrap().tabs[&tab_key].session.end();

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while open_session_count(&state, "run-eof") > 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the agent's process ended and the session never closed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let s = state.lock().unwrap();
    assert_eq!(
        primary_thread(&s.runs["run-eof"].agents).sessions.len(),
        1,
        "the dead session is closed, not replaced"
    );
}

#[test]
fn stale_session_end_cannot_clear_its_replacement_execution() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let root = insert_run(
        &mut app,
        &repo,
        dir.path(),
        "run-replaced-session",
        RunState::Building,
    );
    let agent_id = crate::agent::derived_agent_id("run-replaced-session");
    let first = app
        .record_agent_session_start(
            "run-replaced-session",
            &agent_id,
            &root,
            &ModelChoice::default(),
            "build",
        )
        .unwrap();
    let replacement = app
        .record_agent_session_start(
            "run-replaced-session",
            &agent_id,
            &root,
            &ModelChoice::default(),
            "build",
        )
        .unwrap();
    app.record_agent_working_since(
        "run-replaced-session",
        &agent_id,
        Some("2026-09-08T10:01:00Z".to_string()),
    );

    app.record_agent_session_end("run-replaced-session", &agent_id, &first);

    let agent = app.runs["run-replaced-session"]
        .agents
        .by_id(&agent_id)
        .unwrap();
    assert_eq!(
        agent.thread.open_session_instance(&agent_id),
        Some(replacement)
    );
    assert_eq!(
        agent.working_since.as_deref(),
        Some("2026-09-08T10:01:00Z"),
        "S1's delayed end cannot stop S2's execution clock"
    );
}

#[test]
fn default_after_a_native_override_restarts_fresh_before_delivery() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut app, &repo, dir.path(), "choice-reset");
    let root = app.entity_agent_root(&run_id).unwrap();
    let agent_id = primary_agent_id(&app, &run_id);
    let choice_a = ModelChoice {
        model: Some("model-a".to_string()),
        ..ModelChoice::default()
    };
    let choice_b = ModelChoice {
        model: Some("model-b".to_string()),
        ..ModelChoice::default()
    };
    app.set_agent_model_choice(&run_id, &agent_id, choice_a.clone())
        .unwrap();
    let log = SessionLog::default();
    let key = insert_agent_tab(
        &mut app,
        &root,
        &run_id,
        &agent_id,
        DictatedSession::reporting(AgentStatus::Waiting)
            .recording_into(&log)
            .natively_accepting(choice_b.clone()),
    );
    let instance = app.tabs[&key].session_instance.clone().unwrap();
    app.note_self_report(
        &run_id,
        &agent_id,
        &instance,
        SelfReport {
            named: Some("sticky-session".to_string()),
            model: Some("model-a".to_string()),
        },
    );
    app.resume_id_probe = Arc::new(|_, _, _| true);
    let launches: Arc<Mutex<Vec<(ModelChoice, SpawnOptions)>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&launches);
    let worktrees = app.worktrees_root.clone();
    app.projects[0].orch = Orchestrator::new(
        repo.clone(),
        worktrees,
        Agent::WarmBuilder(Arc::new(move |_prompt, choice, options| {
            recorded
                .lock()
                .unwrap()
                .push((choice.clone(), options.clone()));
            Ok(warm_tui_spec())
        })),
        Templates::default(),
        test_bridge_exe(),
    );
    app.set_agent_model_choice(&run_id, &agent_id, choice_b.clone())
        .unwrap();
    let state = app.shared();

    let (_, b_spawned) = deliver(
        &state,
        &root,
        &run_id,
        &agent_id,
        &choice_b,
        "build",
        ["cold-b", "warm-b"],
    )
    .unwrap();
    assert_eq!(b_spawned, Spawned::Warm);
    assert_eq!(log.choices(), vec![Some(choice_b)]);

    {
        let mut app = state.lock().unwrap();
        app.record_agent_working_since(&run_id, &agent_id, None);
        app.set_agent_model_choice(&run_id, &agent_id, ModelChoice::default())
            .unwrap();
    }
    let (_, default_spawned) = deliver(
        &state,
        &root,
        &run_id,
        &agent_id,
        &ModelChoice::default(),
        "build",
        ["cold-default", "warm-default"],
    )
    .unwrap();
    assert_eq!(default_spawned, Spawned::Fresh);
    let launches = launches.lock().unwrap();
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0].0, ModelChoice::default());
    assert_eq!(launches[0].1.resume_session_id, None);
    let app = state.lock().unwrap();
    let current = app
        .agent_conversation(&run_id, Some(&agent_id))
        .unwrap()
        .open_session_instance(&agent_id)
        .unwrap();
    let row = app
        .agent_conversation(&run_id, Some(&agent_id))
        .unwrap()
        .sessions
        .iter()
        .find(|session| session.id == current.id)
        .unwrap();
    assert_eq!(row.model, None, "the fresh session uses configured default");
    assert_eq!(row.effort, None, "sticky effort is cleared at the boundary");
}

#[test]
fn deliveries_preserve_each_agents_existing_execution_interval() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut app, &repo, dir.path(), "timer-isolation");
    let root = app.entity_agent_root(&run_id).unwrap();
    let primary = primary_agent_id(&app, &run_id);
    let second = app
        .runs
        .get_mut(&run_id)
        .unwrap()
        .agents
        .add(&run_id, ModelChoice::default(), "2026-09-08T09:00:00Z")
        .id
        .clone();
    let primary_choice = app.runs[&run_id]
        .agents
        .by_id(&primary)
        .unwrap()
        .choice
        .clone();
    let second_choice = app.runs[&run_id]
        .agents
        .by_id(&second)
        .unwrap()
        .choice
        .clone();
    insert_agent_tab(
        &mut app,
        &root,
        &run_id,
        &primary,
        DictatedSession::reporting(AgentStatus::Waiting),
    );
    insert_agent_tab(
        &mut app,
        &root,
        &run_id,
        &second,
        DictatedSession::reporting(AgentStatus::Waiting),
    );
    let first_started = "2026-09-08T10:00:00Z".to_string();
    app.record_agent_working_since(&run_id, &primary, Some(first_started.clone()));
    let state = app.shared();

    deliver(
        &state,
        &root,
        &run_id,
        &second,
        &second_choice,
        "build",
        ["second-cold", "second-warm"],
    )
    .unwrap();
    deliver(
        &state,
        &root,
        &run_id,
        &primary,
        &primary_choice,
        "build",
        ["first-cold", "first-warm"],
    )
    .unwrap();

    let app = state.lock().unwrap();
    let roster = &app.runs[&run_id].agents;
    assert_eq!(
        roster.by_id(&primary).unwrap().working_since.as_ref(),
        Some(&first_started),
        "a second send must not reset an execution already in flight"
    );
    assert!(
        roster.by_id(&second).unwrap().working_since.is_some(),
        "the second agent starts its own interval"
    );
}

#[test]
fn starting_one_issue_agent_preserves_another_valid_issue_session_in_the_same_checkout() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_qa_state_and_handler(&repo, dir.path());
    insert_plan_without_agent(&state, &repo, dir.path().join("issue-a"), "issue-a");
    insert_plan_without_agent(&state, &repo, dir.path().join("issue-b"), "issue-b");
    let (root, first_agent, second_agent) = {
        let app = state.lock().unwrap();
        let first_root = app.entity_agent_root("issue-a").unwrap();
        let second_root = app.entity_agent_root("issue-b").unwrap();
        assert_eq!(
            first_root, second_root,
            "both Issue agents use the primary checkout"
        );
        (
            first_root,
            primary_agent_id(&app, "issue-a"),
            primary_agent_id(&app, "issue-b"),
        )
    };
    let first_log = SessionLog::default();
    {
        let mut app = state.lock().unwrap();
        insert_agent_tab(
            &mut app,
            &root,
            "issue-a",
            &first_agent,
            DictatedSession::reporting(AgentStatus::Waiting).recording_into(&first_log),
        );
    }

    ensure_agent_tab(
        &state,
        &root,
        "issue-b",
        &second_agent,
        &ModelChoice::default(),
        "plan",
    )
    .expect("the second Issue agent starts");

    let app = state.lock().unwrap();
    assert!(
        app.tabs.contains_key(&TabKey::agent(&root, &first_agent)),
        "starting Issue B retired Issue A's valid session"
    );
    assert!(app.tabs.contains_key(&TabKey::agent(&root, &second_agent)));
    assert!(!first_log.ended());
}
