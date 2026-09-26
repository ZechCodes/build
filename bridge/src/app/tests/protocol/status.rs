use super::*;

#[test]
fn agent_interrupt_stops_only_the_exact_running_turn() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let root = {
        let mut app = state.lock().unwrap();
        insert_run(
            &mut app,
            &repo,
            dir.path(),
            "run-interrupt",
            RunState::Building,
        )
    };
    let log = SessionLog::default();
    let key = {
        let mut app = state.lock().unwrap();
        insert_dictated_agent_tab(
            &mut app,
            &root,
            "run-interrupt",
            DictatedSession::reporting(AgentStatus::Working)
                .recording_into(&log)
                .interruptible(),
        )
    };
    let agent_id = crate::agent::derived_agent_id("run-interrupt");
    let before_items = state.lock().unwrap().runs["run-interrupt"]
        .agents
        .primary()
        .unwrap()
        .thread
        .items
        .len();

    let response = call(
        &handler,
        "agent.interrupt",
        json!({
            "entity_id": "run-interrupt",
            "agent_id": agent_id,
            "conversation_id": agent_id,
        }),
    );

    assert_eq!(response["ok"], true, "{response:?}");
    assert_eq!(response["result"]["interrupted"], true);
    assert!(
        log.interrupted(),
        "the live session received the stop request"
    );
    let app = state.lock().unwrap();
    assert!(
        app.session_registry.contains(&key),
        "the session remains attached"
    );
    assert_eq!(
        app.runs["run-interrupt"]
            .agents
            .primary()
            .unwrap()
            .thread
            .items
            .len(),
        before_items,
        "stopping a turn posts no conversation message"
    );
}

#[test]
fn agent_interrupt_refuses_stale_idle_and_unsupported_sessions() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let root = {
        let mut app = state.lock().unwrap();
        insert_run(
            &mut app,
            &repo,
            dir.path(),
            "run-interrupt-refused",
            RunState::Building,
        )
    };
    let agent_id = crate::agent::derived_agent_id("run-interrupt-refused");
    {
        let mut app = state.lock().unwrap();
        insert_dictated_agent_tab(
            &mut app,
            &root,
            "run-interrupt-refused",
            DictatedSession::reporting(AgentStatus::Working),
        );
    }
    let request = |conversation_id: &str| {
        json!({
            "entity_id": "run-interrupt-refused",
            "agent_id": agent_id,
            "conversation_id": conversation_id,
        })
    };

    let stale = call(&handler, "agent.interrupt", request("conversation-stale"));
    assert_eq!(stale["ok"], false, "{stale:?}");
    assert!(stale["error"]
        .as_str()
        .unwrap()
        .contains("stale conversation_id"));

    let unsupported = call(&handler, "agent.interrupt", request(&agent_id));
    assert_eq!(unsupported["ok"], false, "{unsupported:?}");
    assert!(unsupported["error"]
        .as_str()
        .unwrap()
        .contains("cannot be interrupted"));

    {
        let mut app = state.lock().unwrap();
        insert_dictated_agent_tab(
            &mut app,
            &root,
            "run-interrupt-refused",
            DictatedSession::reporting(AgentStatus::Waiting).interruptible(),
        );
    }
    let idle = call(&handler, "agent.interrupt", request(&agent_id));
    assert_eq!(idle["ok"], false, "{idle:?}");
    assert!(idle["error"]
        .as_str()
        .unwrap()
        .contains("not running a turn"));
}

