use super::*;

#[tokio::test]
async fn agent_attach_streams_a_live_run_and_retains_the_last_screen() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let (tab_key, wire_id) = insert_live_run(&state, &repo, dir.path().join("side"), "run-9");

    let (sender, mut pushes, key) = SessionSender::observable("s1");
    let res = handler.call(
        sender,
        req(
            "agent.attach",
            json!({ "id": "run-9", "cols": 100, "rows": 30 }),
        ),
    );
    assert_eq!(res["ok"], true, "{res:?}");
    assert_eq!(
        res["result"]["term_id"], wire_id,
        "an agent is addressed by its worktree, not by the run that owns it"
    );
    assert_eq!(res["result"]["live"], true);

    let seen = wait_for_pushes(&mut pushes, &key, |seen| {
        output_text(seen, &wire_id).contains("agent-beat")
    })
    .await;
    assert_eq!(seen[0]["type"], "term.reset", "{seen:?}");

    // The agent's process ends → clients hear agent_session_ended and the
    // tab keeps showing the last screen.
    state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&tab_key)
        .unwrap()
        .session
        .end();
    wait_for_push(&mut pushes, &key, |p| {
        p["type"] == "term.closed"
            && p["term_id"] == wire_id
            && p["reason"] == "agent_session_ended"
    })
    .await;

    let again = handler.call(
        SessionSender::detached("s2"),
        req("agent.attach", json!({ "id": "run-9" })),
    );
    assert_eq!(again["result"]["live"], false, "{again:?}");
    assert!(
        !b64decode(again["result"]["snapshot"].as_str().unwrap())
            .unwrap()
            .is_empty(),
        "a dead agent still shows what it last painted"
    );
}

/// The Agent tab is a fixture on every worktree surface, and most of those
/// surfaces have no entity to name: an unadopted external worktree and the
/// project's primary checkout are directories, not runs. So `agent.attach`
/// takes the same scope shapes `term.create`/`term.list` take, resolves
/// them server-side, and answers for the tab rooted there — empty when no
/// agent has run, the live tab once one has.
#[tokio::test]
async fn agent_attach_addresses_a_worktree_by_scope_before_any_run_owns_it() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    add_external_worktree(&repo, dir.path(), "feature-x", "feature-x");
    let external = state
        .lock()
        .unwrap()
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|w| w.branch.as_deref() == Some("feature-x"))
        .expect("the external worktree is discoverable");

    // Nothing has ever run here: the tab exists as an empty screen, never
    // as an error — mounting it must not spawn anything.
    let empty = handler.call(
        SessionSender::detached("s1"),
        req(
            "agent.attach",
            json!({ "project_id": project_id, "worktree_id": external.id }),
        ),
    );
    assert_eq!(empty["ok"], true, "{empty:?}");
    assert_eq!(empty["result"]["live"], false);
    assert_eq!(
        empty["result"]["term_id"],
        json!(format!("agent:{}", external.id))
    );

    // The primary checkout answers to the project scope alone, the same way
    // its shells do.
    let primary = handler.call(
        SessionSender::detached("s1"),
        req("agent.attach", json!({ "project_id": project_id })),
    );
    assert_eq!(primary["ok"], true, "{primary:?}");
    assert_eq!(primary["result"]["live"], false);
    assert_eq!(
        primary["result"]["term_id"],
        json!(format!(
            "agent:{}",
            crate::worktree::external_worktree_id(&AppState::canonical_root(&repo))
        ))
    );

    // Once an agent runs in that worktree, the same scope reaches the tab
    // itself — one agent, one wire id, whichever shape asked for it.
    let root = AppState::canonical_root(&external.path);
    let (tab, rx) = Tab::spawn_agent(
        "run-x".to_string(),
        crate::agent::derived_agent_id("run-x"),
        test_agent_session_request(
            AgentProvider::default(),
            warm_tui_spec(),
            root.clone(),
            terminal_size(120, 40),
        ),
    )
    .expect("the agent tab spawns");
    let wire_id = tab.wire_id();
    let key = derived_agent_key(&root, "run-x");
    state
        .lock()
        .unwrap()
        .session_registry
        .test_insert_tab(key.clone(), tab);
    spawn_tab_pumps(&state, key.clone(), rx);

    let live = handler.call(
        SessionSender::detached("s2"),
        req(
            "agent.attach",
            json!({ "project_id": project_id, "worktree_id": external.id }),
        ),
    );
    assert_eq!(live["ok"], true, "{live:?}");
    assert_eq!(live["result"]["live"], true);
    assert_eq!(live["result"]["term_id"], json!(wire_id));
}