/// Step 3's refusals, live in production for the first time.
///
/// Until a provider answered `has_terminal` false, every terminal verb's
/// refusal was walked only by tests that built the terminal-free session by
/// hand. This drives the real one: a headless agent the daemon spawned, and
/// every verb a client could reach its basement through.
#[tokio::test]
async fn the_terminal_verbs_refuse_the_headless_agent_the_daemon_spawned() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-refused");
    let key = derived_agent_key(&root, "run-refused");
    use crate::harness::adk::fake;
    run_on_a_headless_provider(
        &state,
        &repo,
        "run-refused",
        fake::stream_json_harness(&[fake::RESULT]),
    );
    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-refused", "body": "have a look" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    wait_for(Duration::from_secs(10), || {
        let s = state.lock().unwrap();
        s.session_registry
            .test_tab(&key)
            .filter(|tab| tab.session_is_live())
            .map(|_| ())
    })
    .await
    .expect("the headless agent is running");
    let term_id = key.tab_id.clone();

    let attached = call(&handler, "agent.attach", json!({ "id": "run-refused" }));
    assert_eq!(attached["ok"], false, "{attached:?}");
    let refusal = attached["error"].as_str().unwrap().to_string();
    assert!(
        refusal.contains(&term_id) && refusal.contains("conversation"),
        "the refusal names the agent and where its work is read: {refusal}"
    );
    for (method, params) in [
        ("term.attach", json!({ "term_id": term_id })),
        (
            "term.input",
            json!({ "term_id": term_id, "data": b64encode(b"ls\r") }),
        ),
        (
            "term.resize",
            json!({ "term_id": term_id, "cols": 100, "rows": 30 }),
        ),
        ("term.ack", json!({ "term_id": term_id, "cursor": 0 })),
    ] {
        let refused = call(&handler, method, params);
        assert_eq!(refused["error"], json!(refusal), "{method}: {refused:?}");
    }

    state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&key)
        .unwrap()
        .session
        .end();
}

/// The terminal verbs refuse an agent whose session has no terminal, and
/// say where that agent's work actually is.
///
/// Following `require_shell_kind`: never fall back. Swallowing keystrokes
/// no process will read, or answering `ok` to a resize of a grid that does
/// not exist, is the same silent-wrong-program failure that refusal exists
/// to prevent — and here it would leave a human typing into a void.
#[test]
fn the_terminal_verbs_refuse_an_agent_with_no_terminal() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = AppState::canonical_root(&repo);
    let agent_id = "agent-protocol";
    state.lock().unwrap().session_registry.test_insert_tab(
        TabKey::agent(&root, agent_id),
        terminal_free_agent_tab(&root, "run-protocol", agent_id),
    );
    let term_id = agent_tab_id(agent_id);

    let typed = call(
        &handler,
        "term.input",
        json!({ "term_id": term_id, "data": b64encode(b"ls\r") }),
    );
    assert_eq!(typed["ok"], false, "{typed:?}");
    let refusal = typed["error"].as_str().unwrap().to_string();
    assert!(
        refusal.contains(&term_id) && refusal.contains("conversation"),
        "the refusal names the agent and where its work is read: {refusal}"
    );

    let resized = call(
        &handler,
        "term.resize",
        json!({ "term_id": term_id, "cols": 100, "rows": 30 }),
    );
    assert_eq!(
        resized["ok"], false,
        "a viewport means nothing to a session with no grid: {resized:?}"
    );
    assert_eq!(resized["error"], json!(refusal));
}

/// Attaching to a terminal-free agent refuses the same way, whichever verb
/// asks — `agent.attach` by what a surface holds, `term.attach` by wire id.
/// One capability, one question, one answer.
#[tokio::test]
async fn agent_attach_refuses_an_agent_with_no_terminal() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let root = AppState::canonical_root(&repo);
    let agent_id = "agent-protocol";
    let project_id = {
        let mut s = state.lock().unwrap();
        s.session_registry.test_insert_tab(
            TabKey::agent(&root, agent_id),
            terminal_free_agent_tab(&root, "run-protocol", agent_id),
        );
        s.project_at(0).id.clone()
    };

    let attached = call(
        &handler,
        "agent.attach",
        json!({ "project_id": project_id }),
    );
    assert_eq!(attached["ok"], false, "{attached:?}");
    assert!(
        attached["error"].as_str().unwrap().contains("conversation"),
        "{attached:?}"
    );

    let by_wire = call(
        &handler,
        "term.attach",
        json!({ "term_id": agent_tab_id(agent_id) }),
    );
    assert_eq!(by_wire["error"], attached["error"], "{by_wire:?}");

    // A client that was never allowed to attach has nothing to acknowledge
    // either, and hears why rather than acking into a screen that does not
    // exist.
    let acked = call(
        &handler,
        "term.ack",
        json!({ "term_id": agent_tab_id(agent_id), "cursor": 0 }),
    );
    assert_eq!(acked["error"], attached["error"], "{acked:?}");
}

fn agent_role() -> TabRole {
    TabRole::Agent {
        owner: "run-status".to_string(),
        agent_id: "agent-status".to_string(),
        provider: AgentProvider::Claude,
    }
}