/// An agent that exited leaves a screen and nothing else — and the offer to
/// start one again leads with the harness that painted it. Only the tab
/// knows which one that was (the entity's model choice can have moved since,
/// and most agent-bearing worktrees have no entity at all), so the attach
/// says it. A worktree nothing has run in names none: there is nothing to
/// report, and the client leads with its own default instead.
#[tokio::test]
async fn agent_attach_names_the_provider_that_painted_the_screen() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    let root = AppState::canonical_root(&repo);

    // Nothing has run here yet: no harness to name.
    let empty = handler.call(
        SessionSender::detached("s1"),
        req("agent.attach", json!({ "project_id": project_id })),
    );
    assert_eq!(empty["ok"], true, "{empty:?}");
    assert!(
        empty["result"]["provider"].is_null(),
        "a worktree with no agent tab has no harness to report: {empty:?}"
    );

    // One ran, on codex, and died. The retained screen still answers for it.
    let (tab, rx) = Tab::spawn_agent(
        "run-codex".to_string(),
        crate::agent::derived_agent_id("run-codex"),
        test_agent_session_request(
            AgentProvider::Codex,
            HarnessSpec::new("true"),
            root.clone(),
            terminal_size(120, 40),
        ),
    )
    .expect("the agent tab spawns");
    let key = derived_agent_key(&root, "run-codex");
    state
        .lock()
        .unwrap()
        .session_registry
        .test_insert_tab(key.clone(), tab);
    spawn_tab_pumps(&state, key.clone(), rx);

    let ran = handler.call(
        SessionSender::detached("s2"),
        req("agent.attach", json!({ "project_id": project_id })),
    );
    assert_eq!(ran["ok"], true, "{ran:?}");
    assert_eq!(
        ran["result"]["provider"], "codex",
        "the retained screen names the harness that painted it: {ran:?}"
    );

    // A user's shell is not an agent, and reports no harness.
    let shell = handler.call(
        SessionSender::detached("s3"),
        req("term.create", json!({ "project_id": project_id })),
    );
    assert_eq!(shell["ok"], true, "{shell:?}");
    let attached = handler.call(
        SessionSender::detached("s3"),
        req(
            "term.attach",
            json!({ "term_id": shell["result"]["term_id"] }),
        ),
    );
    assert!(
        attached["result"]["provider"].is_null(),
        "a login shell runs no harness: {attached:?}"
    );
}