/// The pulse is whatever the session says it is doing, and the daemon does
/// not second-guess it with the terminal's own signals. The age of the last
/// paint is how a PTY — which has no better answer — synthesizes its
/// status; a harness that knows its own turn boundaries has to be able to
/// contradict it.
#[test]
fn the_pulse_reads_the_session_status_and_nothing_else() {
    let cases = [
        (AgentStatus::Working, true),
        (AgentStatus::Waiting, false),
        (AgentStatus::Starting, false),
        (AgentStatus::Ended { code: Some(0) }, false),
        (AgentStatus::Ended { code: None }, false),
    ];
    for (status, working) in cases {
        assert_eq!(
            agent_is_working(&tab_reporting(agent_role(), status)),
            working,
            "{status:?}"
        );
    }
}

/// The two conjuncts the session cannot see survive the move: a shell is
/// the human's own hands however busy it looks, and a tab whose stream has
/// ended is holding a corpse, not a heartbeat.
#[test]
fn a_working_status_still_needs_a_live_agent_tab() {
    assert!(!agent_is_working(&tab_reporting(
        TabRole::Shell,
        AgentStatus::Working
    )));
    let mut retained = tab_reporting(agent_role(), AgentStatus::Working);
    retained.live = false;
    assert!(!agent_is_working(&retained));
}

/// An agent tab whose session reports `status`, rooted where the daemon
/// will look for it.
pub(in crate::app::tests) fn insert_dictated_agent_tab(
    state: &mut AppState,
    root: &std::path::Path,
    owner: &str,
    session: DictatedSession,
) -> TabKey {
    let agent_id = crate::agent::derived_agent_id(owner);
    insert_agent_tab(state, root, owner, &agent_id, session)
}

pub(in crate::app::tests) fn insert_agent_tab(
    state: &mut AppState,
    root: &std::path::Path,
    owner: &str,
    agent_id: &str,
    session: impl AgentSession + 'static,
) -> TabKey {
    let key = TabKey::agent(root, agent_id);
    if let Some(previous) = state
        .session_registry
        .test_tab(&key)
        .and_then(|tab| tab.session_instance.clone())
    {
        state.record_agent_session_end(owner, agent_id, &previous);
    }
    let working = matches!(session.status(), AgentStatus::Working);
    let choice = state
        .entity_agents(owner)
        .ok()
        .and_then(|agents| agents.by_id(agent_id))
        .map(|agent| agent.choice.clone())
        .unwrap_or_default();
    let instance = state.record_agent_session_start(owner, agent_id, root, &choice, "build");
    let mut tab = dictated_agent_tab(root, owner, agent_id, session);
    tab.session_instance = instance;
    state.session_registry.test_insert_tab(key.clone(), tab);
    state.record_agent_working_since(owner, agent_id, working.then(now_rfc3339));
    key
}

#[test]
fn a_secondary_agent_keeps_the_entry_working_and_its_stop_is_activity() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let root = insert_run(
        &mut state,
        &repo,
        dir.path(),
        "run-secondary-working",
        RunState::Building,
    );
    let added = state.handle(req(
        "agent.add",
        json!({ "entity_id": "run-secondary-working" }),
    ));
    let secondary = added["result"]["agent"]["id"].as_str().unwrap();
    insert_agent_tab(
        &mut state,
        &root,
        "run-secondary-working",
        secondary,
        DictatedSession::reporting(AgentStatus::Working),
    );

    let row = work_item_row_for(&mut state, "run-secondary-working");
    assert_eq!(row["working"], true, "{row:?}");

    insert_agent_tab(
        &mut state,
        &root,
        "run-secondary-working",
        secondary,
        DictatedSession::reporting(AgentStatus::Waiting),
    );
    let row = work_item_row_for(&mut state, "run-secondary-working");
    assert_eq!(row["working"], false, "{row:?}");
}

#[tokio::test]
async fn protocol_status_completion_records_activity_without_a_board_poll() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let root = insert_run(
        &mut app,
        &repo,
        dir.path(),
        "run-status-watch",
        RunState::Building,
    );
    let initial = crate::harness::SessionStatusSnapshot::new(AgentStatus::Working);
    let completed = initial.transition(AgentStatus::Waiting).unwrap();
    let (status_tx, status_rx) = tokio::sync::watch::channel(initial);
    let key = insert_agent_tab(
        &mut app,
        &root,
        "run-status-watch",
        &crate::agent::derived_agent_id("run-status-watch"),
        DictatedSession::reporting(AgentStatus::Working).watching_status(status_rx),
    );
    let session = Arc::clone(&app.session_registry.test_tab(&key).unwrap().session);
    let instance = app
        .session_registry
        .test_tab(&key)
        .unwrap()
        .session_instance
        .clone();
    let state = app.shared();
    spawn_status_pump(&state, key, session, instance, Some(status_tx.subscribe()));

    status_tx.send(completed.clone()).unwrap();
    let recorded = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let matches = state
                .lock()
                .unwrap()
                .board
                .attention()
                .last_worked_at("run-status-watch")
                == completed.last_worked_at.as_deref();
            if matches {
                break true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or(false);
    assert!(
        recorded,
        "the protocol boundary is persisted without reading the board"
    );
}