/// The Agent tab is a fixture on every worktree surface, so clients mount
/// it long before anything has ever run there — the state EVERY worktree is
/// in right after a daemon restart. Such a client is attached to a screen
/// with no PTY, and when the agent finally starts it must go live WHERE IT
/// STANDS: the session's first frames reach it without an unmount and
/// remount. The viewport it attached at is the one the new PTY is sized to,
/// the same rule a live attach follows.
#[tokio::test]
async fn a_client_attached_before_the_first_spawn_streams_the_session_it_waited_for() {
    let (dir, repo) = init_repo();
    let (state, handler, _) =
        agent_tab_fixture_at(&repo, dir.path(), "run-waited-for", repo.clone());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    let wire_id = format!(
        "agent:{}",
        crate::worktree::external_worktree_id(&AppState::canonical_root(&repo))
    );

    // Nothing has ever run in the primary checkout: a blank, dead screen.
    let (sender, mut pushes, key) = SessionSender::observable("s1");
    let empty = handler.call(
        sender,
        req(
            "agent.attach",
            json!({ "project_id": project_id, "cols": 100, "rows": 30 }),
        ),
    );
    assert_eq!(empty["ok"], true, "{empty:?}");
    assert_eq!(empty["result"]["live"], false);
    assert_eq!(
        empty["result"]["term_id"],
        json!(wire_id),
        "a worktree with no agent yet answers for itself"
    );
    assert_eq!(
        empty["result"]["cursor"], 0,
        "a screen that has painted nothing starts the cursor at zero"
    );

    // The agent starts later, from a delivery. This client never re-attached.
    let (delivered_to, spawned) = deliver(
        &state,
        &repo,
        "run-waited-for",
        &crate::agent::derived_agent_id("run-waited-for"),
        &ModelChoice::default(),
        "build",
        ["COLD-PROMPT-FOR-A-WAITING-CLIENT", "WARM-NUDGE"],
    )
    .expect("the delivery spawns the worktree's agent");
    assert_eq!(spawned, Spawned::Fresh);
    // The screen was addressed by the WORKTREE, because there was no agent
    // to name yet; the agent born onto it is addressed by itself, and the
    // opening reset is where the waiting client learns that id.
    assert_eq!(
        delivered_to,
        agent_tab_id(&crate::agent::derived_agent_id("run-waited-for"))
    );

    let seen = wait_for_pushes(&mut pushes, &key, |seen| {
        output_text(seen, &delivered_to).contains("COLD-PROMPT-FOR-A-WAITING-CLIENT")
    })
    .await;
    assert_eq!(
        seen[0]["type"], "term.reset",
        "the waiting client hears the session start: {seen:?}"
    );
    assert_eq!(
        seen[0]["cursor"], 0,
        "the opening reset lands at the cursor the attach handed out — \
         no gap, and the client applies it rather than deduping it away"
    );
    let mut cursor = 0;
    for push in &seen {
        let pushed = push["cursor"].as_u64().expect("every frame carries one");
        assert!(pushed >= cursor, "the cursor never rewinds: {seen:?}");
        cursor = pushed;
    }
    assert!(
        cursor > 0,
        "the session's output moved the cursor: {seen:?}"
    );

    let s = state.lock().unwrap();
    let screen = screen_of(
        s.session_registry
            .test_tab(&derived_agent_key(
                &AppState::canonical_root(&repo),
                "run-waited-for",
            ))
            .unwrap(),
    );
    assert_eq!(
        screen.size(),
        (100, 30),
        "the spawned agent is sized to the viewport of the client already watching it"
    );
}