/// Whether an agent is live is the same question the pulse asks, one state
/// further out, and it is now asked the same way: a session that reports
/// `Ended` is over, whatever a process table would have said about it.
///
/// `has_exited` was how a terminal answered this. A session with no process
/// behind it has no such question to poll, and it must still be able to say
/// its session is over.
#[test]
fn liveness_is_read_off_the_reported_status() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let root = insert_run(
        &mut state,
        &repo,
        dir.path(),
        "run-liveness",
        RunState::Building,
    );
    let agent_id = crate::agent::derived_agent_id("run-liveness");

    insert_dictated_agent_tab(
        &mut state,
        &root,
        "run-liveness",
        DictatedSession::reporting(AgentStatus::Waiting),
    );
    assert!(
        state.agent_is_live(&root, &agent_id),
        "a session waiting at its prompt is live"
    );
    assert_eq!(
        state.agent_digests("run-liveness", DigestScope::List)[0]["state"],
        "live"
    );

    insert_dictated_agent_tab(
        &mut state,
        &root,
        "run-liveness",
        DictatedSession::reporting(AgentStatus::Ended { code: Some(1) }),
    );
    assert!(
        !state.agent_is_live(&root, &agent_id),
        "a session that reports it is over is not live"
    );
    assert_ne!(
        state.agent_digests("run-liveness", DigestScope::List)[0]["state"],
        "live"
    );
}

#[test]
fn an_agents_digest_carries_the_model_it_is_actually_running() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    insert_run(
        &mut state,
        &repo,
        dir.path(),
        "run-active-model",
        RunState::Building,
    );
    let agent_id = crate::agent::derived_agent_id("run-active-model");

    assert_eq!(
        state.agent_digests("run-active-model", DigestScope::List)[0]["active_model"],
        "",
        "an agent that has never run is running nothing Build knows of"
    );

    state.record_agent_active_model(
        "run-active-model",
        &agent_id,
        Some("claude-fable-5-1".to_string()),
    );
    assert_eq!(
        state.agent_digests("run-active-model", DigestScope::List)[0]["active_model"],
        "claude-fable-5-1"
    );

    state.record_agent_active_model(
        "run-active-model",
        &agent_id,
        Some("claude-opus-5".to_string()),
    );
    assert_eq!(
        state.agent_digests("run-active-model", DigestScope::List)[0]["active_model"],
        "claude-opus-5",
        "the newer announcement wins"
    );

    state.record_agent_runtime_choice(
        "run-active-model",
        &agent_id,
        Some("claude-opus-5".to_string()),
        Some("high".to_string()),
    );
    let digest = state.agent_digests("run-active-model", DigestScope::List)[0].clone();
    assert_eq!(digest["active_model"], "claude-opus-5");
    assert_eq!(digest["active_effort"], "high");
}

#[test]
fn an_agent_that_announced_nothing_reports_the_model_its_next_start_spends() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    insert_run(
        &mut state,
        &repo,
        dir.path(),
        "run-projected-model",
        RunState::Building,
    );
    let agent_id = crate::agent::derived_agent_id("run-projected-model");

    let digest = state.agent_digests("run-projected-model", DigestScope::List)[0].clone();
    assert_eq!(digest["model"], "");
    assert_eq!(digest["effort"], "");
    assert_eq!(digest["active_model"], "");

    state
        .set_agent_model_choice(
            "run-projected-model",
            &agent_id,
            ModelChoice {
                provider: AgentProvider::default(),
                model: Some("claude-opus-5".to_string()),
                effort: None,
            },
        )
        .expect("the choice persists");
    let digest = state.agent_digests("run-projected-model", DigestScope::List)[0].clone();
    assert_eq!(digest["model"], "claude-opus-5");
    assert_eq!(digest["effort"], "");
    assert_eq!(
        digest["active_model"], "claude-opus-5",
        "with nothing announced, what the next start spends is what it runs"
    );

    state.record_agent_active_model(
        "run-projected-model",
        &agent_id,
        Some("claude-haiku-4-5".to_string()),
    );
    assert_eq!(
        state.agent_digests("run-projected-model", DigestScope::List)[0]["active_model"],
        "claude-haiku-4-5",
        "the session's own announcement outranks the projection"
    );
}