/// The window inside a respawn: the reservation has taken the dead tab out
/// of the registry, so a client mounting the Agent tab right then lands on
/// a waiting screen even though this worktree HAS a retained screen with a
/// cursor. That client must be carried onto the replacement — but its
/// screen must not be: the retained cursor is what reconnect dedupes on and
/// it never rewinds, so the waiting screen contributes its clients and
/// nothing else.
#[tokio::test]
async fn a_client_attaching_inside_a_respawn_is_carried_without_rewinding_the_cursor() {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-respawn-race");
    let choice = ModelChoice::default();
    let canonical = AppState::canonical_root(&root);
    let key = derived_agent_key(&canonical, "run-respawn-race");

    // A first session paints, then dies: its screen and cursor are retained.
    deliver(
        &state,
        &root,
        "run-respawn-race",
        &crate::agent::derived_agent_id("run-respawn-race"),
        &choice,
        "build",
        ["FIRST-SESSION", "warm"],
    )
    .expect("the first delivery spawns");
    wait_for_agent_screen(&state, &root, "FIRST-SESSION").await;
    state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&key)
        .unwrap()
        .session
        .end();
    let retained_total = loop {
        {
            let s = state.lock().unwrap();
            let tab = &s.session_registry.test_tab(&key).unwrap();
            if !tab.live {
                break screen_of(tab).cursor();
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert!(retained_total > 0, "the dead session left a cursor behind");

    // The state a client that attached inside the spawn window is in.
    let (sender, mut pushes, session_key) = SessionSender::observable("late");
    {
        let mut s = state.lock().unwrap();
        let waiting = ScreenHandle::new(&key.tab_id, 90, 25);
        waiting.attach(&sender, None);
        s.session_registry
            .test_remember_waiting_screen(key.clone(), waiting);
    }

    let (wire_id, spawned) = deliver(
        &state,
        &root,
        "run-respawn-race",
        &crate::agent::derived_agent_id("run-respawn-race"),
        &choice,
        "build",
        ["SECOND-SESSION", "warm"],
    )
    .expect("a dead agent is replaced");
    assert_eq!(spawned, Spawned::Fresh, "a dead agent is not an agent");

    let seen = wait_for_pushes(&mut pushes, &session_key, |seen| {
        output_text(seen, &wire_id).contains("SECOND-SESSION")
    })
    .await;
    let opening = seen
        .iter()
        .find(|push| push["type"] == "term.reset")
        .expect("the waiting client hears the new session start");
    assert!(
        opening["cursor"].as_u64().unwrap() >= retained_total,
        "the retained cursor is carried forward, never rewound to the \
         waiting screen's zero: {seen:?}"
    );
}

/// Point a fixture's project at a provider running `spec`.
///
/// The program and the carrier are two halves of one launch config — the
/// provider on the caller's `ModelChoice` is what `Tab::spawn` asks which
/// carrier to open — so a caller that swaps the spec swaps the choice
/// beside it, or runs a stream-json child inside a PTY and proves nothing.
pub(in crate::app::tests) fn a_provider_running(
    state: &Arc<Mutex<AppState>>,
    repo: &std::path::Path,
    spec: HarnessSpec,
) {
    let mut s = state.lock().unwrap();
    let worktrees = s.worktrees_root.clone();
    let agent = Agent::WarmBuilder(Arc::new(
        move |_prompt: &str, _choice: &ModelChoice, _options: &SpawnOptions| Ok(spec.clone()),
    ));
    s.project_at_mut(0).orch = Orchestrator::new(
        repo.to_path_buf(),
        worktrees,
        agent,
        Templates::default(),
        test_bridge_exe(),
    );
}

/// The same, for a carrier with no terminal, handing back the choice that
/// opens it.
pub(in crate::app::tests) fn a_headless_provider_running(
    state: &Arc<Mutex<AppState>>,
    repo: &std::path::Path,
    spec: HarnessSpec,
) -> ModelChoice {
    a_provider_running(state, repo, spec);
    ModelChoice {
        provider: AgentProvider::ClaudeAdk,
        ..ModelChoice::default()
    }
}

/// A spawn with no terminal closes the screens waiting on it.
///
/// Clients that mount the Agent tab before a worktree has an agent are held
/// on a screen with no PTY, and a spawn carries them onto the real one. A
/// session with no terminal has no real screen to carry them to, so the
/// carry would drop them silently and leave them attached to a grid nothing
/// will ever paint. They are told instead — the way the reaper and
/// `retire_agent` tell one — and the rail, which reads `has_terminal: false`
/// off the digest, stops offering the basement they were waiting for.
#[tokio::test]
async fn a_headless_spawn_closes_the_screens_that_were_waiting_for_a_terminal() {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-headless-wait");
    let choice = a_headless_provider_running(
        &state,
        &repo,
        crate::harness::adk::fake::stream_json_harness(&[crate::harness::adk::fake::RESULT]),
    );
    let agent_id = crate::agent::derived_agent_id("run-headless-wait");
    let key = derived_agent_key(&AppState::canonical_root(&root), "run-headless-wait");

    let (sender, mut pushes, session_key) = SessionSender::observable("waiting");
    {
        let mut s = state.lock().unwrap();
        let waiting = ScreenHandle::new(&key.tab_id, 90, 25);
        waiting.attach(&sender, None);
        s.session_registry
            .test_remember_waiting_screen(key.clone(), waiting);
    }

    deliver(
        &state,
        &root,
        "run-headless-wait",
        &agent_id,
        &choice,
        "build",
        ["cold", "warm"],
    )
    .expect("the headless agent spawns");

    let seen = wait_for_pushes(&mut pushes, &session_key, |seen| {
        seen.iter().any(|push| push["type"] == "term.closed")
    })
    .await;
    let closed = seen
        .iter()
        .find(|push| push["type"] == "term.closed")
        .expect("the waiting client is told, rather than left on a dead grid");
    assert_eq!(closed["term_id"], key.tab_id, "{closed:?}");

    let s = state.lock().unwrap();
    assert!(
        s.session_registry.test_counts().waiting_screens == 0,
        "and the screen is not left behind for some later spawn to inherit"
    );
    let tab = &s.session_registry.test_tab(&key).unwrap();
    assert!(
        tab.screen.is_none(),
        "a session with no terminal has no grid, so there is none to hand anyone"
    );
    assert!(
        tab.live,
        "the agent itself is running — it just has no basement"
    );
    s.session_registry.test_tab(&key).unwrap().session.end();
}

/// The same rule for the other screen a spawn can be holding: the grid the
/// session being replaced left behind.
///
/// A retained screen is carried onto the replacement so the cursor never
/// rewinds — but a replacement with no terminal has nothing to carry it to,
/// and a human who changed this agent's provider between the two sessions
/// would otherwise be left watching the dead one's last frame forever.
#[tokio::test]
async fn a_headless_respawn_closes_the_grid_the_terminal_left_behind() {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-io-swap");
    let agent_id = crate::agent::derived_agent_id("run-io-swap");
    let key = derived_agent_key(&AppState::canonical_root(&root), "run-io-swap");

    // A terminal session, watched by a client, that then dies.
    deliver(
        &state,
        &root,
        "run-io-swap",
        &agent_id,
        &ModelChoice::default(),
        "build",
        ["FIRST-SESSION", "warm"],
    )
    .expect("the first delivery spawns a PTY");
    wait_for_agent_screen(&state, &root, "FIRST-SESSION").await;
    let (sender, mut pushes, session_key) = SessionSender::observable("watching");
    {
        let s = state.lock().unwrap();
        screen_of(s.session_registry.test_tab(&key).unwrap()).attach(&sender, None);
    }
    state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&key)
        .unwrap()
        .session
        .end();
    wait_for(Duration::from_secs(5), || {
        (!state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(&key)
            .unwrap()
            .live)
            .then_some(())
    })
    .await
    .expect("the dead session leaves a retained screen behind");

    // The human changed this agent's provider while it was down, so its
    // replacement reports itself instead of painting.
    let choice = a_headless_provider_running(
        &state,
        &repo,
        crate::harness::adk::fake::stream_json_harness(&[crate::harness::adk::fake::RESULT]),
    );
    deliver(
        &state,
        &root,
        "run-io-swap",
        &agent_id,
        &choice,
        "build",
        ["SECOND-SESSION", "warm"],
    )
    .expect("the headless replacement spawns");

    let seen = wait_for_pushes(&mut pushes, &session_key, |seen| {
        seen.iter()
            .any(|push| push["type"] == "term.closed" && push["reason"] == "no_terminal")
    })
    .await;
    assert!(
        seen.iter().any(|push| push["term_id"] == key.tab_id),
        "the client watching the old grid is told which tab closed: {seen:?}"
    );
    let s = state.lock().unwrap();
    assert!(
        s.session_registry.test_tab(&key).unwrap().screen.is_none(),
        "and the retained grid is not hung on a session that cannot paint it"
    );
    s.session_registry.test_tab(&key).unwrap().session.end();
}

/// A spawn that never opens closes the grid it took.
///
/// The reservation takes the dead session's tab out of the registry and
/// keeps its screen, deliberately telling the clients on it nothing: they
/// are about to be handed to the replacement. A replacement that fails to
/// open — the binary is gone, the harness was reconfigured wrongly — has
/// nobody to hand them to, and the screen is in no registry for a reaper
/// or a close to find. So the failure says the words itself, rather than
/// leaving browsers on a grid nothing will paint and nothing will close.
#[tokio::test]
async fn a_spawn_that_fails_closes_the_grid_it_took_from_the_dead_session() {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-spawn-fails");
    let agent_id = crate::agent::derived_agent_id("run-spawn-fails");
    let key = derived_agent_key(&AppState::canonical_root(&root), "run-spawn-fails");

    // A terminal session, watched by a client, that then dies.
    deliver(
        &state,
        &root,
        "run-spawn-fails",
        &agent_id,
        &ModelChoice::default(),
        "test",
        ["FIRST-SESSION", "warm"],
    )
    .expect("the first delivery spawns a PTY");
    wait_for_agent_screen(&state, &root, "FIRST-SESSION").await;
    let (sender, mut pushes, session_key) = SessionSender::observable("watching");
    {
        let s = state.lock().unwrap();
        screen_of(s.session_registry.test_tab(&key).unwrap()).attach(&sender, None);
    }
    state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&key)
        .unwrap()
        .session
        .end();
    wait_for(Duration::from_secs(5), || {
        (!state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(&key)
            .unwrap()
            .live)
            .then_some(())
    })
    .await
    .expect("the dead session leaves a retained screen behind");

    // The harness this agent is locked to is no longer on the machine.
    a_provider_running(
        &state,
        &repo,
        HarnessSpec::new(dir.path().join("no-such-harness").display().to_string()),
    );
    let refused = deliver(
        &state,
        &root,
        "run-spawn-fails",
        &agent_id,
        &ModelChoice::default(),
        "test",
        ["SECOND-SESSION", "warm"],
    )
    .expect_err("a harness that is not there cannot be opened");
    assert!(!refused.is_empty(), "and the delivery says why");

    let seen = wait_for_pushes(&mut pushes, &session_key, |seen| {
        seen.iter().any(|push| push["reason"] == SPAWN_NEVER_OPENED)
    })
    .await;
    let closed = seen
        .iter()
        .find(|push| push["reason"] == SPAWN_NEVER_OPENED)
        .expect("the client is told, rather than left on an orphaned grid");
    assert_eq!(closed["type"], "term.closed", "{closed:?}");
    assert_eq!(closed["term_id"], key.tab_id, "{closed:?}");
    assert!(
        !state.lock().unwrap().session_registry.contains(&key),
        "the failed spawn leaves no tab behind either"
    );
}