#[test]
fn the_digest_reports_the_model_the_next_start_will_spend() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    insert_run(
        &mut state,
        &repo,
        dir.path(),
        "run-choose",
        RunState::Building,
    );
    let provider_before =
        state.agent_digests("run-choose", DigestScope::List)[0]["provider"].clone();
    let agent_id = primary_agent_id(&state, "run-choose");

    state
        .set_agent_model_choice(
            "run-choose",
            &agent_id,
            ModelChoice {
                provider: AgentProvider::default(),
                model: Some("claude-opus-5".to_string()),
                effort: Some("high".to_string()),
            },
        )
        .expect("the choice persists");

    let digest = state.agent_digests("run-choose", DigestScope::List)[0].clone();
    assert_eq!(digest["model"], "claude-opus-5");
    assert_eq!(digest["effort"], "high");
    assert_eq!(digest["provider"], provider_before);
}

#[test]
fn choosing_a_model_stales_the_entity_for_every_browser() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    insert_run(
        &mut state,
        &repo,
        dir.path(),
        "run-stale-choice",
        RunState::Building,
    );
    state.changes().flush();
    assert!(!state.changes().has_pending());
    let agent_id = primary_agent_id(&state, "run-stale-choice");

    state
        .set_agent_model_choice(
            "run-stale-choice",
            &agent_id,
            ModelChoice {
                provider: AgentProvider::default(),
                model: Some("claude-opus-5".to_string()),
                effort: None,
            },
        )
        .expect("the choice persists");

    assert!(
        state.changes().has_pending(),
        "another browser repaints instead of waiting for its own poll"
    );
}

/// The idle sweep's two clocks come off the session: the exit it explains a
/// crash with is the code inside `Ended`, and the silence it demotes on is
/// `quiet_for`.
///
/// Both used to be the PTY's — `has_exited` + `exit_code`, and the age of
/// the last paint. A session protocol has neither a screen nor a wait, and
/// has to be able to answer both.
#[test]
fn the_idle_sweep_reads_the_exit_and_the_quiet_off_the_session() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let root = insert_run(
        &mut state,
        &repo,
        dir.path(),
        "run-swept",
        RunState::Building,
    );

    // Live, and heard from inside the threshold: nothing to explain.
    insert_dictated_agent_tab(
        &mut state,
        &root,
        "run-swept",
        DictatedSession::reporting(AgentStatus::Waiting).silent_for(Duration::from_secs(10)),
    );
    assert!(
        state.mark_idle_tasks(Duration::from_secs(300)).is_empty(),
        "a session heard from inside the threshold is not an anomaly"
    );

    // Live, and quiet past it: demoted, and no exit code is invented.
    insert_dictated_agent_tab(
        &mut state,
        &root,
        "run-swept",
        DictatedSession::reporting(AgentStatus::Waiting).silent_for(Duration::from_secs(600)),
    );
    assert_eq!(
        state.mark_idle_tasks(Duration::from_secs(300)),
        vec!["run-swept".to_string()],
        "silence past the threshold is the anomaly the sweep exists for"
    );
    let got = state.handle(req("run.get", json!({ "run_id": "run-swept" })));
    assert_eq!(got["result"]["state"], "idle_unreported", "{got:?}");
    assert!(
        got["result"]["last_error"].is_null(),
        "nothing exited, so nothing claims an exit code: {got:?}"
    );

    // Ended: the code inside the status is what the crash is explained by.
    state.runs.get_mut("run-swept").unwrap().run.state = RunState::Building;
    insert_dictated_agent_tab(
        &mut state,
        &root,
        "run-swept",
        DictatedSession::reporting(AgentStatus::Ended { code: Some(9) }),
    );
    assert_eq!(
        state.mark_idle_tasks(Duration::from_secs(300)),
        vec!["run-swept".to_string()],
    );
    let got = state.handle(req("run.get", json!({ "run_id": "run-swept" })));
    assert!(
        got["result"]["last_error"]
            .as_str()
            .unwrap_or_default()
            .contains("exit code 9"),
        "the code the session reported explains the crash: {got:?}"
    );
}

/// The nudge is a turn, and it travels as one. It used to be a `write_prompt`
/// — keystroke mechanics — and the whole point of a value is that a
/// session with no keyboard can still be told what to say.
#[test]
fn the_nudge_hands_the_agent_a_turn() {
    let root = AppState::canonical_root(&PathBuf::from("/nowhere"));
    let agent_id = "agent-nudged";
    let mut tabs: HashMap<TabKey, Tab> = HashMap::new();
    let log = SessionLog::default();
    let mut tab = tab_running(
        TabRole::Agent {
            owner: "run-nudge".to_string(),
            agent_id: agent_id.to_string(),
            provider: AgentProvider::default(),
        },
        DictatedSession::reporting(AgentStatus::Waiting).recording_into(&log),
    );
    tab.root = root.clone();
    tabs.insert(TabKey::agent(&root, agent_id), tab);

    nudge_live_agent_tab(&tabs, &root, agent_id, "run-nudge", false);
    assert_eq!(
        log.turns(),
        vec![NEW_THREAD_MESSAGES_PROMPT.to_string()],
        "the waiting agent is told a message arrived, as one turn"
    );

    // A session that reports it is over is not told anything: the message
    // is durable on the thread, and its replacement reads it there.
    let over = SessionLog::default();
    let mut tab = tab_running(
        TabRole::Agent {
            owner: "run-nudge".to_string(),
            agent_id: agent_id.to_string(),
            provider: AgentProvider::default(),
        },
        DictatedSession::reporting(AgentStatus::Ended { code: Some(0) }).recording_into(&over),
    );
    tab.root = root.clone();
    tabs.insert(TabKey::agent(&root, agent_id), tab);

    nudge_live_agent_tab(&tabs, &root, agent_id, "run-nudge", false);
    assert!(over.turns().is_empty(), "a dead agent hears nothing");
}

/// Closing a worktree's agents ends their sessions — through `end`, which
/// carries the reap obligation the kill-and-reap pair used to name.
#[test]
fn closing_a_worktrees_agents_ends_their_sessions() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let root = AppState::canonical_root(&repo);
    let log = SessionLog::default();
    insert_dictated_agent_tab(
        &mut state,
        &root,
        "run-closed",
        DictatedSession::reporting(AgentStatus::Working).recording_into(&log),
    );

    state.retire_agent_tabs(&root);

    assert!(
        log.ended(),
        "an agent whose owner is gone must be ended, not merely forgotten"
    );
    assert!(
        state.session_registry.test_counts().tabs == 0,
        "and forgotten too"
    );
}

/// The board reports it per worktree, so a bare worktree — which has no run
/// state to read — can still say whether something is happening in it.
#[tokio::test]
async fn the_board_reports_whether_an_agent_is_working_in_a_worktree() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    add_external_worktree(&repo, dir.path(), "hand-made", "hand-made");
    let worktree_id = state
        .lock()
        .unwrap()
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|w| w.branch.as_deref() == Some("hand-made"))
        .expect("discoverable")
        .id;

    let entry_of = |state: &Arc<Mutex<AppState>>| {
        let board = state.lock().unwrap().handle(req("board.list", json!({})));
        board["result"]["external_worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["worktree_id"] == json!(worktree_id.clone()))
            .cloned()
            .unwrap_or_else(|| panic!("worktree missing: {board:?}"))
    };
    assert_eq!(
        entry_of(&state)["agent_working"],
        false,
        "nothing running yet"
    );
    assert_eq!(entry_of(&state)["can_finish"], false);

    // A shell is not an agent, so opening one must not start the pulse.
    let created = handler.call(
        SessionSender::detached("s1"),
        req(
            "term.create",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "kind": "shell" }),
        ),
    );
    assert_eq!(created["ok"], true, "{created:?}");
    assert_eq!(
        entry_of(&state)["agent_working"],
        false,
        "a shell is the human's own hands"
    );
    assert_eq!(entry_of(&state)["can_finish"], false);

    // Build's agent starts in that same worktree: the pulse is on, and it
    // is reported against the WORKTREE — the run that owns the agent is
    // not what the board asked about.
    let root = state
        .lock()
        .unwrap()
        .resolve_external_worktree(&project_id, &worktree_id)
        .unwrap()
        .path;
    let key = insert_unmanaged_agent_tab(&state, &root, "run-in-the-worktree")
        .expect("the unmanaged agent fixture spawns");
    assert_eq!(
        entry_of(&state)["agent_working"],
        true,
        "an agent painting in this worktree is the pulse"
    );
    assert_eq!(entry_of(&state)["can_finish"], false);

    state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&key)
        .unwrap()
        .session
        .backdate_last_output(AGENT_WORKING_WINDOW + Duration::from_secs(1));
    assert_eq!(entry_of(&state)["agent_working"], false);
    assert_eq!(
        entry_of(&state)["can_finish"],
        true,
        "a managed agent that has stopped working makes finish advisable"
    );
    state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&key)
        .unwrap()
        .session
        .end();
}