/// A reservation that cannot be completed takes nothing out of the registry.
///
/// Retiring the dead tab and registering the MCP token are the reservation
/// giving things up on the registry's behalf, and a failure after either
/// bypasses the one primitive that gives them back. So every read that can
/// fail runs first, with the registry untouched: the dead tab keeps its
/// retained grid and the clients on it, the token the last session held
/// stands, and a later spawn can still replace both.
#[tokio::test]
async fn a_reservation_that_cannot_resolve_its_project_takes_nothing_from_the_registry() {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-unresolvable");
    let agent_id = crate::agent::derived_agent_id("run-unresolvable");
    let key = derived_agent_key(&AppState::canonical_root(&root), "run-unresolvable");

    deliver(
        &state,
        &root,
        "run-unresolvable",
        &agent_id,
        &ModelChoice::default(),
        "test",
        ["FIRST-SESSION", "warm"],
    )
    .expect("the first delivery spawns a PTY");
    wait_for_agent_screen(&state, &root, "FIRST-SESSION").await;
    state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&key)
        .unwrap()
        .session
        .end();
    wait_for(Duration::from_secs(5), || {
        (!state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(&key)
            .unwrap()
            .live)
            .then_some(())
    })
    .await
    .expect("the dead session leaves a retained screen behind");
    let (sender, mut pushes, session_key) = SessionSender::observable("watching");
    let token_before = {
        let s = state.lock().unwrap();
        screen_of(s.session_registry.test_tab(&key).unwrap()).attach(&sender, None);
        s.session_registry
            .test_token(&agent_id)
            .unwrap()
            .to_string()
    };

    // The owner's project binding is gone, so the reservation cannot say
    // which orchestrator builds the harness.
    state
        .lock()
        .unwrap()
        .projects
        .unbind_live_entity("run-unresolvable");
    let refused = deliver(
        &state,
        &root,
        "run-unresolvable",
        &agent_id,
        &ModelChoice::default(),
        "test",
        ["SECOND-SESSION", "warm"],
    )
    .expect_err("an owner with no project cannot be spawned for");
    assert!(refused.contains("unknown entity id"), "{refused}");

    let s = state.lock().unwrap();
    let dead = s
        .session_registry
        .test_tab(&key)
        .expect("the dead tab is still in the registry");
    assert!(
        dead.screen.is_some(),
        "and still holds the grid its clients are attached to"
    );
    assert_eq!(
        s.session_registry.test_token(&agent_id).unwrap(),
        token_before,
        "no token was registered for a child that never existed"
    );
    assert!(
        s.session_registry.test_counts().claims == 0,
        "no claim was left behind"
    );
    drop(s);
    let mut seen = Vec::new();
    while let Ok(message) = pushes.try_recv() {
        seen.push(SessionSender::decrypt_push(&session_key, &message));
    }
    assert!(
        !seen.iter().any(|push| push["type"] == "term.closed"),
        "the attached client was told nothing, because nothing changed: {seen:?}"
    );
}