/// A worktree Build never touched — no run, no adoption — is not the
/// user's inbox to clutter, and nothing short of the user bringing it into
/// Build changes that: not a commit landing on it, not an agent tab
/// somehow live in it (a raw terminal opened by hand, say). Only adopting
/// it — the same act that mints a run for it — earns it a row, and from
/// there it leaves the inbox the way every other row does.
#[tokio::test]
async fn a_bare_external_worktree_stays_off_the_board_no_matter_what_happens_in_it() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    let checkout = add_external_worktree(&repo, dir.path(), "hand-made", "hand-made");
    let worktree_id = state
        .lock()
        .unwrap()
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|w| w.branch.as_deref() == Some("hand-made"))
        .expect("discoverable")
        .id;

    let has_branch_row = |state: &Arc<Mutex<AppState>>| {
        let board = state.lock().unwrap().handle(req("board.list", json!({})));
        board["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["kind"] == "branch" && row["branch"] == json!("hand-made"))
    };
    assert!(
        !has_branch_row(&state),
        "nobody has touched it yet — it stays out of the inbox"
    );

    commit_in(&checkout, "some work landed");
    assert!(
        !has_branch_row(&state),
        "a commit alone does not earn it a row"
    );

    let root = state
        .lock()
        .unwrap()
        .resolve_external_worktree(&project_id, &worktree_id)
        .unwrap()
        .path;
    let key = insert_unmanaged_agent_tab(&state, &root, "agent-in-the-worktree")
        .expect("the unmanaged agent fixture spawns");
    assert!(
        !has_branch_row(&state),
        "an agent happening to be live in it is not the same as Build having adopted it"
    );
    state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&key)
        .unwrap()
        .session
        .end();

    let adopted = state.lock().unwrap().handle(req(
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    ));
    assert_eq!(adopted["ok"], true, "{adopted:?}");
    assert!(has_branch_row(&state), "adopting it is what earns the row");
}

/// The relay calls `dispatch` directly — `handle` is a test convenience — so
/// a stamp wired into `handle` would pass every test and fire in no real
/// session. This drives the wire path the daemon actually uses.
#[tokio::test]
async fn stamping_happens_on_the_path_the_relay_uses() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();

    let created = handler.call(
        SessionSender::detached("s1"),
        req(
            "worktree.create",
            json!({ "project_id": project_id, "name": "over the wire" }),
        ),
    );
    assert_eq!(created["ok"], true, "{created:?}");
    let worktree_id = created["result"]["worktree_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        state
            .lock()
            .unwrap()
            .board
            .attention()
            .attention(&worktree_id)
            .is_some(),
        "the wire path must stamp too"
    );
}

/// Attention outlives the daemon, or Monday would look like a fresh install.
#[test]
fn attention_survives_a_restart() {
    let (dir, repo) = init_repo();
    let run_id;
    {
        let mut state = qa_state(&repo, dir.path());
        let (_, id) = planned_run_in_review(&mut state, "durable attention");
        run_id = id;
        state.handle(req("entity.seen", json!({ "entity_id": run_id })));
        assert_eq!(attention_of(&mut state, &run_id)["seen"], true);
    }
    let mut reloaded = qa_state(&repo, dir.path());
    let after = attention_of(&mut reloaded, &run_id);
    assert_eq!(after["seen"], true, "{after:?}");
    // The read cursor with it: a badge derived from a cursor that reset
    // would make every restart a wall of unread.
    let entry = board_entry(&mut reloaded, &run_id);
    assert_eq!(entry["unread"], false, "{entry:?}");
}