/// A client can be waiting on the Agent tab of a worktree that is then
/// deleted out from under it. The reaper closes the tabs of a vanished
/// worktree; the screen its agent was going to be born onto is the same
/// thing one step earlier, so it goes the same way — the client hears
/// `reaped` instead of waiting forever on a directory that is gone, and no
/// future spawn inherits it.
#[test]
fn the_reaper_drops_an_agent_screen_whose_worktree_vanished_before_a_spawn() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let vanishing = dir.path().join("vanishing");
    std::fs::create_dir_all(&vanishing).unwrap();
    let root = AppState::canonical_root(&vanishing);
    let key = derived_agent_key(&root, "run-vanishing");
    let wire_id = key.tab_id.clone();

    let (sender, mut pushes, session_key) = SessionSender::observable("s1");
    {
        let mut s = state.lock().unwrap();
        let waiting = ScreenHandle::new(&key.tab_id, 80, 24);
        waiting.attach(&sender, None);
        s.session_registry
            .test_remember_waiting_screen(key, waiting);
    }
    std::fs::remove_dir_all(&vanishing).unwrap();

    let reaped = state.lock().unwrap().reap_orphaned_terminals();
    assert_eq!(reaped, vec![wire_id.clone()]);
    assert!(
        state
            .lock()
            .unwrap()
            .session_registry
            .test_counts()
            .waiting_screens
            == 0,
        "a screen for a directory that is gone is never handed to a future spawn"
    );
    let closed = SessionSender::decrypt_push(
        &session_key,
        &pushes.try_recv().expect("the client hears its tab is gone"),
    );
    assert_eq!(closed["type"], "term.closed", "{closed:?}");
    assert_eq!(closed["term_id"], wire_id);
    assert_eq!(closed["reason"], "reaped");
}

/// A human can close the browser while waiting on the Agent tab of a
/// worktree whose agent has not started yet. `drop_session` detaches an
/// ended session from every tab so the pumps stop encrypting frames into a
/// session the relay will only drop — and a screen waiting for its first
/// spawn is a tab one step early, so it goes the same way. Otherwise the
/// spawn that finally comes carries a dead client onto the real screen and
/// pushes to it for the life of the tab, and sizes the new PTY to a
/// viewport nobody is looking at.
#[tokio::test]
async fn a_session_that_ended_while_waiting_is_not_carried_onto_the_agent() {
    let (dir, repo) = init_repo();
    let (state, handler, _) =
        agent_tab_fixture_at(&repo, dir.path(), "run-closed-client", repo.clone());
    let project_id = state.lock().unwrap().project_at(0).id.clone();

    let (sender, _pushes, _key) = SessionSender::observable("closing");
    let waiting = handler.call(
        sender,
        req(
            "agent.attach",
            json!({ "project_id": project_id, "cols": 90, "rows": 25 }),
        ),
    );
    assert_eq!(waiting["ok"], true, "{waiting:?}");
    assert_eq!(waiting["result"]["live"], false, "nothing runs here yet");

    state.lock().unwrap().drop_session("closing");
    assert!(
        state
            .lock()
            .unwrap()
            .session_registry
            .test_waiting_screens()
            .all(|screen| screen.attached() == 0),
        "an ended session is detached from the screen it was waiting on"
    );

    deliver(
        &state,
        &repo,
        "run-closed-client",
        &crate::agent::derived_agent_id("run-closed-client"),
        &ModelChoice::default(),
        "build",
        ["COLD-PROMPT", "WARM-NUDGE"],
    )
    .expect("the delivery spawns the worktree's agent");

    let s = state.lock().unwrap();
    let tab = s
        .session_registry
        .test_tab(&derived_agent_key(
            &AppState::canonical_root(&repo),
            "run-closed-client",
        ))
        .unwrap();
    assert!(
        screen_of(tab).attached() == 0,
        "a session that ended is never carried onto the agent it waited for"
    );
    assert_eq!(
        screen_of(tab).size(),
        (120, 40),
        "with nobody left waiting, the spawn keeps the size Build chose"
    );
}

/// An attach clones the waiting screen under the app mutex and registers
/// on it with the mutex released, so the last client already on that
/// screen can leave in between. The screen must still be the one the spawn
/// carries from: dropped from the registry the moment it emptied, the
/// client arriving on it would be on a screen nothing feeds and nothing
/// closes, blank for the life of the tab.
#[tokio::test]
async fn a_client_attaching_as_the_last_waiting_client_leaves_is_carried_onto_the_agent() {
    let (dir, repo) = init_repo();
    let (state, handler, _) =
        agent_tab_fixture_at(&repo, dir.path(), "run-attach-race", repo.clone());
    let project_id = state.lock().unwrap().project_at(0).id.clone();

    let (leaving, _pushes, _key) = SessionSender::observable("leaving");
    let waiting = handler.call(
        leaving,
        req(
            "agent.attach",
            json!({ "project_id": project_id, "cols": 90, "rows": 25 }),
        ),
    );
    assert_eq!(waiting["ok"], true, "{waiting:?}");

    // What a second attach, already past the app mutex, is holding.
    let in_flight = state
        .lock()
        .unwrap()
        .session_registry
        .test_waiting_screens()
        .next()
        .expect("the first attach left a screen waiting for the spawn")
        .clone();
    state.lock().unwrap().drop_session("leaving");
    let (arriving, mut pushes, session_key) = SessionSender::observable("arriving");
    in_flight.attach(&arriving, Some((90, 25)));

    let (wire_id, _) = deliver(
        &state,
        &repo,
        "run-attach-race",
        &crate::agent::derived_agent_id("run-attach-race"),
        &ModelChoice::default(),
        "test",
        ["COLD-PROMPT", "WARM-NUDGE"],
    )
    .expect("the delivery spawns the worktree's agent");

    let key = derived_agent_key(&AppState::canonical_root(&repo), "run-attach-race");
    {
        let s = state.lock().unwrap();
        let screen = screen_of(s.session_registry.test_tab(&key).unwrap());
        assert_eq!(
            screen.attached_sessions(),
            vec!["arriving".to_string()],
            "the client that arrived as the last one left is on the agent's screen"
        );
        assert_eq!(
            screen.size(),
            (90, 25),
            "at the viewport it is rendering at"
        );
    }
    wait_for_push(&mut pushes, &session_key, |push| {
        push["type"] == "term.reset" && push["term_id"] == wire_id
    })
    .await;
    state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&key)
        .unwrap()
        .session
        .end();
}

#[tokio::test]
async fn keyed_run_terminal_roundtrips() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    insert_live_run(&state, &repo, dir.path().join("side"), "run-7");

    let created = handler.call(
        SessionSender::detached("s1"),
        req("term.create", json!({ "run_id": "run-7" })),
    );
    assert_eq!(created["ok"], true, "{created:?}");
    let term_id = created["result"]["term_id"].as_str().unwrap().to_string();

    let (sender, mut pushes, key) = SessionSender::observable("s1");
    let attached = handler.call(sender, req("term.attach", json!({ "term_id": term_id })));
    assert_eq!(attached["ok"], true, "{attached:?}");

    handler.call(
        SessionSender::detached("s1"),
        req(
            "term.input",
            json!({ "term_id": term_id, "data": b64encode(b"echo hi-there\n") }),
        ),
    );
    wait_for_push(&mut pushes, &key, |p| {
        p["type"] == "term.output"
            && output_text(std::slice::from_ref(p), &term_id).contains("hi-there")
    })
    .await;

    let listed = handler.call(
        SessionSender::detached("s1"),
        req("term.list", json!({ "run_id": "run-7" })),
    );
    assert!(!listed["result"]["terminals"].as_array().unwrap().is_empty());
}

#[test]
fn term_scope_parses_run_project_and_external() {
    assert!(matches!(
        TermScope::parse(&json!({ "run_id": "run-1" })).unwrap(),
        TermScope::Run { .. }
    ));
    assert!(matches!(
        TermScope::parse(&json!({ "project_id": "proj-1" })).unwrap(),
        TermScope::Primary { .. }
    ));
    assert!(matches!(
        TermScope::parse(&json!({ "project_id": "proj-1", "worktree_id": "w" })).unwrap(),
        TermScope::ExternalWorktree { .. }
    ));
    assert!(TermScope::parse(&json!({})).is_err());
}
